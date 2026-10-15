-- 阅读记录：有记录即已读（READ-001）。业务规则见 docs/business-rules/feed-search.md 1.4、3.3 节。
-- 已读水位（READ-011/012）按 D-024 建议默认首版不实现。

CREATE TABLE post_reads (
    user_id     BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    post_id     BIGINT      NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- 未看流的 NOT EXISTS 按 (user_id, post_id) 探测（UNRD-003）。
    PRIMARY KEY (user_id, post_id)
);

-- post_id 外键。
CREATE INDEX post_reads_post_idx ON post_reads (post_id);

-- 热榜活动窗口：按时间范围扫点赞与评论（HOT-005）。
CREATE INDEX post_likes_recent_idx ON post_likes (created_at);
CREATE INDEX comments_recent_idx ON comments (created_at);
