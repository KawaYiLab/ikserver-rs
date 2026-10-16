//! 预检：只读地统计旧数据里需要注意的情况，导入前先跑一遍、交给运维或产品确认。
//!
//! - `Blocking`：导入会拒绝执行，必须先在旧库里处理（例如规范化后重复的邮箱，ACC-007）。
//! - `Fixed`：导入时按固定规则自动修正，报告里列出数量（例如超长的签名截断）。
//! - `Info`：只是告知（例如数据量、时间列的时区判断依据）。

use serde::Serialize;
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocking,
    Fixed,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub key: &'static str,
    pub severity: Severity,
    pub count: i64,
    pub message: String,
}

impl Finding {
    fn new(key: &'static str, severity: Severity, count: i64, message: impl Into<String>) -> Self {
        Self {
            key,
            severity,
            count,
            message: message.into(),
        }
    }
}

/// 跑全部检查。只报告 `count > 0` 的项（`Info` 总是报告）。
pub async fn check(db: &PgPool) -> anyhow::Result<Vec<Finding>> {
    crate::install_helpers(db).await?;
    let mut out = Vec::new();
    accounts(db, &mut out).await?;
    media(db, &mut out).await?;
    content(db, &mut out).await?;
    out.retain(|f| f.severity == Severity::Info || f.count > 0);
    Ok(out)
}

async fn accounts(db: &PgPool, out: &mut Vec<Finding>) -> anyhow::Result<()> {
    let users = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM legacy.up_users"#)
        .fetch_one(db)
        .await?;
    out.push(Finding::new(
        "users.total",
        Severity::Info,
        users,
        "旧库账号总数",
    ));

    let dup_email = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM (
               SELECT email FROM legacy.ik_users WHERE email IS NOT NULL
               GROUP BY email HAVING count(*) > 1
           ) d"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "users.duplicate_email",
        Severity::Blocking,
        dup_email,
        "规范化（trim + 小写）后重复的邮箱：先在旧库合并或改掉，再导入（ACC-007）",
    ));

    let renamed =
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM legacy.ik_users WHERE renamed"#)
            .fetch_one(db)
            .await?;
    out.push(Finding::new(
        "users.renamed",
        Severity::Fixed,
        renamed,
        "用户名含 '@'、为空、超过 32 字或处理后撞名：'@' 换成全角、截断，撞名的改为「用户_<id>」",
    ));

    let long_bio = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM legacy.ik_profiles WHERE char_length(bio_text) > 100"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "profiles.bio_truncated",
        Severity::Fixed,
        long_bio,
        "个人签名超过 100 字，截断到 100 字",
    ));

    // 判断旧库时间列存的是哪个时区：最新一条记录分别按 UTC 和北京时间解读，
    // 与导出时间对照，明显「在未来」的那种解读就是错的。
    let latest = sqlx::query_scalar!(r#"SELECT max(created_at)::text AS "t" FROM legacy.up_users"#)
        .fetch_one(db)
        .await?;
    out.push(Finding::new(
        "time.latest_user_created_at",
        Severity::Info,
        0,
        format!(
            "最新注册时间（原样）：{}。按 UTC 解读与按 Asia/Shanghai 解读相差 8 小时；\
             对照导出旧库的时刻判断 --legacy-timezone 该填哪个",
            latest.as_deref().unwrap_or("（无数据）")
        ),
    ));
    Ok(())
}

async fn media(db: &PgPool, out: &mut Vec<Finding>) -> anyhow::Result<()> {
    let missing = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM legacy.files f
           WHERE NOT EXISTS (SELECT 1 FROM legacy.ik_media_digests d WHERE d.file_id = f.id)"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "media.missing_digest",
        Severity::Blocking,
        missing,
        "还没读取过的文件：先运行 `ik-server legacy media`",
    ));

    let failed = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM legacy.ik_media_digests WHERE error IS NOT NULL"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "media.failed",
        Severity::Fixed,
        failed,
        "读取失败的文件（不存在、不是图片、超过 30 MB……）不导入，引用它们的封面、配图、头像随之去掉；\
         原因见 legacy.ik_media_digests.error，可以用 --retry-failed 重试",
    ));
    Ok(())
}

