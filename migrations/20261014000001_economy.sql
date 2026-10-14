-- 经济：经验、等级、丁尼、签到、投币。业务规则见 docs/business-rules/social-economy.md，
-- 设计说明见 docs/schema.md。等级结果仍存在 users.level（鉴权上下文读取），由 economy 域通过 auth 的接口写入。

-- 每个用户一行，第一次发生经济行为时创建（初始 30 丁尼，DENNY-001）。
CREATE TABLE user_economy (
    user_id            BIGINT      PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    exp                BIGINT      NOT NULL DEFAULT 0,
    denny_balance      INTEGER     NOT NULL DEFAULT 30,
    -- 累计投出的丁尼。
    denny_given        INTEGER     NOT NULL DEFAULT 0,
    last_check_in_day  DATE,
    -- 连签天数：断签时不清零，下次签到时重新计算（CHECKIN-003）。
    check_in_streak    INTEGER     NOT NULL DEFAULT 0,
    check_in_total     INTEGER     NOT NULL DEFAULT 0,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT user_economy_exp_nonneg CHECK (exp >= 0),
    -- 全局封顶 9999（D-002 方案 A：投币收入也按 9999 封顶）。
    CONSTRAINT user_economy_balance_range CHECK (denny_balance BETWEEN 0 AND 9999),
    CONSTRAINT user_economy_counters_nonneg CHECK (
        denny_given >= 0 AND check_in_streak >= 0 AND check_in_total >= 0
    )
);

-- 经验流水。只记实际发放量 > 0 的行（EXP-010）。
CREATE TABLE exp_logs (
    id            BIGINT      PRIMARY KEY,
    user_id       BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    action        TEXT        NOT NULL,
    -- 实际发放量（被每日上限截断后）。
    amount        INTEGER     NOT NULL,
    requested     INTEGER     NOT NULL,
    -- 每日上限桶：active 50 / passive 1000 / exempt 不计上限（EXP-009）。
    bucket        TEXT        NOT NULL,
    -- 业务日（北京时间 04:00 切日）。
    biz_day       DATE        NOT NULL,
    -- 去重键：按日去重为 `day:<YYYY-MM-DD>`，按实体去重为 `ref:<...>`（EXP-008）。
    dedup_key     TEXT        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT exp_logs_dedup_uq UNIQUE (user_id, action, dedup_key),
    CONSTRAINT exp_logs_amount_pos CHECK (amount > 0 AND requested >= amount),
    CONSTRAINT exp_logs_bucket_check CHECK (bucket IN ('active', 'passive', 'exempt')),
    -- like_received / comment_received / favorite_received 没有写入方，只为迁移历史流水保留（D-049）。
    CONSTRAINT exp_logs_action_check CHECK (action IN (
        'check_in', 'create_post', 'create_comment', 'like_give', 'comment_accepted',
        'denny_received', 'exam_passed', 'like_received', 'comment_received', 'favorite_received'
    ))
);

-- 按日统计已用额度。
CREATE INDEX exp_logs_daily_idx ON exp_logs (user_id, biz_day, bucket);

-- 丁尼流水。
CREATE TABLE denny_logs (
    id           BIGINT      PRIMARY KEY,
    user_id      BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    action       TEXT        NOT NULL,
    -- 有符号：入账为正，出账为负。
    amount       INTEGER     NOT NULL,
    -- 变动后的余额。
    balance      INTEGER     NOT NULL,
    -- 幂等键：同一用户、同一动作、同一键只记一次（DENNY-005）。NULL 表示不做幂等（如改名扣费）。
    ref_key      TEXT,
    description  TEXT        NOT NULL DEFAULT '',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT denny_logs_amount_nonzero CHECK (amount <> 0),
    CONSTRAINT denny_logs_balance_range CHECK (balance BETWEEN 0 AND 9999),
    CONSTRAINT denny_logs_description_len CHECK (char_length(description) <= 255),
    -- spend_delete_post / refund / level_up 已不再产生（D-007、D-047），只为迁移历史流水保留。
    CONSTRAINT denny_logs_action_check CHECK (action IN (
        'check_in', 'gift_sent', 'gift_received', 'spend_rename', 'spend_avatar', 'exam_passed',
        'admin_grant', 'spend_delete_post', 'refund', 'level_up'
    ))
);

CREATE UNIQUE INDEX denny_logs_ref_uq ON denny_logs (user_id, action, ref_key) WHERE ref_key IS NOT NULL;
CREATE INDEX denny_logs_user_idx ON denny_logs (user_id, created_at DESC, id DESC);

-- 签到记录：每人每个业务日一行（CHECKIN-002）。
CREATE TABLE check_ins (
    user_id        BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    day            DATE        NOT NULL,
    -- 当日第几个签到（CHECKIN-007），由 check_in_days 计数器原子分配。
    rank           INTEGER     NOT NULL,
    streak         INTEGER     NOT NULL,
    exp_granted    INTEGER     NOT NULL,
    denny_granted  INTEGER     NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (user_id, day),
    CONSTRAINT check_ins_positive CHECK (rank >= 1 AND streak >= 1),
    CONSTRAINT check_ins_granted_nonneg CHECK (exp_granted >= 0 AND denny_granted >= 0)
);

-- 每个业务日的签到人数，用来给出唯一且连续的当日排名。
CREATE TABLE check_in_days (
    day    DATE    PRIMARY KEY,
    count  INTEGER NOT NULL,

    CONSTRAINT check_in_days_count_pos CHECK (count >= 1)
);

-- 投币：每人每帖终身一次，每次固定 1 丁尼（DENNY-008）。
CREATE TABLE post_denny_gifts (
    post_id     BIGINT      NOT NULL REFERENCES posts (id) ON DELETE CASCADE,
    giver_id    BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    amount      SMALLINT    NOT NULL DEFAULT 1,
    message     TEXT        NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (post_id, giver_id),
    CONSTRAINT post_denny_gifts_amount CHECK (amount = 1),
    CONSTRAINT post_denny_gifts_message_len CHECK (char_length(message) <= 100)
);

CREATE INDEX post_denny_gifts_giver_idx ON post_denny_gifts (giver_id, created_at DESC);
