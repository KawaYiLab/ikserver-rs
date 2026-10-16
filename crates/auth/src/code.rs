//! 邮箱验证码：生成、哈希、校验。只存 HMAC，不存明文（CODE-004）。

use chrono::Datelike;
use hmac::{Hmac, KeyInit, Mac};
use ik_kernel::Problem;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use utoipa::ToSchema;

/// 有效期 10 分钟（CODE-003）。
pub const CODE_TTL_SECS: i64 = 600;
/// 同一邮箱同一用途的重发冷却（CODE-003）。
pub const SEND_COOLDOWN_SECS: i64 = 60;
/// 错误尝试上限（CODE-003）；与表上的 CHECK 约束一致。
pub const MAX_ATTEMPTS: i32 = 5;
/// 单个 IP 每小时最多发码次数，防止批量轰炸。
pub const MAX_CODES_PER_IP_PER_HOUR: i64 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CodePurpose {
    Register,
    Login,
    ResetPassword,
    /// 绑定 / 更换邮箱，只能经 `POST /v1/me/email/code` 申请。
    #[serde(skip)]
    BindEmail,
    /// 注销账号，只能经 `POST /v1/me/delete-account/code` 申请。
    #[serde(skip)]
    DeleteAccount,
}

impl CodePurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Login => "login",
            Self::ResetPassword => "reset_password",
            Self::BindEmail => "bind_email",
            Self::DeleteAccount => "delete_account",
        }
    }

    /// 邮件主题与正文里的用途名（CODE-015）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Register => "注册",
            Self::Login => "登录",
            Self::ResetPassword => "重置密码",
            Self::BindEmail => "绑定邮箱",
            Self::DeleteAccount => "注销账号",
        }
    }
}

/// 均匀分布的 6 位数字（允许前导零）。
pub fn generate_code() -> Result<String, Problem> {
    // 4_294_000_000 是不超过 u32::MAX 的最大的 1_000_000 的倍数，拒绝采样避免取模偏差。
    const LIMIT: u32 = 4_294_000_000;
    loop {
        let value = getrandom::u32().map_err(|e| Problem::internal_from(&e))?;
        if value < LIMIT {
            return Ok(format!("{:06}", value % 1_000_000));
        }
    }
}