async fn content(db: &PgPool, out: &mut Vec<Finding>) -> anyhow::Result<()> {
    let counts = sqlx::query!(
        r#"SELECT
               count(*) AS "posts!",
               count(*) FILTER (WHERE author_id IS NULL) AS "no_author!",
               count(*) FILTER (WHERE char_length(btrim(coalesce(title, ''))) > 50) AS "long_title!",
               count(*) FILTER (WHERE char_length(text) > 16000) AS "long_body!",
               count(*) FILTER (WHERE is_hidden IS NULL) AS "hidden_null!",
               count(*) FILTER (WHERE jsonb_typeof(external_videos) = 'array'
                                  AND jsonb_array_length(external_videos) > 1) AS "extra_videos!"
           FROM legacy.ik_posts"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "posts.total",
        Severity::Info,
        counts.posts,
        "帖子数（草稿行与发布行合并后）",
    ));
    out.push(Finding::new(
        "posts.no_author",
        Severity::Fixed,
        counts.no_author,
        "作者已不存在的帖子不导入（连同它的评论、点赞等）",
    ));
    out.push(Finding::new(
        "posts.title_truncated",
        Severity::Fixed,
        counts.long_title,
        "标题超过 50 字，截断",
    ));
    out.push(Finding::new(
        "posts.body_truncated",
        Severity::Fixed,
        counts.long_body,
        "正文超过 16000 字，截断",
    ));
    out.push(Finding::new(
        "posts.extra_videos",
        Severity::Fixed,
        counts.extra_videos,
        "外链视频多于 1 个，只保留第一个",
    ));
    out.push(Finding::new(
        "posts.hidden_null",
        Severity::Info,
        counts.hidden_null,
        "隐藏标记为 NULL 的帖子，按可见导入（D-012）",
    ));

    let blocks = sqlx::query_scalar!(
        r#"SELECT count(DISTINCT entity_id) AS "n!" FROM legacy.articles_cmps"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "posts.blocks_dropped",
        Severity::Fixed,
        blocks,
        "使用了 Strapi 动态区块（blocks）的文章行：新系统没有这个字段，区块内容不导入",
    ));

    let tags = sqlx::query!(
        r#"SELECT count(*) FILTER (WHERE id <> survivor_id) AS "merged!" FROM legacy.ik_tags"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "tags.merged",
        Severity::Fixed,
        tags.merged,
        "slug 相同的标签合并到 id 最小的那个",
    ));

    let slugs = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM legacy.ik_categories
           WHERE new_slug IS DISTINCT FROM slug OR new_name IS DISTINCT FROM name"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "categories.fixed",
        Severity::Fixed,
        slugs,
        "分区 slug 不合规改为 c-<id>，名称超过 30 字截断",
    ));

    let comments = sqlx::query!(
        r#"SELECT
               (SELECT count(*) FROM legacy.comments) AS "total!",
               count(*) AS "kept!",
               count(*) FILTER (WHERE reparented) AS "reparented!",
               count(*) FILTER (WHERE char_length(content) > 2000) AS "long_body!"
           FROM legacy.ik_comments c
           WHERE EXISTS (SELECT 1 FROM legacy.ik_posts p
                         WHERE p.post_id = c.post_id AND p.author_id IS NOT NULL)"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "comments.dropped",
        Severity::Fixed,
        comments.total - comments.kept,
        "所在帖子或作者已不存在的评论不导入",
    ));
    out.push(Finding::new(
        "comments.reparented",
        Severity::Fixed,
        comments.reparented,
        "回复的回复（楼中楼只有一层，CMT-025）挂到所属的顶层评论下",
    ));
    out.push(Finding::new(
        "comments.body_truncated",
        Severity::Fixed,
        comments.long_body,
        "评论超过 2000 字，截断",
    ));
    Ok(())
}
