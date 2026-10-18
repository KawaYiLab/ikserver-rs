-- 管理后台的帖子 / 评论管理：版主不经举报直接隐藏、恢复、删除内容。

-- 「只看已隐藏」：隐藏的内容很少，部分索引足够小。
CREATE INDEX posts_hidden_idx ON posts (hidden_at DESC, id DESC) WHERE hidden_at IS NOT NULL;
CREATE INDEX comments_hidden_idx ON comments (hidden_at DESC, id DESC) WHERE hidden_at IS NOT NULL;

-- 直接恢复被隐藏的内容也记一笔。
ALTER TABLE moderation_log DROP CONSTRAINT moderation_log_action_check;
ALTER TABLE moderation_log ADD CONSTRAINT moderation_log_action_check CHECK (
    action IN ('auto_hide', 'reject', 'hide_content', 'unhide_content', 'delete_content',
               'warn_user', 'block_user', 'unblock_user')
);

-- 被处置的人（内容的作者或被处置的用户），供账号详情页的处罚记录按人查询。
-- 匿名内容同样记录真实作者（处置要用），但按人查询时必须排除 subject_anonymous（AGENTS.md 规则 10）。
ALTER TABLE moderation_log
    ADD COLUMN subject_user_id   BIGINT,
    ADD COLUMN subject_anonymous BOOLEAN NOT NULL DEFAULT FALSE;

UPDATE moderation_log SET subject_user_id = target_id WHERE target_type = 'user';

-- 内容类的旧记录：作者取自同一目标的举报（举报时记下了作者和是否匿名）。没有举报可查的保持 NULL。
UPDATE moderation_log m
   SET subject_user_id = r.owner_user_id,
       subject_anonymous = COALESCE((r.snapshot ->> 'anonymous')::boolean, FALSE)
  FROM (
        SELECT DISTINCT ON (target_type, target_id) target_type, target_id, owner_user_id, snapshot
          FROM reports
         WHERE owner_user_id IS NOT NULL
         ORDER BY target_type, target_id, created_at DESC
       ) r
 WHERE m.target_type <> 'user'
   AND m.target_type = r.target_type
   AND m.target_id = r.target_id;

CREATE INDEX moderation_log_subject_idx ON moderation_log (subject_user_id, created_at DESC)
    WHERE subject_user_id IS NOT NULL;
