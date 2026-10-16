-- 管理后台调整丁尼与经验（media-ops.md ADMINCRUD：用户「调整丁尼 / 经验」；DENNY-020 的 admin_grant 第一次有写入方）。
ALTER TABLE denny_logs DROP CONSTRAINT denny_logs_action_check;
ALTER TABLE denny_logs ADD CONSTRAINT denny_logs_action_check CHECK (action IN (
    'check_in', 'gift_sent', 'gift_received', 'spend_rename', 'spend_avatar', 'exam_passed',
    'admin_grant', 'admin_deduct', 'spend_delete_post', 'refund', 'level_up'
));
ALTER TABLE exp_logs DROP CONSTRAINT exp_logs_action_check;
ALTER TABLE exp_logs ADD CONSTRAINT exp_logs_action_check CHECK (action IN (
    'check_in', 'create_post', 'create_comment', 'like_give', 'comment_accepted',
    'denny_received', 'exam_passed', 'admin_grant', 'like_received', 'comment_received', 'favorite_received'
));
