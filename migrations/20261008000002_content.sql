-- 分区、标签、帖子。业务规则见 docs/business-rules/posts.md，设计说明见 docs/schema.md。

CREATE TABLE categories (
    id                    BIGINT PRIMARY KEY,
    slug                  TEXT        NOT NULL,
    name                  TEXT        NOT NULL,
    description           TEXT        NOT NULL DEFAULT '',
    sort_order            INTEGER     NOT NULL DEFAULT 0,
    is_active             BOOLEAN     NOT NULL DEFAULT TRUE,
    -- 只有管理员能在该分区发帖（POST-033）。
    admin_only            BOOLEAN     NOT NULL DEFAULT FALSE,
    -- 是否允许帖主采纳评论（POST-034）。
    allow_comment_accept  BOOLEAN     NOT NULL DEFAULT FALSE,
    legacy_id             TEXT,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT categories_slug_uq UNIQUE (slug),
    CONSTRAINT categories_legacy_id_uq UNIQUE (legacy_id),
    CONSTRAINT categories_slug_shape CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,47}$'),
    CONSTRAINT categories_name_len CHECK (char_length(name) BETWEEN 1 AND 30)
);

CREATE TABLE tags (
    id          BIGINT PRIMARY KEY,
    -- 归一化后的展示名（POST-021）。
    name        TEXT        NOT NULL,
    -- 由 name 派生，唯一（POST-022）。保留 + 与 #：c / c++ / c# 是三个标签。
    slug        TEXT        NOT NULL,
    is_blocked  BOOLEAN     NOT NULL DEFAULT FALSE,
    legacy_id   TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT tags_slug_uq UNIQUE (slug),
    CONSTRAINT tags_legacy_id_uq UNIQUE (legacy_id),
    CONSTRAINT tags_name_len CHECK (char_length(name) BETWEEN 1 AND 30),
    CONSTRAINT tags_slug_len CHECK (char_length(slug) BETWEEN 1 AND 60)
);

CREATE TABLE posts (
    id                   BIGINT PRIMARY KEY,
    author_id            BIGINT      NOT NULL REFERENCES users (id),
    category_id          BIGINT      REFERENCES categories (id) ON DELETE SET NULL,
    status               TEXT        NOT NULL DEFAULT 'draft',
    title                TEXT        NOT NULL,
    -- Markdown 正文。上限按等级在服务端校验（POST-012），这里只兜底最高档（D-014 方案 A）。
    body                 TEXT        NOT NULL DEFAULT '',
    -- 编辑器状态，只回给作者本人的编辑接口（POST-004）。
    editor_state         JSONB,
    -- 规范化后的外部视频（B 站），最多 1 个（POST-005）。
    external_videos      JSONB       NOT NULL DEFAULT '[]',
    is_anonymous         BOOLEAN     NOT NULL DEFAULT FALSE,
    -- 服务端生成的随机串，派生匿名展示名；永不下发（POST-090）。旧数据原样迁移以保持匿名名不变。
    anonymous_seed       TEXT,
    -- 乐观并发版本号：每次改动工作副本 +1，对外是 ETag "rev:<n>"。
    revision             INTEGER     NOT NULL DEFAULT 1,

    first_published_at   TIMESTAMPTZ,
    published_at         TIMESTAMPTZ,
    edited_at            TIMESTAMPTZ,
    -- 信息流排序键：首次发布与新评论时刷新（POST-052/062），编辑重发不刷新（POST-100）。
    bumped_at            TIMESTAMPTZ,
    -- 最后一次顶帖的人。只用于计算 bumpedBySelf，原值永不下发（POST-063）。
    last_bumped_by       BIGINT,

    pinned_at            TIMESTAMPTZ,
    pinned_by            BIGINT      REFERENCES users (id),
    solved_at            TIMESTAMPTZ,
    accepted_comment_id  BIGINT,

    -- D-009：用时间戳代替布尔，NULL = 可见，消除三态并保留审计信息。
    hidden_at            TIMESTAMPTZ,
    hidden_by            BIGINT      REFERENCES users (id),
    hidden_reason        TEXT,

    view_count           BIGINT      NOT NULL DEFAULT 0,
    like_count           INTEGER     NOT NULL DEFAULT 0,
    comment_count        INTEGER     NOT NULL DEFAULT 0,
    favorite_count       INTEGER     NOT NULL DEFAULT 0,
    denny_count          INTEGER     NOT NULL DEFAULT 0,

    legacy_id            TEXT,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at           TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT posts_legacy_id_uq UNIQUE (legacy_id),
    CONSTRAINT posts_status_check CHECK (status IN ('draft', 'published')),
    CONSTRAINT posts_title_len CHECK (char_length(title) BETWEEN 1 AND 50),
    CONSTRAINT posts_body_len CHECK (char_length(body) <= 16000),
    CONSTRAINT posts_videos_shape CHECK (
        jsonb_typeof(external_videos) = 'array' AND jsonb_array_length(external_videos) <= 1
    ),
    CONSTRAINT posts_anonymous_seed CHECK (NOT is_anonymous OR anonymous_seed IS NOT NULL),
    CONSTRAINT posts_published_fields CHECK (
        status = 'draft'
        OR (published_at IS NOT NULL AND first_published_at IS NOT NULL AND bumped_at IS NOT NULL)
    ),
    CONSTRAINT posts_counters_nonneg CHECK (
        view_count >= 0 AND like_count >= 0 AND comment_count >= 0
        AND favorite_count >= 0 AND denny_count >= 0
    )
);

