//! 会话令牌：`iks_` + 32 字节随机数的 base64url。数据库只存 SHA-256。
//!
//! 固定前缀便于密钥扫描工具识别泄露的令牌。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ik_kernel::Problem;
use sha2::{Digest, Sha256};

const PREFIX: &str = "iks_";
/// 前缀 + 32 字节 base64url（无填充为 43 字符）。
const TOKEN_LEN: usize = 4 + 43;

pub struct NewToken {
    pub token: String,
    pub hash: Vec<u8>,
}

pub fn generate() -> Result<NewToken, Problem> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| Problem::internal_from(&e))?;
    let token = format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
    let hash = hash(&token);
    Ok(NewToken { token, hash })
}

pub fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// 形状不对的令牌直接判无效，不必查库。
pub fn looks_valid(token: &str) -> bool {
    token.len() == TOKEN_LEN
        && token.starts_with(PREFIX)
        && token[PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_tokens_are_well_formed_and_unique() {
        let a = generate().unwrap();
        let b = generate().unwrap();
        assert!(looks_valid(&a.token));
        assert_ne!(a.token, b.token);
        assert_eq!(a.hash, hash(&a.token));
        assert_eq!(a.hash.len(), 32);
    }

    #[test]
    fn rejects_malformed_tokens() {
        assert!(!looks_valid(""));
        assert!(!looks_valid("iks_short"));
        assert!(!looks_valid(&format!("xxx_{}", "a".repeat(43))));
        assert!(!looks_valid(&format!("iks_{}", "a!".repeat(21) + "a")));
    }
}
