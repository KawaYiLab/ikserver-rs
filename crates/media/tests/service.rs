//! 直传上传集成测试：真实 PostgreSQL + S3 兼容存储。
//!
//! 需要 `IK_TEST_S3_URL`（例如 MinIO 或 moto 的 `http://127.0.0.1:9000`），可选 `IK_TEST_S3_ACCESS_KEY` /
//! `IK_TEST_S3_SECRET_KEY`；每个测试用独立的桶。没有设置时依赖存储的用例直接通过。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stderr)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ik_auth::Principal;
use ik_kernel::config::{Secret, Storage as StorageConfig};
use ik_kernel::{Id, Role, Snowflake};
use ik_media::MediaState;
use ik_media::dto::{CompleteRequest, GalleryQuery, SignRequest, SignResult};
use ik_media::service;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// 最小的合法 PNG 头（嗅探只看文件头）+ 不同的尾巴，保证内容各不相同。
fn png(tag: u8) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend(std::iter::repeat_n(tag, 64));
    bytes
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn s3_config() -> Option<StorageConfig> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let url = std::env::var("IK_TEST_S3_URL").ok()?;
    Some(StorageConfig {
        endpoint: url,
        region: "us-east-1".into(),
        bucket: format!(
            "ik-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        access_key: Secret::new(
            std::env::var("IK_TEST_S3_ACCESS_KEY").unwrap_or_else(|_| "test".into()),
        ),
        secret_key: Secret::new(
            std::env::var("IK_TEST_S3_SECRET_KEY").unwrap_or_else(|_| "testsecret".into()),
        ),
        root_prefix: "uploads".into(),
        ..StorageConfig::default()
    })
}

struct Harness {
    state: MediaState,
    storage: ik_media::storage::Storage,
    bucket_url: String,
    db: PgPool,
    http: reqwest::Client,
}

async fn harness(db: PgPool) -> Option<Harness> {
    let Some(config) = s3_config() else {
        eprintln!("未设置 IK_TEST_S3_URL，跳过对象存储用例");
        return None;
    };
    let state = MediaState::new(
        db.clone(),
        Arc::new(Snowflake::new(1).unwrap()),
        &config,
        b"test-secret",
    );
    state.ensure_bucket().await.unwrap();
    let h = Harness {
        state,
        storage: ik_media::storage::Storage::new(&config),
        bucket_url: format!("{}/{}/", config.endpoint, config.bucket),
        db,
        http: reqwest::Client::new(),
    };
    for id in [1, 2, 3] {
        h.user(id, true).await;
    }
    h.user(4, false).await;
    Some(h)
}

fn principal(id: i64, exam_passed: bool) -> Principal {
    Principal {
        user_id: Id::new(id),
        session_id: Id::new(id),
        role: Role::User,
        exam_passed,
        is_ai_agent: false,
        level: 1,
    }
}

