//! 媒体摘要预取：需要 `IK_TEST_S3_URL`（moto 或 MinIO）；没有设置时直接通过。

mod common;

use std::sync::atomic::{AtomicU64, Ordering};

use ik_kernel::config::{Secret, Storage as StorageConfig};
use ik_legacy::media::{MediaOptions, fetch_digests};
use ik_media::storage::Storage;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

fn storage() -> Option<Storage> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let url = std::env::var("IK_TEST_S3_URL").ok()?;
    Some(Storage::new(&StorageConfig {
        endpoint: url,
        region: "us-east-1".into(),
        bucket: format!(
            "ik-legacy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ),
        access_key: Secret::new(
            std::env::var("IK_TEST_S3_ACCESS_KEY").unwrap_or_else(|_| "test".into()),
        ),
        secret_key: Secret::new(
            std::env::var("IK_TEST_S3_SECRET_KEY").unwrap_or_else(|_| "testsecret".into()),
        ),
        ..StorageConfig::default()
    }))
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn digests_are_read_from_storage_and_resumable(db: PgPool) {
    let Some(storage) = storage() else {
        eprintln!("未设置 IK_TEST_S3_URL，跳过对象存储用例");
        return;
    };
    storage.ensure_bucket().await.unwrap();
    let png = [b"\x89PNG\r\n\x1a\n".as_slice(), b"not really pixels"].concat();
    storage
        .put("uploads/a.png", png.clone(), "image/png")
        .await
        .unwrap();
    storage
        .put("uploads/b.txt", b"hello".to_vec(), "text/plain")
        .await
        .unwrap();

    common::setup(&db).await;
    sqlx::query!(
        "INSERT INTO legacy.files (id, name, url) VALUES
           (1, 'a.png', 'https://im.tiwat.cn/uploads/a.png'),
           (2, 'b.txt', 'https://im.tiwat.cn/uploads/b.txt'),
           (3, 'c.png', 'https://im.tiwat.cn/uploads/missing.png'),
           (4, 'd.png', 'https://elsewhere.example/d.png')"
    )
    .execute(&db)
    .await
    .unwrap();

    let options = MediaOptions {
        url_prefixes: vec!["https://im.tiwat.cn/".into()],
        concurrency: 2,
        retry_failed: false,
    };
    let report = fetch_digests(&db, &storage, &options).await.unwrap();
    assert_eq!((report.fetched, report.failed), (1, 3));

    let rows = sqlx::query!(
        "SELECT file_id, object_key, sha256, mime, size_bytes, error
         FROM legacy.ik_media_digests ORDER BY file_id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let expected: String = Sha256::digest(&png)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(rows[0].object_key.as_deref(), Some("uploads/a.png"));
    assert_eq!(rows[0].sha256.as_deref(), Some(expected.as_str()));
    assert_eq!(rows[0].mime.as_deref(), Some("image/png"));
    assert_eq!(rows[0].size_bytes, Some(i64::try_from(png.len()).unwrap()));
    assert_eq!(rows[0].error, None);
    assert_eq!(rows[1].error.as_deref(), Some("不是支持的图片类型"));
    assert_eq!(rows[2].error.as_deref(), Some("对象不存在"));
    assert!(rows[3].error.as_deref().unwrap().contains("公开前缀"));

    // 再跑一次：成功的不再读取，失败的也不重试。
    let again = fetch_digests(&db, &storage, &options).await.unwrap();
    assert_eq!((again.fetched, again.failed), (0, 0));

    // 补上缺失的对象后重试失败项。
    storage
        .put("uploads/missing.png", png.clone(), "image/png")
        .await
        .unwrap();
    let retry = MediaOptions {
        retry_failed: true,
        ..options
    };
    let report = fetch_digests(&db, &storage, &retry).await.unwrap();
    assert_eq!((report.fetched, report.failed), (1, 2));
}
