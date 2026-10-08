//! 匿名身份（POST-090/091/092）：服务端随机种子派生稳定的展示名，种子永不下发。

use ik_kernel::Problem;
use sha2::{Digest, Sha256};

const NAME_PREFIX: &str = "绳网用户#";

/// 16 字节随机数的小写 hex（32 字符），与旧系统同格式，迁移来的旧种子派生出相同的名字。
pub fn new_seed() -> Result<String, Problem> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| Problem::internal_from(&e))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// `绳网用户#` + sha256(种子) 的前 4 位大写 hex。对所有人（包括作者本人）都一样。
pub fn display_name(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let tag: String = digest.iter().take(2).map(|b| format!("{b:02X}")).collect();
    format!("{NAME_PREFIX}{tag}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_is_32_hex_and_unique() {
        let a = new_seed().unwrap();
        let b = new_seed().unwrap();
        assert_eq!(a.len(), 32);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(a, b);
    }

    #[test]
    fn name_is_deterministic() {
        let name = display_name("0123456789abcdef0123456789abcdef");
        assert_eq!(name, display_name("0123456789abcdef0123456789abcdef"));
        assert!(name.starts_with(NAME_PREFIX));
        let tag = name.trim_start_matches(NAME_PREFIX);
        assert_eq!(tag.len(), 4);
        assert!(
            tag.chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        );
    }
}
