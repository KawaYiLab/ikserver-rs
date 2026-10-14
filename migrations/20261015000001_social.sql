-- 用户之间的关系：关注、拉黑。业务规则见 docs/business-rules/social-economy.md 第 3、4 节，
-- 设计说明见 docs/schema.md。

-- 关注。旧系统关注的是「作者」，新系统作者就是用户本身。
CREATE TABLE follows (
    follower_id  BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    followee_id  BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (follower_id, followee_id),
    -- 不能关注自己（FOLLOW-002）。
    CONSTRAINT follows_not_self CHECK (follower_id <> followee_id)
);

-- 我的关注列表（新的在前）。
CREATE INDEX follows_follower_recent_idx ON follows (follower_id, created_at DESC, followee_id DESC);
-- 粉丝数、新帖扇出 + followee_id 外键。
CREATE INDEX follows_followee_idx ON follows (followee_id, follower_id);

-- 拉黑。单向记录，但几乎所有生效点按「任一方向」判断（BLOCK-002）。记录不可修改，只能删除（BLOCK-001）。
CREATE TABLE user_blocks (
    blocker_id  BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    blocked_id  BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (blocker_id, blocked_id),
    CONSTRAINT user_blocks_not_self CHECK (blocker_id <> blocked_id)
);

-- 我的拉黑列表（新的在前）。
CREATE INDEX user_blocks_blocker_recent_idx ON user_blocks (blocker_id, created_at DESC, blocked_id DESC);
-- 「谁拉黑了我」+ blocked_id 外键。
CREATE INDEX user_blocks_blocked_idx ON user_blocks (blocked_id, blocker_id);
