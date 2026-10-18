-- 对外展示的「第 N 位用户」编号（会员号）。主键仍是雪花 ID（AGENTS.md 规则 8），这一列只用于展示和查找。
--
-- 编号规则：
-- - 从旧系统迁移来的账号：沿用旧的整数 id（用户以前看到的 UID）。雪花 ID 的时间戳部分大于 0，
--   一定 >= 2^22；小于它的就是迁移来的旧 id（见 ik_kernel::id）。
-- - 新注册的账号：接在最大的旧 id 后面，按注册顺序递增。
-- 列带默认值，旧版本的 ik-server 插入账号时同样会拿到编号，新旧二进制可以并行。

CREATE SEQUENCE users_member_no_seq AS BIGINT;

ALTER TABLE users ADD COLUMN member_no BIGINT;

UPDATE users SET member_no = id WHERE id < 4194304;

SELECT setval('users_member_no_seq', COALESCE((SELECT max(member_no) FROM users), 0) + 1, false);

UPDATE users u
   SET member_no = n.member_no
  FROM (
        SELECT id, nextval('users_member_no_seq') AS member_no
          FROM (SELECT id FROM users WHERE member_no IS NULL ORDER BY created_at, id) ordered
       ) n
 WHERE u.id = n.id;

ALTER TABLE users
    ALTER COLUMN member_no SET DEFAULT nextval('users_member_no_seq'),
    ALTER COLUMN member_no SET NOT NULL,
    ADD CONSTRAINT users_member_no_uq UNIQUE (member_no);

ALTER SEQUENCE users_member_no_seq OWNED BY users.member_no;
