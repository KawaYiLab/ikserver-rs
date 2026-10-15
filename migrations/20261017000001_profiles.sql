-- 个人资料：签名、主页隐藏、主页精选。业务规则见 docs/business-rules/social-economy.md 第 11 节，
-- 设计说明见 docs/schema.md。用户名在 users 表（auth 域），头像、名片随 media 模块加入。

-- 每个用户一行，第一次修改资料时创建；没有行时按默认值（空签名、不隐藏、未配置精选）。
CREATE TABLE user_profiles (
    user_id          BIGINT      PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    -- 纯文本签名，最多 100 字（PROFILE-007）。
    bio              TEXT        NOT NULL DEFAULT '',
    -- 隐藏主页：访客看到精简视图，也搜不到（PROFILE-009）。
    hidden           BOOLEAN     NOT NULL DEFAULT FALSE,
    -- 主页精选，有序。NULL = 未配置（主页按时间展示全部），空数组 = 主动清空（PROFILE-010）。
    pinned_post_ids  BIGINT[],
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT user_profiles_bio_len CHECK (char_length(bio) <= 100),
    CONSTRAINT user_profiles_pinned_max CHECK (
        pinned_post_ids IS NULL OR cardinality(pinned_post_ids) <= 6
    )
);
