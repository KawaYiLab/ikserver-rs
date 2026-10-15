-- AI 助手（ai-assistant.md）：全局配置、模型、角色卡、额度账本。归 ik-ai。

-- 全局配置（AI-001），单行。两个上限的单位都是加权 token，0 表示不限制。
CREATE TABLE ai_settings (
    id                      SMALLINT    PRIMARY KEY DEFAULT 1,
    global_enabled          BOOLEAN     NOT NULL DEFAULT TRUE,
    daily_tokens_per_user   BIGINT      NOT NULL DEFAULT 1000000,
    daily_tokens            BIGINT      NOT NULL DEFAULT 0,
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_settings_single CHECK (id = 1),
    CONSTRAINT ai_settings_limits CHECK (daily_tokens_per_user >= 0 AND daily_tokens >= 0)
);
INSERT INTO ai_settings (id) VALUES (1);

-- 模型（AI-003）。`key` 对外；`provider` 与 `provider_model` 是上游路由，只在服务端流转（AI-022）。
-- 必须恰好有一条默认模型（D-005 方案 A：结算按实际模型的倍率，没有「无模型」一级）。
CREATE TABLE ai_models (
    id              BIGINT      PRIMARY KEY,
    key             TEXT        NOT NULL,
    display_name    TEXT        NOT NULL,
    description     TEXT        NOT NULL DEFAULT '',
    badge           TEXT,
    provider        TEXT        NOT NULL,
    provider_model  TEXT        NOT NULL,
    -- 配额倍率（QUOTA-001）。
    cost_weight     DOUBLE PRECISION NOT NULL DEFAULT 1,
    enabled         BOOLEAN     NOT NULL DEFAULT TRUE,
    is_default      BOOLEAN     NOT NULL DEFAULT FALSE,
    sort_order      INTEGER     NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_models_key_uq UNIQUE (key),
    CONSTRAINT ai_models_key_shape CHECK (key ~ '^[a-z0-9][a-z0-9._-]{0,63}$'),
    CONSTRAINT ai_models_display_name_len CHECK (char_length(display_name) BETWEEN 1 AND 64),
    CONSTRAINT ai_models_badge_len CHECK (badge IS NULL OR char_length(badge) <= 16),
    CONSTRAINT ai_models_cost_weight CHECK (cost_weight > 0 AND cost_weight <= 1000),
    CONSTRAINT ai_models_default_enabled CHECK (NOT is_default OR enabled)
);
CREATE UNIQUE INDEX ai_models_default_uq ON ai_models (is_default) WHERE is_default;

-- 角色卡（AI-005）：人格（system prompt）、绑定的 AI 账号、开关、模型白名单。
CREATE TABLE ai_role_cards (
    id                  BIGINT      PRIMARY KEY,
    slug                TEXT        NOT NULL,
    display_name        TEXT        NOT NULL,
    bio                 TEXT        NOT NULL DEFAULT '',
    avatar_media_id     BIGINT      REFERENCES media_files (id),
    bound_user_id       BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    enabled             BOOLEAN     NOT NULL DEFAULT FALSE,
    knock_visible       BOOLEAN     NOT NULL DEFAULT TRUE,
    dm_enabled          BOOLEAN     NOT NULL DEFAULT TRUE,
    comment_enabled     BOOLEAN     NOT NULL DEFAULT TRUE,
    sort_order          INTEGER     NOT NULL DEFAULT 0,
    output_mode         TEXT        NOT NULL DEFAULT 'plain',
    system_prompt       TEXT        NOT NULL DEFAULT '',
    -- 示例对话（GEN-071）：[{ "user": "...", "assistant": "..." }]，作为 system prompt 里的 few-shot 块。
    examples            JSONB       NOT NULL DEFAULT '[]',
    default_model_id    BIGINT      REFERENCES ai_models (id) ON DELETE SET NULL,
    -- 允许的模型；空 = 全部启用模型。
    allowed_model_ids   BIGINT[]    NOT NULL DEFAULT '{}',
    suggested_questions TEXT[]      NOT NULL DEFAULT '{}',
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_role_cards_slug_uq UNIQUE (slug),
    CONSTRAINT ai_role_cards_slug_shape CHECK (slug ~ '^[a-z0-9][a-z0-9-]{0,63}$'),
    CONSTRAINT ai_role_cards_display_name_len CHECK (char_length(display_name) BETWEEN 1 AND 64),
    CONSTRAINT ai_role_cards_bio_len CHECK (char_length(bio) <= 500),
    CONSTRAINT ai_role_cards_output_mode CHECK (output_mode IN ('plain', 'markdown')),
    CONSTRAINT ai_role_cards_prompt_len CHECK (char_length(system_prompt) <= 20000),
    CONSTRAINT ai_role_cards_questions CHECK (cardinality(suggested_questions) <= 10)
);
-- 「某用户的角色卡」= 启用的、按排序取第一张（AI-006）。
CREATE INDEX ai_role_cards_user_idx ON ai_role_cards (bound_user_id, sort_order, id) WHERE enabled;
CREATE INDEX ai_role_cards_bound_user_fk_idx ON ai_role_cards (bound_user_id);
CREATE INDEX ai_role_cards_avatar_idx ON ai_role_cards (avatar_media_id) WHERE avatar_media_id IS NOT NULL;
CREATE INDEX ai_role_cards_default_model_idx ON ai_role_cards (default_model_id) WHERE default_model_id IS NOT NULL;

-- 额度计数器（QUOTA-003）：每个配额日（北京时间 04:00 切换）每人一行，`user_id = 0` 是全站。
-- 计数含在飞的预占；预占与结算都在行锁下增减，并发请求在这里串行（QUOTA-005）。
CREATE TABLE ai_quota_counters (
    day         DATE        NOT NULL,
    user_id     BIGINT      NOT NULL,
    used        BIGINT      NOT NULL DEFAULT 0,

    PRIMARY KEY (day, user_id),
    CONSTRAINT ai_quota_counters_used CHECK (used >= 0)
);

-- 预占（QUOTA-010）：结算时删除这一行就是认领；过期未结算的由定时任务退回（QUOTA-016）。
CREATE TABLE ai_quota_holds (
    id          BIGINT      PRIMARY KEY,
    user_id     BIGINT      NOT NULL,
    day         DATE        NOT NULL,
    amount      BIGINT      NOT NULL,
    expires_at  TIMESTAMPTZ NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_quota_holds_amount CHECK (amount > 0)
);
CREATE INDEX ai_quota_holds_expires_idx ON ai_quota_holds (expires_at);

-- 结算去重（QUOTA-015）：键是预占 id 或执行 id，保留 48 小时。
CREATE TABLE ai_quota_settlements (
    key         TEXT        PRIMARY KEY,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ai_quota_settlements_created_idx ON ai_quota_settlements (created_at);