/// HMAC-SHA256(key, "<purpose>:<email>:<code>")：换邮箱或换用途都会失效。
pub fn code_hash(
    key: &[u8],
    purpose: CodePurpose,
    email: &str,
    code: &str,
) -> Result<Vec<u8>, Problem> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|e| Problem::internal_from(&e))?;
    mac.update(purpose.as_str().as_bytes());
    mac.update(b":");
    mac.update(email.as_bytes());
    mac.update(b":");
    mac.update(code.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

pub fn hashes_match(a: &[u8], b: &[u8]) -> bool {
    a.ct_eq(b).into()
}

pub fn code_invalid() -> Problem {
    Problem::new(
        http::StatusCode::BAD_REQUEST,
        "CODE_INVALID",
        "验证码错误或已过期",
    )
}

pub fn code_too_many_attempts() -> Problem {
    Problem::new(
        http::StatusCode::TOO_MANY_REQUESTS,
        "CODE_TOO_MANY_ATTEMPTS",
        "验证码错误次数过多，请重新获取",
    )
}

pub struct MailContent {
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// HTML 版共用的深色卡片模板（CODE-016），`%NAME%` 占位符在 `html_body` 里替换。
const HTML_TEMPLATE: &str = include_str!("code_mail.html");

/// HTML 版各用途的标题、大标题、说明与页脚（CODE-016，逐字保留）。
struct HtmlCopy {
    title: &'static str,
    heading: &'static str,
    description: &'static str,
    footer: &'static str,
}

const fn html_copy(purpose: CodePurpose) -> HtmlCopy {
    match purpose {
        CodePurpose::Register => HtmlCopy {
            title: "验证码",
            heading: "注册验证码",
            description: "请使用以下验证码完成账号注册",
            footer: "如果你没有请求此验证码，请忽略此邮件。",
        },
        CodePurpose::ResetPassword => HtmlCopy {
            title: "重置密码",
            heading: "重置密码",
            description: "请使用以下验证码完成密码重置",
            footer: "如果你没有请求重置密码，请忽略此邮件，你的密码不会被更改。",
        },
        CodePurpose::BindEmail => HtmlCopy {
            title: "绑定邮箱",
            heading: "绑定邮箱",
            description: "请使用以下验证码完成邮箱绑定",
            footer: "如果你没有请求绑定邮箱，请忽略此邮件。",
        },
        CodePurpose::DeleteAccount => HtmlCopy {
            title: "注销账号",
            heading: "注销账号",
            description: "请使用以下验证码确认注销账号，此操作将不可撤回",
            footer: "如果你没有请求注销账号，请立即修改密码并忽略此邮件，你的账号不会被注销。",
        },
        CodePurpose::Login => HtmlCopy {
            title: "登录验证码",
            heading: "登录验证码",
            description: "请使用以下验证码登录你的账号",
            footer: "如果你没有尝试登录，请忽略此邮件，并考虑修改账号密码。",
        },
    }
}

/// 替换的值都是常量文案、6 位数字和年份，不含外部输入，不需要转义。
fn html_body(purpose: CodePurpose, code: &str, minutes: i64, year: i32) -> String {
    let copy = html_copy(purpose);
    HTML_TEMPLATE
        .replace("%TITLE%", copy.title)
        .replace("%HEADING%", copy.heading)
        .replace("%DESCRIPTION%", copy.description)
        .replace("%FOOTER%", copy.footer)
        .replace("%CODE%", code)
        .replace("%MINUTES%", &minutes.to_string())
        .replace("%YEAR%", &year.to_string())
}

pub fn mail_content(purpose: CodePurpose, code: &str) -> MailContent {
    let label = purpose.label();
    let minutes = CODE_TTL_SECS / 60;
    MailContent {
        subject: format!("【{code}】{label}验证码 - 绳网"),
        html: html_body(
            purpose,
            code,
            minutes,
            chrono::Utc::now()
                .with_timezone(&chrono_tz::Asia::Shanghai)
                .year(),
        ),
        text: match purpose {
            // 注销不可撤回，提示收到却不是本人操作时改密码（CODE-016）。
            CodePurpose::DeleteAccount => format!(
                "您的{label}验证码是 {code}，{minutes} 分钟内有效。此操作将不可撤回。\
                 如果你没有请求注销账号，请立即修改密码并忽略此邮件，你的账号不会被注销。"
            ),
            _ => format!("您的{label}验证码是 {code}，{minutes} 分钟内有效。如非本人操作请忽略。"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..1000 {
            let code = generate_code().unwrap();
            assert_eq!(code.len(), 6);
            assert!(code.bytes().all(|b| b.is_ascii_digit()));
        }
    }

    #[test]
    fn hash_binds_purpose_email_and_code() {
        let key = [7u8; 32];
        let base = code_hash(&key, CodePurpose::Register, "a@b.cn", "123456").unwrap();
        assert!(hashes_match(
            &base,
            &code_hash(&key, CodePurpose::Register, "a@b.cn", "123456").unwrap()
        ));
        assert!(!hashes_match(
            &base,
            &code_hash(&key, CodePurpose::Login, "a@b.cn", "123456").unwrap()
        ));
        assert!(!hashes_match(
            &base,
            &code_hash(&key, CodePurpose::Register, "c@b.cn", "123456").unwrap()
        ));
        assert!(!hashes_match(
            &base,
            &code_hash(&key, CodePurpose::Register, "a@b.cn", "123457").unwrap()
        ));
        assert!(!hashes_match(
            &base,
            &code_hash(&[8u8; 32], CodePurpose::Register, "a@b.cn", "123456").unwrap()
        ));
    }

    #[test]
    fn mail_subject_format() {
        let mail = mail_content(CodePurpose::Login, "042817");
        assert_eq!(mail.subject, "【042817】登录验证码 - 绳网");
        assert!(mail.text.contains("10 分钟内有效"));
    }

    #[test]
    fn html_mail_fills_every_placeholder() {
        for purpose in [
            CodePurpose::Register,
            CodePurpose::Login,
            CodePurpose::ResetPassword,
            CodePurpose::BindEmail,
            CodePurpose::DeleteAccount,
        ] {
            let html = html_body(purpose, "042137", 10, 2026);
            for name in [
                "TITLE",
                "HEADING",
                "DESCRIPTION",
                "FOOTER",
                "CODE",
                "MINUTES",
                "YEAR",
            ] {
                assert!(
                    !html.contains(&format!("%{name}%")),
                    "{purpose:?} 没替换 {name}"
                );
            }
            assert!(html.contains(">042137</span>"));
            assert!(html.contains(html_copy(purpose).footer));
            assert!(html.contains("&copy; 2026 绳网"));
            assert!(html.contains("（10 分钟内有效）"));
        }
        let content = mail_content(CodePurpose::Login, "042137");
        assert!(content.html.starts_with("<!DOCTYPE html>"));
    }
}
