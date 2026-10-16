//! 账号：users、user_passwords、user_profiles（头像、装备、主页精选在后面的步骤补）。

use sqlx::{PgConnection, Postgres, Transaction};

use crate::import::{ImportOptions, ImportReport};

pub async fn run(
    tx: &mut Transaction<'_, Postgres>,
    options: &ImportOptions,
    report: &mut ImportReport,
) -> anyhow::Result<()> {
    report.add("users", users(tx, options).await?);
    report.add("user_passwords", passwords(tx).await?);
    report.add("user_profiles", profiles(tx, options).await?);
    Ok(())
}

/// 角色：旧的 `isAdmin` → admin（旧系统没有论坛内的版主角色，审核员是 Strapi 后台账号，D-062）。
/// 考试：已通过的账号沿用通过时间，没有时间的（EXAM-014 存量豁免）用导入时刻。
async fn users(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO users (id, username, email, status, role, is_ai_agent, exam_passed_at,
                              legacy_id, level, created_at, updated_at)
           SELECT u.id, u.username, u.email, u.status,
                  CASE WHEN u.is_admin THEN 'admin' ELSE 'user' END,
                  coalesce(u.is_ai_agent, false),
                  CASE WHEN u.exam_passed THEN coalesce(u.exam_passed_at AT TIME ZONE $1, $2) END,
                  coalesce(ua.author_document_id, u.document_id),
                  least(greatest(coalesce(u.level, 0), 0), 7),
                  coalesce(u.created_at AT TIME ZONE $1, $2),
                  coalesce(u.updated_at AT TIME ZONE $1, u.created_at AT TIME ZONE $1, $2)
           FROM legacy.ik_users u
           LEFT JOIN legacy.ik_user_authors ua ON ua.user_id = u.id"#,
        options.legacy_timezone,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 只迁移真正由用户设置的 bcrypt 哈希（登录时透明升级成 Argon2id，ACC-012）。米游社首登账号的
/// 随机密码（provider = 'mihoyo'，LOGIN-009）和注销后写入的随机串都不算「设置过密码」。
async fn passwords(conn: &mut PgConnection) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO user_passwords (user_id, hash, updated_at)
           SELECT u.id, u.password, now()
           FROM legacy.ik_users u
           WHERE u.status IN ('active', 'blocked')
             AND u.password ~ '^\$2[aby]\$\d{2}\$'
             AND u.provider IS DISTINCT FROM 'mihoyo'"#
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}

/// 只给有内容的账号建资料行（其余按需惰性创建）。签名超过 100 字截断（预检里报告）。
async fn profiles(conn: &mut PgConnection, options: &ImportOptions) -> anyhow::Result<u64> {
    let result = sqlx::query!(
        r#"INSERT INTO user_profiles (user_id, bio, hidden, updated_at)
           SELECT p.user_id, left(btrim(p.bio_text), 100), p.hidden, $1
           FROM legacy.ik_profiles p
           WHERE btrim(p.bio_text) <> '' OR p.hidden"#,
        options.now,
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected())
}
