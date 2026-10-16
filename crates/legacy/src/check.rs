//! 预检：只读地统计旧数据里需要注意的情况，导入前先跑一遍、交给运维或产品确认。
//!
//! - `Blocking`：导入会拒绝执行，必须先在旧库里处理（例如规范化后重复的邮箱，ACC-007）。
//! - `Fixed`：导入时按固定规则自动修正，报告里列出数量（例如超长的签名截断）。
//! - `Info`：只是告知（例如数据量、时间列的时区判断依据）。

use serde::Serialize;
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocking,
    Fixed,
    Info,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub key: &'static str,
    pub severity: Severity,
    pub count: i64,
    pub message: String,
}

impl Finding {
    fn new(key: &'static str, severity: Severity, count: i64, message: impl Into<String>) -> Self {
        Self {
            key,
            severity,
            count,
            message: message.into(),
        }
    }
}

/// 跑全部检查。只报告 `count > 0` 的项（`Info` 总是报告）。
pub async fn check(db: &PgPool) -> anyhow::Result<Vec<Finding>> {
    crate::install_helpers(db).await?;
    let mut out = Vec::new();
    accounts(db, &mut out).await?;
    out.retain(|f| f.severity == Severity::Info || f.count > 0);
    Ok(out)
}

async fn accounts(db: &PgPool, out: &mut Vec<Finding>) -> anyhow::Result<()> {
    let users = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM legacy.up_users"#)
        .fetch_one(db)
        .await?;
    out.push(Finding::new(
        "users.total",
        Severity::Info,
        users,
        "旧库账号总数",
    ));

    let dup_email = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM (
               SELECT email FROM legacy.ik_users WHERE email IS NOT NULL
               GROUP BY email HAVING count(*) > 1
           ) d"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "users.duplicate_email",
        Severity::Blocking,
        dup_email,
        "规范化（trim + 小写）后重复的邮箱：先在旧库合并或改掉，再导入（ACC-007）",
    ));

    let renamed =
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM legacy.ik_users WHERE renamed"#)
            .fetch_one(db)
            .await?;
    out.push(Finding::new(
        "users.renamed",
        Severity::Fixed,
        renamed,
        "用户名含 '@'、为空、超过 32 字或处理后撞名：'@' 换成全角、截断，撞名的改为「用户_<id>」",
    ));

    let long_bio = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM legacy.ik_profiles WHERE char_length(bio_text) > 100"#
    )
    .fetch_one(db)
    .await?;
    out.push(Finding::new(
        "profiles.bio_truncated",
        Severity::Fixed,
        long_bio,
        "个人签名超过 100 字，截断到 100 字",
    ));

    // 判断旧库时间列存的是哪个时区：最新一条记录分别按 UTC 和北京时间解读，
    // 与导出时间对照，明显「在未来」的那种解读就是错的。
    let latest = sqlx::query_scalar!(r#"SELECT max(created_at)::text AS "t" FROM legacy.up_users"#)
        .fetch_one(db)
        .await?;
    out.push(Finding::new(
        "time.latest_user_created_at",
        Severity::Info,
        0,
        format!(
            "最新注册时间（原样）：{}。按 UTC 解读与按 Asia/Shanghai 解读相差 8 小时；\
             对照导出旧库的时刻判断 --legacy-timezone 该填哪个",
            latest.as_deref().unwrap_or("（无数据）")
        ),
    ));
    Ok(())
}
