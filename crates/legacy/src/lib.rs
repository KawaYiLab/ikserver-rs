//! 一次性数据迁移：把旧 Strapi 库导入新表（ADR 0003，操作步骤见 crates/legacy/README.md）。
//!
//! 旧库先整体恢复到新库的 `legacy` schema，导入在**一个事务**里用 `INSERT … SELECT` 完成，
//! 不经过各业务域的 service（这是迁移，与 `migrations/` 一样直接写表，是模块边界规则的明确例外）。
//!
//! - 旧数据保留原来的整数 id（新雪花 ID 远大于它们，不会冲突，见 `ik_kernel::id`）。
//! - 映射规则集中在 `sql/helpers.sql` 的 `legacy.ik_*` 视图里。
//! - 旧库的时间列不带时区，存的是旧服务进程所在时区的墙上时间，导入时必须显式给出这个时区。

mod check;
mod import;
mod steps;

pub use check::{Finding, Severity, check};
pub use import::{ImportOptions, ImportReport, StepReport, import};

/// 导入工具在 `legacy` schema 里建的辅助视图（幂等）。
pub const HELPERS_SQL: &str = include_str!("../sql/helpers.sql");

/// 旧 Strapi 的表结构（只用于编译期检查与测试，生产环境的 legacy schema 来自真实导出）。
pub const STRAPI_SCHEMA_SQL: &str = include_str!("../sql/strapi-schema.sql");

/// 建好（或刷新）辅助视图。
pub async fn install_helpers(db: &sqlx::PgPool) -> anyhow::Result<()> {
    // 静态的 DDL 文件（随二进制编译进来，不含任何外部输入）；DDL 无法用编译期宏检查。
    sqlx::raw_sql(HELPERS_SQL).execute(db).await?;
    Ok(())
}
