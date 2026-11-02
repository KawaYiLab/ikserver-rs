-- 帖子的短编号，帖子地址用它（`/post/{postNo}`）。主键仍是雪花 ID（AGENTS.md 规则 8），这一列只用于展示和查找。
--
-- 编号规则（跟用户的会员号一样，见 20261106000001_user_member_no.sql）：
-- - 从旧系统迁移来的帖子：沿用旧的整数 id（旧链接 /post/6973 不变）。雪花 ID 一定 >= 2^22，
--   小于它的就是迁移来的旧 id（见 ik_kernel::id）。
-- - 新帖：接在最大的旧 id 后面，按创建顺序递增。草稿建好就有编号，没发出去的会留下空号。
-- 列带默认值，旧版本的 ik-server 插入帖子时同样会拿到编号，新旧二进制可以并行。

CREATE SEQUENCE posts_post_no_seq AS BIGINT;

ALTER TABLE posts ADD COLUMN post_no BIGINT;

UPDATE posts SET post_no = id WHERE id < 4194304;

SELECT setval('posts_post_no_seq', COALESCE((SELECT max(post_no) FROM posts), 0) + 1, false);

UPDATE posts p
   SET post_no = n.post_no
  FROM (
        SELECT id, nextval('posts_post_no_seq') AS post_no
          FROM (SELECT id FROM posts WHERE post_no IS NULL ORDER BY created_at, id) ordered
       ) n
 WHERE p.id = n.id;

ALTER TABLE posts
    ALTER COLUMN post_no SET DEFAULT nextval('posts_post_no_seq'),
    ALTER COLUMN post_no SET NOT NULL,
    ADD CONSTRAINT posts_post_no_uq UNIQUE (post_no);

ALTER SEQUENCE posts_post_no_seq OWNED BY posts.post_no;
