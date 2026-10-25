-- AI 记忆第一期（ADR 0007，ai-assistant.md 第 11 节 MEM）：会话滚动摘要、长期记忆（档案 + 关系）、用户开关。

-- 会话滚动摘要（MEM-001～004）：历史窗口装不下的更早消息压成一段摘要，随代际失效（重置后不再使用）。
-- 长期记忆的提炼进度（MEM-020）：只前进；不随重置变化（重置清的是当前会话的上下文，不是长期记忆）。
ALTER TABLE ai_sessions ADD COLUMN summary TEXT;
ALTER TABLE ai_sessions ADD COLUMN summary_until TIMESTAMPTZ;
ALTER TABLE ai_sessions ADD COLUMN summary_epoch INTEGER;
ALTER TABLE ai_sessions ADD COLUMN memory_until TIMESTAMPTZ;
ALTER TABLE ai_sessions ADD CONSTRAINT ai_sessions_summary_len
    CHECK (summary IS NULL OR char_length(summary) BETWEEN 1 AND 2000);
ALTER TABLE ai_sessions ADD CONSTRAINT ai_sessions_summary_shape
    CHECK ((summary IS NULL) = (summary_until IS NULL) AND (summary IS NULL) = (summary_epoch IS NULL));

-- 长期记忆（MEM-010～016）：一条一个事实。
-- profile = 用户档案（称呼、所在地、喜好……），所有 AI 角色共享，ai_user_id 为空；
-- relation = 与某个 AI 角色之间的关系记忆（约定、昵称、共同经历），只给那个角色用。
CREATE TABLE ai_memories (
    id                      BIGINT      PRIMARY KEY,
    user_id                 BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    ai_user_id              BIGINT      REFERENCES users (id) ON DELETE CASCADE,
    kind                    TEXT        NOT NULL,
    content                 TEXT        NOT NULL,
    -- 用户置顶：不会被自动整理改写、删除或淘汰。
    pinned                  BOOLEAN     NOT NULL DEFAULT FALSE,
    -- extracted = 后台从对话里提炼；told = 对话里让 AI 记下（remember 工具）；manual = 用户在记忆面板里写或改过。
    origin                  TEXT        NOT NULL,
    source_conversation_id  BIGINT      REFERENCES conversations (id) ON DELETE SET NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_memories_kind_check CHECK (kind IN ('profile', 'relation')),
    CONSTRAINT ai_memories_scope CHECK ((kind = 'profile') = (ai_user_id IS NULL)),
    CONSTRAINT ai_memories_origin_check CHECK (origin IN ('extracted', 'told', 'manual')),
    CONSTRAINT ai_memories_content_len CHECK (char_length(content) BETWEEN 1 AND 200)
);
CREATE INDEX ai_memories_owner_idx ON ai_memories (user_id, ai_user_id, updated_at DESC);
CREATE INDEX ai_memories_ai_user_idx ON ai_memories (ai_user_id) WHERE ai_user_id IS NOT NULL;
CREATE INDEX ai_memories_conversation_idx ON ai_memories (source_conversation_id)
    WHERE source_conversation_id IS NOT NULL;

-- 用户的记忆开关（MEM-030）。没有行 = 开启。关闭后不读、不写、不跨会话衔接，已有记忆保留。
CREATE TABLE ai_memory_prefs (
    user_id     BIGINT      PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    enabled     BOOLEAN     NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
