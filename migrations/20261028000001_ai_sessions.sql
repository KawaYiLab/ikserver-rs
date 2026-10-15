-- AI 私聊会话（ai-assistant.md 第 3.3、4.3、6.1、6.2 节；messaging-notifications.md CONV-001/002/012）。
--
-- messaging 的表：AI 会话是 kind = 'ai' 的一对一会话，同一对用户可以有多条（每次新建，CONV-012），
-- 所以「同一对用户至多一个会话」的唯一约束只约束 direct。AI 回复是一条普通消息，带展示用的
-- `ai_state`、`ai_model_key`（其余内部字段在 ik-ai 的 `ai_jobs` 里）。
-- 部署顺序：先上线带 `ON CONFLICT (user_low, user_high) WHERE kind = 'direct'` 的二进制，再跑本迁移
-- （新写法同时兼容旧的完整唯一索引与新的部分唯一索引）。

ALTER TABLE conversations DROP CONSTRAINT conversations_kind_check;
ALTER TABLE conversations ADD CONSTRAINT conversations_kind_check CHECK (kind IN ('direct', 'ai'));
-- AI 会话的标题（TRIG-024、TRIG-025）；普通私聊恒为 NULL。
ALTER TABLE conversations ADD COLUMN title TEXT;
ALTER TABLE conversations ADD CONSTRAINT conversations_title_len
    CHECK (title IS NULL OR (kind = 'ai' AND char_length(title) BETWEEN 1 AND 64));

CREATE UNIQUE INDEX conversations_direct_pair_uq ON conversations (user_low, user_high) WHERE kind = 'direct';
DROP INDEX conversations_direct_uq;
-- user_low 外键索引（原来由完整唯一索引兼任）。
CREATE INDEX conversations_user_low_idx ON conversations (user_low);

-- 成员的 (user_id, peer_id) 唯一性由会话的 direct 唯一约束保证；AI 会话可以重复。
CREATE INDEX conversation_members_user_peer_idx ON conversation_members (user_id, peer_id);
DROP INDEX conversation_members_peer_uq;

-- AI 回复（GEN-001～008）：streaming = 占位中（正文可以为空），done = 已定稿，failed = 生成失败
-- （同一任务重试会继续写这条），stopped = 用户停止或被重置 / 撤回打断。
ALTER TABLE messages ADD COLUMN ai_state TEXT;
ALTER TABLE messages ADD COLUMN ai_model_key TEXT;
ALTER TABLE messages ADD CONSTRAINT messages_ai_state_check
    CHECK (ai_state IS NULL OR ai_state IN ('streaming', 'done', 'failed', 'stopped'));
ALTER TABLE messages ADD CONSTRAINT messages_ai_model_key_len
    CHECK (ai_model_key IS NULL OR char_length(ai_model_key) BETWEEN 1 AND 64);
ALTER TABLE messages ADD CONSTRAINT messages_content_len_v2
    CHECK (char_length(content) <= 4000 AND (char_length(content) >= 1 OR ai_state = 'streaming'));
ALTER TABLE messages DROP CONSTRAINT messages_content_len;

-- ik-ai：AI 会话的状态（代际、会话级模型选择）。
CREATE TABLE ai_sessions (
    conversation_id BIGINT      PRIMARY KEY REFERENCES conversations (id) ON DELETE CASCADE,
    user_id         BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    ai_user_id      BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 上下文代际（CTX-020～025）：重置一次加一；AI 只读最近一次重置之后的消息。
    context_epoch   INTEGER     NOT NULL DEFAULT 0,
    context_from    TIMESTAMPTZ,
    -- 会话级模型选择（CTX-032），NULL = 跟随角色卡。存 key：模型停用或移出白名单时静默跳过。
    model_key       TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_sessions_epoch CHECK (context_epoch >= 0)
);
CREATE INDEX ai_sessions_user_idx ON ai_sessions (user_id);
CREATE INDEX ai_sessions_ai_user_idx ON ai_sessions (ai_user_id);

-- ik-ai：AI 任务（AI-008、TRIG-030～037）。一个任务至多产出一条回复；重试复用同一行。
CREATE TABLE ai_jobs (
    id                  BIGINT      PRIMARY KEY,
    kind                TEXT        NOT NULL,
    -- 去重键（TRIG-030）：`dm:<触发消息 id>`、`regenerate:<旧回复 id>`、`comment:<触发评论 id>`。
    execution_key       TEXT        NOT NULL,
    status              TEXT        NOT NULL DEFAULT 'queued',
    quota_user_id       BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    ai_user_id          BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    conversation_id     BIGINT      REFERENCES conversations (id) ON DELETE CASCADE,
    trigger_message_id  BIGINT      REFERENCES messages (id) ON DELETE CASCADE,
    -- 触发时的会话代际（CTX-024）。
    generation_epoch    INTEGER     NOT NULL DEFAULT 0,
    -- 单次覆盖的模型（换模型重新生成，CTX-030 ①）。
    model_override      TEXT,
    -- 占位 / 回复消息（GEN-002：同一任务重试复用）。
    reply_message_id    BIGINT      REFERENCES messages (id) ON DELETE SET NULL,
    -- 停止信号（GEN-010/011、CTX-022/026）：生成方轮询。
    stop_requested_at   TIMESTAMPTZ,
    attempts            SMALLINT    NOT NULL DEFAULT 0,
    reason              TEXT,
    model_key           TEXT,
    prompt_tokens       BIGINT      NOT NULL DEFAULT 0,
    completion_tokens   BIGINT      NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at         TIMESTAMPTZ,

    CONSTRAINT ai_jobs_execution_key_uq UNIQUE (execution_key),
    CONSTRAINT ai_jobs_reply_uq UNIQUE (reply_message_id),
    CONSTRAINT ai_jobs_kind_check CHECK (kind IN ('dm', 'comment')),
    CONSTRAINT ai_jobs_status_check
        CHECK (status IN ('queued', 'processing', 'completed', 'failed', 'skipped')),
    CONSTRAINT ai_jobs_reason_len CHECK (reason IS NULL OR char_length(reason) <= 1000)
);
CREATE INDEX ai_jobs_conversation_idx ON ai_jobs (conversation_id, created_at)
    WHERE conversation_id IS NOT NULL;
CREATE INDEX ai_jobs_trigger_idx ON ai_jobs (trigger_message_id) WHERE trigger_message_id IS NOT NULL;
CREATE INDEX ai_jobs_quota_user_idx ON ai_jobs (quota_user_id);
CREATE INDEX ai_jobs_ai_user_idx ON ai_jobs (ai_user_id);
-- 死信查看（AI-027）。
CREATE INDEX ai_jobs_failed_idx ON ai_jobs (updated_at DESC) WHERE status = 'failed';
