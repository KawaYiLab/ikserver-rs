-- 评论与楼中楼。业务规则见 docs/business-rules/comments.md，设计说明见 docs/schema.md。

CREATE TABLE comments (
    id              BIGINT PRIMARY KEY,
    post_id         BIGINT      NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    -- 只有一层楼中楼：parent 必须是同帖的一级评论（CMT-025），由服务端保证。
    -- 删除一级评论时级联删除它的回复（CMT-048）。
    parent_id       BIGINT      REFERENCES comments (id) ON DELETE CASCADE,
    author_id       BIGINT      NOT NULL REFERENCES users (id),
    -- 一级评论的楼层号，发表时分配，之后不再变化（删除前面的评论不会让楼层号移动）。
    floor           INTEGER,
    -- 纯文本，内嵌表情 `:ik-xxx:` 与提及 `@[名字](用户id)` 两种 token（CMT-007）。
    body            TEXT        NOT NULL,
    is_anonymous    BOOLEAN     NOT NULL DEFAULT FALSE,
    -- 每条匿名评论独立的随机种子，与帖子的种子无关（CMT-125）。永不下发。
    anonymous_seed  TEXT,
    like_count      INTEGER     NOT NULL DEFAULT 0,
    pinned_at       TIMESTAMPTZ,
    pinned_by       BIGINT      REFERENCES users (id),
    accepted_at     TIMESTAMPTZ,
    accepted_by     BIGINT      REFERENCES users (id),
    hidden_at       TIMESTAMPTZ,
    hidden_by       BIGINT      REFERENCES users (id),
    hidden_reason   TEXT,
    legacy_id       TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT comments_legacy_id_uq UNIQUE (legacy_id),
    CONSTRAINT comments_floor_uq UNIQUE (post_id, floor),
    CONSTRAINT comments_body_len CHECK (char_length(body) <= 2000),
    CONSTRAINT comments_anonymous_seed CHECK (NOT is_anonymous OR anonymous_seed IS NOT NULL),
    CONSTRAINT comments_floor_top_level CHECK ((parent_id IS NULL) = (floor IS NOT NULL)),
    CONSTRAINT comments_flags_top_level CHECK (
        parent_id IS NULL OR (pinned_at IS NULL AND accepted_at IS NULL)
    ),
    CONSTRAINT comments_like_count_nonneg CHECK (like_count >= 0)
);

-- 每帖最多一条置顶、一条采纳（CMT-053/058），由 DDL 兜底。
CREATE UNIQUE INDEX comments_one_pinned_per_post ON comments (post_id) WHERE pinned_at IS NOT NULL;
CREATE UNIQUE INDEX comments_one_accepted_per_post ON comments (post_id) WHERE accepted_at IS NOT NULL;

-- 一级评论列表（新的在前）。
CREATE INDEX comments_top_level_idx ON comments (post_id, created_at DESC, id DESC)
    WHERE parent_id IS NULL AND hidden_at IS NULL;
-- 回复列表（旧的在前）+ parent_id 外键。
CREATE INDEX comments_replies_idx ON comments (parent_id, created_at, id);
CREATE INDEX comments_author_idx ON comments (author_id, created_at DESC);
CREATE INDEX comments_pinned_by_idx ON comments (pinned_by) WHERE pinned_by IS NOT NULL;
CREATE INDEX comments_accepted_by_idx ON comments (accepted_by) WHERE accepted_by IS NOT NULL;
CREATE INDEX comments_hidden_by_idx ON comments (hidden_by) WHERE hidden_by IS NOT NULL;

-- 帖子的采纳投影指向评论；评论被删时引用自动置空（service 同时清 solved_at，D-027）。
ALTER TABLE posts
    ADD CONSTRAINT posts_accepted_comment_fk
    FOREIGN KEY (accepted_comment_id) REFERENCES comments (id) ON DELETE SET NULL;
CREATE INDEX posts_accepted_comment_idx ON posts (accepted_comment_id) WHERE accepted_comment_id IS NOT NULL;
