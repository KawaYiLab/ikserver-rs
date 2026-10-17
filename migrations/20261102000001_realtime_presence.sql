-- 在线人数（PRES-001～006）：每个实例每 15 秒写一行自己的实时连接快照，读时合并 45 秒内更新过的行。
-- 归 ik-realtime。数据只有几十秒的意义，崩溃后丢失无妨，所以是 UNLOGGED（不写 WAL）。

CREATE UNLOGGED TABLE realtime_presence (
    -- 实例编号（IK_NODE_ID）。
    node_id     INTEGER     PRIMARY KEY,
    -- 有连接的登录用户，最近建立连接的在前。
    user_ids    BIGINT[]    NOT NULL,
    -- 游客连接数。
    guests      INTEGER     NOT NULL CHECK (guests >= 0),
    updated_at  TIMESTAMPTZ NOT NULL
);
