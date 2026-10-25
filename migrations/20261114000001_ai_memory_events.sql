-- AI 记忆第二期（ADR 0007，ai-assistant.md 第 13 节 MEM-050～062）：事件记忆与检索、待跟进、敏感度。

-- 事件记忆（MEM-050）：带日期的具体经历，每个 AI 单独记，按相关度检索。
ALTER TABLE ai_memories DROP CONSTRAINT ai_memories_kind_check;
ALTER TABLE ai_memories ADD CONSTRAINT ai_memories_kind_check
    CHECK (kind IN ('profile', 'relation', 'event'));

-- 敏感度（MEM-060）：public 的可以在公开的评论区里用；默认 private。
ALTER TABLE ai_memories ADD COLUMN sensitivity TEXT NOT NULL DEFAULT 'private';
ALTER TABLE ai_memories ADD CONSTRAINT ai_memories_sensitivity_check
    CHECK (sensitivity IN ('private', 'public'));
-- 重要度 1–5（检索打分与淘汰顺序，MEM-052/053）。
ALTER TABLE ai_memories ADD COLUMN importance SMALLINT NOT NULL DEFAULT 3;
ALTER TABLE ai_memories ADD CONSTRAINT ai_memories_importance_range CHECK (importance BETWEEN 1 AND 5);
-- 事件发生的日期、该问起的日期（待跟进，MEM-055）与已经问起的时刻。只有事件有。
ALTER TABLE ai_memories ADD COLUMN happened_on DATE;
ALTER TABLE ai_memories ADD COLUMN follow_up_on DATE;
ALTER TABLE ai_memories ADD COLUMN followed_up_at TIMESTAMPTZ;
ALTER TABLE ai_memories ADD CONSTRAINT ai_memories_event_dates
    CHECK (kind = 'event' OR (happened_on IS NULL AND follow_up_on IS NULL AND followed_up_at IS NULL));
-- 语义向量（MEM-052）：每个用户与每个 AI 的事件不超过几百条，检索时在进程内算相似度，不需要向量索引。
-- 向量模型换了就作废（embedding_model 不一致时当作没有），后台补算。
ALTER TABLE ai_memories ADD COLUMN embedding REAL[];
ALTER TABLE ai_memories ADD COLUMN embedding_model TEXT;
ALTER TABLE ai_memories ADD CONSTRAINT ai_memories_embedding_shape
    CHECK ((embedding IS NULL) = (embedding_model IS NULL)
           AND (embedding IS NULL OR cardinality(embedding) BETWEEN 1 AND 4096));

-- 到期的待跟进（会话开场时查）。
CREATE INDEX ai_memories_follow_up_idx ON ai_memories (user_id, ai_user_id, follow_up_on)
    WHERE follow_up_on IS NOT NULL AND followed_up_at IS NULL;
