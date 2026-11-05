-- 角色卡的开场白与人设提醒（ai-assistant.md AI-029、GEN-073）。

-- 开场白：新建 AI 对话时以角色的名义先发一条，有多条时随机挑一条；空数组 = 不发。
ALTER TABLE ai_role_cards ADD COLUMN greetings TEXT[] NOT NULL DEFAULT '{}'
    CONSTRAINT ai_role_cards_greetings CHECK (cardinality(greetings) <= 10);

-- 人设提醒：每轮附在最后一条用户消息后面，压住长对话里的人设漂移；空串 = 不附。
ALTER TABLE ai_role_cards ADD COLUMN reminder TEXT NOT NULL DEFAULT ''
    CONSTRAINT ai_role_cards_reminder_len CHECK (char_length(reminder) <= 500);
