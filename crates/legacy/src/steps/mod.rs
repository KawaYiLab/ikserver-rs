//! 导入步骤，按依赖顺序执行。每一步都是 `INSERT … SELECT`，读 `legacy.*` 与 `legacy.ik_*` 视图。

pub mod accounts;
pub mod catalog;
pub mod content;
pub mod economy;
pub mod media;
pub mod reactions;
