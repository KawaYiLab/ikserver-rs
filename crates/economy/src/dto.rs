//! 对外的请求与响应类型。

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// 一档等级的权益。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Benefits {
    /// 帖子最多插图数。
    pub post_max_images: u16,
    /// 评论最多图片数。
    pub comment_max_images: u16,
    /// 帖子正文最多字数。
    pub post_max_body_chars: u32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BenefitsView {
    /// 当前权益等级（未登录为 0；管理员与 AI 账号恒为 7）。
    pub level: i16,
    pub max_level: i16,
    pub benefits: Benefits,
    /// 已满级时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_level: Option<i16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_benefits: Option<Benefits>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WalletView {
    /// 丁尼余额（0–9999）。
    pub balance: i32,
    /// 累计投出的丁尼。
    pub given: i32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DennyLogView {
    /// `check_in` | `gift_sent` | `gift_received` | `spend_rename` | `spend_avatar` |
    /// `exam_passed` | `admin_grant`（以及迁移来的历史动作）
    pub action: String,
    /// 有符号：入账为正，出账为负。
    pub amount: i32,
    /// 变动后的余额。
    pub balance: i32,
    pub description: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}

/// 今日某项主动经验的完成情况。
#[derive(Debug, Clone, Copy, Default, Serialize, ToSchema)]
pub struct ExpSource {
    pub done: bool,
    pub exp: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExpSources {
    pub check_in: ExpSource,
    pub create_post: ExpSource,
    pub create_comment: ExpSource,
    pub like_give: ExpSource,
    pub comment_accepted: ExpSource,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExpView {
    pub exp: i64,
    /// 0–7（0 = 未通过入站考试）。
    pub level: i16,
    /// 下一级所需的总经验；满级或未通过考试时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_level_exp: Option<i64>,
    /// 当前业务日（北京时间 04:00 切日）。
    pub today: NaiveDate,
    /// 今天主动行为已获得的经验（含被采纳，D-051 方案 A）。
    pub today_active_gained: i64,
    pub today_active_cap: i32,
    /// 今天被动互动（被投币等）已获得的经验。
    pub today_passive_gained: i64,
    pub today_passive_cap: i32,
    pub sources: ExpSources,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckInStatusView {
    pub can_check_in: bool,
    /// 当前签到日（北京时间 04:00 切日）。
    pub day: NaiveDate,
    pub total_days: i32,
    /// 已存的连签天数（断签后要到下次签到才重新计算）。
    pub streak: i32,
    /// 今天的排名；还没签到时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<i32>,
    /// 现在签到会得到的连签天数与经验。
    pub next_streak: i32,
    pub next_exp: i32,
    pub next_denny: i32,
    /// 下一次可以签到的时间（已签到时）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_eligible_at: Option<DateTime<Utc>>,
    pub balance: i32,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckInResult {
    pub day: NaiveDate,
    pub streak: i32,
    pub total_days: i32,
    /// 当日第几个签到。
    pub rank: i32,
    /// 实际获得的经验（今日主动经验额度用完时为 0，签到仍然成功）。
    pub exp_gained: i32,
    pub denny_gained: i32,
    pub exp: i64,
    pub level: i16,
    pub balance: i32,
    pub next_eligible_at: DateTime<Utc>,
}