impl Harness {
    async fn user(&self, id: i64, exam: bool) {
        sqlx::query!(
            "INSERT INTO users (id, username, email, level, exam_passed_at)
             VALUES ($1, $2, $3, 1, CASE WHEN $4 THEN now() END)",
            id,
            format!("user{id}"),
            format!("user{id}@example.com"),
            exam,
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    async fn sign(&self, user: i64, bytes: &[u8], hash: Option<String>) -> SignResult {
        service::sign(
            &self.state,
            &principal(user, true),
            SignRequest {
                filename: "图.png".into(),
                mime_type: "image/png".into(),
                size: bytes.len() as u64,
                content_hash: hash,
            },
        )
        .await
        .unwrap()
    }

    async fn put(&self, ticket: &SignResult, bytes: &[u8]) {
        let mut request = self.http.put(ticket.upload_url.as_deref().unwrap());
        for (k, v) in ticket.headers.as_ref().unwrap() {
            request = request.header(k, v);
        }
        let response = request.body(bytes.to_vec()).send().await.unwrap();
        assert!(
            response.status().is_success(),
            "{:?}",
            response.text().await
        );
    }

    async fn complete(
        &self,
        user: i64,
        ticket: &SignResult,
    ) -> Result<(bool, ik_media::dto::MediaView), ik_kernel::Problem> {
        service::complete(
            &self.state,
            &principal(user, true),
            ticket.upload_id.as_deref().unwrap(),
            CompleteRequest {
                width: Some(640),
                height: Some(480),
            },
        )
        .await
    }

    async fn upload(&self, user: i64, bytes: &[u8]) -> ik_media::dto::MediaView {
        let ticket = self.sign(user, bytes, Some(sha(bytes))).await;
        assert!(!ticket.existing);
        self.put(&ticket, bytes).await;
        self.complete(user, &ticket).await.unwrap().1
    }

    async fn gallery(&self, user: i64) -> Vec<i64> {
        service::gallery(
            &self.state,
            &principal(user, true),
            GalleryQuery {
                cursor: None,
                limit: None,
            },
        )
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|m| m.id.get())
        .collect()
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn upload_is_verified_published_and_idempotent(db: PgPool) {
    let Some(h) = harness(db).await else { return };
    let bytes = png(1);
    let ticket = h.sign(1, &bytes, None).await;
    assert_eq!(ticket.method.as_deref(), Some("PUT"));
    assert_eq!(
        ticket
            .headers
            .as_ref()
            .unwrap()
            .get("Content-Type")
            .map(String::as_str),
        Some("image/png")
    );
    h.put(&ticket, &bytes).await;
    let (created, media) = h.complete(1, &ticket).await.unwrap();
    assert!(created);
    assert_eq!(media.mime, "image/png");
    assert_eq!(media.size, bytes.len() as i64);
    assert_eq!(media.width, Some(640));
    assert_eq!(media.name, "图.png");
    assert!(
        media.url.contains("/uploads/20"),
        "发布到新 key：{}",
        media.url
    );
    assert!(!media.url.contains("staging"));
    // 发布后的对象可以读到，内容与上传的一致。
    let key = media.url.strip_prefix(&h.bucket_url).unwrap();
    let (size, etag) = h.storage.head(key).await.unwrap();
    let published = h.storage.read_exact(key, &etag, size).await.unwrap();
    assert_eq!(published, bytes);
    // 暂存对象已删除（UPL-036）。
    let claims = ik_media::token::TokenKey::derive(b"test-secret")
        .verify(ticket.upload_id.as_deref().unwrap())
        .unwrap();
    assert!(claims.key.starts_with("uploads/staging/"));
    assert!(matches!(
        h.storage.head(&claims.key).await,
        Err(ik_media::storage::StorageError::NotFound)
    ));

    // 同一凭证重试：返回同一个文件（收据），即使暂存对象已经删了。
    let (created, again) = h.complete(1, &ticket).await.unwrap();
    assert!(!created);
    assert_eq!(again.id, media.id);
    assert_eq!(h.gallery(1).await, vec![media.id.get()]);

    // 声明了哈希、自己有相同内容 → 签名阶段直接复用。
    let reuse = h.sign(1, &bytes, Some(sha(&bytes))).await;
    assert!(reuse.existing);
    assert_eq!(reuse.media.unwrap().id, media.id);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cross_user_dedupe_requires_uploading_the_bytes(db: PgPool) {
    let Some(h) = harness(db).await else { return };
    let bytes = png(2);
    let first = h.upload(1, &bytes).await;
    // 只知道哈希不够：签名阶段不会把别人的文件给我（D-077 方案 A）。
    let ticket = h.sign(2, &bytes, Some(sha(&bytes))).await;
    assert!(!ticket.existing);
    // 真的上传了相同字节 → 校验通过后复用同一个文件。
    h.put(&ticket, &bytes).await;
    let (created, second) = h.complete(2, &ticket).await.unwrap();
    assert!(!created);
    assert_eq!(second.id, first.id);
    assert_eq!(h.gallery(2).await, vec![first.id.get()]);
    let files = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM media_files"#)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(files, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn verification_rejects_bad_uploads(db: PgPool) {
    let Some(h) = harness(db).await else { return };

    // 还没上传。
    let ticket = h.sign(1, &png(3), None).await;
    assert_eq!(
        h.complete(1, &ticket).await.unwrap_err().code(),
        "UPLOAD_NOT_FOUND"
    );

    // 不是图片（声明成 png）。
    let text = b"definitely not an image, just some text bytes padding".to_vec();
    let ticket = h.sign(1, &text, None).await;
    h.put(&ticket, &text).await;
    assert_eq!(
        h.complete(1, &ticket).await.unwrap_err().code(),
        "UPLOAD_NOT_IMAGE"
    );

    // 声明的哈希与上传的字节不符。
    let bytes = png(4);
    let ticket = h.sign(1, &bytes, Some(sha(&png(5)))).await;
    h.put(&ticket, &bytes).await;
    assert_eq!(
        h.complete(1, &ticket).await.unwrap_err().code(),
        "UPLOAD_CHECKSUM_MISMATCH"
    );

    // 别人的凭证、被篡改的凭证。
    let bytes = png(6);
    let ticket = h.sign(1, &bytes, None).await;
    h.put(&ticket, &bytes).await;
    assert_eq!(
        h.complete(2, &ticket).await.unwrap_err().code(),
        "UPLOAD_TOKEN_INVALID"
    );
    let mut forged = ticket.upload_id.clone().unwrap();
    forged.insert(3, 'x');
    let err = service::complete(
        &h.state,
        &principal(1, true),
        &forged,
        CompleteRequest::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "UPLOAD_TOKEN_INVALID");
    // 正主仍然可以完成。
    assert!(h.complete(1, &ticket).await.unwrap().0);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn sign_validates_input_and_requires_exam(db: PgPool) {
    let Some(h) = harness(db).await else { return };
    let request = |filename: &str, mime: &str, size: u64| SignRequest {
        filename: filename.into(),
        mime_type: mime.into(),
        size,
        content_hash: None,
    };
    let me = principal(1, true);
    for (req, field) in [
        (request("a.svg", "image/png", 10), "filename"),
        (request("a.png", "image/svg+xml", 10), "mimeType"),
        (request("a.png", "image/png", 0), "size"),
        (request("a.png", "image/png", 31 * 1024 * 1024), "size"),
    ] {
        let err = service::sign(&h.state, &me, req).await.unwrap_err();
        assert_eq!(err.code(), "VALIDATION_FAILED");
        assert_eq!(err.field_errors()[0].field, field);
    }
    // image/jpg 归一成 jpeg；扩展名不必与类型一致（真实类型由嗅探把关）。
    service::sign(&h.state, &me, request("a.png", "IMAGE/JPG", 10))
        .await
        .unwrap();
    let err = service::sign(
        &h.state,
        &principal(4, false),
        request("a.png", "image/png", 10),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "EXAM_REQUIRED");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn gallery_pages_and_removal_only_unlinks(db: PgPool) {
    let Some(h) = harness(db).await else { return };
    let mut uploaded = Vec::new();
    for tag in 10..15 {
        uploaded.push(h.upload(1, &png(tag)).await.id.get());
    }
    uploaded.reverse();
    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = service::gallery(
            &h.state,
            &principal(1, true),
            GalleryQuery {
                cursor,
                limit: Some(2),
            },
        )
        .await
        .unwrap();
        seen.extend(page.items.iter().map(|m| m.id.get()));
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(seen, uploaded);

    let target = uploaded[0];
    let err = service::remove(&h.state, &principal(2, true), target)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND", "别人的文件");
    service::remove(&h.state, &principal(1, true), target)
        .await
        .unwrap();
    assert_eq!(
        service::remove(&h.state, &principal(1, true), target)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    assert!(!h.gallery(1).await.contains(&target));
    // 文件本身还在（D-078 方案 A）。
    assert_eq!(
        h.state
            .views(&mut h.db.acquire().await.unwrap(), &[target])
            .await
            .unwrap()
            .len(),
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_completion_of_one_ticket_creates_one_file(db: PgPool) {
    let Some(h) = harness(db).await else { return };
    let bytes = png(20);
    let ticket = h.sign(1, &bytes, None).await;
    h.put(&ticket, &bytes).await;
    let (a, b) = tokio::join!(h.complete(1, &ticket), h.complete(1, &ticket));
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a.1.id, b.1.id);
    assert!(!(a.0 && b.0), "最多一个是新建");
    let files = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM media_files"#)
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(files, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn without_storage_uploads_are_unavailable(db: PgPool) {
    let state = MediaState::new(
        db,
        Arc::new(Snowflake::new(1).unwrap()),
        &StorageConfig::default(),
        b"x",
    );
    let err = service::sign(
        &state,
        &principal(1, true),
        SignRequest {
            filename: "a.png".into(),
            mime_type: "image/png".into(),
            size: 1,
            content_hash: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "SERVICE_UNAVAILABLE");
}

fn real_png() -> Vec<u8> {
    let img = image::RgbaImage::from_fn(200, 120, |x, y| {
        image::Rgba([(x % 255) as u8, (y % 255) as u8, 90, 255])
    });
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .unwrap();
    out
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn real_images_get_a_thumbhash(db: PgPool) {
    let Some(h) = harness(db).await else { return };
    let media = h.upload(1, &real_png()).await;
    assert!(media.thumbhash.is_some(), "真实图片算出 ThumbHash");
    // 只有文件头的「图片」解码失败：照样上传成功，只是没有占位。
    let fake = h.upload(1, &png(30)).await;
    assert!(fake.thumbhash.is_none());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_staging_objects_are_swept(db: PgPool) {
    let Some(h) = harness(db).await else { return };
    let bytes = png(31);
    let ticket = h.sign(1, &bytes, None).await;
    h.put(&ticket, &bytes).await;
    let claims = ik_media::token::TokenKey::derive(b"test-secret")
        .verify(ticket.upload_id.as_deref().unwrap())
        .unwrap();
    // 还没过保留期：不删。
    assert_eq!(h.state.sweep_staging().await.unwrap(), 0);
    assert!(h.storage.head(&claims.key).await.is_ok());
    // 截止时间放到将来，模拟已经过期。
    let deleted = h
        .state
        .delete_staging_before(chrono::Utc::now() + chrono::Duration::minutes(1))
        .await
        .unwrap();
    assert_eq!(deleted, 1);
    assert!(matches!(
        h.storage.head(&claims.key).await,
        Err(ik_media::storage::StorageError::NotFound)
    ));
    // 发布后的对象不在暂存前缀下，不受影响。
    let kept = h.upload(1, &png(32)).await;
    h.state
        .delete_staging_before(chrono::Utc::now() + chrono::Duration::minutes(1))
        .await
        .unwrap();
    let key = kept.url.strip_prefix(&h.bucket_url).unwrap();
    assert!(h.storage.head(key).await.is_ok());
}
