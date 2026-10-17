-- 搜索索引增量扫描：每轮按 updated_at 取 since 之后改动的帖子（ik_search::indexer::sweep）。
-- 没有这个索引时每轮都要扫整张帖子表。计数（浏览、点赞、评论数）不改 updated_at，所以这个索引写入很少。
CREATE INDEX posts_updated_idx ON posts (updated_at, id);
