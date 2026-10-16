//! 媒体：media_files（只导入摘要读取成功的文件）、media_uploaders、账号头像。

use sqlx::{PgConnection, Postgres, Transaction};

use crate::import::{ImportOptions, ImportReport};

pub async fn run(
    tx: &mut Transaction<'_, Postgres>,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    report.add("media_files", files(tx, options).await?);
    report.add("media_uploaders", uploaders(tx).await?);
    report.add("users.avatar", avatars(tx).await?);
    Ok(())
}

/// 收据（`receipt`）只用于新上传的重试幂等，旧文件给一个不会与新收据撞上的占位值。
async fn files(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO media_files (id, object_key, mime, size_bytes, width, height, sha256, receipt,
                                    name, created_at, thumbhash)
           SELECT f.id, d.object_key, d.mime, d.size_bytes,
                  CASE WHEN f.width > 0 THEN f.width END,
                  CASE WHEN f.height > 0 THEN f.height END,
                  d.sha256, 'legacy:' || f.id, left(coalesce(f.name, ''), 255),
                  coalesce(f.created_at AT TIME ZONE $1, $2), d.thumbhash
           FROM legacy.files f
           JOIN legacy.ik_media_digests d ON d.file_id = f.id AND d.error IS NULL
           ON CONFLICT (object_key) DO NOTHING"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 「我的图库」：旧系统记在文件的 uploaders 关系上。
async fn uploaders(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO media_uploaders (media_id, user_id, created_at)
           SELECT DISTINCT ON (m.id, u.id) m.id, u.id, m.created_at
           FROM legacy.files_uploaders_lnk l
           JOIN media_files m ON m.id = l.file_id
           JOIN users u ON u.id = l.user_id
           ON CONFLICT DO NOTHING"#
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 自定义头像：旧系统挂在作者的 avatar 字段上。
async fn avatars(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"UPDATE users u SET avatar_media_id = x.file_id
           FROM (
               SELECT DISTINCT ON (au.user_id) au.user_id, r.file_id
               FROM legacy.ik_file_refs r
               JOIN legacy.ik_author_users au ON au.author_id = r.related_id
               JOIN media_files m ON m.id = r.file_id
               WHERE r.related_type = 'api::author.author' AND r.field = 'avatar'
               ORDER BY au.user_id, r."order", r.file_id
           ) x
           WHERE u.id = x.user_id"#
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}
