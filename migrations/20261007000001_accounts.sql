-- 账号、密码、邮箱验证码、会话。业务规则见 docs/business-rules/auth-account.md。

CREATE TABLE users (
    id              BIGINT PRIMARY KEY,
    -- 大小写敏感唯一（ACC-008）；禁止含 '@'，从结构上杜绝用户名与邮箱互相冒用（ACC-009）。
    username        TEXT        NOT NULL,
    -- 已规范化（trim + 小写）。NULL 表示没有可用邮箱（如仅米游社登录的账号，ACC-013）。
    email           TEXT,
    status          TEXT        NOT NULL DEFAULT 'active',
    role            TEXT        NOT NULL DEFAULT 'user',
    is_ai_agent     BOOLEAN     NOT NULL DEFAULT FALSE,
    exam_passed_at  TIMESTAMPTZ,
    -- 旧系统 documentId，仅供迁移与旧链接跳转。
    legacy_id       TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT users_username_uq UNIQUE (username),
    CONSTRAINT users_email_uq UNIQUE (email),
    CONSTRAINT users_legacy_id_uq UNIQUE (legacy_id),
    CONSTRAINT users_username_shape CHECK (
        char_length(username) BETWEEN 1 AND 32 AND position('@' IN username) = 0
    ),
    CONSTRAINT users_email_normalized CHECK (email IS NULL OR email = lower(btrim(email))),
    CONSTRAINT users_status_check CHECK (status IN ('active', 'blocked', 'deleted', 'merged')),
    CONSTRAINT users_role_check CHECK (role IN ('user', 'moderator', 'admin'))
);

-- 只有设置过密码的账号才有一行（米游社首登账号没有）。哈希串自描述算法：$argon2id$… 或旧的 $2a$…。
CREATE TABLE user_passwords (
    user_id     BIGINT      PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    hash        TEXT        NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE email_codes (
    id           BIGINT      PRIMARY KEY,
    email        TEXT        NOT NULL,
    purpose      TEXT        NOT NULL,
    -- HMAC-SHA256(key, "<purpose>:<email>:<code>")，绑定用途与邮箱（CODE-004）。
    code_hash    BYTEA       NOT NULL,
    attempts     INTEGER     NOT NULL DEFAULT 0,
    sent_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL,
    consumed_at  TIMESTAMPTZ,
    request_ip   TEXT,

    CONSTRAINT email_codes_purpose_check CHECK (
        purpose IN ('register', 'login', 'reset_password', 'bind_email', 'delete_account')
    ),
    CONSTRAINT email_codes_attempts_check CHECK (attempts BETWEEN 0 AND 5),
    CONSTRAINT email_codes_email_normalized CHECK (email = lower(btrim(email)))
);

CREATE INDEX email_codes_latest_idx ON email_codes (email, purpose, sent_at DESC, id DESC);
CREATE INDEX email_codes_ip_recent_idx ON email_codes (request_ip, sent_at DESC);

CREATE TABLE sessions (
    id              BIGINT      PRIMARY KEY,
    user_id         BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- SHA-256(令牌原文)。令牌原文只在签发时返回一次，服务端不保存。
    token_hash      BYTEA       NOT NULL,
    login_method    TEXT        NOT NULL,
    user_agent      TEXT,
    ip              TEXT,
    location        TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 滑动过期时间；活跃时续期，但永远不超过 max_expires_at。
    expires_at      TIMESTAMPTZ NOT NULL,
    max_expires_at  TIMESTAMPTZ NOT NULL,
    revoked_at      TIMESTAMPTZ,
    revoke_reason   TEXT,

    CONSTRAINT sessions_token_hash_uq UNIQUE (token_hash),
    CONSTRAINT sessions_login_method_check CHECK (
        login_method IN ('password', 'email_code', 'register', 'mihoyo', 'password_reset')
    ),
    CONSTRAINT sessions_expiry_check CHECK (expires_at <= max_expires_at)
);

CREATE INDEX sessions_user_active_idx ON sessions (user_id, last_seen_at DESC) WHERE revoked_at IS NULL;
CREATE INDEX sessions_expiry_idx ON sessions (expires_at);
