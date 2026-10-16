//! 导入入口：预检 → 确认目标库为空 → 一个事务里按依赖顺序跑完全部步骤。

use anyhow::{bail, ensure};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;

use crate::check::{Severity, check};
use crate::steps;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// 旧库时间列所用的时区（IANA 名，例如 `Asia/Shanghai` 或 `UTC`），没有默认值。
    pub legacy_timezone: String,
    /// 导入时刻：没有原始时间、又必须有值的字段（例如存量用户的考试通过时间，EXAM-014）用它。
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StepReport {
    pub step: &'static str,
    pub rows: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct ImportReport {
    pub steps: Vec<StepReport>,
}

impl ImportReport {
    pub(crate) fn add(&mut self, step: &'static str, rows: u64) {
        tracing::info!(step, rows, "导入步骤完成");
        self.steps.push(StepReport { step, rows });
    }
}

pub async fn import(db: &PgPool, options: &ImportOptions) -> anyhow::Result<ImportReport> {
    let valid_zone = sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1) AS "ok!""#,
        options.legacy_timezone
    )
    .fetch_one(db)
    .await?;
    ensure!(valid_zone, "未知的时区：{}", options.legacy_timezone);

    let blocking: Vec<_> = check(db)
        .await?
        .into_iter()
        .filter(|f| f.severity == Severity::Blocking)
        .collect();
    if !blocking.is_empty() {
        let lines: Vec<String> = blocking
            .iter()
            .map(|f| format!("{}（{} 处）：{}", f.key, f.count, f.message))
            .collect();
        bail!("预检有必须先处理的问题，未导入：\n{}", lines.join("\n"));
    }

    let occupied = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM users) AS "x!""#)
        .fetch_one(db)
        .await?;
    ensure!(!occupied, "目标库已经有账号数据；导入只能对全新的库执行");

    let mut report = ImportReport::default();
    let mut tx = db.begin().await?;
    steps::accounts::run(&mut tx, options, &mut report).await?;
    tx.commit().await?;
    Ok(report)
}
