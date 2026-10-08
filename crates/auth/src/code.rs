//! 邮箱验证码：生成、哈希、校验。只存 HMAC，不存明文（CODE-004）。

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
}

impl CodePurpose {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Login => "login",
            Self::ResetPassword => "reset_password",
        }
    }

    /// 邮件主题与正文里的用途名（CODE-015）。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Register => "注册",
            Self::Login => "登录",
            Self::ResetPassword => "重置密码",
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
}

pub fn mail_content(purpose: CodePurpose, code: &str) -> MailContent {
    let label = purpose.label();
    let minutes = CODE_TTL_SECS / 60;
    MailContent {
        subject: format!("【{code}】{label}验证码 - 绳网"),
        text: format!("您的{label}验证码是 {code}，{minutes} 分钟内有效。如非本人操作请忽略。"),
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
}
