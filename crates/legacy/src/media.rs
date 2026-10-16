//! 媒体摘要预取：逐个读取旧文件在对象存储里的原始字节，算 SHA-256、嗅探真实类型、生成 ThumbHash，
//! 结果写进 `legacy.ik_media_digests`。
//!
//! 文件不搬家：新系统和旧系统用同一个桶与 CDN，`object_key` 就是旧 URL 去掉公开前缀后的路径
//! （UPL-007、UPL-009）。读取是外部调用，不能放进导入事务，所以是单独一步；旧文件不会再变，
//! 可以在停机切换之前先跑。可续跑：已成功的文件不再读取，失败的加 `--retry-failed` 重试。

use futures_util::{StreamExt, stream};
use ik_media::sniff;
use ik_media::storage::{Storage, StorageError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

/// 新系统只接受这么大的文件（UPL-004）；更大的旧文件不导入。
const MAX_BYTES: u64 = sniff::MAX_UPLOAD_BYTES;

#[derive(Debug, Clone)]
pub struct MediaOptions {
    /// 旧文件 URL 的公开前缀（例如 `https://im.tiwat.cn/`），去掉它剩下的就是对象 key。
    /// 按顺序匹配第一个。
    pub url_prefixes: Vec<String>,
    /// 同时读取的文件数。
    pub concurrency: usize,
    /// 重新读取之前失败的文件。
    pub retry_failed: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct MediaReport {
    pub fetched: u64,
    pub failed: u64,
}

struct Pending {
    file_id: i32,
    url: Option<String>,
}

struct Outcome {
    object_key: Option<String>,
    result: Result<FileDigest, String>,
}

struct FileDigest {
    sha256: String,
    mime: &'static str,
    size_bytes: i64,
    thumbhash: Option<String>,
}

pub async fn fetch_digests(
    db: &PgPool,
    storage: &Storage,
    options: &MediaOptions,
) -> anyhow::Result<MediaReport> {
    crate::install_helpers(db).await?;
    let pending = sqlx::query_as!(
        Pending,
        r#"SELECT f.id AS "file_id!", f.url
           FROM legacy.files f
           LEFT JOIN legacy.ik_media_digests d ON d.file_id = f.id
           WHERE d.file_id IS NULL OR ($1 AND d.error IS NOT NULL)
           ORDER BY f.id"#,
        options.retry_failed,
    )
    .fetch_all(db)
    .await?;
    tracing::info!(files = pending.len(), "开始读取旧文件");

    let mut report = MediaReport::default();
    let mut results = stream::iter(pending)
        .map(|file| async move {
            let outcome = digest(storage, &options.url_prefixes, file.url.as_deref()).await;
            (file.file_id, outcome)
        })
        .buffer_unordered(options.concurrency.max(1));
    while let Some((file_id, outcome)) = results.next().await {
        let (sha256, mime, size_bytes, thumbhash, error) = match outcome.result {
            Ok(d) => (
                Some(d.sha256),
                Some(d.mime),
                Some(d.size_bytes),
                d.thumbhash,
                None,
            ),
            Err(reason) => (None, None, None, None, Some(reason)),
        };
        if error.is_some() {
            report.failed += 1;
        } else {
            report.fetched += 1;
        }
        sqlx::query!(
            "INSERT INTO legacy.ik_media_digests
                 (file_id, object_key, sha256, mime, size_bytes, thumbhash, error, fetched_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, now())
             ON CONFLICT (file_id) DO UPDATE SET
                 object_key = EXCLUDED.object_key, sha256 = EXCLUDED.sha256, mime = EXCLUDED.mime,
                 size_bytes = EXCLUDED.size_bytes, thumbhash = EXCLUDED.thumbhash,
                 error = EXCLUDED.error, fetched_at = EXCLUDED.fetched_at",
            file_id,
            outcome.object_key,
            sha256,
            mime,
            size_bytes,
            thumbhash,
            error,
        )
        .execute(db)
        .await?;
        let done = report.fetched + report.failed;
        if done % 200 == 0 {
            tracing::info!(done, failed = report.failed, "读取进度");
        }
    }
    Ok(report)
}

/// 去掉公开前缀得到对象 key；去掉查询串与片段。
pub fn object_key(url: &str, prefixes: &[String]) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    prefixes.iter().find_map(|prefix| {
        path.strip_prefix(prefix.as_str())
            .map(|rest| rest.trim_start_matches('/'))
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
    })
}

async fn digest(storage: &Storage, prefixes: &[String], url: Option<&str>) -> Outcome {
    let Some(key) = url.and_then(|u| object_key(u, prefixes)) else {
        return Outcome {
            object_key: None,
            result: Err(format!("URL 不在已知的公开前缀下：{}", url.unwrap_or(""))),
        };
    };
    let result = read(storage, &key).await;
    Outcome {
        object_key: Some(key),
        result,
    }
}

async fn read(storage: &Storage, key: &str) -> Result<FileDigest, String> {
    let (size, etag) = storage.head(key).await.map_err(describe)?;
    if size == 0 || size > MAX_BYTES {
        return Err(format!("大小 {size} 字节，超出 1 字节–30 MB"));
    }
    let bytes = storage
        .read_exact(key, &etag, size)
        .await
        .map_err(describe)?;
    let Some(mime) = sniff::sniff(&bytes) else {
        return Err("不是支持的图片类型".to_owned());
    };
    let sha256 = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let thumbhash = tokio::task::spawn_blocking(move || ik_media::thumb::compute(&bytes, mime))
        .await
        .ok()
        .flatten();
    Ok(FileDigest {
        sha256,
        mime,
        size_bytes: i64::try_from(size).unwrap_or(i64::MAX),
        thumbhash,
    })
}

fn describe(error: StorageError) -> String {
    match error {
        StorageError::NotFound => "对象不存在".to_owned(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::object_key;

    #[test]
    fn object_keys_strip_known_prefixes() {
        let prefixes = vec![
            "https://im.tiwat.cn/".to_owned(),
            "https://image.tiwat.cn".to_owned(),
        ];
        assert_eq!(
            object_key("https://im.tiwat.cn/uploads/2025/01/ab.png?x=1", &prefixes).as_deref(),
            Some("uploads/2025/01/ab.png")
        );
        assert_eq!(
            object_key("https://image.tiwat.cn/c.jpg", &prefixes).as_deref(),
            Some("c.jpg")
        );
        assert_eq!(object_key("https://other.example/c.jpg", &prefixes), None);
        assert_eq!(object_key("https://im.tiwat.cn/", &prefixes), None);
    }
}
