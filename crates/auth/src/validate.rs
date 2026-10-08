//! 输入校验。错误一律以字段级 Problem 返回，文案面向用户。

use ik_kernel::Problem;

pub const PASSWORD_MIN_CHARS: usize = 6;
pub const PASSWORD_MAX_CHARS: usize = 128;
const EMAIL_MIN_LEN: usize = 6;
const EMAIL_MAX_LEN: usize = 254;

/// trim + 小写（ACC-005），并按 `^[^\s@]+@[^\s@]+\.[^\s@]+$` 校验（ACC-006）。
pub fn normalize_email(raw: &str) -> Result<String, Problem> {
    let email = raw.trim().to_lowercase();
    if is_valid_email(&email) {
        Ok(email)
    } else {
        Err(Problem::field("email", "INVALID", "邮箱格式不正确"))
    }
}

fn is_valid_email(email: &str) -> bool {
    if email.len() < EMAIL_MIN_LEN || email.len() > EMAIL_MAX_LEN {
        return false;
    }
    if email.chars().any(char::is_whitespace) {
        return false;
    }
    let mut parts = email.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if local.is_empty() {
        return false;
    }
    // 域名里至少有一个点，且点的两侧都有字符。
    domain
        .rsplit_once('.')
        .is_some_and(|(head, tail)| !head.is_empty() && !tail.is_empty())
}

pub fn validate_password(field: &str, password: &str) -> Result<(), Problem> {
    let chars = password.chars().count();
    if chars < PASSWORD_MIN_CHARS {
        return Err(Problem::field(field, "TOO_SHORT", "密码长度不能少于 6 位"));
    }
    if chars > PASSWORD_MAX_CHARS {
        return Err(Problem::field(field, "TOO_LONG", "密码长度不能超过 128 位"));
    }
    Ok(())
}

/// 验证码必须是 6 位数字（CODE-002）。
pub fn validate_code_format(code: &str) -> Result<(), Problem> {
    if code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err(Problem::field(
            "code",
            "INVALID_FORMAT",
            "验证码必须是 6 位数字",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_validates_email() {
        assert_eq!(
            normalize_email("  Alice@Example.COM ").unwrap(),
            "alice@example.com"
        );
        for bad in [
            "",
            "a@b.c",
            "no-at.example.com",
            "two@@example.com",
            "a b@x.com",
            "a@x",
            "a@.com",
            "a@x.",
        ] {
            assert!(normalize_email(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn password_length_rules() {
        assert!(validate_password("password", "12345").is_err());
        assert!(validate_password("password", "123456").is_ok());
        assert!(validate_password("password", "密码密码密码").is_ok());
        assert!(validate_password("password", &"x".repeat(129)).is_err());
    }

    #[test]
    fn code_must_be_six_digits() {
        assert!(validate_code_format("012345").is_ok());
        for bad in ["12345", "1234567", "12a456", "１２３４５６"] {
            assert!(validate_code_format(bad).is_err(), "{bad}");
        }
    }
}
