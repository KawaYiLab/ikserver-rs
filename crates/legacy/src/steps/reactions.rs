//! 互动与社交：点赞（帖子 / 评论）、收藏、投币、阅读记录、关注、拉黑；最后按导入的行重算计数。
//!
//! 旧表的关系都在 `*_lnk` 链接表里；指向帖子的一律按文档换成帖子 id（草稿行和发布行都可能被指向）。
//! 同一对象重复的记录只留最早的一条；对象或账号已不存在的记录不导入。

use sqlx::{PgConnection, Postgres, Transaction};

use crate::import::{ImportOptions, ImportReport};

pub async fn run(
    tx: &mut Transaction<'_, Postgres>,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    report.add("post_likes", post_likes(tx, options).await?);
    report.add("comment_likes", comment_likes(tx, options).await?);
    report.add("post_favorites", favorites(tx, options).await?);
    report.add("post_denny_gifts", gifts(tx, options).await?);
    report.add("post_reads", reads(tx, options).await?);
    report.add("follows", follows(tx, options).await?);
    report.add("user_blocks", blocks(tx, options).await?);
    report.add("posts.counters", post_counters(tx).await?);
    report.add("comments.like_count", comment_counters(tx).await?);
    Ok(())
}

/// 旧的点赞同时可能挂着帖子与评论：有评论关系的算评论点赞。
async fn post_likes(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO post_likes (post_id, user_id, created_at)
           SELECT DISTINCT ON (pr.post_id, u.user_id) pr.post_id, u.user_id,
                  coalesce(k.created_at AT TIME ZONE $1, $2)
           FROM legacy.likes k
           JOIN legacy.likes_article_lnk a ON a.like_id = k.id
           JOIN legacy.ik_post_rows pr ON pr.row_id = a.article_id
           JOIN legacy.likes_user_lnk u ON u.like_id = k.id
           JOIN posts p ON p.id = pr.post_id
           JOIN users x ON x.id = u.user_id
           WHERE NOT EXISTS (SELECT 1 FROM legacy.likes_comment_lnk c WHERE c.like_id = k.id)
           ORDER BY pr.post_id, u.user_id, k.created_at, k.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

async fn comment_likes(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO comment_likes (comment_id, user_id, created_at)
           SELECT DISTINCT ON (c.comment_id, u.user_id) c.comment_id, u.user_id,
                  coalesce(k.created_at AT TIME ZONE $1, $2)
           FROM legacy.likes k
           JOIN legacy.likes_comment_lnk c ON c.like_id = k.id
           JOIN legacy.likes_user_lnk u ON u.like_id = k.id
           JOIN comments nc ON nc.id = c.comment_id
           JOIN users x ON x.id = u.user_id
           ORDER BY c.comment_id, u.user_id, k.created_at, k.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

async fn favorites(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO post_favorites (post_id, user_id, created_at)
           SELECT DISTINCT ON (pr.post_id, u.user_id) pr.post_id, u.user_id,
                  coalesce(f.created_at AT TIME ZONE $1, $2)
           FROM legacy.favorites f
           JOIN legacy.favorites_article_lnk a ON a.favorite_id = f.id
           JOIN legacy.ik_post_rows pr ON pr.row_id = a.article_id
           JOIN legacy.favorites_user_lnk u ON u.favorite_id = f.id
           JOIN posts p ON p.id = pr.post_id
           JOIN users x ON x.id = u.user_id
           ORDER BY pr.post_id, u.user_id, f.created_at, f.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 投币每次固定 1 枚（DENNY-004）；留言最多 100 字。
async fn gifts(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO post_denny_gifts (post_id, giver_id, amount, message, created_at)
           SELECT DISTINCT ON (pr.post_id, u.user_id) pr.post_id, u.user_id, 1,
                  left(coalesce(g.message, ''), 100),
                  coalesce(g.created_at AT TIME ZONE $1, $2)
           FROM legacy.denny_givens g
           JOIN legacy.denny_givens_article_lnk a ON a.denny_given_id = g.id
           JOIN legacy.ik_post_rows pr ON pr.row_id = a.article_id
           JOIN legacy.denny_givens_user_lnk u ON u.denny_given_id = g.id
           JOIN posts p ON p.id = pr.post_id
           JOIN users x ON x.id = u.user_id
           ORDER BY pr.post_id, u.user_id, g.created_at, g.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 阅读记录：有记录就算已读，`isRead` 不参与判定（READ-001）。
async fn reads(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO post_reads (user_id, post_id, created_at)
           SELECT DISTINCT ON (u.user_id, pr.post_id) u.user_id, pr.post_id,
                  coalesce(r.created_at AT TIME ZONE $1, $2)
           FROM legacy.article_reads r
           JOIN legacy.article_reads_article_lnk a ON a.article_read_id = r.id
           JOIN legacy.ik_post_rows pr ON pr.row_id = a.article_id
           JOIN legacy.article_reads_user_lnk u ON u.article_read_id = r.id
           JOIN posts p ON p.id = pr.post_id
           JOIN users x ON x.id = u.user_id
           ORDER BY u.user_id, pr.post_id, r.created_at, r.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 旧的关注对象是作者，换成作者的账号；关注自己的不导入。
async fn follows(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO follows (follower_id, followee_id, created_at)
           SELECT DISTINCT ON (fr.user_id, au.user_id) fr.user_id, au.user_id,
                  coalesce(f.created_at AT TIME ZONE $1, $2)
           FROM legacy.follows f
           JOIN legacy.follows_follower_lnk fr ON fr.follow_id = f.id
           JOIN legacy.follows_following_lnk fg ON fg.follow_id = f.id
           JOIN legacy.ik_author_users au ON au.author_id = fg.author_id
           JOIN users a ON a.id = fr.user_id
           JOIN users b ON b.id = au.user_id
           WHERE fr.user_id <> au.user_id
           ORDER BY fr.user_id, au.user_id, f.created_at, f.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

async fn blocks(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO user_blocks (blocker_id, blocked_id, created_at)
           SELECT DISTINCT ON (br.user_id, bd.user_id) br.user_id, bd.user_id,
                  coalesce(b.created_at AT TIME ZONE $1, $2)
           FROM legacy.user_blocks b
           JOIN legacy.user_blocks_blocker_lnk br ON br.user_block_id = b.id
           JOIN legacy.user_blocks_blocked_user_lnk bd ON bd.user_block_id = b.id
           JOIN users x ON x.id = br.user_id
           JOIN users y ON y.id = bd.user_id
           WHERE br.user_id <> bd.user_id
           ORDER BY br.user_id, bd.user_id, b.created_at, b.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 计数按导入的行重算（旧计数可能漂移）：评论数含回复与被隐藏的评论（只有删除才减，见 comments）。
async fn post_counters(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"UPDATE posts p SET
               like_count = (SELECT count(*) FROM post_likes l WHERE l.post_id = p.id)::int,
               favorite_count = (SELECT count(*) FROM post_favorites f WHERE f.post_id = p.id)::int,
               denny_count = (SELECT count(*) FROM post_denny_gifts g WHERE g.post_id = p.id)::int,
               comment_count = (SELECT count(*) FROM comments c WHERE c.post_id = p.id)::int"#
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

async fn comment_counters(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"UPDATE comments c SET
               like_count = (SELECT count(*) FROM comment_likes l WHERE l.comment_id = c.id)::int"#
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}
