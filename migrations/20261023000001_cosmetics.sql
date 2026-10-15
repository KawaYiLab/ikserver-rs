-- 装扮：系统头像、论坛名片（COSMETIC-001～009）。目录归 ik-cosmetics；装备状态是账号上的列（auth 的表）。
-- 所有头像、名片免费、对所有人开放（COSMETIC-002）；目录只由管理员维护。

CREATE TABLE system_avatars (
    id              BIGINT      PRIMARY KEY,
    name            TEXT        NOT NULL,
    kind            TEXT        NOT NULL DEFAULT 'character',
    image_media_id  BIGINT      NOT NULL REFERENCES media_files (id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT system_avatars_name_uq UNIQUE (name),
    CONSTRAINT system_avatars_name_len CHECK (char_length(name) BETWEEN 1 AND 50),
    CONSTRAINT system_avatars_kind_check CHECK (kind IN ('character', 'city', 'news'))
);
-- 列表顺序：类型，再按创建先后（雪花 id 即时间序，COSMETIC-004）。
CREATE INDEX system_avatars_order_idx ON system_avatars (kind, id);
CREATE INDEX system_avatars_image_idx ON system_avatars (image_media_id);

CREATE TABLE business_cards (
    id              BIGINT      PRIMARY KEY,
    name            TEXT        NOT NULL,
    kind            TEXT        NOT NULL DEFAULT 'character',
    description     TEXT        NOT NULL DEFAULT '',
    -- 背景故事（Markdown；旧系统是富文本 blocks）。
    story           TEXT        NOT NULL DEFAULT '',
    image_media_id  BIGINT      NOT NULL REFERENCES media_files (id),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT business_cards_name_uq UNIQUE (name),
    CONSTRAINT business_cards_name_len CHECK (char_length(name) BETWEEN 1 AND 50),
    CONSTRAINT business_cards_description_len CHECK (char_length(description) <= 500),
    CONSTRAINT business_cards_story_len CHECK (char_length(story) <= 20000),
    CONSTRAINT business_cards_kind_check CHECK (kind IN ('character', 'city', 'news'))
);
CREATE INDEX business_cards_order_idx ON business_cards (kind, id);
CREATE INDEX business_cards_image_idx ON business_cards (image_media_id);

-- 装备。系统头像的图片 id 冗余一份在账号上：各域展示作者时经 auth 一次取到「装备头像优先，
-- 其次自定义头像」（COSMETIC-008、D-056 方案 A），不必再读装扮目录。目录换图时由 cosmetics
-- 在同一事务里同步（ik_auth::interop::refresh_equipped_avatar）。
-- 外键不级联：删除目录项前必须先卸下。
ALTER TABLE users
    ADD COLUMN equipped_avatar_id BIGINT REFERENCES system_avatars (id),
    ADD COLUMN equipped_avatar_media_id BIGINT REFERENCES media_files (id),
    ADD COLUMN equipped_card_id BIGINT REFERENCES business_cards (id),
    ADD CONSTRAINT users_equipped_avatar_pair CHECK (
        (equipped_avatar_id IS NULL) = (equipped_avatar_media_id IS NULL)
    );
CREATE INDEX users_equipped_avatar_idx ON users (equipped_avatar_id) WHERE equipped_avatar_id IS NOT NULL;
CREATE INDEX users_equipped_avatar_media_idx ON users (equipped_avatar_media_id)
    WHERE equipped_avatar_media_id IS NOT NULL;
CREATE INDEX users_equipped_card_idx ON users (equipped_card_id) WHERE equipped_card_id IS NOT NULL;
