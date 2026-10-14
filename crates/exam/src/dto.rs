//! 对外的请求与响应类型。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::{ExamAttempt, ExamQuestion};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

pub use crate::paper::{ExamOption, QuestionKind};

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExamConfigView {
    pub question_count: u32,
    /// 及格线（百分比，含边界）。
    pub pass_score_percent: u32,
    pub time_limit_secs: i64,
    /// 最近这么多次全部未通过就进入冷却。
    pub max_fails_before_cooldown: u32,
    pub fail_cooldown_secs: i64,
    pub reward_denny: i32,
    pub reward_exp: i32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ActiveAttemptView {
    #[schema(value_type = String)]
    pub id: Id<ExamAttempt>,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub question_count: usize,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExamStatusView {
    pub passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub passed_at: Option<DateTime<Utc>>,
    /// 冷却剩余秒数；不在冷却中为 0。
    pub cooldown_remaining_secs: i64,
    /// 进行中（未交卷且未过期）的场次。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_attempt: Option<ActiveAttemptView>,
    pub config: ExamConfigView,
}

/// 试卷上的一道题（不含正确答案）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PaperQuestion {
    #[schema(value_type = String)]
    pub id: Id<ExamQuestion>,
    pub question: String,
    pub kind: QuestionKind,
    pub options: Vec<ExamOption>,
    pub weight: i32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartView {
    #[schema(value_type = String)]
    pub attempt_id: Id<ExamAttempt>,
    /// 恢复的是之前未交卷的场次（题目与选项顺序不变）。
    pub resumed: bool,
    pub started_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub questions: Vec<PaperQuestion>,
    pub config: ExamConfigView,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubmitRequest {
    /// 题目 id → 选中的选项 key。单选、判断题也传数组。没作答的题可以省略。
    #[schema(value_type = Object, example = json!({"7385159247626227712": ["A"]}))]
    pub answers: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct RewardView {
    pub denny: i32,
    pub exp: i32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitResult {
    pub passed: bool,
    pub score: i32,
    pub total_score: i32,
    pub score_percent: i16,
    pub correct_count: i32,
    pub question_count: i32,
    pub pass_score_percent: u32,
    /// 未通过且进入冷却时的剩余秒数；否则为 0。
    pub cooldown_remaining_secs: i64,
    /// 首次通过的奖励（实际到账数）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reward: Option<RewardView>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct ReviewQuery {
    /// 指定场次；省略时回顾最近一次已交卷的场次。
    #[param(value_type = Option<String>)]
    pub attempt_id: Option<Id<ExamAttempt>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewQuestion {
    #[schema(value_type = String)]
    pub id: Id<ExamQuestion>,
    pub question: String,
    pub kind: QuestionKind,
    pub options: Vec<ExamOption>,
    pub user_answer: Vec<String>,
    pub is_correct: bool,
    pub weight: i32,
    pub score: i32,
    /// 解析（读题库当前内容；题目已删除时省略）。不下发正确答案。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewView {
    #[schema(value_type = String)]
    pub attempt_id: Id<ExamAttempt>,
    pub passed: bool,
    pub score: i32,
    pub total_score: i32,
    pub score_percent: i16,
    pub correct_count: i32,
    pub question_count: i32,
    pub submitted_at: DateTime<Utc>,
    pub questions: Vec<ReviewQuestion>,
}

/// 题库录入（运维导入与将来的管理后台共用）。
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuestionInput {
    pub question: String,
    #[serde(default = "default_kind")]
    pub kind: QuestionKind,
    pub options: Vec<ExamOption>,
    pub correct_keys: Vec<String>,
    #[serde(default)]
    pub explanation: Option<String>,
    /// 不是 ≥1 的整数时按 1（EXAMADM-005）。
    #[serde(default)]
    pub weight: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub tag: Option<String>,
}

fn default_kind() -> QuestionKind {
    QuestionKind::Single
}
