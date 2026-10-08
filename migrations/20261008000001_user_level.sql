-- 用户等级（0–7）。0 = 未通过入站考试；通过后至少为 1（EXP-002/003，D-017 按 0 起步）。
-- 经验值与等级计算归 economy 域，这里只存结果，供鉴权上下文与权益矩阵读取。
ALTER TABLE users
    ADD COLUMN level SMALLINT NOT NULL DEFAULT 0,
    ADD CONSTRAINT users_level_range CHECK (level BETWEEN 0 AND 7);
