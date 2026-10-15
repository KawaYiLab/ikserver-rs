-- 米游社扫码登录与绑定（MHY）。归 ik-mihoyo。业务规则见 docs/business-rules/auth-account.md 第 7 节。

-- 绑定即米游社身份：aid 是米游社的稳定标识，mid 是历史别名（MHY-001、IDENT-002）。
-- 一个账号最多一个米游社账号（主键），一个米游社账号最多绑一个账号（aid 唯一、mid 唯一）。
CREATE TABLE mihoyo_bindings (
    user_id          BIGINT      PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    aid              TEXT        NOT NULL,
    mid              TEXT,
    -- 绝区零主角色（MHY-P5），拉取失败时为空。
    zzz_uid          TEXT,
    zzz_nickname     TEXT,
    zzz_level        INTEGER,
    zzz_region       TEXT,
    zzz_region_name  TEXT,
    last_synced_at   TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT mihoyo_bindings_aid_uq UNIQUE (aid),
    CONSTRAINT mihoyo_bindings_aid_len CHECK (char_length(aid) BETWEEN 1 AND 64),
    CONSTRAINT mihoyo_bindings_mid_len CHECK (mid IS NULL OR char_length(mid) BETWEEN 1 AND 64)
);
CREATE UNIQUE INDEX mihoyo_bindings_mid_uq ON mihoyo_bindings (mid) WHERE mid IS NOT NULL;

-- 扫码会话（旧系统存在 Redis，MHY-002）。客户端拿到的是我们自己的随机 qrId（这里只存它的 SHA-256），
-- 不是米游社的 ticket：ticket 印在二维码里，看到二维码的人不能凭它轮询拿走登录会话。
CREATE TABLE mihoyo_qr_logins (
    id_hash          BYTEA       PRIMARY KEY,
    ticket           TEXT        NOT NULL,
    mode             TEXT        NOT NULL,
    -- 绑定模式：发起绑定的账号。
    user_id          BIGINT      REFERENCES users (id) ON DELETE CASCADE,
    -- 上游要求轮询时的设备信息与创建时一致（device_id、型号、系统、UA）。
    device           JSONB       NOT NULL,
    expires_at       TIMESTAMPTZ NOT NULL,
    -- 状态微缓存（MHY-006，2 秒内不重复查上游）。
    last_status      TEXT,
    last_polled_at   TIMESTAMPTZ,
    -- 处理「已确认」的互斥（MHY-007）。
    claimed_until    TIMESTAMPTZ,
    -- 处理结果，供后续轮询重放（MHY-008）。
    result           JSONB,
    result_expires_at TIMESTAMPTZ,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT mihoyo_qr_logins_hash_len CHECK (octet_length(id_hash) = 32),
    CONSTRAINT mihoyo_qr_logins_mode_check CHECK (mode IN ('login', 'bind')),
    CONSTRAINT mihoyo_qr_logins_user_check CHECK ((mode = 'bind') = (user_id IS NOT NULL)),
    CONSTRAINT mihoyo_qr_logins_result_check CHECK ((result IS NULL) = (result_expires_at IS NULL))
);
CREATE INDEX mihoyo_qr_logins_expires_idx ON mihoyo_qr_logins (expires_at);
CREATE INDEX mihoyo_qr_logins_user_idx ON mihoyo_qr_logins (user_id) WHERE user_id IS NOT NULL;
