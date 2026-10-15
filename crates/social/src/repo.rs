//! SQL。只被本 crate 的 service / interop 调用。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

/// 串行化同一对用户之间的关注 / 拉黑写入（与参数顺序无关）。
///
/// 否则「A 关注 B」与「B 拉黑 A」并发时，关注事务看不到未提交的拉黑、拉黑事务也看不到
/// 未提交的关注，两边都提交后就留下一条拉黑双方之间的关注。哈希碰撞只会让无关的两对
/// 用户偶尔排队，不影响正确性。
pub(crate) async fn lock_pair(conn: &mut PgConnection, a: i64, b: i64) -> sqlx::Result<()> {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended('ik:relation:' || $1::bigint || ':' || $2::bigint, 0))",
        lo,
        hi,
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- 关注 ----------

pub(crate) async fn is_following(
    conn: &mut PgConnection,
    follower: i64,
    followee: i64,
) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM follows WHERE follower_id = $1 AND followee_id = $2
           ) AS "exists!""#,
        follower,
        followee,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn insert_follow(
    conn: &mut PgConnection,
    follower: i64,
    followee: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO follows (follower_id, followee_id, created_at) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
        follower,
        followee,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn delete_follow(
    conn: &mut PgConnection,
    follower: i64,
    followee: i64,
) -> sqlx::Result<()> {
    sqlx::query!(
        "DELETE FROM follows WHERE follower_id = $1 AND followee_id = $2",
        follower,
        followee,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 拉黑时双向删除关注（BLOCK-008）。
pub(crate) async fn delete_follows_between(
    conn: &mut PgConnection,
    a: i64,
    b: i64,
) -> sqlx::Result<()> {
    sqlx::query!(
        "DELETE FROM follows
         WHERE (follower_id = $1 AND followee_id = $2) OR (follower_id = $2 AND followee_id = $1)",
        a,
        b,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn follower_count(conn: &mut PgConnection, user: i64) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM follows WHERE followee_id = $1"#,
        user,
    )
    .fetch_one(conn)
    .await
}

pub(crate) struct RelationRow {
    pub user_id: i64,
    pub created_at: DateTime<Utc>,
}

/// 我关注的人，新关注的在前。
pub(crate) async fn following_page(
    conn: &mut PgConnection,
    follower: i64,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<RelationRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        RelationRow,
        "SELECT followee_id AS user_id, created_at FROM follows
         WHERE follower_id = $1
           AND ($2::timestamptz IS NULL OR (created_at, followee_id) < ($2, $3::bigint))
         ORDER BY created_at DESC, followee_id DESC
         LIMIT $4",
        follower,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn followers_without_blocks(
    conn: &mut PgConnection,
    user: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT f.follower_id FROM follows f
         WHERE f.followee_id = $1
           AND NOT EXISTS (
               SELECT 1 FROM user_blocks b
               WHERE (b.blocker_id = $1 AND b.blocked_id = f.follower_id)
                  OR (b.blocker_id = f.follower_id AND b.blocked_id = $1))
         ORDER BY f.follower_id",
        user,
    )
    .fetch_all(conn)
    .await
}

/// `targets` 中我关注了的人。
pub(crate) async fn following_among(
    conn: &mut PgConnection,
    follower: i64,
    targets: &[i64],
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT followee_id FROM follows WHERE follower_id = $1 AND followee_id = ANY($2)",
        follower,
        targets,
    )
    .fetch_all(conn)
    .await
}

// ---------- 拉黑 ----------

pub(crate) async fn insert_block(
    conn: &mut PgConnection,
    blocker: i64,
    blocked: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO user_blocks (blocker_id, blocked_id, created_at) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
        blocker,
        blocked,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn delete_block(
    conn: &mut PgConnection,
    blocker: i64,
    blocked: i64,
) -> sqlx::Result<()> {
    sqlx::query!(
        "DELETE FROM user_blocks WHERE blocker_id = $1 AND blocked_id = $2",
        blocker,
        blocked,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 两人之间任一方向存在拉黑。
pub(crate) async fn any_block(conn: &mut PgConnection, a: i64, b: i64) -> sqlx::Result<bool> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM user_blocks
               WHERE (blocker_id = $1 AND blocked_id = $2) OR (blocker_id = $2 AND blocked_id = $1)
           ) AS "exists!""#,
        a,
        b,
    )
    .fetch_one(conn)
    .await
}

/// 「我拉黑的人 ∪ 拉黑我的人」。
pub(crate) async fn related_by_blocks(
    conn: &mut PgConnection,
    user: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        r#"SELECT blocked_id AS "id!" FROM user_blocks WHERE blocker_id = $1
           UNION
           SELECT blocker_id FROM user_blocks WHERE blocked_id = $1"#,
        user,
    )
    .fetch_all(conn)
    .await
}

/// 我拉黑的人，新拉黑的在前。
pub(crate) async fn blocks_page(
    conn: &mut PgConnection,
    blocker: i64,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<RelationRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        RelationRow,
        "SELECT blocked_id AS user_id, created_at FROM user_blocks
         WHERE blocker_id = $1
           AND ($2::timestamptz IS NULL OR (created_at, blocked_id) < ($2, $3::bigint))
         ORDER BY created_at DESC, blocked_id DESC
         LIMIT $4",
        blocker,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// `targets` 中我拉黑了的人（只有这一个方向，D-053）。
pub(crate) async fn blocking_among(
    conn: &mut PgConnection,
    blocker: i64,
    targets: &[i64],
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT blocked_id FROM user_blocks WHERE blocker_id = $1 AND blocked_id = ANY($2)",
        blocker,
        targets,
    )
    .fetch_all(conn)
    .await
}
