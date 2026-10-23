-- AI 识图（ai-assistant.md LLM-015）：模型的识图开关；私信可以发图片（MSG-030，D-021 改为方案 B）。

-- 支持识图的模型才把图片发给上游；其余模型只看到「[图片]」占位（不识图的上游收到图片会报 400）。
ALTER TABLE ai_models ADD COLUMN vision BOOLEAN NOT NULL DEFAULT FALSE;

-- 私信图片（有序），真人私聊与 AI 会话都能发，最多 4 张。和评论图片一样用数组：引用的文件从不物理删除（D-078），
-- 归属在写入时校验（D-012）。带图的消息正文可以为空。
ALTER TABLE messages
    ADD COLUMN image_ids BIGINT[] NOT NULL DEFAULT '{}',
    ADD CONSTRAINT messages_image_count CHECK (cardinality(image_ids) <= 4);
ALTER TABLE messages ADD CONSTRAINT messages_content_len_v3
    CHECK (char_length(content) <= 4000
           AND (char_length(content) >= 1 OR ai_state = 'streaming' OR cardinality(image_ids) > 0));
ALTER TABLE messages DROP CONSTRAINT messages_content_len_v2;
