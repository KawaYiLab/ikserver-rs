//! 通知（敲敲）与举报。
//!
//! 匿名历史通知（D-032）：旧系统里匿名评论引起的评论 / 回复 / @ 通知（NOTIF-020 归进匿名桶的那些）
//! 存着真实发送者。新表没有匿名分组，照搬会把匿名者的身份暴露在「来自某人」的会话里，违反
//! 隐私红线（AGENTS.md 规则 10、NOTIF-013）。这类通知改为系统通知：去掉发送者，正文说明是匿名
//! 评论，保留评论摘要和帖子链接。

use sqlx::{PgConnection, Postgres, Transaction};

use crate::import::{ImportOptions, ImportReport};

pub async fn run(
    tx: &mut Transaction<'_, Postgres>,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    report.add("notifications", notifications(tx, options).await?);
    report.add("reports", reports(tx, options).await?);
    Ok(())
}

/// 发送者桶 `user-<id>`，系统通知 `system`；不认识的类型、找不到发送者的非系统通知、
/// 自己发给自己的不导入。摘要：系统通知取正文，其余取关联评论的内容。
async fn notifications(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO notifications (id, recipient_id, sender_id, kind, bucket_key, post_id, comment_id,
                                      post_title, excerpt, is_read, created_at)
           SELECT n.id, rcp.user_id,
                  CASE WHEN f.as_system THEN NULL ELSE snd.user_id END,
                  CASE WHEN f.as_system THEN 'system' ELSE n.type END,
                  CASE WHEN f.as_system THEN 'system' ELSE 'user-' || snd.user_id END,
                  p.id, c.id, left(p.title, 100),
                  left(CASE
                           WHEN n.type = 'system' THEN coalesce(n.message, '')
                           WHEN f.anonymous THEN
                               CASE n.type WHEN 'comment' THEN '有人匿名评论了你的帖子'
                                           WHEN 'reply' THEN '有人匿名回复了你的评论'
                                           ELSE '有人在匿名评论里提到了你' END
                               || coalesce('：' || nullif(legacy.ik_fix_hosts(lc.content), ''), '')
                           ELSE legacy.ik_fix_hosts(lc.content)
                       END, 500),
                  coalesce(n.is_read, false),
                  coalesce(n.created_at AT TIME ZONE $1, $2)
           FROM legacy.notifications n
           JOIN LATERAL (SELECT l.user_id FROM legacy.notifications_recipient_lnk l
                         WHERE l.notification_id = n.id ORDER BY l.id LIMIT 1) rcp ON true
           JOIN users ru ON ru.id = rcp.user_id
           LEFT JOIN LATERAL (SELECT su.id AS user_id FROM legacy.notifications_sender_lnk l
                              JOIN users su ON su.id = l.user_id
                              WHERE l.notification_id = n.id ORDER BY l.id LIMIT 1) snd ON true
           LEFT JOIN LATERAL (SELECT np.id, np.title FROM legacy.notifications_article_lnk l
                              JOIN legacy.ik_post_rows pr ON pr.row_id = l.article_id
                              JOIN posts np ON np.id = pr.post_id
                              WHERE l.notification_id = n.id ORDER BY l.id LIMIT 1) p ON true
           LEFT JOIN LATERAL (SELECT l.comment_id FROM legacy.notifications_comment_lnk l
                              WHERE l.notification_id = n.id ORDER BY l.id LIMIT 1) cl ON true
           LEFT JOIN legacy.comments lc ON lc.id = cl.comment_id
           LEFT JOIN comments c ON c.id = cl.comment_id
           CROSS JOIN LATERAL (
               SELECT n.type IN ('comment', 'reply', 'mention')
                      AND (coalesce(lc.is_anonymous, false) OR coalesce(n.bucket_key, '') LIKE 'anonymous:%')
                      AS anonymous
           ) a
           CROSS JOIN LATERAL (SELECT a.anonymous, (n.type = 'system' OR a.anonymous) AS as_system) f
           WHERE n.type IN ('comment', 'reply', 'mention', 'like', 'favorite', 'denny', 'accepted', 'system')
             AND (f.as_system OR (snd.user_id IS NOT NULL AND snd.user_id <> rcp.user_id))"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 举报：目标换成新 id（帖子按文档），记下内容作者 / 被举报的人。处理人与领取人旧系统是 Strapi
/// 后台账号（RPTADM-053：handledBy 存邮箱、用户名或 admin:<id>），按邮箱对应到论坛账号，对不上
/// 留空。已结案的必须有结果与处理时间；复核中却没有领取人的退回待处理；「其他」理由必须有说明。
async fn reports(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO reports (id, reporter_id, target_type, target_id, owner_user_id, reason, detail,
                                status, resolution, resolution_note, target_resolution_note,
                                assigned_to, assigned_at, handled_by, handled_at, snapshot, created_at)
           SELECT r.id, rep.user_id, t.target_type, t.target_id, t.owner_id, r.reason,
                  CASE WHEN r.reason = 'other' THEN coalesce(nullif(left(btrim(r.detail), 500), ''), '（未填写）')
                       ELSE nullif(left(btrim(r.detail), 500), '') END,
                  s.status,
                  CASE WHEN s.status IN ('accepted', 'rejected') THEN coalesce(r.resolution, 'none') END,
                  left(r.resolution_note, 500), left(r.target_resolution_note, 500),
                  CASE WHEN s.status = 'reviewing' THEN assignee.id END,
                  CASE WHEN s.status = 'reviewing' THEN coalesce(r.assigned_at, r.updated_at, r.created_at) AT TIME ZONE $1 END,
                  CASE WHEN s.status IN ('accepted', 'rejected') THEN handler.id END,
                  CASE WHEN s.status IN ('accepted', 'rejected')
                       THEN coalesce(r.handled_at, r.updated_at, r.created_at) AT TIME ZONE $1 END,
                  coalesce(r.snapshot, '{}'::jsonb),
                  coalesce(r.created_at AT TIME ZONE $1, $2)
           FROM legacy.reports r
           JOIN LATERAL (SELECT u.id AS user_id FROM legacy.reports_reporter_lnk l
                         JOIN users u ON u.id = l.user_id
                         WHERE l.report_id = r.id ORDER BY l.id LIMIT 1) rep ON true
           CROSS JOIN LATERAL (
               SELECT CASE r.target_type WHEN 'article' THEN 'post' ELSE r.target_type END AS target_type,
                      CASE r.target_type
                          WHEN 'article' THEN (SELECT coalesce(pr.post_id, l.article_id)
                                               FROM legacy.reports_article_lnk l
                                               LEFT JOIN legacy.ik_post_rows pr ON pr.row_id = l.article_id
                                               WHERE l.report_id = r.id ORDER BY l.id LIMIT 1)
                          WHEN 'comment' THEN (SELECT l.comment_id FROM legacy.reports_comment_lnk l
                                               WHERE l.report_id = r.id ORDER BY l.id LIMIT 1)
                          WHEN 'user' THEN (SELECT l.user_id FROM legacy.reports_target_user_lnk l
                                            WHERE l.report_id = r.id ORDER BY l.id LIMIT 1)
                      END::bigint AS target_id
           ) tt
           CROSS JOIN LATERAL (
               SELECT tt.target_type, tt.target_id,
                      CASE tt.target_type
                          WHEN 'post' THEN (SELECT np.author_id FROM posts np WHERE np.id = tt.target_id)
                          WHEN 'comment' THEN (SELECT nc.author_id FROM comments nc WHERE nc.id = tt.target_id)
                          WHEN 'user' THEN (SELECT nu.id FROM users nu WHERE nu.id = tt.target_id)
                      END AS owner_id
           ) t
           CROSS JOIN LATERAL (
               SELECT CASE
                          WHEN r.report_status IN ('accepted', 'rejected') THEN r.report_status
                          WHEN r.report_status = 'reviewing'
                               AND EXISTS (SELECT 1 FROM legacy.reports_assigned_to_lnk l WHERE l.report_id = r.id)
                          THEN 'reviewing'
                          ELSE 'pending'
                      END AS status
           ) s
           LEFT JOIN LATERAL (SELECT u.id FROM legacy.reports_assigned_to_lnk l
                              JOIN legacy.admin_users au ON au.id = l.user_id
                              JOIN users u ON u.email = lower(btrim(au.email))
                              WHERE l.report_id = r.id ORDER BY l.id LIMIT 1) assignee ON true
           LEFT JOIN LATERAL (SELECT u.id FROM legacy.admin_users au
                              JOIN users u ON u.email = lower(btrim(au.email))
                              WHERE r.handled_by IN (au.email, au.username, 'admin:' || au.id)
                              ORDER BY au.id LIMIT 1) handler ON true
           WHERE r.target_type IN ('article', 'comment', 'user')
             AND r.reason IN ('spam', 'abuse', 'porn', 'illegal', 'privacy', 'misinfo', 'plagiarism', 'other')
             AND t.target_id IS NOT NULL"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}
