//! 内容：分区、标签、帖子（含待发布修改与标签）、评论、采纳。

use sqlx::{PgConnection, Postgres, Transaction};

use crate::import::{ImportOptions, ImportReport};

pub async fn run(
    tx: &mut Transaction<'_, Postgres>,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    report.add("categories", categories(tx, options).await?);
    report.add("tags", tags(tx, options).await?);
    report.add("posts", posts(tx, options).await?);
    report.add("post_drafts", drafts(tx, options).await?);
    report.add("post_tags", post_tags(tx).await?);
    report.add("comments", comments(tx, options).await?);
    report.add("posts.accepted_comment", accepted(tx).await?);
    report.add("user_profiles.pinned", pinned(tx, options).await?);
    Ok(())
}

async fn categories(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO categories (id, slug, name, description, sort_order, is_active, admin_only,
                                   allow_comment_accept, legacy_id, created_at, updated_at)
           SELECT c.id, c.new_slug, c.new_name, coalesce(c.description, ''), coalesce(c."order", 0),
                  coalesce(c.is_active, true), coalesce(c.admin_only, false),
                  coalesce(c.allow_comment_accept, false), c.document_id,
                  coalesce(c.created_at AT TIME ZONE $1, $2),
                  coalesce(c.updated_at AT TIME ZONE $1, c.created_at AT TIME ZONE $1, $2)
           FROM legacy.ik_categories c"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

async fn tags(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO tags (id, name, slug, is_blocked, legacy_id, created_at)
           SELECT t.id, t.new_name, t.new_slug, coalesce(t.is_blocked, false), t.document_id,
                  coalesce(t.created_at AT TIME ZONE $1, $2)
           FROM legacy.ik_tags t
           WHERE t.id = t.survivor_id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 作者已不存在的帖子不导入（预检里报告）。超长的标题、正文截断；外链视频只保留第一个；
/// 封面只认摘要读取成功的文件、最多 40 张。匿名帖缺 seed 时由 documentId 派生（稳定，重导不变）。
/// 隐藏：旧 `isHidden` 为 NULL 视为可见（D-012）。
async fn posts(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO posts (id, author_id, category_id, status, title, body, editor_state,
                              external_videos, is_anonymous, anonymous_seed, first_published_at,
                              published_at, edited_at, bumped_at, last_bumped_by, pinned_at,
                              pinned_by, solved_at, hidden_at, hidden_reason, view_count, legacy_id,
                              created_at, updated_at, cover_ids)
           SELECT p.post_id, p.author_id, cat.id,
                  CASE WHEN p.published THEN 'published' ELSE 'draft' END,
                  coalesce(nullif(left(btrim(coalesce(p.title, '')), 50), ''), '无标题'),
                  left(legacy.ik_fix_hosts(p.text), 16000),
                  p.editor_state,
                  CASE WHEN jsonb_typeof(p.external_videos) = 'array'
                            AND jsonb_array_length(p.external_videos) > 0
                       THEN jsonb_build_array(p.external_videos -> 0) ELSE '[]'::jsonb END,
                  p.is_anonymous,
                  CASE WHEN p.is_anonymous THEN coalesce(p.anonymous_seed, md5('legacy-post:' || p.document_id))
                       ELSE p.anonymous_seed END,
                  coalesce(p.first_published_at, p.published_at) AT TIME ZONE $1,
                  p.published_at AT TIME ZONE $1,
                  p.edited_at AT TIME ZONE $1,
                  CASE WHEN p.published THEN coalesce(p.bumped_at, p.published_at) AT TIME ZONE $1 END,
                  bumper.id,
                  CASE WHEN p.is_pinned
                       THEN coalesce(p.pinned_at, p.published_at, p.created_at) AT TIME ZONE $1 END,
                  CASE WHEN p.is_pinned THEN pinner.id END,
                  p.solved_at AT TIME ZONE $1,
                  CASE WHEN p.is_hidden
                       THEN coalesce(p.row_updated_at, p.updated_at, p.created_at) AT TIME ZONE $1 END,
                  CASE WHEN p.is_hidden THEN p.hidden_reason END,
                  greatest(coalesce(p.views, 0), 0),
                  p.document_id,
                  coalesce(p.created_at AT TIME ZONE $1, $2),
                  coalesce(p.updated_at AT TIME ZONE $1, p.created_at AT TIME ZONE $1, $2),
                  coalesce((SELECT array_agg(c.file_id ORDER BY c."order", c.file_id)
                            FROM (SELECT r.file_id, r."order" FROM legacy.ik_file_refs r
                                  WHERE r.related_type = 'api::article.article' AND r.field = 'cover'
                                    AND r.related_id = p.main_row
                                  ORDER BY r."order", r.file_id LIMIT 40) c), '{}')
           FROM legacy.ik_posts p
           LEFT JOIN categories cat ON cat.id = p.category_id
           LEFT JOIN users bumper ON bumper.id = p.last_bumped_by_user_id
           LEFT JOIN users pinner ON pinner.id = p.pinned_by
           WHERE p.author_id IS NOT NULL"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 已发布帖子的待发布修改（草稿行与发布行内容不同）。
async fn drafts(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO post_drafts (post_id, title, body, editor_state, external_videos, category_id,
                                    tag_ids, cover_ids, updated_at)
           SELECT p.post_id,
                  coalesce(nullif(left(btrim(coalesce(d.title, '')), 50), ''), '无标题'),
                  left(legacy.ik_fix_hosts(d.text), 16000),
                  d.editor_state,
                  CASE WHEN jsonb_typeof(d.external_videos) = 'array'
                            AND jsonb_array_length(d.external_videos) > 0
                       THEN jsonb_build_array(d.external_videos -> 0) ELSE '[]'::jsonb END,
                  (SELECT cat.id FROM legacy.articles_category_lnk l
                   JOIN categories cat ON cat.id = l.category_id
                   WHERE l.article_id = d.id ORDER BY l.id LIMIT 1),
                  coalesce((SELECT array_agg(x.tag_id ORDER BY x.ord, x.tag_id)
                            FROM (SELECT y.tag_id, y.ord
                                  FROM (SELECT DISTINCT ON (t.survivor_id) t.survivor_id AS tag_id,
                                               l.tag_ord AS ord
                                        FROM legacy.articles_tags_lnk l
                                        JOIN legacy.ik_tags t ON t.id = l.tag_id
                                        WHERE l.article_id = d.id
                                        ORDER BY t.survivor_id, l.tag_ord) y
                                  ORDER BY y.ord, y.tag_id LIMIT 5) x), '{}'),
                  coalesce((SELECT array_agg(c.file_id ORDER BY c."order", c.file_id)
                            FROM (SELECT r.file_id, r."order" FROM legacy.ik_file_refs r
                                  WHERE r.related_type = 'api::article.article' AND r.field = 'cover'
                                    AND r.related_id = d.id
                                  ORDER BY r."order", r.file_id LIMIT 40) c), '{}'),
                  coalesce(d.updated_at AT TIME ZONE $1, $2)
           FROM legacy.ik_posts p
           JOIN legacy.articles d ON d.id = p.pending_row
           JOIN posts np ON np.id = p.post_id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 帖子的标签按原顺序，最多 5 个；合并过的标签改指存活的那个。
async fn post_tags(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO post_tags (post_id, tag_id, position)
           SELECT x.post_id, x.tag_id, (x.rn - 1)::smallint
           FROM (
               SELECT d.post_id, d.tag_id,
                      row_number() OVER (PARTITION BY d.post_id ORDER BY d.ord, d.tag_id) AS rn
               FROM (
                   SELECT DISTINCT ON (p.post_id, t.survivor_id)
                          p.post_id, t.survivor_id AS tag_id, l.tag_ord AS ord
                   FROM legacy.ik_posts p
                   JOIN posts np ON np.id = p.post_id
                   JOIN legacy.articles_tags_lnk l ON l.article_id = p.main_row
                   JOIN legacy.ik_tags t ON t.id = l.tag_id
                   ORDER BY p.post_id, t.survivor_id, l.tag_ord
               ) d
           ) x
           WHERE x.rn <= 5"#
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 评论：楼层号、一层楼中楼、置顶与采纳只留在顶层评论上（comments_flags_top_level）。
async fn comments(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO comments (id, post_id, parent_id, author_id, floor, body, is_anonymous,
                                 anonymous_seed, pinned_at, pinned_by, accepted_at, accepted_by,
                                 hidden_at, hidden_reason, legacy_id, created_at, image_ids)
           SELECT c.id, c.post_id, c.parent_id, c.author_id, c.floor::int,
                  left(legacy.ik_fix_hosts(c.content), 2000),
                  c.is_anonymous,
                  CASE WHEN c.is_anonymous
                       THEN coalesce(c.anonymous_seed, md5('legacy-comment:' || coalesce(c.document_id, c.id::text)))
                       ELSE c.anonymous_seed END,
                  CASE WHEN c.parent_id IS NULL THEN c.pinned_at AT TIME ZONE $1 END,
                  CASE WHEN c.parent_id IS NULL AND c.pinned_at IS NOT NULL THEN pinner.id END,
                  CASE WHEN c.parent_id IS NULL THEN c.accepted_at AT TIME ZONE $1 END,
                  CASE WHEN c.parent_id IS NULL AND c.accepted_at IS NOT NULL THEN acceptor.id END,
                  CASE WHEN c.is_hidden
                       THEN coalesce(c.updated_at, c.created_at) AT TIME ZONE $1 END,
                  CASE WHEN c.is_hidden THEN c.hidden_reason END,
                  c.document_id,
                  coalesce(c.created_at AT TIME ZONE $1, $2),
                  coalesce((SELECT array_agg(i.file_id ORDER BY i."order", i.file_id)
                            FROM (SELECT r.file_id, r."order" FROM legacy.ik_file_refs r
                                  WHERE r.related_type = 'api::comment.comment' AND r.field = 'images'
                                    AND r.related_id = c.id
                                  ORDER BY r."order", r.file_id LIMIT 18) i), '{}')
           FROM legacy.ik_comments c
           JOIN posts p ON p.id = c.post_id
           LEFT JOIN users pinner ON pinner.id = c.pinned_by
           LEFT JOIN users acceptor ON acceptor.id = c.accepted_by"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 采纳的评论：旧系统存的是评论的 documentId；只认同一帖子下的顶层评论。
async fn accepted(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"UPDATE posts np SET accepted_comment_id = c.id
           FROM legacy.ik_posts p
           JOIN comments c ON c.legacy_id = p.accepted_comment_document_id
           WHERE np.id = p.post_id AND c.post_id = np.id AND c.parent_id IS NULL"#
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 主页精选：旧系统存文章 documentId 数组，换成帖子 id，最多 6 个，只保留本人仍存在的帖子。
async fn pinned(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO user_profiles (user_id, pinned_post_ids, updated_at)
           SELECT x.user_id, x.ids, $1
           FROM (
               SELECT pr.user_id,
                      (array_agg(np.id ORDER BY d.ord))[1:6] AS ids
               FROM legacy.ik_profiles pr
               CROSS JOIN LATERAL jsonb_array_elements_text(pr.pinned_document_ids)
                    WITH ORDINALITY AS d(document_id, ord)
               JOIN posts np ON np.legacy_id = d.document_id AND np.author_id = pr.user_id
               WHERE pr.pinned_document_ids IS NOT NULL
               GROUP BY pr.user_id
           ) x
           ON CONFLICT (user_id) DO UPDATE SET pinned_post_ids = EXCLUDED.pinned_post_ids"#,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}
