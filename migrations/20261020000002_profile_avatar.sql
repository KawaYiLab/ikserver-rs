-- 自定义头像（AVA-001～003）：资料里引用一个媒体文件。文件不会被物理删除，ON DELETE 只是兜底。
ALTER TABLE user_profiles
    ADD COLUMN avatar_media_id BIGINT REFERENCES media_files (id) ON DELETE SET NULL;

CREATE INDEX user_profiles_avatar_media_idx ON user_profiles (avatar_media_id)
    WHERE avatar_media_id IS NOT NULL;
