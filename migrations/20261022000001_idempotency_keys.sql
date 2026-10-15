-- 创建类接口的幂等键（Idempotency-Key 头）。属于 ik-server 的横切中间件，不属于任何业务域。
-- 只保存成功（2xx）的响应；失败的请求没有副作用，删掉记录让客户端用同一个键重试。

CREATE TABLE idempotency_keys (
    user_id       BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    key           UUID        NOT NULL,
    -- SHA-256(方法、路径与查询、请求体)，用来发现同一个键被用在了不同的请求上。
    fingerprint   BYTEA       NOT NULL,
    -- 处理中：占用到这个时间，过期视为放弃（实例崩溃），允许重新处理。处理完为 NULL。
    locked_until  TIMESTAMPTZ,
    status        SMALLINT,
    -- 需要原样重放的响应头（content-type、etag、location）。
    headers       JSONB,
    body          BYTEA,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (user_id, key),
    CONSTRAINT idempotency_keys_fingerprint_len CHECK (octet_length(fingerprint) = 32),
    CONSTRAINT idempotency_keys_state CHECK (
        (locked_until IS NOT NULL AND status IS NULL AND headers IS NULL AND body IS NULL)
        OR (locked_until IS NULL AND status BETWEEN 200 AND 299 AND headers IS NOT NULL AND body IS NOT NULL)
    )
);

-- 每日清理超过 24 小时的键。
CREATE INDEX idempotency_keys_created_at_idx ON idempotency_keys (created_at);
