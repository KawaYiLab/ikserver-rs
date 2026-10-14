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
        "SELECT id, author_id, category_id, status, title, body, editor_state, external_videos,
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
        "SELECT id, author_id, category_id, status, title, body, editor_state, external_videos,
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
                            is_anonymous, anonymous_seed, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)",
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
                revision = revision + 1, updated_at = $9
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
        "SELECT title, body, editor_state, external_videos, category_id, tag_ids, updated_at
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
                                  tag_ids, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (post_id) DO UPDATE SET
             title = EXCLUDED.title, body = EXCLUDED.body, editor_state = EXCLUDED.editor_state,
             external_videos = EXCLUDED.external_videos, category_id = EXCLUDED.category_id,
             tag_ids = EXCLUDED.tag_ids, updated_at = EXCLUDED.updated_at",
        post_id,
        content.title,
        content.body,
        content.editor_state,
        content.external_videos,
        content.category_id,
        &content.tag_ids,
        now,
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
    pub category_id: Option<i64>,
    pub category_slug: Option<String>,
    pub category_name: Option<String>,
}

pub(crate) struct FeedFilter {
    pub category_id: Option<i64>,
    pub tag_id: Option<i64>,
    pub exclude_pinned: bool,
    pub after: Option<(DateTime<Utc>, i64)>,
    pub limit: i64,
    /// 与读者有拉黑关系的作者；他们的匿名帖不受影响（COMMON-008）。
    pub excluded_authors: Vec<i64>,
}

/// 公开帖子按 (bumped_at, id) 倒序。可选按分区、标签筛选。
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
                  p.edited_at, p.last_bumped_by,
                  c.id AS "category_id?", c.slug AS "category_slug?", c.name AS "category_name?"
           FROM posts p LEFT JOIN categories c ON c.id = p.category_id
           WHERE p.status = 'published' AND p.hidden_at IS NULL
             AND ($1::bigint IS NULL OR p.category_id = $1)
             AND ($2::bigint IS NULL OR EXISTS (
                     SELECT 1 FROM post_tags pt WHERE pt.post_id = p.id AND pt.tag_id = $2))
             AND (NOT $3 OR p.pinned_at IS NULL)
             AND ($4::timestamptz IS NULL OR (p.bumped_at, p.id) < ($4, $5::bigint))
             AND (p.is_anonymous OR NOT (p.author_id = ANY($7)))
           ORDER BY p.bumped_at DESC, p.id DESC
           LIMIT $6"#,
        filter.category_id,
        filter.tag_id,
        filter.exclude_pinned,
        after_at,
        after_id,
        filter.limit,
        &filter.excluded_authors,
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
                  p.edited_at, p.last_bumped_by,
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
    pub is_anonymous: bool,
    pub is_public: bool,
    pub allow_comment_accept: bool,
}

pub(crate) async fn post_ref(conn: &mut PgConnection, id: i64) -> sqlx::Result<Option<PostRefRow>> {
    sqlx::query_as!(
        PostRefRow,
        r#"SELECT p.id, p.author_id, p.is_anonymous,
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
        r#"SELECT p.id, p.author_id, p.is_anonymous,
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
pub(crate) async fn comment_added(
    conn: &mut PgConnection,
    id: i64,
    by: i64,
    now: DateTime<Utc>,
) -> sqlx::Result<()> {
    sqlx::query!(
        "UPDATE posts SET comment_count = comment_count + 1, bumped_at = $3, last_bumped_by = $2
         WHERE id = $1",
        id,
        by,
        now,
    )
    .execute(conn)
    .await?;
    Ok(())
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
