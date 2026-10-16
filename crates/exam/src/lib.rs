//! 入站考试：题库、开考（抽题与恢复）、交卷判分、冷却、首次通过奖励、回顾。
//!
//! 业务规则：`docs/business-rules/moderation-exam.md` 第 3–4 节。通过状态写在
//! `users.exam_passed_at`（通过 auth 的 interop），奖励经 economy 发放，三者同一事务。

pub mod admin;
pub mod dto;
pub mod paper;
mod repo;
mod routes;
pub mod service;
mod state;

pub use routes::router;
pub use state::ExamState;
