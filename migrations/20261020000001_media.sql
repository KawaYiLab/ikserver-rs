-- 媒体文件：直传上传校验后的图片。业务规则见 docs/business-rules/media-ops.md 第 1～5 节，
-- 设计说明见 docs/schema.md。

CREATE TABLE media_files (
    id               BIGINT      PRIMARY KEY,
    -- 对象存储里的 key（服务端生成，客户端不可写，UPL-052）。
    object_key       TEXT        NOT NULL,
    -- 嗅探得到的真实类型（不信任客户端，UPL-015）。
    mime             TEXT        NOT NULL,
    size_bytes       BIGINT      NOT NULL,
    -- 客户端测得的尺寸（UPL-016，服务端不校验）。
    width            INTEGER,
    height           INTEGER,
    -- 服务端对已上传字节计算的 SHA-256（小写十六进制），去重只信它（UPL-051）。
    sha256           TEXT        NOT NULL,
    -- 收据：完成令牌的 SHA-256。同一令牌重试返回同一文件（UPL-014）。
    receipt          TEXT        NOT NULL,
    -- 客户端给的文件名（只用于展示）。
    name             TEXT        NOT NULL DEFAULT '',
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT media_files_object_key_uq UNIQUE (object_key),
    CONSTRAINT media_files_receipt_uq UNIQUE (receipt),
    CONSTRAINT media_files_mime_check CHECK (
        mime IN ('image/jpeg', 'image/png', 'image/gif', 'image/webp', 'image/avif')
    ),
    CONSTRAINT media_files_size_check CHECK (size_bytes BETWEEN 1 AND 31457280),
    CONSTRAINT media_files_dims_check CHECK (
        (width IS NULL OR width > 0) AND (height IS NULL OR height > 0)
    ),
    CONSTRAINT media_files_sha256_shape CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    CONSTRAINT media_files_name_len CHECK (char_length(name) <= 255)
);

-- 内容去重（完成阶段跨用户，签名阶段只限本人，D-077 方案 A）。
CREATE INDEX media_files_sha256_idx ON media_files (sha256);

-- 谁上传过这个文件（多对多）。永不对外暴露（防匿名内容溯源，UPL-056）。
-- 从图库移除只删这里的行，从不物理删除文件（MEUP-004、D-078 方案 A）。
CREATE TABLE media_uploaders (
    media_id    BIGINT      NOT NULL REFERENCES media_files (id) ON DELETE CASCADE,
    user_id     BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (media_id, user_id)
);

-- 我的图库（新的在前）+ user_id 外键。
CREATE INDEX media_uploaders_user_recent_idx ON media_uploaders (user_id, created_at DESC, media_id DESC);
