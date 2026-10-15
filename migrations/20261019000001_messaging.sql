-- 私信：会话、成员、消息。业务规则见 docs/business-rules/messaging-notifications.md 第 1、2 节，
-- 设计说明见 docs/schema.md。

CREATE TABLE conversations (
    id          BIGINT      PRIMARY KEY,
    -- 首版只有一对一私聊（D-073：群聊不实现）。
    kind        TEXT        NOT NULL DEFAULT 'direct',
    -- 私聊双方，较小的 id 在前；同一对用户至多一个私聊会话（CONV-002）。
    user_low    BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    user_high   BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT conversations_kind_check CHECK (kind IN ('direct')),
    CONSTRAINT conversations_pair_order CHECK (user_low < user_high)
);

CREATE UNIQUE INDEX conversations_direct_uq ON conversations (user_low, user_high);
CREATE INDEX conversations_user_high_idx ON conversations (user_high);

CREATE TABLE conversation_members (
    conversation_id BIGINT      NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    user_id         BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 私聊的对方（反范式）：按对方找会话（通知到达时推进排序、收件箱合并同一对端的通知分组）。
    peer_id         BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 收件箱排序键：这个成员视角下的最后活动时间。只前进不后退（CONV-005）；
    -- 新消息推进双方，对方发来的通知只推进收件人自己。
    activity_at     TIMESTAMPTZ NOT NULL,
    -- 已读水位：晚于它、别人发的、未撤回的消息算未读（CONV-027）。
    last_read_at    TIMESTAMPTZ,
    muted           BOOLEAN     NOT NULL DEFAULT FALSE,
    -- 非空 = 置顶，按置顶时间倒序。每人最多 50 个（CONV-023）。
    pinned_at       TIMESTAMPTZ,
    -- 非空 = 已从自己的列表移除（「删除会话」）。对方再发消息或自己重新发起时恢复。
    left_at         TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (conversation_id, user_id),
    CONSTRAINT conversation_members_not_self CHECK (peer_id <> user_id)
);

-- 收件箱（未置顶部分）游标分页。
CREATE INDEX conversation_members_inbox_idx
    ON conversation_members (user_id, activity_at DESC, conversation_id DESC)
    WHERE left_at IS NULL AND pinned_at IS NULL;
-- 按对方找会话；同时充当 user_id 外键索引。
CREATE UNIQUE INDEX conversation_members_peer_uq ON conversation_members (user_id, peer_id);
CREATE INDEX conversation_members_peer_idx ON conversation_members (peer_id);

CREATE TABLE messages (
    id              BIGINT      PRIMARY KEY,
    conversation_id BIGINT      NOT NULL REFERENCES conversations (id) ON DELETE CASCADE,
    sender_id       BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 首版只有文本（D-021：图片消息不实现）；system 预留给 AI 会话的分界消息。
    kind            TEXT        NOT NULL DEFAULT 'text',
    content         TEXT        NOT NULL,
    reply_to_id     BIGINT      REFERENCES messages (id) ON DELETE SET NULL,
    -- 客户端生成的去重键：同一发送者重试时返回第一次的结果。
    client_id       TEXT,
    edited_at       TIMESTAMPTZ,
    -- 非空 = 已撤回：行保留，正文对所有人不可见（MSG-003）。
    withdrawn_at    TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT messages_kind_check CHECK (kind IN ('text', 'system')),
    CONSTRAINT messages_content_len CHECK (char_length(content) BETWEEN 1 AND 4000),
    CONSTRAINT messages_client_id_len CHECK (client_id IS NULL OR char_length(client_id) BETWEEN 1 AND 64)
);

-- 消息流（新的在前）、最后一条预览、未读计数。
CREATE INDEX messages_conversation_idx ON messages (conversation_id, created_at DESC, id DESC);
CREATE INDEX messages_sender_idx ON messages (sender_id);
CREATE INDEX messages_reply_to_idx ON messages (reply_to_id) WHERE reply_to_id IS NOT NULL;
CREATE UNIQUE INDEX messages_client_uq ON messages (sender_id, client_id) WHERE client_id IS NOT NULL;
