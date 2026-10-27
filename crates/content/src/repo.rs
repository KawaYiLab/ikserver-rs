//! SQL。只被本 crate 的 service 调用。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

// ---------- 分区 ----------

pub(crate) struct CategoryRow {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub is_active: bool,
    pub admin_only: bool,
    pub allow_comment_accept: bool,
}

pub(crate) async fn active_categories(conn: &mut PgConnection) -> sqlx::Result<Vec<CategoryRow>> {
    sqlx::query_as!(
        CategoryRow,
        "SELECT id, slug, name, description, is_active, admin_only, allow_comment_accept
         FROM categories WHERE is_active ORDER BY sort_order, id",
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn category_by_id(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<CategoryRow>> {
    sqlx::query_as!(
        CategoryRow,
        "SELECT id, slug, name, description, is_active, admin_only, allow_comment_accept
         FROM categories WHERE id = $1",
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn category_by_slug(
    conn: &mut PgConnection,
    slug: &str,
) -> sqlx::Result<Option<CategoryRow>> {
    sqlx::query_as!(
        CategoryRow,
        "SELECT id, slug, name, description, is_active, admin_only, allow_comment_accept
         FROM categories WHERE slug = $1",
        slug,
    )
    .fetch_optional(conn)
    .await
}

// ---------- 标签 ----------

pub(crate) struct TagRow {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub is_blocked: bool,
}

pub(crate) async fn tags_by_slugs(
    conn: &mut PgConnection,
    slugs: &[String],
) -> sqlx::Result<Vec<TagRow>> {
    sqlx::query_as!(
        TagRow,
        "SELECT id, name, slug, is_blocked FROM tags WHERE slug = ANY($1)",
        slugs,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn tags_by_ids(conn: &mut PgConnection, ids: &[i64]) -> sqlx::Result<Vec<TagRow>> {
    sqlx::query_as!(
        TagRow,
        "SELECT id, name, slug, is_blocked FROM tags WHERE id = ANY($1)",
        ids,
    )
    .fetch_all(conn)
    .await
}

/// 并发创建同一 slug 时由唯一约束收敛：输家什么也不插，随后重查即可拿到赢家的行（POST-026）。
pub(crate) async fn insert_tags(
    conn: &mut PgConnection,
    ids: &[i64],
    names: &[String],
    slugs: &[String],
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO tags (id, name, slug)
         SELECT * FROM unnest($1::bigint[], $2::text[], $3::text[])
         ON CONFLICT (slug) DO NOTHING",
        ids,
        names,
        slugs,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn suggest_tags(
    conn: &mut PgConnection,
    like_pattern: &str,
    limit: i64,
) -> sqlx::Result<Vec<TagRow>> {
    sqlx::query_as!(
        TagRow,
        r"SELECT id, name, slug, is_blocked FROM tags
          WHERE NOT is_blocked AND slug LIKE $1 ESCAPE '\'
          ORDER BY slug LIMIT $2",
        like_pattern,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn tag_by_slug(
    conn: &mut PgConnection,
    slug: &str,
) -> sqlx::Result<Option<TagRow>> {
    sqlx::query_as!(
        TagRow,
        "SELECT id, name, slug, is_blocked FROM tags WHERE slug = $1",
        slug,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct PostTagRow {
    pub post_id: i64,
    pub name: String,
    pub slug: String,
}

/// 多个帖子的可见标签（排除被停用的标签，POST-029），按作者给出的顺序。
pub(crate) async fn tags_for_posts(
    conn: &mut PgConnection,
    post_ids: &[i64],
) -> sqlx::Result<Vec<PostTagRow>> {
    sqlx::query_as!(
        PostTagRow,
        "SELECT pt.post_id, t.name, t.slug
         FROM post_tags pt JOIN tags t ON t.id = pt.tag_id
         WHERE pt.post_id = ANY($1) AND NOT t.is_blocked
         ORDER BY pt.post_id, pt.position",
        post_ids,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn post_tag_ids(conn: &mut PgConnection, post_id: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT tag_id FROM post_tags WHERE post_id = $1 ORDER BY position",
        post_id,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn set_post_tags(
    conn: &mut PgConnection,
    post_id: i64,
    tag_ids: &[i64],
) -> sqlx::Result<()> {
    sqlx::query!("DELETE FROM post_tags WHERE post_id = $1", post_id)
        .execute(&mut *conn)
        .await?;
    sqlx::query!(
        "INSERT INTO post_tags (post_id, tag_id, position)
         SELECT $1, tag_id, (ord - 1)::smallint
         FROM unnest($2::bigint[]) WITH ORDINALITY AS t(tag_id, ord)",
        post_id,
        tag_ids,
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- 帖子 ----------

pub(crate) struct PostRow {
    pub id: i64,
    pub author_id: i64,
    pub category_id: Option<i64>,
    pub status: String,
    pub title: String,
    pub body: String,
    pub editor_state: Option<serde_json::Value>,
    pub external_videos: serde_json::Value,
    pub cover_ids: Vec<i64>,
    pub is_anonymous: bool,
    pub anonymous_seed: Option<String>,
    pub revision: i32,
    pub first_published_at: Option<DateTime<Utc>>,
    pub published_at: Option<DateTime<Utc>>,
    pub edited_at: Option<DateTime<Utc>>,
    pub bumped_at: Option<DateTime<Utc>>,
    pub pinned_at: Option<DateTime<Utc>>,
    pub solved_at: Option<DateTime<Utc>>,
    pub hidden_at: Option<DateTime<Utc>>,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
    pub favorite_count: i32,
    pub denny_count: i32,
    pub updated_at: DateTime<Utc>,
}

pub(crate) async fn get_post(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<PostRow>> {
    sqlx::query_as!(
        PostRow,
        "SELECT id, author_id, category_id, status, title, body, editor_state, external_videos, cover_ids,
                is_anonymous, anonymous_seed, revision, first_published_at, published_at, edited_at,
                bumped_at, pinned_at, solved_at, hidden_at, view_count, like_count, comment_count,
                favorite_count, denny_count, updated_at
         FROM posts WHERE id = $1",
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 锁住帖子行，串行化同一帖子的编辑、发布、下线、删除。
pub(crate) async fn lock_post(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<PostRow>> {
    sqlx::query_as!(
        PostRow,
        "SELECT id, author_id, category_id, status, title, body, editor_state, external_videos, cover_ids,
                is_anonymous, anonymous_seed, revision, first_published_at, published_at, edited_at,
                bumped_at, pinned_at, solved_at, hidden_at, view_count, like_count, comment_count,
                favorite_count, denny_count, updated_at
         FROM posts WHERE id = $1 FOR UPDATE",
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 校验后的工作副本。
pub(crate) struct Content {
    pub title: String,
    pub body: String,
    pub editor_state: Option<serde_json::Value>,
    pub external_videos: serde_json::Value,
    pub cover_ids: Vec<i64>,
    pub category_id: Option<i64>,
    pub tag_ids: Vec<i64>,
    pub is_anonymous: bool,
}

pub(crate) async fn insert_post(
    conn: &mut PgConnection,
    id: i64,
    author_id: i64,
    content: &Content,
    anonymous_seed: Option<&str>,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO posts (id, author_id, category_id, title, body, editor_state, external_videos,
                            is_anonymous, anonymous_seed, created_at, updated_at, cover_ids)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10, $11)",
        id,
        author_id,
        content.category_id,
        content.title,
        content.body,
        content.editor_state,
        content.external_videos,
        content.is_anonymous,
        anonymous_seed,
        now,
        &content.cover_ids,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 改写 posts 行上的内容（草稿状态的帖子，或发布时写回待发布修改）。
pub(crate) async fn update_post_content(
    conn: &mut PgConnection,
    id: i64,
    content: &Content,
    anonymous_seed: Option<&str>,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET category_id = $2, title = $3, body = $4, editor_state = $5,
                external_videos = $6, is_anonymous = $7, anonymous_seed = $8,
                revision = revision + 1, updated_at = $9, cover_ids = $10
         WHERE id = $1",
        id,
        content.category_id,
        content.title,
        content.body,
        content.editor_state,
        content.external_videos,
        content.is_anonymous,
        anonymous_seed,
        now,
        &content.cover_ids,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn bump_revision(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET revision = revision + 1, updated_at = $2 WHERE id = $1",
        id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 发布：`bump` 为真时刷新信息流排序（首次发布或下线后重新发布）。
pub(crate) async fn mark_published(
    conn: &mut PgConnection,
    id: i64,
    by: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET status = 'published', published_at = $3,
                first_published_at = COALESCE(first_published_at, $3),
                bumped_at = $3, last_bumped_by = $2,
                revision = revision + 1, updated_at = $3
         WHERE id = $1",
        id,
        by,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 编辑重发：只记编辑时间，不顶帖（POST-100）。
pub(crate) async fn mark_edited(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET edited_at = $2, updated_at = $2 WHERE id = $1",
        id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 下线：回到草稿，同时取消置顶。点赞、评论等关系都挂在同一行上，不受影响。
pub(crate) async fn mark_unpublished(
    conn: &mut PgConnection,
    id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET status = 'draft', published_at = NULL, pinned_at = NULL, pinned_by = NULL,
                revision = revision + 1, updated_at = $2
         WHERE id = $1",
        id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn delete_post(conn: &mut PgConnection, id: i64) -> sqlx::Result<()> {
    sqlx::query!("DELETE FROM posts WHERE id = $1", id)
        .execute(conn)
        .await?;
    Ok(())
}

/// 公开帖子浏览量 +1，返回新值；帖子不存在或不公开时返回 None。
pub(crate) async fn increment_views(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar!(
        "UPDATE posts SET view_count = view_count + 1
         WHERE id = $1 AND status = 'published' AND hidden_at IS NULL
         RETURNING view_count",
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 置顶（幂等：已置顶时保留原置顶时间）。只对公开帖子生效，返回是否命中。
pub(crate) async fn pin(
    conn: &mut PgConnection,
    id: i64,
    by: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE posts SET pinned_at = COALESCE(pinned_at, $3), pinned_by = COALESCE(pinned_by, $2)
         WHERE id = $1 AND status = 'published' AND hidden_at IS NULL",
        id,
        by,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn unpin(conn: &mut PgConnection, id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE posts SET pinned_at = NULL, pinned_by = NULL WHERE id = $1",
        id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

// ---------- 待发布修改 ----------

pub(crate) struct DraftRow {
    pub title: String,
    pub body: String,
    pub editor_state: Option<serde_json::Value>,
    pub external_videos: serde_json::Value,
    pub cover_ids: Vec<i64>,
    pub category_id: Option<i64>,
    pub tag_ids: Vec<i64>,
    pub updated_at: DateTime<Utc>,
}

pub(crate) async fn pending_draft(
    conn: &mut PgConnection,
    post_id: i64,
) -> sqlx::Result<Option<DraftRow>> {
    sqlx::query_as!(
        DraftRow,
        "SELECT title, body, editor_state, external_videos, cover_ids, category_id, tag_ids,
                updated_at
         FROM post_drafts WHERE post_id = $1",
        post_id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn upsert_draft(
    conn: &mut PgConnection,
    post_id: i64,
    content: &Content,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO post_drafts (post_id, title, body, editor_state, external_videos, category_id,
                                  tag_ids, updated_at, cover_ids)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (post_id) DO UPDATE SET
             title = EXCLUDED.title, body = EXCLUDED.body, editor_state = EXCLUDED.editor_state,
             external_videos = EXCLUDED.external_videos, category_id = EXCLUDED.category_id,
             tag_ids = EXCLUDED.tag_ids, updated_at = EXCLUDED.updated_at,
             cover_ids = EXCLUDED.cover_ids",
        post_id,
        content.title,
        content.body,
        content.editor_state,
        content.external_videos,
        content.category_id,
        &content.tag_ids,
        now,
        &content.cover_ids,
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn delete_draft(conn: &mut PgConnection, post_id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query!("DELETE FROM post_drafts WHERE post_id = $1", post_id)
        .execute(conn)
        .await?;
    Ok(result.rows_affected() > 0)
}

// ---------- 信息流 ----------

pub(crate) struct CardRow {
    pub id: i64,
    pub author_id: i64,
    pub title: String,
    pub excerpt_source: String,
    pub is_anonymous: bool,
    pub anonymous_seed: Option<String>,
    pub pinned_at: Option<DateTime<Utc>>,
    pub solved_at: Option<DateTime<Utc>>,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
    pub published_at: DateTime<Utc>,
    pub bumped_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub last_bumped_by: Option<i64>,
    /// 第一张封面（卡片只展示一张，OPS-004）。
    pub cover_id: Option<i64>,
    pub category_id: Option<i64>,
    pub category_slug: Option<String>,
    pub category_name: Option<String>,
}

pub(crate) struct VideoRow {
    pub id: i64,
    pub videos: serde_json::Value,
}

/// 这些帖子里嵌了视频的（卡片展示视频摘要）。
pub(crate) async fn videos_for_posts(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<VideoRow>> {
    sqlx::query_as!(
        VideoRow,
        r#"SELECT id, external_videos AS videos FROM posts
           WHERE id = ANY($1) AND external_videos <> '[]'::jsonb"#,
        ids,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct FeedFilter {
    pub category_id: Option<i64>,
    pub tag_id: Option<i64>,
    pub exclude_pinned: bool,
    pub after: Option<(DateTime<Utc>, i64)>,
    pub limit: i64,
    /// 与读者有拉黑关系的作者；他们的匿名帖不受影响（COMMON-008）。
    pub excluded_authors: Vec<i64>,
    /// 已经在本页别处展示过的帖子（置顶、热门），翻页时也一直排除（FEED-143）。
    pub exclude_ids: Vec<i64>,
    /// 关注流：只看这些作者的**非匿名**帖子（FEED-130）。
    pub author_ids: Option<Vec<i64>>,
    /// 未看流：去掉该用户读过的帖子（UNRD-003，NOT EXISTS 按主键探测）。
    pub unread_for: Option<i64>,
}

/// 公开帖子按 (bumped_at, id) 倒序。可选按分区、标签筛选，以及关注流、未看流的附加条件。
pub(crate) async fn feed(
    conn: &mut PgConnection,
    filter: &FeedFilter,
) -> sqlx::Result<Vec<CardRow>> {
    let (after_at, after_id) = filter.after.unzip();
    sqlx::query_as!(
        CardRow,
        r#"SELECT p.id, p.author_id, p.title, left(p.body, 400) AS "excerpt_source!",
                  p.is_anonymous, p.anonymous_seed, p.pinned_at, p.solved_at,
                  p.view_count, p.like_count, p.comment_count,
                  p.published_at AS "published_at!", p.bumped_at AS "bumped_at!",
                  p.edited_at, p.last_bumped_by, p.cover_ids[1] AS "cover_id?",
                  c.id AS "category_id?", c.slug AS "category_slug?", c.name AS "category_name?"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.status = 'published' AND p.hidden_at IS NULL
             AND ($1::bigint IS NULL OR p.category_id = $1)
             AND ($2::bigint IS NULL OR EXISTS (
                     SELECT 1 FROM post_tags pt WHERE pt.post_id = p.id AND pt.tag_id = $2))
             AND (NOT $3 OR p.pinned_at IS NULL)
             AND ($4::timestamptz IS NULL OR (p.bumped_at, p.id) < ($4, $5::bigint))
             AND (p.is_anonymous OR NOT (p.author_id = ANY($7)))
             AND NOT (p.id = ANY($8))
             AND ($9::bigint[] IS NULL OR (NOT p.is_anonymous AND p.author_id = ANY($9)))
             AND ($10::bigint IS NULL OR NOT EXISTS (
                     SELECT 1 FROM post_reads r WHERE r.user_id = $10 AND r.post_id = p.id))
           ORDER BY p.bumped_at DESC, p.id DESC
           LIMIT $6"#,
        filter.category_id,
        filter.tag_id,
        filter.exclude_pinned,
        after_at,
        after_id,
        filter.limit,
        &filter.excluded_authors,
        &filter.exclude_ids,
        filter.author_ids.as_deref(),
        filter.unread_for,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn pinned_cards(
    conn: &mut PgConnection,
    excluded_authors: &[i64],
    limit: i64,
) -> sqlx::Result<Vec<CardRow>> {
    sqlx::query_as!(
        CardRow,
        r#"SELECT p.id, p.author_id, p.title, left(p.body, 400) AS "excerpt_source!",
                  p.is_anonymous, p.anonymous_seed, p.pinned_at, p.solved_at,
                  p.view_count, p.like_count, p.comment_count,
                  p.published_at AS "published_at!", p.bumped_at AS "bumped_at!",
                  p.edited_at, p.last_bumped_by, p.cover_ids[1] AS "cover_id?",
                  c.id AS "category_id?", c.slug AS "category_slug?", c.name AS "category_name?"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.pinned_at IS NOT NULL AND p.status = 'published' AND p.hidden_at IS NULL
             AND (p.is_anonymous OR NOT (p.author_id = ANY($2)))
           ORDER BY p.pinned_at DESC, p.id DESC
           LIMIT $1"#,
        limit,
        excluded_authors,
    )
    .fetch_all(conn)
    .await
}

// ---------- 我的帖子 ----------

pub(crate) struct MyPostRow {
    pub id: i64,
    pub status: String,
    pub title: String,
    pub is_anonymous: bool,
    pub has_pending_changes: bool,
    pub hidden_at: Option<DateTime<Utc>>,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
    pub published_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// 作者自己的帖子，按最近修改倒序。已发布的匿名帖不列出（POST-065/067）。
pub(crate) async fn my_posts(
    conn: &mut PgConnection,
    author_id: i64,
    status: Option<&str>,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<MyPostRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        MyPostRow,
        r#"SELECT p.id, p.status, p.title, p.is_anonymous,
                  EXISTS (SELECT 1 FROM post_drafts d WHERE d.post_id = p.id) AS "has_pending_changes!",
                  p.hidden_at, p.view_count, p.like_count, p.comment_count, p.published_at,
                  p.updated_at
           FROM posts p
           WHERE p.author_id = $1
             AND NOT (p.is_anonymous AND p.status = 'published')
             AND ($2::text IS NULL OR p.status = $2)
             AND ($3::timestamptz IS NULL OR (p.updated_at, p.id) < ($3, $4::bigint))
           ORDER BY p.updated_at DESC, p.id DESC
           LIMIT $5"#,
        author_id,
        status,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct NewCategory<'a> {
    pub id: i64,
    pub slug: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub sort_order: i32,
    pub admin_only: bool,
    pub allow_comment_accept: bool,
}

pub(crate) async fn insert_category(
    conn: &mut PgConnection,
    c: &NewCategory<'_>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "INSERT INTO categories (id, slug, name, description, sort_order, admin_only, allow_comment_accept)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        c.id,
        c.slug,
        c.name,
        c.description,
        c.sort_order,
        c.admin_only,
        c.allow_comment_accept,
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- 供评论域在同一事务里调用 ----------

pub(crate) struct PostRefRow {
    pub id: i64,
    pub author_id: i64,
    pub title: String,
    pub is_anonymous: bool,
    pub is_public: bool,
    pub allow_comment_accept: bool,
}

pub(crate) async fn post_ref(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<PostRefRow>> {
    sqlx::query_as!(
        PostRefRow,
        r#"SELECT p.id, p.author_id, p.title, p.is_anonymous,
                  (p.status = 'published' AND p.hidden_at IS NULL) AS "is_public!",
                  COALESCE(c.allow_comment_accept, FALSE) AS "allow_comment_accept!"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.id = $1"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) async fn lock_post_ref(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<PostRefRow>> {
    sqlx::query_as!(
        PostRefRow,
        r#"SELECT p.id, p.author_id, p.title, p.is_anonymous,
                  (p.status = 'published' AND p.hidden_at IS NULL) AS "is_public!",
                  COALESCE(c.allow_comment_accept, FALSE) AS "allow_comment_accept!"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.id = $1
           FOR UPDATE OF p"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 新评论：评论数 +1 并顶帖（CMT-084/088）。
/// 返回分给这条评论的楼层号。
pub(crate) async fn comment_added(
    conn: &mut PgConnection,
    id: i64,
    by: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<i32> {
    sqlx::query_scalar!(
        "UPDATE posts SET comment_count = comment_count + 1, last_comment_floor = last_comment_floor + 1,
                          bumped_at = $3, last_bumped_by = $2
         WHERE id = $1
         RETURNING last_comment_floor",
        id,
        by,
        now,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn comments_removed(conn: &mut PgConnection, id: i64, n: i32) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET comment_count = GREATEST(comment_count - $2, 0) WHERE id = $1",
        id,
        n,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 投币：帖子丁尼数 +1，返回新值。
pub(crate) async fn increment_denny_count(conn: &mut PgConnection, id: i64) -> sqlx::Result<i32> {
    sqlx::query_scalar!(
        "UPDATE posts SET denny_count = denny_count + 1 WHERE id = $1 RETURNING denny_count",
        id,
    )
    .fetch_one(conn)
    .await
}

pub(crate) async fn mark_solved(
    conn: &mut PgConnection,
    id: i64,
    comment_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET solved_at = $3, accepted_comment_id = $2 WHERE id = $1",
        id,
        comment_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 只有帖子当前指向的正是这条评论时才清除（防止误清并发采纳的另一条，CMT-060）。
pub(crate) async fn clear_solved(
    conn: &mut PgConnection,
    id: i64,
    comment_id: i64,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET solved_at = NULL, accepted_comment_id = NULL
         WHERE id = $1 AND accepted_comment_id = $2",
        id,
        comment_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

// ---------- 点赞与收藏 ----------

/// 新增点赞，返回是否真的插入（已赞过时为 false）。
pub(crate) async fn insert_like(
    conn: &mut PgConnection,
    post_id: i64,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "INSERT INTO post_likes (post_id, user_id, created_at) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
        post_id,
        user_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn delete_like(
    conn: &mut PgConnection,
    post_id: i64,
    user_id: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "DELETE FROM post_likes WHERE post_id = $1 AND user_id = $2",
        post_id,
        user_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn insert_favorite(
    conn: &mut PgConnection,
    post_id: i64,
    user_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "INSERT INTO post_favorites (post_id, user_id, created_at) VALUES ($1, $2, $3)
         ON CONFLICT DO NOTHING",
        post_id,
        user_id,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn delete_favorite(
    conn: &mut PgConnection,
    post_id: i64,
    user_id: i64,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "DELETE FROM post_favorites WHERE post_id = $1 AND user_id = $2",
        post_id,
        user_id,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) struct ReactionCounts {
    pub like_count: i32,
    pub favorite_count: i32,
}

/// 调整点赞数、收藏数（单条原子 UPDATE，下限 0，LIKE-003），返回调整后的值。
/// 两个增量都为 0 时就是读取当前值。
pub(crate) async fn add_reaction_counts(
    conn: &mut PgConnection,
    post_id: i64,
    like_delta: i32,
    favorite_delta: i32,
) -> sqlx::Result<Option<ReactionCounts>> {
    if like_delta == 0 && favorite_delta == 0 {
        return sqlx::query_as!(
            ReactionCounts,
            "SELECT like_count, favorite_count FROM posts WHERE id = $1",
            post_id,
        )
        .fetch_optional(conn)
        .await;
    }
    sqlx::query_as!(
        ReactionCounts,
        "UPDATE posts SET like_count = GREATEST(like_count + $2, 0),
                          favorite_count = GREATEST(favorite_count + $3, 0)
         WHERE id = $1
         RETURNING like_count, favorite_count",
        post_id,
        like_delta,
        favorite_delta,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct ViewerMarks {
    pub liked: bool,
    pub favorited: bool,
}

pub(crate) async fn viewer_marks(
    conn: &mut PgConnection,
    post_id: i64,
    user_id: i64,
) -> sqlx::Result<ViewerMarks> {
    sqlx::query_as!(
        ViewerMarks,
        r#"SELECT EXISTS (SELECT 1 FROM post_likes WHERE post_id = $1 AND user_id = $2) AS "liked!",
                  EXISTS (SELECT 1 FROM post_favorites WHERE post_id = $1 AND user_id = $2) AS "favorited!""#,
        post_id,
        user_id,
    )
    .fetch_one(conn)
    .await
}

/// 我的点赞 / 收藏列表里的一行：卡片字段 + 点赞（收藏）时间。
pub(crate) struct SavedCardRow {
    pub saved_at: DateTime<Utc>,
    pub id: i64,
    pub author_id: i64,
    pub title: String,
    pub excerpt_source: String,
    pub is_anonymous: bool,
    pub anonymous_seed: Option<String>,
    pub pinned_at: Option<DateTime<Utc>>,
    pub solved_at: Option<DateTime<Utc>>,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
    pub published_at: DateTime<Utc>,
    pub bumped_at: DateTime<Utc>,
    pub edited_at: Option<DateTime<Utc>>,
    pub last_bumped_by: Option<i64>,
    /// 第一张封面（卡片只展示一张，OPS-004）。
    pub cover_id: Option<i64>,
    pub category_id: Option<i64>,
    pub category_slug: Option<String>,
    pub category_name: Option<String>,
}

impl SavedCardRow {
    pub(crate) fn split(self) -> (DateTime<Utc>, CardRow) {
        (
            self.saved_at,
            CardRow {
                id: self.id,
                author_id: self.author_id,
                title: self.title,
                excerpt_source: self.excerpt_source,
                is_anonymous: self.is_anonymous,
                anonymous_seed: self.anonymous_seed,
                pinned_at: self.pinned_at,
                solved_at: self.solved_at,
                view_count: self.view_count,
                like_count: self.like_count,
                comment_count: self.comment_count,
                published_at: self.published_at,
                bumped_at: self.bumped_at,
                edited_at: self.edited_at,
                last_bumped_by: self.last_bumped_by,
                cover_id: self.cover_id,
                category_id: self.category_id,
                category_slug: self.category_slug,
                category_name: self.category_name,
            },
        )
    }
}

/// 我点赞过、当前仍公开且不被拉黑过滤的帖子，按点赞时间倒序（LIKE-012、BLOCK-021）。
/// 过滤在 SQL 里完成，翻页不会出现「不满的一页」（D-020 方案 B）。
pub(crate) async fn liked_cards(
    conn: &mut PgConnection,
    user_id: i64,
    excluded_authors: &[i64],
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<SavedCardRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        SavedCardRow,
        r#"SELECT s.created_at AS saved_at,
                  p.id, p.author_id, p.title, left(p.body, 400) AS "excerpt_source!",
                  p.is_anonymous, p.anonymous_seed, p.pinned_at, p.solved_at,
                  p.view_count, p.like_count, p.comment_count,
                  p.published_at AS "published_at!", p.bumped_at AS "bumped_at!",
                  p.edited_at, p.last_bumped_by, p.cover_ids[1] AS "cover_id?",
                  c.id AS "category_id?", c.slug AS "category_slug?", c.name AS "category_name?"
           FROM post_likes s
           JOIN posts p ON p.id = s.post_id
           LEFT JOIN categories c ON c.id = p.category_id
           WHERE s.user_id = $1 AND p.status = 'published' AND p.hidden_at IS NULL
             AND (p.is_anonymous OR NOT (p.author_id = ANY($2)))
             AND ($3::timestamptz IS NULL OR (s.created_at, s.post_id) < ($3, $4::bigint))
           ORDER BY s.created_at DESC, s.post_id DESC
           LIMIT $5"#,
        user_id,
        excluded_authors,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 我收藏的帖子，规则同 [`liked_cards`]（FAV-008）。
pub(crate) async fn favorite_cards(
    conn: &mut PgConnection,
    user_id: i64,
    excluded_authors: &[i64],
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<SavedCardRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        SavedCardRow,
        r#"SELECT s.created_at AS saved_at,
                  p.id, p.author_id, p.title, left(p.body, 400) AS "excerpt_source!",
                  p.is_anonymous, p.anonymous_seed, p.pinned_at, p.solved_at,
                  p.view_count, p.like_count, p.comment_count,
                  p.published_at AS "published_at!", p.bumped_at AS "bumped_at!",
                  p.edited_at, p.last_bumped_by, p.cover_ids[1] AS "cover_id?",
                  c.id AS "category_id?", c.slug AS "category_slug?", c.name AS "category_name?"
           FROM post_favorites s
           JOIN posts p ON p.id = s.post_id
           LEFT JOIN categories c ON c.id = p.category_id
           WHERE s.user_id = $1 AND p.status = 'published' AND p.hidden_at IS NULL
             AND (p.is_anonymous OR NOT (p.author_id = ANY($2)))
             AND ($3::timestamptz IS NULL OR (s.created_at, s.post_id) < ($3, $4::bigint))
           ORDER BY s.created_at DESC, s.post_id DESC
           LIMIT $5"#,
        user_id,
        excluded_authors,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}
pub(crate) async fn denny_count(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<i32>> {
    sqlx::query_scalar!("SELECT denny_count FROM posts WHERE id = $1", id)
        .fetch_optional(conn)
        .await
}

// ---------- 作者主页 ----------

pub(crate) struct AuthorStatsRow {
    pub post_count: i64,
    pub total_views: i64,
    pub total_comments: i64,
    pub total_likes: i64,
}

/// 作者公开的非匿名帖子的统计（PROFILE-015）。匿名帖对任何人都不计入，作者本人也一样。
pub(crate) async fn author_stats(
    conn: &mut PgConnection,
    author_id: i64,
) -> sqlx::Result<AuthorStatsRow> {
    sqlx::query_as!(
        AuthorStatsRow,
        r#"SELECT count(*) AS "post_count!",
                  COALESCE(sum(view_count), 0)::bigint AS "total_views!",
                  COALESCE(sum(comment_count), 0)::bigint AS "total_comments!",
                  COALESCE(sum(like_count), 0)::bigint AS "total_likes!"
           FROM posts
           WHERE author_id = $1 AND status = 'published' AND hidden_at IS NULL AND NOT is_anonymous"#,
        author_id,
    )
    .fetch_one(conn)
    .await
}

/// 作者主页的帖子：已发布、非匿名；`include_hidden` 时包含被隐藏的（作者本人看自己）。按发布时间倒序。
pub(crate) async fn author_cards(
    conn: &mut PgConnection,
    author_id: i64,
    include_hidden: bool,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<CardRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        CardRow,
        r#"SELECT p.id, p.author_id, p.title, left(p.body, 400) AS "excerpt_source!",
                  p.is_anonymous, p.anonymous_seed, p.pinned_at, p.solved_at,
                  p.view_count, p.like_count, p.comment_count,
                  p.published_at AS "published_at!", p.bumped_at AS "bumped_at!",
                  p.edited_at, p.last_bumped_by, p.cover_ids[1] AS "cover_id?",
                  c.id AS "category_id?", c.slug AS "category_slug?", c.name AS "category_name?"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.author_id = $1 AND p.status = 'published' AND NOT p.is_anonymous
             AND ($2 OR p.hidden_at IS NULL)
             AND ($3::timestamptz IS NULL OR (p.published_at, p.id) < ($3, $4::bigint))
           ORDER BY p.published_at DESC, p.id DESC
           LIMIT $5"#,
        author_id,
        include_hidden,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 指定 id 中属于作者、已发布、非匿名的帖子（顺序不保证）。
pub(crate) async fn author_cards_by_ids(
    conn: &mut PgConnection,
    author_id: i64,
    ids: &[i64],
    include_hidden: bool,
) -> sqlx::Result<Vec<CardRow>> {
    sqlx::query_as!(
        CardRow,
        r#"SELECT p.id, p.author_id, p.title, left(p.body, 400) AS "excerpt_source!",
                  p.is_anonymous, p.anonymous_seed, p.pinned_at, p.solved_at,
                  p.view_count, p.like_count, p.comment_count,
                  p.published_at AS "published_at!", p.bumped_at AS "bumped_at!",
                  p.edited_at, p.last_bumped_by, p.cover_ids[1] AS "cover_id?",
                  c.id AS "category_id?", c.slug AS "category_slug?", c.name AS "category_name?"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.id = ANY($2) AND p.author_id = $1 AND p.status = 'published'
             AND NOT p.is_anonymous AND ($3 OR p.hidden_at IS NULL)"#,
        author_id,
        ids,
        include_hidden,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct PinCandidateRow {
    pub id: i64,
    pub title: String,
    pub published_at: DateTime<Utc>,
    pub hidden_at: Option<DateTime<Utc>>,
}

/// 可以设为主页精选的帖子：自己的、已发布、非匿名（含被隐藏的，D-026），按发布时间倒序，可按标题搜索。
pub(crate) async fn pin_candidates(
    conn: &mut PgConnection,
    author_id: i64,
    title_pattern: Option<&str>,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
) -> sqlx::Result<Vec<PinCandidateRow>> {
    let (after_at, after_id) = after.unzip();
    sqlx::query_as!(
        PinCandidateRow,
        r#"SELECT id, title, published_at AS "published_at!", hidden_at
           FROM posts
           WHERE author_id = $1 AND status = 'published' AND NOT is_anonymous
             AND ($2::text IS NULL OR title ILIKE $2 ESCAPE '\')
             AND ($3::timestamptz IS NULL OR (published_at, id) < ($3, $4::bigint))
           ORDER BY published_at DESC, id DESC
           LIMIT $5"#,
        author_id,
        title_pattern,
        after_at,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// `ids` 中可以设为精选的（规则同 [`pin_candidates`]）。
pub(crate) async fn pinnable_ids(
    conn: &mut PgConnection,
    author_id: i64,
    ids: &[i64],
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT id FROM posts
         WHERE id = ANY($2) AND author_id = $1 AND status = 'published' AND NOT is_anonymous",
        author_id,
        ids,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct PublicPostRow {
    pub id: i64,
    pub title: String,
}

/// `ids` 中公开（已发布且未隐藏）的帖子及其标题。
pub(crate) async fn public_posts(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<PublicPostRow>> {
    sqlx::query_as!(
        PublicPostRow,
        "SELECT id, title FROM posts WHERE id = ANY($1) AND status = 'published' AND hidden_at IS NULL",
        ids,
    )
    .fetch_all(conn)
    .await
}

// ---------- 阅读记录 ----------

/// `post_ids` 中该用户读过的。
pub(crate) async fn read_among(
    conn: &mut PgConnection,
    user_id: i64,
    post_ids: &[i64],
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT post_id FROM post_reads WHERE user_id = $1 AND post_id = ANY($2)",
        user_id,
        post_ids,
    )
    .fetch_all(conn)
    .await
}

/// `ids` 中存在的帖子（不论状态）。
pub(crate) async fn existing_post_ids(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!("SELECT id FROM posts WHERE id = ANY($1)", ids)
        .fetch_all(conn)
        .await
}

/// 记为已读（已有的不重复写），返回新写入条数。
pub(crate) async fn mark_read(
    conn: &mut PgConnection,
    user_id: i64,
    post_ids: &[i64],
    now: DateTime<Utc>,
) -> sqlx::Result<u64> {
    let result = sqlx::query!(
        "INSERT INTO post_reads (user_id, post_id, created_at)
         SELECT $1, id, $3 FROM unnest($2::bigint[]) AS t(id)
         ON CONFLICT DO NOTHING",
        user_id,
        post_ids,
        now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 按给定 id 取公开、且不被拉黑过滤的帖子卡片（顺序不保证）。热门榜与首页热门位用。
pub(crate) async fn public_cards_by_ids(
    conn: &mut PgConnection,
    ids: &[i64],
    excluded_authors: &[i64],
) -> sqlx::Result<Vec<CardRow>> {
    sqlx::query_as!(
        CardRow,
        r#"SELECT p.id, p.author_id, p.title, left(p.body, 400) AS "excerpt_source!",
                  p.is_anonymous, p.anonymous_seed, p.pinned_at, p.solved_at,
                  p.view_count, p.like_count, p.comment_count,
                  p.published_at AS "published_at!", p.bumped_at AS "bumped_at!",
                  p.edited_at, p.last_bumped_by, p.cover_ids[1] AS "cover_id?",
                  c.id AS "category_id?", c.slug AS "category_slug?", c.name AS "category_name?"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.id = ANY($1) AND p.status = 'published' AND p.hidden_at IS NULL
             AND (p.is_anonymous OR NOT (p.author_id = ANY($2)))"#,
        ids,
        excluded_authors,
    )
    .fetch_all(conn)
    .await
}

// ---------- 相关推荐 ----------

pub(crate) struct RelatedRow {
    pub id: i64,
    pub author_id: i64,
    pub is_anonymous: bool,
}

pub(crate) struct RelatedQuery<'a> {
    pub seed_id: i64,
    pub seed_category: Option<i64>,
    pub viewer: Option<i64>,
    pub excluded_authors: &'a [i64],
    pub limit: i64,
    pub named_only: bool,
}

/// 相关推荐的候选，按相关度从高到低。
///
/// 候选池 = 与种子帖共享标签的帖子 ∪ 同分区最近活跃的 300 篇 ∪ 全站最近活跃的 300 篇（都只看公开帖子），
/// 限一年内发布、去掉种子帖与观众自己的帖子（含自己的匿名帖，SUGG-003）、拉黑过滤（匿名帖放行）。
/// 相关度 = 3 × 共享标签数 + 2 × 同分区 + 2 × 观众没读过 + 新鲜度（30 天衰减，0–1）+ 热度（0–1）。
pub(crate) async fn related_candidates(
    conn: &mut PgConnection,
    q: &RelatedQuery<'_>,
) -> sqlx::Result<Vec<RelatedRow>> {
    sqlx::query_as!(
        RelatedRow,
        r#"WITH tagged AS (
               SELECT pt.post_id AS id, count(*) AS shared
               FROM post_tags pt
               WHERE pt.tag_id IN (SELECT tag_id FROM post_tags WHERE post_id = $1)
                 AND pt.post_id <> $1
               GROUP BY pt.post_id
           ),
           pool AS (
               SELECT id FROM tagged
               UNION
               (SELECT id FROM posts
                WHERE category_id = $2 AND status = 'published' AND hidden_at IS NULL
                ORDER BY bumped_at DESC, id DESC LIMIT 300)
               UNION
               (SELECT id FROM posts
                WHERE status = 'published' AND hidden_at IS NULL
                ORDER BY bumped_at DESC, id DESC LIMIT 300)
           )
           SELECT p.id, p.author_id, p.is_anonymous
           FROM pool
           JOIN posts p ON p.id = pool.id
           LEFT JOIN tagged t ON t.id = p.id
           WHERE p.id <> $1 AND p.status = 'published' AND p.hidden_at IS NULL
             AND p.published_at > now() - interval '365 days'
             AND ($3::bigint IS NULL OR p.author_id <> $3)
             AND (p.is_anonymous OR NOT (p.author_id = ANY($4)))
             AND (NOT $6 OR NOT p.is_anonymous)
           ORDER BY
               3.0 * coalesce(t.shared, 0)
             + CASE WHEN $2::bigint IS NOT NULL AND p.category_id = $2 THEN 2.0 ELSE 0.0 END
             + CASE WHEN $3::bigint IS NOT NULL AND NOT EXISTS (
                         SELECT 1 FROM post_reads r WHERE r.user_id = $3 AND r.post_id = p.id)
                    THEN 2.0 ELSE 0.0 END
             + exp(-extract(epoch FROM now() - p.bumped_at) / (30.0 * 86400.0))
             + least(1.0, ln(1.0 + p.like_count + 2 * p.comment_count + p.favorite_count) / ln(101.0))
               DESC,
               p.bumped_at DESC, p.id DESC
           LIMIT $5"#,
        q.seed_id,
        q.seed_category,
        q.viewer,
        q.excluded_authors,
        q.limit,
        q.named_only,
    )
    .fetch_all(conn)
    .await
}

// ---------- 热榜数据 ----------

pub(crate) struct HotPostRow {
    pub id: i64,
    pub author_id: i64,
    pub is_anonymous: bool,
    pub created_at: DateTime<Utc>,
    pub bumped_at: DateTime<Utc>,
    pub like_count: i32,
}

/// 最近活跃的公开帖子（HOT-004 候选集的第一部分）。c 取首次发布时间（EDGE-016）。
pub(crate) async fn hot_recent(
    conn: &mut PgConnection,
    limit: i64,
) -> sqlx::Result<Vec<HotPostRow>> {
    sqlx::query_as!(
        HotPostRow,
        r#"SELECT id, author_id, is_anonymous,
                  COALESCE(first_published_at, published_at) AS "created_at!",
                  bumped_at AS "bumped_at!", like_count
           FROM posts
           WHERE status = 'published' AND hidden_at IS NULL
           ORDER BY bumped_at DESC, id DESC
           LIMIT $1"#,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 指定 id 中公开的帖子（候选集的第二部分：窗口内有活动但不在最近活跃里的）。
pub(crate) async fn hot_by_ids(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<HotPostRow>> {
    sqlx::query_as!(
        HotPostRow,
        r#"SELECT id, author_id, is_anonymous,
                  COALESCE(first_published_at, published_at) AS "created_at!",
                  bumped_at AS "bumped_at!", like_count
           FROM posts
           WHERE id = ANY($1) AND status = 'published' AND hidden_at IS NULL"#,
        ids,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct ActivityRow {
    pub post_id: i64,
    pub user_id: i64,
    pub first_at: DateTime<Utc>,
}

/// 窗口内对帖子的点赞：每个 (帖子, 用户) 一行，取最早时间（HOT-005/006）。
pub(crate) async fn like_activity(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> sqlx::Result<Vec<ActivityRow>> {
    sqlx::query_as!(
        ActivityRow,
        r#"SELECT post_id, user_id, min(created_at) AS "first_at!"
           FROM post_likes
           WHERE created_at >= $1 AND created_at <= $2
           GROUP BY post_id, user_id"#,
        since,
        until,
    )
    .fetch_all(conn)
    .await
}

// ---------- 搜索（search 域建索引与数据库兜底用）----------

pub(crate) struct SearchDocRow {
    pub id: i64,
    pub author_id: i64,
    pub is_anonymous: bool,
    pub title: String,
    pub body: String,
    pub category_slug: Option<String>,
    pub category_name: Option<String>,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
    pub bumped_at: DateTime<Utc>,
}

/// `ids` 中公开的帖子（建索引用）。
pub(crate) async fn search_docs_by_ids(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<SearchDocRow>> {
    sqlx::query_as!(
        SearchDocRow,
        r#"SELECT p.id, p.author_id, p.is_anonymous, p.title, p.body,
                  c.slug AS "category_slug?", c.name AS "category_name?",
                  p.view_count, p.like_count, p.comment_count, p.bumped_at AS "bumped_at!"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.id = ANY($1) AND p.status = 'published' AND p.hidden_at IS NULL"#,
        ids,
    )
    .fetch_all(conn)
    .await
}

/// 全部公开帖子，按 id 升序分批（全量对账用）。
pub(crate) async fn search_docs_after(
    conn: &mut PgConnection,
    after_id: i64,
    limit: i64,
) -> sqlx::Result<Vec<SearchDocRow>> {
    sqlx::query_as!(
        SearchDocRow,
        r#"SELECT p.id, p.author_id, p.is_anonymous, p.title, p.body,
                  c.slug AS "category_slug?", c.name AS "category_name?",
                  p.view_count, p.like_count, p.comment_count, p.bumped_at AS "bumped_at!"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.id > $1 AND p.status = 'published' AND p.hidden_at IS NULL
           ORDER BY p.id
           LIMIT $2"#,
        after_id,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// `since` 之后改动过的帖子（任意状态，下线 / 隐藏的也要同步删除）。
pub(crate) async fn changed_post_ids(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
    limit: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT id FROM posts WHERE updated_at >= $1 ORDER BY updated_at, id LIMIT $2",
        since,
        limit,
    )
    .fetch_all(conn)
    .await
}

/// 数据库兜底搜索（SRCH-014/015、SRCH-022）：标题（及正文）包含 `q`，忽略大小写；公开、
/// 分区、拉黑过滤（匿名帖不过滤）。`hot` 按点赞、浏览倒序，否则按信息流时间倒序（D-038 方案 B）。
/// 窗口固定为 `cap`，与翻页位置无关（EDGE-008）。
pub(crate) async fn search_post_ids(
    conn: &mut PgConnection,
    q: &str,
    category_slug: Option<&str>,
    excluded_authors: &[i64],
    title_only: bool,
    hot: bool,
    cap: i64,
) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar!(
        "SELECT p.id FROM posts p LEFT JOIN categories c ON c.id = p.category_id
         WHERE p.status = 'published' AND p.hidden_at IS NULL
           AND ($2::text IS NULL OR c.slug = $2)
           AND (p.is_anonymous OR NOT (p.author_id = ANY($3)))
           AND (strpos(lower(p.title), lower($1)) > 0
                OR (NOT $4 AND strpos(lower(p.body), lower($1)) > 0))
         ORDER BY CASE WHEN $5 THEN p.like_count END DESC NULLS LAST,
                  CASE WHEN $5 THEN p.view_count END DESC NULLS LAST,
                  p.bumped_at DESC, p.id DESC
         LIMIT $6",
        q,
        category_slug,
        excluded_authors,
        title_only,
        hot,
        cap,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct PostMetaRow {
    pub id: i64,
    pub author_id: i64,
    pub is_anonymous: bool,
    pub title: String,
    pub cover_id: Option<i64>,
}

/// `ids` 中公开的帖子：作者、是否匿名、标题。
pub(crate) async fn public_post_meta(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<PostMetaRow>> {
    sqlx::query_as!(
        PostMetaRow,
        r#"SELECT id, author_id, is_anonymous, title, cover_ids[1] AS "cover_id?" FROM posts
           WHERE id = ANY($1) AND status = 'published' AND hidden_at IS NULL"#,
        ids,
    )
    .fetch_all(conn)
    .await
}

/// 帖子的封面（有序）。帖子不存在时为空。
pub(crate) async fn cover_ids(conn: &mut PgConnection, post_id: i64) -> sqlx::Result<Vec<i64>> {
    Ok(
        sqlx::query_scalar!("SELECT cover_ids FROM posts WHERE id = $1", post_id)
            .fetch_optional(conn)
            .await?
            .unwrap_or_default(),
    )
}

// ---------- 审核（moderation） ----------

pub(crate) struct ReportableRow {
    pub author_id: i64,
    pub title: String,
    pub body: String,
    pub cover_ids: Vec<i64>,
    pub is_anonymous: bool,
    pub is_public: bool,
}

pub(crate) async fn reportable(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<ReportableRow>> {
    sqlx::query_as!(
        ReportableRow,
        r#"SELECT author_id, title, body, cover_ids, is_anonymous,
                  (status = 'published' AND hidden_at IS NULL) AS "is_public!"
           FROM posts WHERE id = $1"#,
        id
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct HiddenRow {
    pub was_public: bool,
}

/// 隐藏帖子并取消置顶（RPT-053）。已经隐藏的只改原因与操作人。返回隐藏前的状态。
pub(crate) async fn hide(
    conn: &mut PgConnection,
    id: i64,
    by: Option<i64>,
    reason: &str,
    now: DateTime<Utc>,
) -> sqlx::Result<Option<HiddenRow>> {
    sqlx::query_as!(
        HiddenRow,
        r#"WITH before AS (
               SELECT id, (status = 'published' AND hidden_at IS NULL) AS was_public
               FROM posts WHERE id = $1 FOR UPDATE
           )
           UPDATE posts p
           SET hidden_at = COALESCE(p.hidden_at, $4), hidden_by = $2, hidden_reason = $3,
               pinned_at = NULL, pinned_by = NULL
           FROM before b WHERE p.id = b.id
           RETURNING b.was_public AS "was_public!""#,
        id,
        by,
        reason,
        now
    )
    .fetch_optional(conn)
    .await
}

/// 取消隐藏，只在当前隐藏原因是 `reason` 时生效（驳回只恢复自动隐藏，RPTADM-060）。
/// 返回帖子分区（发布时的事件要用）与恢复后是否公开。
pub(crate) async fn unhide(
    conn: &mut PgConnection,
    id: i64,
    reason: &str,
) -> sqlx::Result<Option<(Option<i64>, bool)>> {
    let row = sqlx::query!(
        "UPDATE posts SET hidden_at = NULL, hidden_by = NULL, hidden_reason = NULL
         WHERE id = $1 AND hidden_at IS NOT NULL AND hidden_reason = $2
         RETURNING category_id, status",
        id,
        reason
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| (r.category_id, r.status == "published")))
}

pub(crate) struct ModerationStateRow {
    pub id: i64,
    pub hidden: bool,
}

pub(crate) async fn moderation_states(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<ModerationStateRow>> {
    sqlx::query_as!(
        ModerationStateRow,
        r#"SELECT id, (hidden_at IS NOT NULL) AS "hidden!" FROM posts WHERE id = ANY($1)"#,
        ids
    )
    .fetch_all(conn)
    .await
}

// ---------- 管理后台：分区与标签 ----------

pub(crate) struct AdminCategoryRow {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub sort_order: i32,
    pub is_active: bool,
    pub admin_only: bool,
    pub allow_comment_accept: bool,
    pub post_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 全部分区（含停用的），附已发布帖子数。
pub(crate) async fn admin_categories(
    conn: &mut PgConnection,
) -> sqlx::Result<Vec<AdminCategoryRow>> {
    sqlx::query_as!(
        AdminCategoryRow,
        r#"SELECT c.id, c.slug, c.name, c.description, c.sort_order, c.is_active, c.admin_only,
                  c.allow_comment_accept, c.created_at, c.updated_at,
                  (SELECT count(*) FROM posts p WHERE p.category_id = c.id) AS "post_count!"
           FROM categories c ORDER BY c.sort_order, c.id"#,
    )
    .fetch_all(conn)
    .await
}

pub(crate) struct CategoryUpdate<'a> {
    pub id: i64,
    pub slug: &'a str,
    pub name: &'a str,
    pub description: &'a str,
    pub sort_order: i32,
    pub is_active: bool,
    pub admin_only: bool,
    pub allow_comment_accept: bool,
    pub now: DateTime<Utc>,
}

pub(crate) async fn update_category(
    conn: &mut PgConnection,
    c: &CategoryUpdate<'_>,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE categories SET slug = $2, name = $3, description = $4, sort_order = $5,
             is_active = $6, admin_only = $7, allow_comment_accept = $8, updated_at = $9
         WHERE id = $1",
        c.id,
        c.slug,
        c.name,
        c.description,
        c.sort_order,
        c.is_active,
        c.admin_only,
        c.allow_comment_accept,
        c.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// 删除没有帖子（含草稿）的分区。返回 None 表示不存在，Some(false) 表示还有帖子。
pub(crate) async fn delete_empty_category(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<bool>> {
    let Some(used) = sqlx::query_scalar!(
        r#"SELECT (EXISTS (SELECT 1 FROM posts WHERE category_id = c.id)
                   OR EXISTS (SELECT 1 FROM post_drafts WHERE category_id = c.id)) AS "used!"
           FROM categories c WHERE c.id = $1 FOR UPDATE"#,
        id,
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(None);
    };
    if used {
        return Ok(Some(false));
    }
    sqlx::query!("DELETE FROM categories WHERE id = $1", id)
        .execute(conn)
        .await?;
    Ok(Some(true))
}

pub(crate) struct AdminTagRow {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub is_blocked: bool,
    pub post_count: i64,
    pub created_at: DateTime<Utc>,
}

pub(crate) async fn count_admin_tags(
    conn: &mut PgConnection,
    pattern: Option<&str>,
    blocked: Option<bool>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM tags
           WHERE ($1::text IS NULL OR name ILIKE $1 OR slug ILIKE $1)
             AND ($2::bool IS NULL OR is_blocked = $2)"#,
        pattern,
        blocked,
    )
    .fetch_one(conn)
    .await
}

/// 标签，按使用次数倒序。
pub(crate) async fn admin_tags(
    conn: &mut PgConnection,
    pattern: Option<&str>,
    blocked: Option<bool>,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<AdminTagRow>> {
    sqlx::query_as!(
        AdminTagRow,
        r#"SELECT t.id, t.name, t.slug, t.is_blocked, t.created_at,
                  (SELECT count(*) FROM post_tags pt WHERE pt.tag_id = t.id) AS "post_count!"
           FROM tags t
           WHERE ($1::text IS NULL OR t.name ILIKE $1 OR t.slug ILIKE $1)
             AND ($2::bool IS NULL OR t.is_blocked = $2)
           ORDER BY "post_count!" DESC, t.id DESC
           LIMIT $3 OFFSET $4"#,
        pattern,
        blocked,
        limit,
        offset,
    )
    .fetch_all(conn)
    .await
}

pub(crate) async fn update_tag(
    conn: &mut PgConnection,
    id: i64,
    name: &str,
    slug: &str,
    is_blocked: bool,
) -> sqlx::Result<bool> {
    let result = sqlx::query!(
        "UPDATE tags SET name = $2, slug = $3, is_blocked = $4 WHERE id = $1",
        id,
        name,
        slug,
        is_blocked,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn admin_tag(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<AdminTagRow>> {
    sqlx::query_as!(
        AdminTagRow,
        r#"SELECT t.id, t.name, t.slug, t.is_blocked, t.created_at,
                  (SELECT count(*) FROM post_tags pt WHERE pt.tag_id = t.id) AS "post_count!"
           FROM tags t WHERE t.id = $1"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

/// 已发布的帖子总数与 `since` 之后首次发布的数量（仪表盘）。
pub(crate) async fn post_counts(
    conn: &mut PgConnection,
    since: DateTime<Utc>,
) -> sqlx::Result<(i64, i64)> {
    let row = sqlx::query!(
        r#"SELECT count(*) FILTER (WHERE published_at IS NOT NULL) AS "total!",
                  count(*) FILTER (WHERE first_published_at >= $1) AS "since!"
           FROM posts"#,
        since,
    )
    .fetch_one(conn)
    .await?;
    Ok((row.total, row.since))
}

// ---------- 管理后台：帖子管理 ----------

pub(crate) struct AdminPostFilter<'a> {
    /// 标题包含（ILIKE 模式，已转义）。
    pub pattern: Option<&'a str>,
    /// 关键词是数字时也按帖子 id 匹配。
    pub id: Option<i64>,
    pub category_id: Option<i64>,
    /// 按作者筛选时排除匿名帖（AGENTS.md 规则 10）。
    pub author_id: Option<i64>,
    pub hidden: Option<bool>,
    pub hidden_reason: Option<&'a str>,
    pub pinned: Option<bool>,
    pub anonymous: Option<bool>,
}

pub(crate) struct AdminPostRow {
    pub id: i64,
    pub title: String,
    pub category_id: Option<i64>,
    pub category_name: Option<String>,
    pub author_id: i64,
    pub is_anonymous: bool,
    pub anonymous_seed: Option<String>,
    pub first_published_at: Option<DateTime<Utc>>,
    pub edited_at: Option<DateTime<Utc>>,
    pub pinned_at: Option<DateTime<Utc>>,
    pub hidden_at: Option<DateTime<Utc>>,
    pub hidden_by: Option<i64>,
    pub hidden_reason: Option<String>,
    pub view_count: i64,
    pub like_count: i32,
    pub comment_count: i32,
}

pub(crate) async fn count_admin_posts(
    conn: &mut PgConnection,
    f: &AdminPostFilter<'_>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM posts p
           WHERE p.status = 'published'
             AND ($1::text IS NULL OR p.title ILIKE $1 OR p.id = $2)
             AND ($3::bigint IS NULL OR p.category_id = $3)
             AND ($4::bigint IS NULL OR (p.author_id = $4 AND NOT p.is_anonymous))
             AND ($5::bool IS NULL OR (p.hidden_at IS NOT NULL) = $5)
             AND ($6::text IS NULL OR p.hidden_reason = $6)
             AND ($7::bool IS NULL OR (p.pinned_at IS NOT NULL) = $7)
             AND ($8::bool IS NULL OR p.is_anonymous = $8)"#,
        f.pattern,
        f.id,
        f.category_id,
        f.author_id,
        f.hidden,
        f.hidden_reason,
        f.pinned,
        f.anonymous,
    )
    .fetch_one(conn)
    .await
}

/// 已发布的帖子（含隐藏的），新发的在前。草稿是作者私有的，不列。
pub(crate) async fn admin_posts(
    conn: &mut PgConnection,
    f: &AdminPostFilter<'_>,
    limit: i64,
    offset: i64,
) -> sqlx::Result<Vec<AdminPostRow>> {
    sqlx::query_as!(
        AdminPostRow,
        r#"SELECT p.id, p.title, p.category_id, c.name AS "category_name?", p.author_id,
                  p.is_anonymous, p.anonymous_seed, p.first_published_at, p.edited_at, p.pinned_at,
                  p.hidden_at, p.hidden_by, p.hidden_reason, p.view_count, p.like_count, p.comment_count
           FROM posts p
           LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.status = 'published'
             AND ($1::text IS NULL OR p.title ILIKE $1 OR p.id = $2)
             AND ($3::bigint IS NULL OR p.category_id = $3)
             AND ($4::bigint IS NULL OR (p.author_id = $4 AND NOT p.is_anonymous))
             AND ($5::bool IS NULL OR (p.hidden_at IS NOT NULL) = $5)
             AND ($6::text IS NULL OR p.hidden_reason = $6)
             AND ($7::bool IS NULL OR (p.pinned_at IS NOT NULL) = $7)
             AND ($8::bool IS NULL OR p.is_anonymous = $8)
           ORDER BY p.first_published_at DESC NULLS LAST, p.id DESC
           LIMIT $9 OFFSET $10"#,
        f.pattern,
        f.id,
        f.category_id,
        f.author_id,
        f.hidden,
        f.hidden_reason,
        f.pinned,
        f.anonymous,
        limit,
        offset,
    )
    .fetch_all(conn)
    .await
}

/// 锁住已发布的帖子，返回当前分区与是否公开。不存在或是草稿时为 None。
pub(crate) async fn lock_published(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<(Option<i64>, bool)>> {
    let row = sqlx::query!(
        r#"SELECT category_id, hidden_at IS NULL AS "public!" FROM posts
           WHERE id = $1 AND status = 'published' FOR UPDATE"#,
        id,
    )
    .fetch_optional(conn)
    .await?;
    Ok(row.map(|r| (r.category_id, r.public)))
}

/// 把帖子移到另一个分区。待发布的修改（`post_drafts`）一并改，否则作者下次发布会改回去。
/// 刷新 `updated_at`，搜索增量扫描据此更新索引里的分区。
pub(crate) async fn move_post(
    conn: &mut PgConnection,
    id: i64,
    category_id: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET category_id = $2, updated_at = $3 WHERE id = $1",
        id,
        category_id,
        now,
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "UPDATE post_drafts SET category_id = $2 WHERE post_id = $1",
        id,
        category_id,
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// 帖子标题（不论是否公开），评论管理列表用。
pub(crate) async fn titles(
    conn: &mut PgConnection,
    ids: &[i64],
) -> sqlx::Result<Vec<(i64, String)>> {
    let rows = sqlx::query!("SELECT id, title FROM posts WHERE id = ANY($1)", ids)
        .fetch_all(conn)
        .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.title)).collect())
}

/// 审核用：锁住帖子行，返回作者、是否匿名、标题、隐藏原因（未隐藏为 None）。
pub(crate) async fn lock_moderation_target(
    conn: &mut PgConnection,
    id: i64,
) -> sqlx::Result<Option<ModerationTargetRow>> {
    sqlx::query_as!(
        ModerationTargetRow,
        r#"SELECT author_id, is_anonymous, title, status = 'published' AS "published!",
                  hidden_at IS NOT NULL AS "hidden!", hidden_reason
           FROM posts WHERE id = $1 FOR UPDATE"#,
        id,
    )
    .fetch_optional(conn)
    .await
}

pub(crate) struct ModerationTargetRow {
    pub author_id: i64,
    pub is_anonymous: bool,
    pub title: String,
    pub published: bool,
    pub hidden: bool,
    pub hidden_reason: Option<String>,
}

/// 每个业务日首次发布的帖子数（仪表盘趋势）。
pub(crate) async fn daily_first_published(
    conn: &mut PgConnection,
    bounds: &[DateTime<Utc>],
) -> sqlx::Result<Vec<(i32, i64)>> {
    let (Some(first), Some(last)) = (bounds.first(), bounds.last()) else {
        return Ok(Vec::new());
    };
    let rows = sqlx::query!(
        r#"SELECT width_bucket(first_published_at, $1::timestamptz[]) AS "bucket!", count(*) AS "n!"
           FROM posts
           WHERE first_published_at >= $2 AND first_published_at < $3
           GROUP BY 1"#,
        bounds,
        first,
        last,
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|r| (r.bucket, r.n)).collect())
}

/// 作者从 `since` 起首次发布的帖子数（新账号限制）。
pub(crate) async fn published_since(
    conn: &mut PgConnection,
    author_id: i64,
    since: DateTime<Utc>,
) -> sqlx::Result<i64> {
    sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM posts WHERE author_id = $1 AND first_published_at >= $2"#,
        author_id,
        since,
    )
    .fetch_one(conn)
    .await
}
