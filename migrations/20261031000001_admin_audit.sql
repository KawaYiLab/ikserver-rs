-- 管理后台的操作审计（api-guidelines.md「管理后台」：写操作一律写审计日志）。归 ik-auth。
--
-- 每个 `/v1/admin/*` 写操作与业务写入同一事务记一行：操作者、动作、目标、参数、IP、UA。
-- 举报结案另有业务上的审核日志（`moderation_log`），两者并存：这里是运维审计，那里是处理记录。
CREATE TABLE admin_audit_log (
    id          BIGINT      PRIMARY KEY,
    actor_id    BIGINT      REFERENCES users (id) ON DELETE SET NULL,
    -- 形如 `exam.question.create`、`ai.model.update`、`user.block`。
    action      TEXT        NOT NULL,
    target_type TEXT        NOT NULL,
    -- 目标的标识（id、key 或 slug），批量操作为 NULL（详见 details）。
    target_id   TEXT,
    -- 请求参数摘要（不含密钥类字段）。
    details     JSONB       NOT NULL DEFAULT '{}',
    ip          INET,
    user_agent  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT admin_audit_log_action_len CHECK (char_length(action) BETWEEN 1 AND 64),
    CONSTRAINT admin_audit_log_target_type_len CHECK (char_length(target_type) BETWEEN 1 AND 32),
    CONSTRAINT admin_audit_log_target_id_len CHECK (target_id IS NULL OR char_length(target_id) <= 128),
    CONSTRAINT admin_audit_log_ua_len CHECK (user_agent IS NULL OR char_length(user_agent) <= 512)
);
CREATE INDEX admin_audit_log_created_idx ON admin_audit_log (created_at DESC, id DESC);
CREATE INDEX admin_audit_log_actor_idx ON admin_audit_log (actor_id, created_at DESC);
CREATE INDEX admin_audit_log_target_idx ON admin_audit_log (target_type, target_id, created_at DESC);
