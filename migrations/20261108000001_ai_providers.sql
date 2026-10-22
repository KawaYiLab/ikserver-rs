-- AI 上游提供方（管理后台维护）：OpenAI 兼容接口的地址与密钥。归 ik-ai。
-- 模型的 `ai_models.provider` 按 `name` 引用。部署配置（`IK_AI__PROVIDERS__<名字>__…`）里的提供方
-- 不进这张表；同名时以部署配置为准，后台也不允许新建同名的。
CREATE TABLE ai_providers (
    id              BIGINT      PRIMARY KEY,
    name            TEXT        NOT NULL,
    display_name    TEXT        NOT NULL,
    -- 例如 `https://api.deepseek.com/v1`（不含 `/chat/completions`）。
    base_url        TEXT        NOT NULL,
    -- 只在服务端流转；后台接口只回末四位提示。空串表示不带密钥（本地部署的上游）。
    api_key         TEXT        NOT NULL DEFAULT '',
    enabled         BOOLEAN     NOT NULL DEFAULT TRUE,
    sort_order      INTEGER     NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_providers_name_uq UNIQUE (name),
    CONSTRAINT ai_providers_name_shape CHECK (name ~ '^[a-z0-9][a-z0-9_-]{0,63}$'),
    CONSTRAINT ai_providers_display_name_len CHECK (char_length(display_name) BETWEEN 1 AND 64),
    CONSTRAINT ai_providers_base_url_shape CHECK (base_url ~ '^https?://[^[:space:]]+$' AND char_length(base_url) <= 512),
    CONSTRAINT ai_providers_api_key_len CHECK (char_length(api_key) <= 4096)
);
