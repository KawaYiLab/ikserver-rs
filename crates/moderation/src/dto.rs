use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::User;
use ik_media::dto::MediaView;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// 被举报的对象。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetType {
    Post,
    Comment,
    User,
}

impl TargetType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Post => "post",
            Self::Comment => "comment",
            Self::User => "user",
        }
    }

    pub(crate) fn parse(raw: &str) -> Self {
        match raw {
            "post" => Self::Post,
            "comment" => Self::Comment,
            _ => Self::User,
        }
    }
}

/// 举报理由（RPT-002）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// 垃圾广告 / 刷屏。
    Spam,
    /// 辱骂攻击 / 引战。
    Abuse,
    /// 色情低俗。
    Porn,
    /// 违法违规。
    Illegal,
    /// 泄露隐私。
    Privacy,
    /// 不实信息。
    Misinfo,
    /// 抄袭搬运。
    Plagiarism,
    /// 其他（必须填写说明）。
    Other,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spam => "spam",
            Self::Abuse => "abuse",
            Self::Porn => "porn",
            Self::Illegal => "illegal",
            Self::Privacy => "privacy",
            Self::Misinfo => "misinfo",
            Self::Plagiarism => "plagiarism",
            Self::Other => "other",
        }
    }

    pub(crate) fn parse(raw: &str) -> Self {
        match raw {
            "spam" => Self::Spam,
            "abuse" => Self::Abuse,
            "porn" => Self::Porn,
            "illegal" => Self::Illegal,
            "privacy" => Self::Privacy,
            "misinfo" => Self::Misinfo,
            "plagiarism" => Self::Plagiarism,
            _ => Self::Other,
        }
    }
}

/// 举报状态（RPT-003）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    /// 待处理。
    Pending,
    /// 复核中（已被领取，或内容已被自动隐藏）。
    Reviewing,
    /// 成立。
    Accepted,
    /// 驳回。
    Rejected,
}

impl ReportStatus {
    pub(crate) fn parse(raw: &str) -> Self {
        match raw {
            "reviewing" => Self::Reviewing,
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            _ => Self::Pending,
        }
    }
}

/// 处理结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    None,
    ContentHidden,
    ContentDeleted,
    UserWarned,
    UserBlocked,
}

impl Resolution {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ContentHidden => "content_hidden",
            Self::ContentDeleted => "content_deleted",
            Self::UserWarned => "user_warned",
            Self::UserBlocked => "user_blocked",
        }
    }

    pub(crate) fn parse(raw: &str) -> Self {
        match raw {
            "content_hidden" => Self::ContentHidden,
            "content_deleted" => Self::ContentDeleted,
            "user_warned" => Self::UserWarned,
            "user_blocked" => Self::UserBlocked,
            _ => Self::None,
        }
    }
}

// ---------- 用户侧 ----------

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateReportRequest {
    pub target_type: TargetType,
    #[schema(value_type = String)]
    pub target_id: Id,
    pub reason: Reason,
    /// 补充说明，最多 500 字；理由为 `other` 时必填。
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportCreated {
    #[schema(value_type = String)]
    pub id: Id,
    pub target_type: TargetType,
    #[schema(value_type = String)]
    pub target_id: Id,
    pub reason: Reason,
    pub status: ReportStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct CheckQuery {
    #[param(inline)]
    pub target_type: TargetType,
    /// 逗号分隔的目标 id，最多 100 个。
    pub ids: String,
}

/// 我对这些目标是否有正在处理中的举报（RPT-031）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    /// 目标 id → 是否已举报（处理中）。
    pub reported: std::collections::BTreeMap<String, bool>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageQuery {
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}

/// 举报时的目标预览（取自快照）。
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetPreview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
}

/// 我的举报（RPT-032）。不含处理人与处理备注（举报人从系统通知里看到备注，RPT-060）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MyReport {
    #[schema(value_type = String)]
    pub id: Id,
    pub target_type: TargetType,
    #[schema(value_type = String)]
    pub target_id: Id,
    pub reason: Reason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub status: ReportStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handled_at: Option<DateTime<Utc>>,
    pub target: TargetPreview,
}

// ---------- 审核侧 ----------

