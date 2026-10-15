use chrono::{DateTime, Utc};
use ik_auth::dto::{AuthResponse, ClientKind};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 扫码的用途（MHY-004）。必须显式给出。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QrMode {
    /// 扫码登录（没有账号时自动建号）。带着登录态也按登录处理。
    Login,
    /// 给当前账号绑定米游社，需要登录。
    Bind,
    /// 注销前核验（没有邮箱的账号）：只核对扫码的米游社账号是不是当前账号绑定的那个，需要登录。
    Delete,
}

impl QrMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::Bind => "bind",
            Self::Delete => "delete",
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateQrRequest {
    pub mode: QrMode,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QrCreated {
    /// 轮询用的凭证。只给发起方；二维码里的地址不含它。
    pub qr_id: String,
    /// 渲染成二维码的地址。
    pub url: String,
    /// 二维码几秒后过期（取上游的真实值，解析不出时为 180，MHY-003）。
    pub expires_in: i64,
    pub mode: QrMode,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PollQrRequest {
    pub qr_id: String,
    /// 登录成功时令牌怎么交付：`web` 写 Cookie，`app` 放进响应体。
    #[serde(default)]
    pub client: ClientKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QrStatus {
    /// 等待扫码。
    Waiting,
    /// 已扫码，等待在手机上确认。
    Scanned,
    /// 二维码过期（或已经处理完、结果也过期了），重新生成。
    Expired,
    /// 用户在手机上取消。
    Cancelled,
    /// 已确认，见 `auth` / `binding`。
    Confirmed,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QrPollResponse {
    pub status: QrStatus,
    /// 扫码会话不存在（过期已清理）时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<QrMode>,
    /// 登录模式：这次扫码新建了账号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_new_user: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<BindingView>,
    /// 注销核验模式：扫码的米游社账号是否就是当前账号绑定的那个。为 true 时可以拿 `qrId` 注销账号（5 分钟内）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<bool>,
    /// 登录模式确认后的登录结果（与密码登录的响应相同）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthResponse>,
}

/// 绝区零主角色。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZzzRole {
    pub uid: String,
    pub nickname: String,
    pub level: i32,
    pub region: String,
    pub region_name: String,
}

/// 自己的米游社绑定（MHY-001 公开视图；mid 不外发）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BindingView {
    /// 米游社账号 id。
    pub aid: String,
    /// 拉取失败或没有角色时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zzz: Option<ZzzRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_synced_at: Option<DateTime<Utc>>,
    pub bound_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MyBinding {
    /// 没有绑定时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binding: Option<BindingView>,
}

/// 作者主页上展示的绝区零角色（PROFILE-017）。不含米游社账号 id。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ZzzProfile {
    pub uid: String,
    pub nickname: String,
    pub level: i32,
    pub region_name: String,
}
