-- 扁平评论（ADR 0006）：取消楼中楼，所有评论按时间排成一条线，各占一个楼层；
-- 回复只记录「回复了哪条」（reply_to_id + 当时的楼层号 reply_to_floor），被回复的评论删除后
-- 回复保留，显示「原评论已删除」。

-- 帖子已分配的最大楼层号。楼层号在锁住帖子行后由它递增分配，删除评论不回收（包括最后一层）。
ALTER TABLE posts ADD COLUMN last_comment_floor INTEGER NOT NULL DEFAULT 0;

ALTER TABLE comments
    DROP CONSTRAINT comments_floor_top_level,
    DROP CONSTRAINT comments_flags_top_level,
    DROP CONSTRAINT comments_floor_uq;

-- 1. 推断旧回复真正回复的是谁：正文以提及开头（前端「回复某条回复」时预填的 chip，CMT-026），
--    就取同一楼里此前最后一条该用户的实名回复；否则是所在的一级评论。
ALTER TABLE comments ADD COLUMN reply_to_new BIGINT;
UPDATE comments r
SET reply_to_new = coalesce(
    (SELECT prev.id FROM comments prev
     WHERE prev.parent_id = r.parent_id
       AND NOT prev.is_anonymous
       AND prev.author_id::text = substring(r.body FROM '^\s*@\[[^\[\]\n]{1,40}\]\((\d{1,20})\)')
       AND (prev.created_at, prev.id) < (r.created_at, r.id)
     ORDER BY prev.created_at DESC, prev.id DESC
     LIMIT 1),
    r.parent_id)
WHERE r.parent_id IS NOT NULL;

-- 2. 全部评论按发表时间重新编号（同一帖子内从 1 开始）。
UPDATE comments c SET floor = n.rn
FROM (SELECT id, row_number() OVER (PARTITION BY post_id ORDER BY created_at, id)::int AS rn
      FROM comments) n
WHERE c.id = n.id;

-- 3. 改成「回复了哪条」：任意一条同帖评论，删除时置空（回复本身保留）。
ALTER TABLE comments DROP CONSTRAINT comments_parent_id_fkey;
DROP INDEX comments_replies_idx;
DROP INDEX comments_top_level_idx;
ALTER TABLE comments RENAME COLUMN parent_id TO reply_to_id;
UPDATE comments SET reply_to_id = reply_to_new WHERE reply_to_new IS NOT NULL;
ALTER TABLE comments DROP COLUMN reply_to_new;
ALTER TABLE comments
    ADD CONSTRAINT comments_reply_to_id_fkey
    FOREIGN KEY (reply_to_id) REFERENCES comments (id) ON DELETE SET NULL;

-- 被回复评论的楼层号快照：原评论删除后 reply_to_id 变成 NULL，仍能显示「回复 #12（已删除）」。
ALTER TABLE comments ADD COLUMN reply_to_floor INTEGER;
UPDATE comments r SET reply_to_floor = t.floor
FROM comments t
WHERE t.id = r.reply_to_id;

ALTER TABLE comments
    ALTER COLUMN floor SET NOT NULL,
    ADD CONSTRAINT comments_floor_uq UNIQUE (post_id, floor),
    ADD CONSTRAINT comments_reply_to_floor CHECK (reply_to_id IS NULL OR reply_to_floor IS NOT NULL);

-- 某条评论收到的回复（旧的在前）+ reply_to_id 外键。评论列表按 (post_id, floor) 走 comments_floor_uq。
CREATE INDEX comments_reply_to_idx ON comments (reply_to_id, floor) WHERE reply_to_id IS NOT NULL;

UPDATE posts p SET last_comment_floor = m.max_floor
FROM (SELECT post_id, max(floor) AS max_floor FROM comments GROUP BY post_id) m
WHERE p.id = m.post_id;
