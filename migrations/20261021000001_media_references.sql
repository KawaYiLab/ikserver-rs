-- 媒体引用：头像改存在账号上、帖子封面、评论图片、缩略哈希。
-- 设计说明见 docs/schema.md「媒体引用」。

-- 头像从 user_profiles 挪到 users：帖子卡片、评论、通知、私信里的作者展示都经 auth 的作者资料带出，
-- 不必各自去读 profile 的表。20261020000002 加的列还没有任何版本上线，这里直接迁走并删除，
-- 不走 expand/contract。
ALTER TABLE users
    ADD COLUMN avatar_media_id BIGINT REFERENCES media_files (id) ON DELETE SET NULL;
CREATE INDEX users_avatar_media_idx ON users (avatar_media_id) WHERE avatar_media_id IS NOT NULL;
UPDATE users u SET avatar_media_id = p.avatar_media_id
FROM user_profiles p
WHERE p.user_id = u.id AND p.avatar_media_id IS NOT NULL;
ALTER TABLE user_profiles DROP COLUMN avatar_media_id;

-- 帖子封面（有序，POST-006）。上限随等级（最多 40 张，POST-012），DDL 只兜底最高档。
-- 引用的文件从不物理删除（D-078），所以用数组而不是关系表；归属在写入时校验（D-012）。
ALTER TABLE posts
    ADD COLUMN cover_ids BIGINT[] NOT NULL DEFAULT '{}',
    ADD CONSTRAINT posts_cover_count CHECK (cardinality(cover_ids) <= 40);
ALTER TABLE post_drafts
    ADD COLUMN cover_ids BIGINT[] NOT NULL DEFAULT '{}',
    ADD CONSTRAINT post_drafts_cover_count CHECK (cardinality(cover_ids) <= 40);

-- 评论图片（有序，CMT-020），最多 18 张。
ALTER TABLE comments
    ADD COLUMN image_ids BIGINT[] NOT NULL DEFAULT '{}',
    ADD CONSTRAINT comments_image_count CHECK (cardinality(image_ids) <= 18);

-- 图片的 ThumbHash（base64），客户端加载前显示的模糊占位。解码失败（或 AVIF）时为空。
ALTER TABLE media_files ADD COLUMN thumbhash TEXT;
