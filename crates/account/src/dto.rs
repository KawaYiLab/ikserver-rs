use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// 注销时用什么核验身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeleteMethod {
    /// 发到绑定邮箱的验证码。
    EmailCode,
    /// 用绑定的米游社账号扫码（`mode = delete`），没有邮箱的账号用这个。
    MihoyoQr,
}

/// 账号安全概览（DEL-001）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SecurityView {
    /// 绑定的邮箱；没有时省略（米游社扫码注册的账号）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// 设置过密码（可以用密码登录）。
    pub has_password: bool,
    pub mihoyo_bound: bool,
    pub delete_method: DeleteMethod,
}

/// 注销账号。有邮箱的账号给 `code`（注销验证码），没有邮箱的给 `qrId`（扫码核验通过的二维码）。
#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteAccountRequest {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub qr_id: Option<String>,
}
