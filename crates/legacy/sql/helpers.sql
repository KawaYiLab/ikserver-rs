-- 导入工具自己在 legacy schema 里建的辅助视图与表（幂等，每次 check / import 前执行）。
-- 映射规则集中在这里，导入步骤只读这些视图；名字一律以 ik_ 开头，与 Strapi 的表区分。
-- 编译期检查同样需要它们：开发库与 CI 在加载 strapi-schema.sql 之后执行本文件。

-- 用户与作者一一对应（ACC-018）。旧系统的公开主页链接用作者的 documentId（RPTADM-015），
-- 所以 users.legacy_id 优先取作者的 documentId，供旧链接 301 跳转。
CREATE OR REPLACE VIEW legacy.ik_user_authors AS
SELECT DISTINCT ON (l.user_id) l.user_id, l.author_id, a.document_id AS author_document_id
FROM legacy.up_users_author_lnk l
JOIN legacy.authors a ON a.id = l.author_id
WHERE l.user_id IS NOT NULL
ORDER BY l.user_id, l.id;

-- 账号的规范化结果。
--   status：注销 → deleted，合并 → merged，blocked = true → blocked，其余 active（D-019 裁决）。
--   email：trim + 小写；米游社占位邮箱、注销脱敏邮箱、注销 / 合并账号一律 NULL（D-069）。
--   username：去掉 '@'（换成全角 '＠'，ACC-009）、截到 32 字；为空、或处理后与别人撞名（原名本来就
--             合法的一方保留）时改成「用户_<id>」。
CREATE OR REPLACE VIEW legacy.ik_users AS
WITH base AS (
    SELECT
        u.*,
        CASE
            WHEN u.account_status = 'deleted' THEN 'deleted'
            WHEN u.account_status = 'merged' THEN 'merged'
            WHEN u.blocked THEN 'blocked'
            ELSE 'active'
        END AS new_status,
        lower(btrim(u.email)) AS email_lower,
        left(replace(btrim(coalesce(u.username, '')), '@', '＠'), 32) AS name_fixed
    FROM legacy.up_users u
),
named AS (
    SELECT
        b.*,
        -- 同名时原名本来就合法的账号优先保留，其次按 id。
        row_number() OVER (
            PARTITION BY b.name_fixed
            ORDER BY (b.name_fixed = b.username) DESC, b.id
        ) AS name_rank
    FROM base b
)
SELECT
    n.id,
    n.document_id,
    n.new_status AS status,
    CASE
        WHEN n.new_status IN ('deleted', 'merged') THEN NULL
        WHEN n.email_lower IS NULL OR n.email_lower = '' THEN NULL
        WHEN n.email_lower LIKE '%@mihoyo-login.inter-knot.invalid' THEN NULL
        WHEN n.email_lower LIKE '%@anonymized.local' THEN NULL
        ELSE n.email_lower
    END AS email,
    CASE
        WHEN n.name_fixed = '' OR n.name_rank > 1 THEN '用户_' || n.id
        ELSE n.name_fixed
    END AS username,
    -- 改过名的账号要在报告里列出来。
    (n.name_fixed = '' OR n.name_rank > 1 OR n.name_fixed IS DISTINCT FROM n.username) AS renamed,
    n.username AS original_username,
    n.password,
    n.provider,
    n.is_admin,
    n.is_ai_agent,
    n.exam_passed,
    n.exam_passed_at,
    n.level,
    n.created_at,
    n.updated_at
FROM named n;

-- 个人资料：签名（作者的 bio 是 Strapi blocks 富文本，取出全部文字节点，段落之间换行）、隐藏主页、
-- 主页精选（作者的 profilePinnedArticles 是文章 documentId 数组，帖子导入后再换成帖子 id）。
CREATE OR REPLACE VIEW legacy.ik_profiles AS
SELECT
    u.id AS user_id,
    coalesce((
        SELECT string_agg(par.text, E'\n' ORDER BY par.ord)
        FROM jsonb_array_elements(CASE WHEN jsonb_typeof(a.bio) = 'array' THEN a.bio ELSE '[]'::jsonb END)
             WITH ORDINALITY AS blk(block, ord)
        CROSS JOIN LATERAL (
            SELECT blk.ord, string_agg(t #>> '{}', '') AS text
            FROM jsonb_path_query(blk.block, 'strict $.**.text') AS t
            WHERE jsonb_typeof(t) = 'string'
        ) par
    ), '') AS bio_text,
    coalesce(u.profile_hidden, false) AS hidden,
    CASE WHEN jsonb_typeof(a.profile_pinned_articles) = 'array' THEN a.profile_pinned_articles END
        AS pinned_document_ids
FROM legacy.up_users u
LEFT JOIN legacy.ik_user_authors ua ON ua.user_id = u.id
LEFT JOIN legacy.authors a ON a.id = ua.author_id;
