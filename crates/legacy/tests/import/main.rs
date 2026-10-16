//! 旧数据导入的集成测试。合成一个测试二进制（每个二进制都要链接全部依赖，分开会让 CI 磁盘吃紧）。

mod common;

mod accounts;
mod catalog;
mod content;
mod economy;
mod media;
mod messaging;
mod notify;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");
