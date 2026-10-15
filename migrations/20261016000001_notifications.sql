-- 通知（敲敲）。业务规则见 docs/business-rules/messaging-notifications.md 第 4、5 节，
-- 设计说明见 docs/schema.md。

CREATE TABLE notifications (
    id            BIGINT      PRIMARY KEY,
    recipient_id  BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 系统通知没有发送者。
    sender_id     BIGINT      REFERENCES users (id) ON DELETE CASCADE,
    kind          TEXT        NOT NULL,
    -- 敲敲会话（分组）键：`user-<发送者 id>` 或 `system`。
    bucket_key    TEXT        NOT NULL,
    -- 关联的帖子 / 评论；删除帖子或评论时通知一并删除。
    post_id       BIGINT      REFERENCES posts (id) ON DELETE CASCADE,
    comment_id    BIGINT      REFERENCES comments (id) ON DELETE CASCADE,
    -- 写入时的快照：帖子标题、评论摘要 / 系统通知正文。notify 不读 posts、comments 表。
    post_title    TEXT,
    excerpt       TEXT        NOT NULL DEFAULT '',
    -- 幂等键（例如新帖扇出 `follow:<帖子 id>`），每个收件人唯一。
    dedup_key     TEXT,
    is_read       BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT notifications_kind_check CHECK (
        kind IN ('comment', 'reply', 'mention', 'like', 'favorite', 'denny', 'accepted', 'system')
    ),
    CONSTRAINT notifications_sender_by_kind CHECK ((kind = 'system') = (sender_id IS NULL)),
    -- 自己不给自己发通知（NOTIF-003）。
    CONSTRAINT notifications_not_self CHECK (sender_id IS DISTINCT FROM recipient_id),
    CONSTRAINT notifications_excerpt_len CHECK (char_length(excerpt) <= 500),
    CONSTRAINT notifications_title_len CHECK (post_title IS NULL OR char_length(post_title) <= 100)
);

-- 通知列表（新的在前）+ recipient_id 外键。
CREATE INDEX notifications_recipient_recent_idx ON notifications (recipient_id, created_at DESC, id DESC);
-- 敲敲会话：分组枚举（松散索引扫描）与会话内分页。
CREATE INDEX notifications_bucket_idx ON notifications (recipient_id, bucket_key, created_at DESC, id DESC);
-- 未读数。
CREATE INDEX notifications_unread_idx ON notifications (recipient_id, bucket_key) WHERE NOT is_read;
CREATE UNIQUE INDEX notifications_dedup_uq ON notifications (recipient_id, dedup_key)
    WHERE dedup_key IS NOT NULL;
-- 外键与撤回（取消点赞、取消收藏）。
CREATE INDEX notifications_sender_idx ON notifications (sender_id, kind) WHERE sender_id IS NOT NULL;
CREATE INDEX notifications_post_idx ON notifications (post_id) WHERE post_id IS NOT NULL;
CREATE INDEX notifications_comment_idx ON notifications (comment_id) WHERE comment_id IS NOT NULL;
