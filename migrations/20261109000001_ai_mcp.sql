-- AI 助手调用 MCP 工具（ADR 0004）。归 ik-ai。

-- 站外 MCP 服务（管理后台维护，Streamable HTTP）。工具名前缀是 `name`。
CREATE TABLE ai_mcp_servers (
    id                BIGINT      PRIMARY KEY,
    name              TEXT        NOT NULL,
    display_name      TEXT        NOT NULL,
    url               TEXT        NOT NULL,
    -- 整行请求头（如 `Authorization: Bearer xxx`），只在服务端流转；后台接口只回末四位提示。空串表示不带。
    auth_header       TEXT        NOT NULL DEFAULT '',
    timeout_ms        INTEGER     NOT NULL DEFAULT 15000,
    max_result_chars  INTEGER     NOT NULL DEFAULT 4000,
    enabled           BOOLEAN     NOT NULL DEFAULT TRUE,
    sort_order        INTEGER     NOT NULL DEFAULT 0,
    -- 最近一次拉取工具清单的时间与失败原因（成功时清空）。
    last_listed_at    TIMESTAMPTZ,
    last_error        TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_mcp_servers_name_uq UNIQUE (name),
    CONSTRAINT ai_mcp_servers_name_shape CHECK (name ~ '^[a-z0-9][a-z0-9_-]{0,31}$'),
    CONSTRAINT ai_mcp_servers_display_name_len CHECK (char_length(display_name) BETWEEN 1 AND 64),
    CONSTRAINT ai_mcp_servers_url_shape CHECK (url ~ '^https?://[^[:space:]]+$' AND char_length(url) <= 512),
    CONSTRAINT ai_mcp_servers_auth_header_len CHECK (char_length(auth_header) <= 4096),
    CONSTRAINT ai_mcp_servers_timeout CHECK (timeout_ms BETWEEN 1000 AND 60000),
    CONSTRAINT ai_mcp_servers_max_result CHECK (max_result_chars BETWEEN 500 AND 16000)
);

-- 站外服务的工具。`remote` 是最近一次拉到的定义，`approved` 是管理员确认过的定义；运行时只用
-- `approved`，且只在两者摘要相同时提供（远端改了描述就自动停用，等管理员重新确认）。
-- 定义的形状：`{"title", "description", "inputSchema", "readOnly"}`。
CREATE TABLE ai_mcp_tools (
    id               BIGINT      PRIMARY KEY,
    server_id        BIGINT      NOT NULL REFERENCES ai_mcp_servers (id) ON DELETE CASCADE,
    remote_name      TEXT        NOT NULL,
    -- 暴露给模型的名字：`<服务名>__<工具名>`（非法字符换成 `_`，最长 64）。
    exposed_name     TEXT        NOT NULL,
    remote           JSONB       NOT NULL,
    remote_digest    BYTEA       NOT NULL,
    approved         JSONB,
    approved_digest  BYTEA,
    enabled          BOOLEAN     NOT NULL DEFAULT FALSE,
    -- 时间线文案（如「检索虚空终端」），空时用 MCP 的 title。
    timeline_text    TEXT,
    -- 远端最近一次清单里已经没有它了。
    missing          BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT ai_mcp_tools_remote_uq UNIQUE (server_id, remote_name),
    CONSTRAINT ai_mcp_tools_exposed_uq UNIQUE (exposed_name),
    CONSTRAINT ai_mcp_tools_exposed_shape CHECK (exposed_name ~ '^[A-Za-z0-9_-]{1,64}$'),
    CONSTRAINT ai_mcp_tools_timeline_len CHECK (timeline_text IS NULL OR char_length(timeline_text) <= 32),
    -- 第一期只允许只读工具（`annotations.readOnlyHint = true`）。
    CONSTRAINT ai_mcp_tools_enabled_approved CHECK (
        NOT enabled OR (approved IS NOT NULL AND approved_digest IS NOT NULL
                        AND (approved ->> 'readOnly')::boolean IS TRUE)
    )
);

-- 角色卡：是否提供剧情库工具；启用哪些站外服务（与 allowed_model_ids 一样用数组，删服务时从数组里去掉）。
ALTER TABLE ai_role_cards
    ADD COLUMN story_tools    BOOLEAN  NOT NULL DEFAULT FALSE,
    ADD COLUMN mcp_server_ids BIGINT[] NOT NULL DEFAULT '{}';
