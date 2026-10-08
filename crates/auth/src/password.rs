//! 密码哈希：新密码一律 Argon2id；迁移来的 bcrypt 哈希可校验，并在登录成功后透明升级（ACC-012）。
//!
//! 哈希计算是 CPU 密集型，放到阻塞线程池执行，避免卡住异步运行时。

use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use ik_kernel::Problem;

pub struct Verification {
    pub ok: bool,
    /// 校验成功但哈希算法过时（bcrypt），应重哈希后保存。
    pub needs_rehash: bool,
}

pub async fn hash(password: String) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || hash_blocking(&password))
        .await
        .map_err(|e| Problem::internal_from(&e))?
}

fn hash_blocking(password: &str) -> Result<String, Problem> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|e| Problem::internal_from(&e))
}

pub async fn verify(password: String, stored: String) -> Result<Verification, Problem> {
    tokio::task::spawn_blocking(move || Ok(verify_blocking(&password, &stored)))
        .await
        .map_err(|e| Problem::internal_from(&e))?
}

fn verify_blocking(password: &str, stored: &str) -> Verification {
    if is_bcrypt(stored) {
        let ok = bcrypt::verify(password, stored).unwrap_or(false);
        return Verification {
            ok,
            needs_rehash: ok,
        };
    }
    let ok = PasswordHash::new(stored)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false);
    Verification {
        ok,
        needs_rehash: false,
    }
}

fn is_bcrypt(stored: &str) -> bool {
    stored.starts_with("$2a$")
        || stored.starts_with("$2b$")
        || stored.starts_with("$2x$")
        || stored.starts_with("$2y$")
}

/// 账号不存在时也做一次同等代价的校验，让响应时间不暴露账号是否存在。
pub async fn verify_dummy(password: String) {
    static DUMMY: OnceLock<Option<String>> = OnceLock::new();
    let dummy = DUMMY
        .get_or_init(|| hash_blocking("ik-dummy-password").ok())
        .clone();
    if let Some(dummy) = dummy {
        let _ = verify(password, dummy).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn argon2_roundtrip() {
        let stored = hash("correct horse".to_owned()).await.unwrap();
        assert!(stored.starts_with("$argon2id$"));
        let good = verify("correct horse".to_owned(), stored.clone())
            .await
            .unwrap();
        assert!(good.ok && !good.needs_rehash);
        let bad = verify("wrong".to_owned(), stored).await.unwrap();
        assert!(!bad.ok);
    }

    #[tokio::test]
    async fn legacy_bcrypt_verifies_and_requests_rehash() {
        let legacy = bcrypt::hash("hunter22", 4).unwrap();
        let good = verify("hunter22".to_owned(), legacy.clone()).await.unwrap();
        assert!(good.ok && good.needs_rehash);
        let bad = verify("nope".to_owned(), legacy).await.unwrap();
        assert!(!bad.ok && !bad.needs_rehash);
    }

    #[tokio::test]
    async fn garbage_hash_never_verifies() {
        let result = verify("x".to_owned(), "not-a-hash".to_owned())
            .await
            .unwrap();
        assert!(!result.ok);
    }
}