-- 全站信息流与分区信息流：只覆盖公开帖子。
CREATE INDEX posts_feed_idx ON posts (bumped_at DESC, id DESC)
    WHERE status = 'published' AND hidden_at IS NULL;
CREATE INDEX posts_category_feed_idx ON posts (category_id, bumped_at DESC, id DESC)
    WHERE status = 'published' AND hidden_at IS NULL;
CREATE INDEX posts_pinned_idx ON posts (pinned_at DESC) WHERE pinned_at IS NOT NULL;
-- 我的帖子 + author_id 外键。
CREATE INDEX posts_author_idx ON posts (author_id, updated_at DESC, id DESC);
CREATE INDEX posts_category_fk_idx ON posts (category_id) WHERE category_id IS NOT NULL;
CREATE INDEX posts_pinned_by_idx ON posts (pinned_by) WHERE pinned_by IS NOT NULL;
CREATE INDEX posts_hidden_by_idx ON posts (hidden_by) WHERE hidden_by IS NOT NULL;

CREATE TABLE post_tags (
    post_id   BIGINT   NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    tag_id    BIGINT   NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    -- 作者给出的顺序。
    position  SMALLINT NOT NULL,
    PRIMARY KEY (post_id, tag_id),
    CONSTRAINT post_tags_position_range CHECK (position BETWEEN 0 AND 4)
);

CREATE INDEX post_tags_tag_idx ON post_tags (tag_id, post_id);

-- 已发布帖子的“待发布修改”（每帖最多一份）。草稿状态的帖子直接改 posts 行，不用这张表。
-- 发布时把这里的内容写回 posts 并删除本行；帖子行本身不换，点赞/评论等关系天然保留（POST-102）。
CREATE TABLE post_drafts (
    post_id          BIGINT      PRIMARY KEY REFERENCES posts (id) ON DELETE CASCADE,
    title            TEXT        NOT NULL,
    body             TEXT        NOT NULL DEFAULT '',
    editor_state     JSONB,
    external_videos  JSONB       NOT NULL DEFAULT '[]',
    category_id      BIGINT      REFERENCES categories (id) ON DELETE SET NULL,
    tag_ids          BIGINT[]    NOT NULL DEFAULT '{}',
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT post_drafts_title_len CHECK (char_length(title) BETWEEN 1 AND 50),
    CONSTRAINT post_drafts_body_len CHECK (char_length(body) <= 16000),
    CONSTRAINT post_drafts_videos_shape CHECK (
        jsonb_typeof(external_videos) = 'array' AND jsonb_array_length(external_videos) <= 1
    ),
    CONSTRAINT post_drafts_tag_count CHECK (cardinality(tag_ids) <= 5)
);

CREATE INDEX post_drafts_category_fk_idx ON post_drafts (category_id) WHERE category_id IS NOT NULL;
