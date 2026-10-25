-- ADR 0008 第二、三期：QQ 绑定码、个人记忆的来源场合、QQ 记忆提炼进度、按场合共享的记忆（QQ 群共享记忆）。

-- ik-qqbot：站内生成的一次性绑定码，只存 HMAC；10 分钟有效、用一次。
CREATE TABLE qq_bind_codes (
    code_hash           BYTEA       PRIMARY KEY,
    user_id             BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    expires_at          TIMESTAMPTZ NOT NULL,
    used_at             TIMESTAMPTZ,
    used_by_identity_id BIGINT      REFERENCES qq_identities (id) ON DELETE SET NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX qq_bind_codes_user_idx ON qq_bind_codes (user_id, created_at DESC);

-- ik-ai：记忆是在哪个公开场合说的（不透明的键，如 `qq:group:<id>`）。为空表示私下说的（私信、QQ 单聊）或后台写的。
-- 公开场合只用 public 的记忆和来源就是这个场合的记忆；私下场合全部可用。
ALTER TABLE ai_memories ADD COLUMN source_scope TEXT;
ALTER TABLE ai_memories ADD CONSTRAINT ai_memories_source_scope_len
    CHECK (source_scope IS NULL OR char_length(source_scope) BETWEEN 1 AND 64);
CREATE INDEX ai_memories_source_scope_idx ON ai_memories (user_id, source_scope) WHERE source_scope IS NOT NULL;

-- ik-qqbot：个人记忆从 QQ 对话里提炼到哪儿了（只前进）。
CREATE TABLE qq_memory_progress (
    user_id    BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    ai_user_id BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    until      TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (user_id, ai_user_id)
);
CREATE INDEX qq_memory_progress_ai_idx ON qq_memory_progress (ai_user_id);

-- ik-ai：按场合共享的记忆（第三期，QQ 群共享记忆）：一个场合 × 一个角色一份，场合内所有人共用。
CREATE TABLE ai_shared_memories (
    id                 BIGINT      PRIMARY KEY,
    scope              TEXT        NOT NULL,
    ai_user_id         BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    content            TEXT        NOT NULL,
    -- extracted = 后台提炼；told = 对话里让 AI 记下；manual = 管理员写或改过。
    origin             TEXT        NOT NULL,
    pinned             BOOLEAN     NOT NULL DEFAULT FALSE,
    created_by_user_id BIGINT      REFERENCES users (id) ON DELETE SET NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_shared_memories_scope_len CHECK (char_length(scope) BETWEEN 1 AND 64),
    CONSTRAINT ai_shared_memories_content_len CHECK (char_length(content) BETWEEN 1 AND 200),
    CONSTRAINT ai_shared_memories_origin_check CHECK (origin IN ('extracted', 'told', 'manual'))
);
CREATE INDEX ai_shared_memories_scope_idx ON ai_shared_memories (scope, ai_user_id, updated_at DESC);
CREATE INDEX ai_shared_memories_ai_user_idx ON ai_shared_memories (ai_user_id);
CREATE INDEX ai_shared_memories_creator_idx ON ai_shared_memories (created_by_user_id)
    WHERE created_by_user_id IS NOT NULL;

-- ik-qqbot：群共享记忆从群对话里提炼到哪儿了。
CREATE TABLE qq_group_memory_progress (
    group_id   BIGINT      NOT NULL REFERENCES qq_groups (id) ON DELETE CASCADE,
    ai_user_id BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    until      TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (group_id, ai_user_id)
);
CREATE INDEX qq_group_memory_progress_ai_idx ON qq_group_memory_progress (ai_user_id);
