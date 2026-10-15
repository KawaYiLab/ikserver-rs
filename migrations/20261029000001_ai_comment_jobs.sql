-- AI 评论回复（ai-assistant.md 第 3.2、4.1、6.4、6.7 节）。
--
-- 评论任务与私信任务共用 `ai_jobs`（kind = 'comment'，去重键 `comment:<触发评论 id>`）。
-- 触发评论或回复被删除时任务行保留（审计），引用置空；准备 / 定稿时复核触发评论仍可见（TRIG-016）。

ALTER TABLE ai_jobs ADD COLUMN post_id BIGINT REFERENCES posts (id) ON DELETE SET NULL;
ALTER TABLE ai_jobs ADD COLUMN trigger_comment_id BIGINT REFERENCES comments (id) ON DELETE SET NULL;
-- 回复评论（LLM-010：一个任务至多一条回复，已有回复时任务重放直接结束）。
ALTER TABLE ai_jobs ADD COLUMN reply_comment_id BIGINT REFERENCES comments (id) ON DELETE SET NULL;
ALTER TABLE ai_jobs ADD CONSTRAINT ai_jobs_reply_comment_uq UNIQUE (reply_comment_id);

CREATE INDEX ai_jobs_trigger_comment_idx ON ai_jobs (trigger_comment_id)
    WHERE trigger_comment_id IS NOT NULL;
-- 评论记忆（CTX-003）：按（帖子、评论者、AI）取之前的问答；也是 post_id 外键的索引。
CREATE INDEX ai_jobs_comment_memory_idx ON ai_jobs (post_id, quota_user_id, ai_user_id, created_at)
    WHERE post_id IS NOT NULL;
