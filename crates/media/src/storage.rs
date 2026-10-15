//! 对象存储（S3 兼容）封装：预签名 PUT、HEAD、带 If-Match 的限长读取、写入、删除。
//! 错误一律转成 [`StorageError`]，由 service 映射成 Problem。

use std::time::Duration;

use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::timeout::TimeoutConfig;
use aws_sdk_s3::config::{BehaviorVersion, Region};
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::ObjectCannedAcl;
use aws_smithy_http_client::tls::{self, rustls_provider::CryptoMode};
use ik_kernel::config::Storage as StorageConfig;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// 对象不存在（客户端还没上传，或暂存对象已被清理）。
    #[error("对象不存在")]
    NotFound,
    /// 读取时对象已被替换（If-Match 失败），或长度与签名不符。
    #[error("对象内容与签名不符：{0}")]
    Mismatch(&'static str),
    #[error("对象存储请求失败：{0}")]
    Unavailable(String),
}

/// 预签名的上传地址。
#[derive(Debug, Clone)]
pub struct PresignedPut {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct Storage {
    client: Client,
    bucket: String,
    acl: Option<ObjectCannedAcl>,
}

fn unavailable(error: impl std::fmt::Debug) -> StorageError {
    StorageError::Unavailable(format!("{error:?}"))
}

impl Storage {
    pub fn new(config: &StorageConfig) -> Self {
        let http = aws_smithy_http_client::Builder::new()
            .tls_provider(tls::Provider::Rustls(CryptoMode::Ring))
            .build_https();
        let credentials = Credentials::new(
            config.access_key.expose(),
            config.secret_key.expose(),
            None,
            None,
            "ik-config",
        );
        let request_timeout = Duration::from_secs(config.request_timeout_secs);
        let s3 = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .endpoint_url(config.endpoint.trim_end_matches('/'))
            .credentials_provider(credentials)
            .force_path_style(true)
            .http_client(http)
            .timeout_config(
                TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(5))
                    .operation_attempt_timeout(request_timeout)
                    .build(),
            )
            .build();
        Self {
            client: Client::from_conf(s3),
            bucket: config.bucket.clone(),
            acl: config
                .public_read_acl
                .then_some(ObjectCannedAcl::PublicRead),
        }
    }

    /// 预签名 PUT：签入 Content-Type 与 Content-Length，上传的字节数必须与声明一致。
    pub async fn presign_put(
        &self,
        key: &str,
        mime: &str,
        size: u64,
        expires: Duration,
    ) -> Result<PresignedPut, StorageError> {
        let presigning = PresigningConfig::expires_in(expires).map_err(unavailable)?;
        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(mime)
            .content_length(i64::try_from(size).unwrap_or(i64::MAX));
        if let Some(acl) = &self.acl {
            request = request.acl(acl.clone());
        }
        let presigned = request.presigned(presigning).await.map_err(unavailable)?;
        let mut headers = vec![("Content-Type".to_owned(), mime.to_owned())];
        if let Some(acl) = &self.acl {
            headers.push(("x-amz-acl".to_owned(), acl.as_str().to_owned()));
        }
        Ok(PresignedPut {
            url: presigned.uri().to_owned(),
            headers,
        })
    }

    /// HEAD：返回 (大小, ETag)。
    pub async fn head(&self, key: &str) -> Result<(u64, String), StorageError> {
        let out = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|e| {
                if e.as_service_error().is_some_and(|s| s.is_not_found()) {
                    StorageError::NotFound
                } else {
                    unavailable(e)
                }
            })?;
        let size = out
            .content_length()
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(StorageError::Mismatch("缺少长度"))?;
        let etag = out
            .e_tag()
            .map(str::to_owned)
            .ok_or(StorageError::Mismatch("缺少 ETag"))?;
        Ok((size, etag))
    }

    /// 读取对象，要求 ETag 未变（UPL-053），最多读 `size` 字节；多一个字节都不行（UPL-033）。
    pub async fn read_exact(
        &self,
        key: &str,
        etag: &str,
        size: u64,
    ) -> Result<Vec<u8>, StorageError> {
        let out = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .if_match(etag)
            .send()
            .await
            .map_err(|e| {
                let status = e.raw_response().map(|r| r.status().as_u16());
                match status {
                    Some(412) => StorageError::Mismatch("读取前对象被替换"),
                    Some(404) => StorageError::NotFound,
                    _ => unavailable(e),
                }
            })?;
        let capacity = usize::try_from(size).map_err(|_| StorageError::Mismatch("大小非法"))?;
        let mut buffer = Vec::with_capacity(capacity);
        let mut body = out.body;
        while let Some(chunk) = body.try_next().await.map_err(unavailable)? {
            if buffer.len() + chunk.len() > capacity {
                return Err(StorageError::Mismatch("超过签名大小"));
            }
            buffer.extend_from_slice(&chunk);
        }
        if buffer.len() != capacity {
            return Err(StorageError::Mismatch("大小与签名不符"));
        }
        Ok(buffer)
    }

    pub async fn put(&self, key: &str, bytes: Vec<u8>, mime: &str) -> Result<(), StorageError> {
        let length = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        let mut request = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(mime)
            .content_length(length)
            .body(ByteStream::from(bytes));
        if let Some(acl) = &self.acl {
            request = request.acl(acl.clone());
        }
        request.send().await.map_err(unavailable)?;
        Ok(())
    }

    pub async fn delete(&self, key: &str) -> Result<(), StorageError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(unavailable)?;
        Ok(())
    }

    /// 测试用：建桶（已存在时忽略错误）。
    #[doc(hidden)]
    pub async fn ensure_bucket(&self) -> Result<(), StorageError> {
        let _ = self
            .client
            .create_bucket()
            .bucket(&self.bucket)
            .send()
            .await;
        Ok(())
    }
}
