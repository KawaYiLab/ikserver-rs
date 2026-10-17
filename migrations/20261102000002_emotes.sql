-- 表情包：评论正文里写作 `:ik-<code>:`（CMT-007/008），客户端按清单渲染成图片。归 ik-cosmetics，只由管理员维护。
-- 下架的表情不出现在表情面板里，但仍在清单中，旧评论照常显示成图片；删除后旧评论里只剩文字。

CREATE TABLE emote_groups (
    id             BIGINT      PRIMARY KEY,
    name           TEXT        NOT NULL,
    -- 面板里的顺序（小的在前，相同时按创建先后）。
    sort_order     INTEGER     NOT NULL DEFAULT 0,
    -- 分组标签上的图标；没有时客户端用组里第一个表情。
    icon_media_id  BIGINT      REFERENCES media_files (id),
    active         BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT emote_groups_name_uq UNIQUE (name),
    CONSTRAINT emote_groups_name_len CHECK (char_length(name) BETWEEN 1 AND 20)
);
CREATE INDEX emote_groups_icon_idx ON emote_groups (icon_media_id) WHERE icon_media_id IS NOT NULL;

CREATE TABLE emotes (
    id              BIGINT      PRIMARY KEY,
    -- 含 `ik-` 前缀，与评论里的写法和前端正则一致。建好后不能改（旧评论靠它渲染）。
    code            TEXT        NOT NULL,
    name            TEXT        NOT NULL,
    -- 删除分组时表情留下、变成未分组。
    group_id        BIGINT      REFERENCES emote_groups (id) ON DELETE SET NULL,
    image_media_id  BIGINT      NOT NULL REFERENCES media_files (id),
    sort_order      INTEGER     NOT NULL DEFAULT 0,
    active          BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT emotes_code_uq UNIQUE (code),
    CONSTRAINT emotes_code_format CHECK (code ~ '^ik-[a-z0-9-]{1,32}$'),
    CONSTRAINT emotes_name_len CHECK (char_length(name) BETWEEN 1 AND 30)
);
CREATE INDEX emotes_group_idx ON emotes (group_id) WHERE group_id IS NOT NULL;
CREATE INDEX emotes_image_idx ON emotes (image_media_id);
