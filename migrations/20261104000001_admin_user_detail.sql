-- 管理后台的账号详情：经验流水按时间翻页、某人内容收到的举报。

-- 原有索引 (user_id, biz_day, bucket) 只服务每日上限的统计，按时间倒序翻页要另建。
CREATE INDEX exp_logs_user_created_idx ON exp_logs (user_id, created_at DESC, id DESC);

-- 某人（作者或被举报的用户）收到的举报。匿名内容的举报同样记录作者，查询时按快照排除（AGENTS.md 规则 10）。
CREATE INDEX reports_owner_idx ON reports (owner_user_id, created_at DESC) WHERE owner_user_id IS NOT NULL;
