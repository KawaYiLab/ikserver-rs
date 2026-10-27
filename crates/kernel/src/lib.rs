//! 所有业务域共享的基础设施：配置、错误、ID、时间、分页游标、事件总线等。
//!
//! 本 crate 不依赖任何业务域。

pub mod bus;
pub mod cluster;
pub mod config;
pub mod cursor;
pub mod db;
pub mod error;
pub mod extract;
pub mod http;
pub mod id;
pub mod jobs;
pub mod mail;
pub mod role;
pub mod site;
pub mod time;

pub use error::{FieldError, Problem};
pub use id::{Id, Snowflake};
pub use role::Role;
