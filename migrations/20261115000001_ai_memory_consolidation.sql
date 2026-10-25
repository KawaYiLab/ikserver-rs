-- AI 记忆第三期（ADR 0007，ai-assistant.md 第 13.9～13.11 节 MEM-080～102）：每晚整理与遗忘、翻聊天记录、久别重逢。

-- 事件最近一次被检索带进对话的时刻（MEM-090）：常被想起的不忘，久不提起的低重要度事件逐渐淡出。
ALTER TABLE ai_memories ADD COLUMN recalled_at TIMESTAMPTZ;

-- 每晚遗忘（MEM-090）按种类、重要度、时间扫描后台提炼的事件。
CREATE INDEX ai_memories_forget_idx ON ai_memories (importance, updated_at)
    WHERE kind = 'event' AND origin = 'extracted' AND NOT pinned;

-- 每个（用户, AI）上次整理的时刻（MEM-080）。没有行 = 还没整理过。
CREATE TABLE ai_memory_pairs (
    user_id          BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    ai_user_id       BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    consolidated_at  TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (user_id, ai_user_id)
);
CREATE INDEX ai_memory_pairs_ai_user_idx ON ai_memory_pairs (ai_user_id);
