//! 测试共用：建旧库表结构、导入选项。

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use sqlx::PgPool;

pub const NOW: DateTime<Utc> = DateTime::from_timestamp_nanos(1_790_000_000_000_000_000);

/// 在测试库里建出旧 Strapi 的表结构和导入工具的辅助视图。
pub async fn setup(db: &PgPool) {
    sqlx::raw_sql(ik_legacy::STRAPI_SCHEMA_SQL)
        .execute(db)
        .await
        .unwrap();
    ik_legacy::install_helpers(db).await.unwrap();
}

pub fn options(zone: &str) -> ik_legacy::ImportOptions {
    ik_legacy::ImportOptions {
        legacy_timezone: zone.to_owned(),
        now: NOW,
    }
}

pub async fn import(db: &PgPool, zone: &str) -> ik_legacy::ImportReport {
    ik_legacy::import(db, &options(zone)).await.unwrap()
}

pub fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

pub fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}
