SET client_min_messages = warning;
-- 导入工具自己在 legacy schema 里建的辅助视图与表（幂等，每次 check / import 前执行）。
-- 映射规则集中在这里，导入步骤只读这些视图；名字一律以 ik_ 开头，与 Strapi 的表区分。
-- 编译期检查同样需要它们：开发库与 CI 在加载 strapi-schema.sql 之后执行本文件。

-- 视图每次重建（改列时 CREATE OR REPLACE 不行）；摘要表 ik_media_digests 是数据，不删。
DO $$
DECLARE v record;
BEGIN
    FOR v IN SELECT viewname FROM pg_views WHERE schemaname = 'legacy' AND viewname LIKE 'ik\_%' LOOP
        EXECUTE format('DROP VIEW IF EXISTS legacy.%I CASCADE', v.viewname);
    END LOOP;
END $$;

-- Strapi blocks 富文本 → 纯文本：取每个块里全部文字节点（含链接里的），块之间用 sep 连接。
CREATE OR REPLACE FUNCTION legacy.ik_blocks_text(blocks JSONB, sep TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
    SELECT coalesce(string_agg(par.text, sep ORDER BY blk.ord), '')
    FROM jsonb_array_elements(CASE WHEN jsonb_typeof(blocks) = 'array' THEN blocks ELSE '[]'::jsonb END)
         WITH ORDINALITY AS blk(block, ord)
    CROSS JOIN LATERAL (
        SELECT string_agg(t #>> '{}', '') AS text
        FROM jsonb_path_query(blk.block, 'strict $.**.text') AS t
        WHERE jsonb_typeof(t) = 'string'
    ) par
$$;

-- 用户与作者一一对应（ACC-018）。旧系统的公开主页链接用作者的 documentId（RPTADM-015），
-- 所以 users.legacy_id 优先取作者的 documentId，供旧链接 301 跳转。
CREATE VIEW legacy.ik_user_authors AS
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
CREATE VIEW legacy.ik_users AS
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
CREATE VIEW legacy.ik_profiles AS
SELECT
    u.id AS user_id,
    legacy.ik_blocks_text(a.bio, E'\n') AS bio_text,
    coalesce(u.profile_hidden, false) AS hidden,
    CASE WHEN jsonb_typeof(a.profile_pinned_articles) = 'array' THEN a.profile_pinned_articles END
        AS pinned_document_ids
FROM legacy.up_users u
LEFT JOIN legacy.ik_user_authors ua ON ua.user_id = u.id
LEFT JOIN legacy.authors a ON a.id = ua.author_id;

-- 媒体摘要（`ik-server legacy media` 预先读取对象存储得到，可续跑；导入只用 error IS NULL 的行）。
-- 读对象存储是外部调用，不能放进导入事务，所以单独一步、单独一张表。
CREATE TABLE IF NOT EXISTS legacy.ik_media_digests (
    file_id     INTEGER     PRIMARY KEY,
    object_key  TEXT,
    sha256      TEXT,
    mime        TEXT,
    size_bytes  BIGINT,
    thumbhash   TEXT,
    -- 读取失败的原因（对象不存在、不是图片、超过 30 MB、URL 前缀不认识……）；成功时为 NULL。
    error       TEXT,
    fetched_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- 作者 → 账号（作者没有对应账号时为空，这类作者的内容不导入）。每个作者取最早的一条链接。
-- 帖子、评论按作者逐行查这张视图：写成 NOT EXISTS 才能按 author_id 走索引；DISTINCT ON
-- 无法下推连接条件，每查一行都要把整张链接表排一遍（5 万条评论要跑一分多钟）。
CREATE VIEW legacy.ik_author_users AS
SELECT l.author_id, l.user_id
FROM legacy.up_users_author_lnk l
WHERE l.author_id IS NOT NULL AND l.user_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM legacy.up_users_author_lnk e
      WHERE e.author_id = l.author_id AND e.user_id IS NOT NULL AND e.id < l.id
  );

-- 帖子：Strapi 的草稿行与发布行（同一 document_id 两行，POST-001）合并成一条，帖子 id 取该文档
-- 最小的行 id。有发布行 → published，内容取发布行；只有草稿行 → draft。
-- 评论、通知按行 id 逐行查这张视图：用相关子查询（两次索引查找），不用窗口函数（窗口里的
-- 条件下推不进去，每查一行都要扫全部文章，20 万条通知要跑一个多小时）。
CREATE VIEW legacy.ik_post_rows AS
SELECT a.id AS row_id,
       (SELECT min(d.id) FROM legacy.articles d WHERE d.document_id = a.document_id) AS post_id
FROM legacy.articles a
WHERE a.document_id IS NOT NULL;

CREATE VIEW legacy.ik_posts AS
WITH docs AS (
    SELECT
        a.document_id,
        min(a.id) AS post_id,
        (array_agg(a.id ORDER BY a.id DESC) FILTER (WHERE a.published_at IS NOT NULL))[1] AS pub_row,
        (array_agg(a.id ORDER BY a.id DESC) FILTER (WHERE a.published_at IS NULL))[1] AS draft_row,
        min(a.created_at) AS created_at,
        max(a.updated_at) AS updated_at
    FROM legacy.articles a
    WHERE a.document_id IS NOT NULL
    GROUP BY a.document_id
)
SELECT
    d.post_id,
    d.document_id,
    m.id AS main_row,
    d.pub_row IS NOT NULL AS published,
    -- 已发布帖子的待发布修改（post_drafts）：草稿行的内容与发布行不同时才算。
    CASE WHEN d.pub_row IS NOT NULL AND d.draft_row IS NOT NULL
              AND (dr.title, dr.text, dr.editor_state::text, dr.external_videos::text)
                  IS DISTINCT FROM (m.title, m.text, m.editor_state::text, m.external_videos::text)
         THEN d.draft_row END AS pending_row,
    (SELECT au.user_id FROM legacy.articles_author_lnk l
     JOIN legacy.ik_author_users au ON au.author_id = l.author_id
     WHERE l.article_id = m.id ORDER BY l.id LIMIT 1) AS author_id,
    (SELECT l.category_id FROM legacy.articles_category_lnk l
     WHERE l.article_id = m.id ORDER BY l.id LIMIT 1) AS category_id,
    (SELECT l.user_id FROM legacy.articles_pinned_by_lnk l
     WHERE l.article_id = m.id ORDER BY l.id LIMIT 1) AS pinned_by,
    m.title,
    m.text,
    m.editor_state,
    m.external_videos,
    coalesce(m.is_anonymous, false) AS is_anonymous,
    m.anonymous_seed,
    m.is_hidden,
    m.hidden_reason,
    m.bumped_at,
    m.last_bumped_by_user_id,
    m.edited_at,
    m.views,
    coalesce(m.is_pinned, false) AS is_pinned,
    m.pinned_at,
    m.solved_at,
    m.accepted_comment_document_id,
    m.published_at,
    m.first_published_at,
    m.updated_at AS row_updated_at,
    d.created_at,
    d.updated_at
FROM docs d
JOIN legacy.articles m ON m.id = coalesce(d.pub_row, d.draft_row)
LEFT JOIN legacy.articles dr ON dr.id = d.draft_row;

-- 评论：帖子按文档换成帖子 id；楼中楼只有一层（CMT-025），旧数据里回复的回复挂到它的顶层评论下；
-- 顶层评论按时间编楼层号。帖子或作者已不存在的评论不导入。
CREATE VIEW legacy.ik_comments AS
WITH RECURSIVE base AS (
    SELECT
        c.*,
        (SELECT pr.post_id FROM legacy.comments_article_lnk l
         JOIN legacy.ik_post_rows pr ON pr.row_id = l.article_id
         WHERE l.comment_id = c.id ORDER BY l.id LIMIT 1) AS post_id,
        (SELECT au.user_id FROM legacy.comments_author_lnk l
         JOIN legacy.ik_author_users au ON au.author_id = l.author_id
         WHERE l.comment_id = c.id ORDER BY l.id LIMIT 1) AS author_id,
        (SELECT l.inv_comment_id FROM legacy.comments_parent_lnk l
         WHERE l.comment_id = c.id ORDER BY l.id LIMIT 1) AS raw_parent_id
    FROM legacy.comments c
),
roots AS (
    SELECT b.id, b.id AS root_id, 0 AS depth FROM base b WHERE b.raw_parent_id IS NULL
    UNION ALL
    SELECT b.id, r.root_id, r.depth + 1
    FROM base b JOIN roots r ON b.raw_parent_id = r.id
    WHERE r.depth < 50
),
usable AS (
    SELECT b.*, r.root_id, r.depth
    FROM base b
    JOIN roots r ON r.id = b.id
    WHERE b.post_id IS NOT NULL AND b.author_id IS NOT NULL
)
SELECT
    u.id,
    u.document_id,
    u.post_id,
    u.author_id,
    CASE WHEN u.depth = 0 THEN NULL ELSE u.root_id END AS parent_id,
    u.depth > 1 AS reparented,
    CASE WHEN u.depth = 0 THEN
        row_number() OVER (PARTITION BY u.post_id, (u.depth = 0) ORDER BY u.created_at, u.id)
    END AS floor,
    u.content,
    coalesce(u.is_anonymous, false) AS is_anonymous,
    u.anonymous_seed,
    u.is_hidden,
    u.hidden_reason,
    u.pinned_at,
    (SELECT l.user_id FROM legacy.comments_pinned_by_lnk l WHERE l.comment_id = u.id
     ORDER BY l.id LIMIT 1) AS pinned_by,
    u.accepted_at,
    (SELECT l.user_id FROM legacy.comments_accepted_by_lnk l WHERE l.comment_id = u.id
     ORDER BY l.id LIMIT 1) AS accepted_by,
    u.created_at,
    u.updated_at
FROM usable u
-- 顶层评论本身被丢弃（帖子或作者不存在）时，它的回复也不导入。
WHERE u.depth = 0 OR EXISTS (
    SELECT 1 FROM base p WHERE p.id = u.root_id AND p.post_id IS NOT NULL AND p.author_id IS NOT NULL
);

-- 正文里的旧图床域名改成现在的（CDN-001）。
CREATE OR REPLACE FUNCTION legacy.ik_fix_hosts(body TEXT) RETURNS TEXT
LANGUAGE sql IMMUTABLE AS $$
    SELECT regexp_replace(coalesce(body, ''), '(https?:)?//image\.tiwat\.cn/', 'https://im.tiwat.cn/', 'g')
$$;

-- 分区：slug 不符合新格式（小写字母数字与连字符）的改为 c-<id>。
CREATE VIEW legacy.ik_categories AS
SELECT
    c.*,
    CASE WHEN lower(btrim(c.slug)) ~ '^[a-z0-9][a-z0-9-]{0,47}$' THEN lower(btrim(c.slug))
         ELSE 'c-' || c.id END AS new_slug,
    coalesce(nullif(left(btrim(c.name), 30), ''), 'c-' || c.id) AS new_name
FROM legacy.categories c;

-- 标签：slug 相同的合并到 id 最小的那个（survivor_id），帖子上的标签随之改指过去。
CREATE VIEW legacy.ik_tags AS
WITH fixed AS (
    SELECT
        t.*,
        coalesce(nullif(left(btrim(t.slug), 60), ''), 't-' || t.id) AS new_slug,
        coalesce(nullif(left(btrim(t.name), 30), ''), 't-' || t.id) AS new_name
    FROM legacy.tags t
)
SELECT f.*, min(f.id) OVER (PARTITION BY f.new_slug) AS survivor_id
FROM fixed f;

-- 帖子、评论、头像引用的文件：只认摘要读取成功的。
CREATE VIEW legacy.ik_file_refs AS
SELECT r.related_type, r.related_id, r.field, r.file_id, r."order"
FROM legacy.files_related_mph r
JOIN legacy.ik_media_digests d ON d.file_id = r.file_id AND d.error IS NULL;

-- 经验动作：旧名 → 新名、每日上限桶（EXP-009）、是否按日去重（新代码里用 Dedup::Daily 的动作）。
-- like_received / comment_received / favorite_received 只为保留历史流水（D-049）。
CREATE VIEW legacy.ik_exp_actions (legacy_action, action, bucket, daily) AS VALUES
    ('check_in', 'check_in', 'active', true),
    ('create_article', 'create_post', 'active', true),
    ('create_comment', 'create_comment', 'active', true),
    ('like_give', 'like_give', 'active', true),
    ('comment_accepted', 'comment_accepted', 'active', false),
    ('like_received', 'like_received', 'passive', false),
    ('comment_received', 'comment_received', 'passive', false),
    ('favorite_received', 'favorite_received', 'passive', false),
    ('denny_received', 'denny_received', 'passive', false),
    ('exam_passed', 'exam_passed', 'exempt', false),
    ('admin_grant', 'admin_grant', 'exempt', false);

-- 丁尼动作：旧名 → 新名。
CREATE VIEW legacy.ik_denny_actions (legacy_action, action) AS VALUES
    ('daily_login', 'check_in'),
    ('give_post', 'gift_sent'),
    ('receive_post_give', 'gift_received'),
    ('spend_rename', 'spend_rename'),
    ('spend_avatar', 'spend_avatar'),
    ('spend_delete_post', 'spend_delete_post'),
    ('refund', 'refund'),
    ('admin_grant', 'admin_grant'),
    ('exam_passed', 'exam_passed'),
    ('level_up', 'level_up');

-- 流水与签到的账号（关系在链接表里）。
CREATE VIEW legacy.ik_exp_log_users AS
SELECT DISTINCT ON (l.experience_log_id) l.experience_log_id AS log_id, l.user_id
FROM legacy.experience_logs_user_lnk l ORDER BY l.experience_log_id, l.id;
CREATE VIEW legacy.ik_denny_log_users AS
SELECT DISTINCT ON (l.denny_log_id) l.denny_log_id AS log_id, l.user_id
FROM legacy.denny_logs_user_lnk l ORDER BY l.denny_log_id, l.id;
CREATE VIEW legacy.ik_check_in_users AS
SELECT DISTINCT ON (l.check_in_log_id) l.check_in_log_id AS log_id, l.user_id
FROM legacy.check_in_logs_user_lnk l ORDER BY l.check_in_log_id, l.id;

-- 题库：题型、选项（trim、去空、key 去重）、正确答案（只留选项里有的 key）。不合格的题不导入
-- （选项少于 2 个或多于 26 个、没有正确答案、题干为空或超过 2000 字），预检里报告。
CREATE VIEW legacy.ik_exam_questions AS
WITH opts AS (
    SELECT q.id,
           coalesce((
               SELECT jsonb_agg(jsonb_build_object('key', o.key, 'text', o.text) ORDER BY o.ord)
               FROM (
                   SELECT DISTINCT ON (btrim(e.value ->> 'key'))
                          btrim(e.value ->> 'key') AS key, btrim(e.value ->> 'text') AS text, e.ord
                   FROM jsonb_array_elements(CASE WHEN jsonb_typeof(q.options) = 'array'
                                                  THEN q.options ELSE '[]'::jsonb END)
                        WITH ORDINALITY AS e(value, ord)
                   WHERE jsonb_typeof(e.value) = 'object'
                     AND btrim(coalesce(e.value ->> 'key', '')) <> ''
                     AND btrim(coalesce(e.value ->> 'text', '')) <> ''
                   ORDER BY btrim(e.value ->> 'key'), e.ord
               ) o
           ), '[]'::jsonb) AS options
    FROM legacy.exam_questions q
)
SELECT
    q.*,
    o.options AS new_options,
    CASE WHEN q.type IN ('single', 'multiple', 'boolean') THEN q.type ELSE 'single' END AS kind,
    coalesce((
        SELECT array_agg(DISTINCT btrim(k #>> '{}') ORDER BY btrim(k #>> '{}'))
        FROM jsonb_array_elements(CASE WHEN jsonb_typeof(q.correct_keys) = 'array'
                                       THEN q.correct_keys ELSE '[]'::jsonb END) AS k
        WHERE EXISTS (SELECT 1 FROM jsonb_array_elements(o.options) x
                      WHERE x ->> 'key' = btrim(k #>> '{}'))
    ), '{}') AS new_correct_keys
FROM legacy.exam_questions q
JOIN opts o ON o.id = q.id;

-- 装扮的图片（只认摘要读取成功的文件）。
CREATE VIEW legacy.ik_cosmetic_images AS
SELECT DISTINCT ON (r.related_type, r.related_id) r.related_type, r.related_id, r.file_id
FROM legacy.ik_file_refs r
WHERE r.field = 'image' AND r.related_type IN ('api::avatar.avatar', 'api::business-card.business-card')
ORDER BY r.related_type, r.related_id, r."order", r.file_id;

-- 私信成员（会话与账号都在链接表里）。
CREATE VIEW legacy.ik_conversation_members AS
SELECT m.*, cl.conversation_id, ul.user_id
FROM legacy.conversation_members m
JOIN LATERAL (SELECT l.conversation_id FROM legacy.conversation_members_conversation_lnk l
              WHERE l.conversation_member_id = m.id ORDER BY l.id LIMIT 1) cl ON true
JOIN LATERAL (SELECT l.user_id FROM legacy.conversation_members_user_lnk l
              WHERE l.conversation_member_id = m.id ORDER BY l.id LIMIT 1) ul ON true
WHERE cl.conversation_id IS NOT NULL AND ul.user_id IS NOT NULL;

-- 私信会话：只导入恰好两个不同成员的一对一会话（群聊不实现，D-073）。一方是 AI 账号的是 AI 会话
-- （kind = 'ai'，同一对可以有多条）；普通私聊同一对用户只留一条（survivor_id，最小的会话 id），
-- 其余会话的消息并进去。
CREATE VIEW legacy.ik_conversations AS
WITH pairs AS (
    SELECT c.id, c.context_epoch, c.last_message_at, c.created_at, c.updated_at,
           min(m.user_id) AS user_low, max(m.user_id) AS user_high, count(DISTINCT m.user_id) AS members
    FROM legacy.conversations c
    JOIN legacy.ik_conversation_members m ON m.conversation_id = c.id
    WHERE coalesce(c.kind, 'direct') = 'direct'
    GROUP BY c.id, c.context_epoch, c.last_message_at, c.created_at, c.updated_at
),
flagged AS (
    SELECT p.*,
           coalesce(lo.is_ai_agent, false) AS low_ai,
           coalesce(hi.is_ai_agent, false) AS high_ai
    FROM pairs p
    JOIN legacy.up_users lo ON lo.id = p.user_low
    JOIN legacy.up_users hi ON hi.id = p.user_high
    WHERE p.members = 2
)
SELECT
    f.id, f.context_epoch, f.last_message_at, f.created_at, f.updated_at, f.user_low, f.user_high,
    (f.low_ai <> f.high_ai) AS is_ai,
    CASE WHEN f.low_ai AND NOT f.high_ai THEN f.user_low
         WHEN f.high_ai AND NOT f.low_ai THEN f.user_high END AS ai_user_id,
    CASE WHEN f.low_ai AND NOT f.high_ai THEN f.user_high
         WHEN f.high_ai AND NOT f.low_ai THEN f.user_low END AS human_id,
    CASE WHEN f.low_ai <> f.high_ai THEN f.id
         ELSE min(f.id) FILTER (WHERE f.low_ai = f.high_ai)
                  OVER (PARTITION BY f.user_low, f.user_high, (f.low_ai = f.high_ai)) END AS survivor_id
FROM flagged f;

-- 私信消息的附件：摘要读取成功的前 4 张转成消息图片（MSG-030），其余（读不到的、超出 4 张的）
-- 只能把图片地址写进正文。
CREATE VIEW legacy.ik_message_attachments AS
SELECT
    r.related_id AS message_id, r.file_id, r."order",
    legacy.ik_fix_hosts(f.url) AS url,
    d.file_id IS NOT NULL
        AND row_number() OVER (PARTITION BY r.related_id, d.file_id IS NOT NULL
                               ORDER BY r."order", r.file_id) <= 4 AS as_image
FROM legacy.files_related_mph r
JOIN legacy.files f ON f.id = r.file_id
LEFT JOIN legacy.ik_media_digests d ON d.file_id = r.file_id AND d.error IS NULL
WHERE r.related_type = 'api::message.message' AND r.field = 'attachments';

-- 私信消息：会话换成存活的会话；附件按上面的规则转成图片（image_ids，媒体 id 就是旧文件 id）
-- 或正文里的图片地址；图片消息什么都取不到时正文为「[图片]」。
CREATE VIEW legacy.ik_messages AS
SELECT
    msg.*,
    c.survivor_id AS conversation_id,
    c.is_ai,
    (SELECT l.user_id FROM legacy.messages_sender_lnk l WHERE l.message_id = msg.id
     ORDER BY l.id LIMIT 1) AS sender_id,
    (SELECT l.inv_message_id FROM legacy.messages_reply_to_lnk l WHERE l.message_id = msg.id
     ORDER BY l.id LIMIT 1) AS reply_to_legacy_id,
    img.image_ids,
    left(coalesce(
        nullif(concat_ws(E'\n', nullif(btrim(coalesce(msg.content, '')), ''), rest.urls), ''),
        CASE WHEN msg.kind = 'image' AND cardinality(img.image_ids) = 0 THEN '[图片]' END,
        ''
    ), 4000) AS new_content
FROM legacy.messages msg
JOIN LATERAL (SELECT l.conversation_id FROM legacy.messages_conversation_lnk l
              WHERE l.message_id = msg.id ORDER BY l.id LIMIT 1) ml ON true
JOIN legacy.ik_conversations c ON c.id = ml.conversation_id
CROSS JOIN LATERAL (
    SELECT coalesce(array_agg(a.file_id ORDER BY a."order", a.file_id) FILTER (WHERE a.as_image),
                    '{}') AS image_ids
    FROM legacy.ik_message_attachments a WHERE a.message_id = msg.id
) img
CROSS JOIN LATERAL (
    SELECT string_agg(a.url, E'\n' ORDER BY a."order", a.file_id) AS urls
    FROM legacy.ik_message_attachments a
    WHERE a.message_id = msg.id AND NOT a.as_image AND a.url IS NOT NULL
) rest;
