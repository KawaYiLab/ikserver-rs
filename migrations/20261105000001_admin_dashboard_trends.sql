-- 管理后台工作台的按天趋势（新增账号、首次发布的帖子、举报）。评论已有 comments_recent_idx (created_at)。
CREATE INDEX users_created_idx ON users (created_at);
CREATE INDEX posts_first_published_idx ON posts (first_published_at) WHERE first_published_at IS NOT NULL;
CREATE INDEX reports_created_idx ON reports (created_at);
