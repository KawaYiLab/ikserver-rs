-- 点赞与收藏。业务规则见 docs/business-rules/social-economy.md 第 1、2、5 节，设计说明见 docs/schema.md。
-- 帖子的赞、藏归 content 所有，评论的赞归 comments 所有；计数是 posts / comments 上已有的物化列。

-- 帖子点赞。每人每帖一条（LIKE-002）；帖子或用户删除时级联删除。
CREATE TABLE post_likes (
    post_id     BIGINT      NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    user_id     BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (post_id, user_id)
);

-- 我的点赞列表（新的在前）+ user_id 外键。
CREATE INDEX post_likes_user_recent_idx ON post_likes (user_id, created_at DESC, post_id DESC);

-- 帖子收藏。只针对帖子（FAV-001）。
CREATE TABLE post_favorites (
    post_id     BIGINT      NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    user_id     BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (post_id, user_id)
);

CREATE INDEX post_favorites_user_recent_idx ON post_favorites (user_id, created_at DESC, post_id DESC);

-- 评论点赞。删除评论（含级联删除的回复）时一并删除，不留孤儿行（D-029 方案 A）。
CREATE TABLE comment_likes (
    comment_id  BIGINT      NOT NULL REFERENCES comments (id) ON DELETE CASCADE,
    user_id     BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (comment_id, user_id)
);

CREATE INDEX comment_likes_user_recent_idx ON comment_likes (user_id, created_at DESC, comment_id DESC);
