-- 「大家都在问」：用户在 AI 会话里的开场提问，按角色统计热门问题（AI-028）。
-- 同一个人对同一个角色问同一个问题只记一行（反复问不刷票）；只保留 30 天，撤回消息或注销账号时一起删除。

CREATE TABLE ai_question_asks (
    -- 角色绑定的 AI 账号。
    ai_user_id   BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 归一化后的问题（去空白、英文小写、去句末标点），同一问题的不同写法合并。
    question_key TEXT        NOT NULL,
    user_id      BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 这个人最近一次的原话（展示时取出现最多的写法）。
    text         TEXT        NOT NULL,
    -- 最近一次提问的消息：撤回它时这行一起删除。
    message_id   BIGINT      NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
    asked_at     TIMESTAMPTZ NOT NULL,

    PRIMARY KEY (ai_user_id, question_key, user_id),
    CONSTRAINT ai_question_asks_text_len CHECK (char_length(text) BETWEEN 1 AND 60)
);

CREATE INDEX ai_question_asks_recent_idx ON ai_question_asks (asked_at);
CREATE INDEX ai_question_asks_user_idx ON ai_question_asks (user_id);
CREATE INDEX ai_question_asks_message_idx ON ai_question_asks (message_id);
