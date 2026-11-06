-- 里区：不进首页、热门、关注流、未看流、全站搜索与 sitemap，只有选中这个分区才看得到（POST-122）。
-- 代码按 slug `inner` 认它（ik_content::INNER_CATEGORY_SLUG），排在现有分区最后，谁都能发帖。
-- id 按雪花 ID 的位布局用当前时间拼（节点 0、序号 0），与运行时生成的 ID 不会冲突。
INSERT INTO categories (id, slug, name, description, sort_order)
SELECT ((floor(extract(epoch FROM clock_timestamp()) * 1000)::bigint - 1767225600000) << 22),
       'inner', '里区', '不进首页，选中本分区才能看到。',
       COALESCE(MAX(sort_order), 0) + 1
  FROM categories
ON CONFLICT (slug) DO NOTHING;

-- 版主把帖子移到别的分区也记一笔审核日志。
ALTER TABLE moderation_log DROP CONSTRAINT moderation_log_action_check;
ALTER TABLE moderation_log ADD CONSTRAINT moderation_log_action_check CHECK (
    action IN ('auto_hide', 'reject', 'hide_content', 'unhide_content', 'delete_content',
               'move_content', 'warn_user', 'block_user', 'unblock_user')
);
