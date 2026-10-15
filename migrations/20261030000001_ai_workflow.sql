-- AI 回复的工作流时间线（ai-assistant.md 第 7 节，WF-003/005）。
--
-- 定稿时整条序列（已脱敏的客户端形态，最多 1000 个事件）写在消息上，刷新后可以回放；
-- 生成中的事件只走实时推送（`dm.message.workflow`）。只有 AI 回复有，其余消息为 NULL。
ALTER TABLE messages ADD COLUMN ai_workflow JSONB;
ALTER TABLE messages ADD CONSTRAINT messages_ai_workflow_array
    CHECK (ai_workflow IS NULL OR (ai_state IS NOT NULL AND jsonb_typeof(ai_workflow) = 'array'));
