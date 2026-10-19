//! `story_corpora` 的读写。表归本 crate 所有。

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgExecutor};

pub(crate) struct CorpusRow {
    pub id: i64,
    pub source_version: String,
    pub language: String,
    pub docs: i32,
    pub lines: i32,
    pub speaker_coverage: f32,
    pub bundle_bytes: i64,
    pub bundle_sha256: String,
    pub active: bool,
    pub uploaded_by: i64,
    pub created_at: DateTime<Utc>,
}

pub(crate) struct NewCorpus<'a> {
    pub id: i64,
    pub source_version: &'a str,
    pub language: &'a str,
    pub docs: i32,
    pub lines: i32,
    pub speaker_coverage: f32,
    pub bundle: &'a [u8],
    pub bundle_sha256: &'a str,
    pub uploaded_by: i64,
    pub now: DateTime<Utc>,
}

/// 串行化「换生效版本」：上传与回滚都先拿这把事务锁，再读写 active。
pub(crate) async fn lock(conn: &mut PgConnection) -> sqlx::Result<()> {
    sqlx::query!("SELECT pg_advisory_xact_lock(hashtextextended('ik:story_corpora', 0))")
        .execute(conn)
        .await?;
    Ok(())
}

/// 生效版本的 id 与语料包。
pub(crate) async fn active_bundle(db: impl PgExecutor<'_>) -> sqlx::Result<Option<(i64, Vec<u8>)>> {
    let row = sqlx::query!("SELECT id, bundle FROM story_corpora WHERE active")
        .fetch_optional(db)
        .await?;
    Ok(row.map(|r| (r.id, r.bundle)))
}

pub(crate) async fn active_id(db: impl PgExecutor<'_>) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!("SELECT id FROM story_corpora WHERE active")
        .fetch_optional(db)
        .await
}

/// 最近上传的同一个包（按内容哈希）。
pub(crate) async fn find_by_sha(
    conn: &mut PgConnection,
    sha256: &str,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "SELECT id FROM story_corpora WHERE bundle_sha256 = $1 ORDER BY created_at DESC, id DESC LIMIT 1",
        sha256
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn insert(conn: &mut PgConnection, c: &NewCorpus<'_>) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO story_corpora
             (id, source_version, language, docs, lines, speaker_coverage, bundle, bundle_sha256,
              active, uploaded_by, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, false, $9, $10)",
        c.id,
        c.source_version,
        c.language,
        c.docs,
        c.lines,
        c.speaker_coverage,
        c.bundle,
        c.bundle_sha256,
        c.uploaded_by,
        c.now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 让 `id` 成为唯一的生效版本。返回 false 表示没有这一行。调用方须已持有 [`lock`]。
pub(crate) async fn activate(conn: &mut PgConnection, id: i64) -> sqlx::Result<bool> {
    let exists = sqlx::query_scalar!(
        "SELECT EXISTS (SELECT 1 FROM story_corpora WHERE id = $1)",
        id
    )
    .fetch_one(&mut *conn)
    .await?
    .unwrap_or(false);
    if !exists {
        return Ok(false);
    }
    sqlx::query!(
        "UPDATE story_corpora SET active = false WHERE active AND id <> $1",
        id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!("UPDATE story_corpora SET active = true WHERE id = $1", id)
        .execute(&mut *conn)
        .await?;
    Ok(true)
}

/// 只保留最近 `keep` 个版本（生效版本总是保留）。返回删除的行数。
pub(crate) async fn prune(conn: &mut PgConnection, keep: i64) -> sqlx::Result<u64> {
    let done = sqlx::query!(
        "DELETE FROM story_corpora
          WHERE NOT active
            AND id NOT IN (SELECT id FROM story_corpora ORDER BY created_at DESC, id DESC LIMIT $1)",
        keep
    )
    .execute(conn)
    .await?;
    Ok(done.rows_affected())
}

pub(crate) async fn list(db: impl PgExecutor<'_>) -> sqlx::Result<Vec<CorpusRow>> {
    sqlx::query_as!(
        CorpusRow,
        r#"SELECT id, source_version, language, docs, lines, speaker_coverage,
                  octet_length(bundle)::bigint AS "bundle_bytes!", bundle_sha256, active, uploaded_by,
                  created_at
             FROM story_corpora
            ORDER BY created_at DESC, id DESC"#
    )
    .fetch_all(db)
    .await
}

pub(crate) async fn get(db: impl PgExecutor<'_>, id: i64) -> sqlx::Result<Option<CorpusRow>> {
    sqlx::query_as!(
        CorpusRow,
        r#"SELECT id, source_version, language, docs, lines, speaker_coverage,
                  octet_length(bundle)::bigint AS "bundle_bytes!", bundle_sha256, active, uploaded_by,
                  created_at
             FROM story_corpora
            WHERE id = $1"#,
        id
    )
    .fetch_optional(db)
    .await
}