/// 后台列表的分页签（RPTADM-010）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    #[default]
    Pending,
    Reviewing,
    /// 已结案（成立与驳回）。
    Closed,
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(rename_all = "camelCase")]
pub struct GroupQuery {
    #[serde(default)]
    #[param(inline)]
    pub tab: Tab,
    #[param(inline)]
    pub target_type: Option<TargetType>,
    #[param(inline)]
    pub reason: Option<Reason>,
    /// 举报时间下界（含）。
    pub from: Option<DateTime<Utc>>,
    /// 举报时间上界（不含）。
    pub to: Option<DateTime<Utc>>,
    /// 只看我领取的（只在 `reviewing` 生效）。
    #[serde(default)]
    pub mine: bool,
    /// 关键词：目标标题、摘要、被举报人用户名，或举报人用户名。
    pub q: Option<String>,
    /// 页码，从 1 开始。
    pub page: Option<u32>,
    /// 每页目标数，默认 20，最大 100。
    pub limit: Option<u32>,
}

/// 后台展示的账号。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserRef {
    #[schema(value_type = String)]
    pub id: Id<User>,
    pub username: String,
    /// active / blocked / deleted / merged。
    pub status: String,
}

/// 被举报目标的现状与快照。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TargetView {
    /// 目标还在（帖子、评论没被删除；用户没有注销）。
    pub exists: bool,
    /// 内容当前被隐藏。
    pub hidden: bool,
    /// 内容是匿名发布的。匿名内容不显示作者（AGENTS.md 规则 10），但可以照常处理与封禁。
    pub anonymous: bool,
    /// 内容作者或被举报的用户；匿名内容省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<UserRef>,
    /// 作者账号当前被封禁（匿名内容也会给出，不暴露是谁）。
    pub owner_blocked: bool,
    #[serde(flatten)]
    pub preview: TargetPreview,
    /// 评论所在的帖子。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub post_id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_title: Option<String>,
    /// 快照里的图片（帖子封面或评论图片，最多 9 张）。
    pub images: Vec<MediaView>,
}

/// 一条举报。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportRow {
    #[schema(value_type = String)]
    pub id: Id,
    pub reporter: Option<UserRef>,
    pub reason: Reason,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub status: ReportStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_resolution_note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_to: Option<UserRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handled_by: Option<UserRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// 同一目标的举报（RPTADM-017）。结案作用于整组。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportGroup {
    pub target_type: TargetType,
    #[schema(value_type = String)]
    pub target_id: Id,
    pub report_count: i64,
    pub first_report_at: DateTime<Utc>,
    pub latest_report_at: DateTime<Utc>,
    /// 组状态：有待处理的为 pending，否则有复核中的为 reviewing，否则取最新一条。
    pub status: ReportStatus,
    /// 领取人（组内最新的一个）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_to: Option<UserRef>,
    pub target: TargetView,
    /// 组内的举报，新的在前。
    pub reports: Vec<ReportRow>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupPage {
    pub items: Vec<ReportGroup>,
    /// 符合条件的目标总数。
    pub total: i64,
    pub page: u32,
    pub limit: u32,
}

/// 结案动作（RPTADM-050）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// 驳回。内容如果是被自动隐藏的，恢复可见。
    Reject,
    /// 隐藏内容。
    HideContent,
    /// 删除内容（评论连同回复）。
    DeleteContent,
    /// 警告作者（只发通知）。
    WarnUser,
    /// 封禁作者或被举报的用户（立即下线）。
    BlockUser,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::HideContent => "hide_content",
            Self::DeleteContent => "delete_content",
            Self::WarnUser => "warn_user",
            Self::BlockUser => "block_user",
        }
    }

    pub(crate) fn resolution(self) -> Resolution {
        match self {
            Self::Reject => Resolution::None,
            Self::HideContent => Resolution::ContentHidden,
            Self::DeleteContent => Resolution::ContentDeleted,
            Self::WarnUser => Resolution::UserWarned,
            Self::BlockUser => Resolution::UserBlocked,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveRequest {
    pub action: Action,
    /// 给举报人的备注（超过 500 字截断）。
    #[serde(default)]
    pub note: Option<String>,
    /// 给被举报人的备注（超过 500 字截断）。
    #[serde(default)]
    pub target_note: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResolveResult {
    pub action: Action,
    /// 一并结案的举报条数。
    pub resolved_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ClaimResult {
    /// 这次新领取的条数（重复领取自己的为 0）。
    pub claimed_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnclaimResult {
    pub released_count: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlockResult {
    /// 这次由正常改为封禁（原本就封禁时为 false）。
    pub blocked: bool,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlockRequest {
    /// 处理备注（只进审核日志，超过 500 字截断）。
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnblockResult {
    /// 原本是封禁状态、现在已经解封。
    pub unblocked: bool,
}
