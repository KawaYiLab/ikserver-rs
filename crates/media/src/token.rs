//! 完成令牌（UPL-012/013/050）：`base64url(JSON) "." base64url(HMAC-SHA256)`。
//!
//! 收据以令牌字符串的 SHA-256 为键（UPL-014），所以同一份载荷只能有一种合法写法：
//! 只接受恰好两段、无填充的 base64url，签名按常数时间比较。

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// 令牌载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadClaims {
    /// 过期时间（Unix 秒）。
    pub exp: i64,
    pub uid: i64,
    /// 暂存对象 key（`staging/` 下）。
    pub key: String,
    /// 签名时声明的大小（字节），完成时必须一致。
    pub size: u64,
    /// 客户端声明的 SHA-256（可选），完成时必须一致。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha: Option<String>,
    /// 客户端的文件名（只用于展示）。
    #[serde(default)]
    pub name: String,
}

#[derive(Clone)]
pub struct TokenKey([u8; 32]);

impl TokenKey {
    /// 从部署密钥派生，与其他用途的密钥隔离。
    pub fn derive(secret: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"ik-upload-token-v1\0");
        hasher.update(secret);
        Self(hasher.finalize().into())
    }

    fn mac(&self) -> Option<HmacSha256> {
        <HmacSha256 as KeyInit>::new_from_slice(&self.0).ok()
    }

    /// 签发令牌。只有在序列化失败这种不可能的情况下返回 None。
    pub fn sign(&self, claims: &UploadClaims) -> Option<String> {
        let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).ok()?);
        let mut mac = self.mac()?;
        mac.update(body.as_bytes());
        let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        Some(format!("{body}.{sig}"))
    }

    /// 校验格式与签名，返回载荷（不检查过期与归属）。
    pub fn verify(&self, token: &str) -> Option<UploadClaims> {
        let (body, sig) = token.split_once('.')?;
        let b64 = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };
        if !b64(body) || !b64(sig) {
            return None;
        }
        let sig_bytes = URL_SAFE_NO_PAD.decode(sig).ok()?;
        // 拒绝别名编码（末尾多余的位）：重新编码必须与原文一致。
        if URL_SAFE_NO_PAD.encode(&sig_bytes) != sig {
            return None;
        }
        let mut mac = self.mac()?;
        mac.update(body.as_bytes());
        let expected = mac.finalize().into_bytes();
        if expected.len() != sig_bytes.len() || !bool::from(expected.ct_eq(&sig_bytes)) {
            return None;
        }
        let json = URL_SAFE_NO_PAD.decode(body).ok()?;
        if URL_SAFE_NO_PAD.encode(&json) != body {
            return None;
        }
        serde_json::from_slice(&json).ok()
    }
}

/// 收据：令牌字符串的 SHA-256（小写十六进制）。
pub fn receipt(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// 客户端声明的 SHA-256：小写后须为 64 位十六进制，否则视为未提供（UPL-004）。
pub fn normalize_sha256(value: Option<&str>) -> Option<String> {
    let v = value?.trim().to_ascii_lowercase();
    (v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit())).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims() -> UploadClaims {
        UploadClaims {
            exp: 1_900_000_000,
            uid: 42,
            key: "staging/2026/10/abc.jpg".into(),
            size: 1234,
            sha: None,
            name: "猫.jpg".into(),
        }
    }

    #[test]
    fn roundtrip_and_tamper() {
        let key = TokenKey::derive(b"secret");
        let token = key.sign(&claims()).unwrap();
        assert_eq!(key.verify(&token), Some(claims()));
        assert!(
            TokenKey::derive(b"other").verify(&token).is_none(),
            "换密钥"
        );
        let (body, sig) = token.split_once('.').unwrap();
        assert!(key.verify(&format!("{body}.{sig}=")).is_none(), "填充");
        assert!(key.verify(&format!("{body}.{sig}.x")).is_none(), "三段");
        assert!(key.verify(&format!("{body}x.{sig}")).is_none(), "改载荷");
        assert!(key.verify("").is_none());
        // 别名编码：改签名最后一个字符的低位，解码结果可能相同但字符串不同。
        let mut alias = sig.to_owned();
        let last = alias.pop().unwrap();
        let swapped = match last {
            'A' => 'B',
            'B' => 'A',
            c => {
                if c == 'Q' {
                    'R'
                } else {
                    'Q'
                }
            }
        };
        alias.push(swapped);
        assert!(key.verify(&format!("{body}.{alias}")).is_none());
    }

    #[test]
    fn receipts_and_hashes() {
        assert_eq!(receipt("x").len(), 64);
        assert_ne!(receipt("x"), receipt("y"));
        let h = "AB".repeat(32);
        assert_eq!(normalize_sha256(Some(&h)).unwrap(), "ab".repeat(32));
        assert_eq!(normalize_sha256(Some("zz")), None);
        assert_eq!(normalize_sha256(None), None);
    }
}
