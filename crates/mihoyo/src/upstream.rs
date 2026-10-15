//! 米游社上游协议（MHY-P1～P6）。只在后端调用，客户端永远接触不到米游社的 Cookie。
//!
//! 用 trait 抽象，测试换成假实现；真实实现见 [`HttpApi`]。

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const QR_APP_ID: &str = "bll8iq97cem8";
const QR_CLIENT_TYPE: &str = "4";
const ZZZ_GAME_BIZ: &str = "nap_cn";
/// 上游会话过期（MHY-P3）。
const RETCODE_EXPIRED: i64 = -3501;
/// 用户在手机上取消（MHY-P3）。
const RETCODE_CANCELLED: i64 = -3505;
pub const DEVICE_NAME: &str = "绳网论坛";
pub const FALLBACK_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 上游看到的「设备」。轮询时必须与创建二维码时一致，所以随扫码会话一起保存。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub device_id: String,
    pub model: String,
    pub os: String,
    pub user_agent: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrCode {
    /// 渲染成二维码的地址（里面含 ticket）。
    pub url: String,
    pub ticket: String,
    /// 从 `url` 的 `expire=<unix 秒>` 解析（MHY-003），解析不出为 None。
    pub expires_at: Option<DateTime<Utc>>,
}

/// 登录 Cookie。只在确认后的这一次请求链路里用，不落库、不打日志（MHY-P6）。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Cookies(pub Vec<(String, String)>);

impl std::fmt::Debug for Cookies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Cookies([{} REDACTED])", self.0.len())
    }
}

impl Cookies {
    fn header(&self) -> String {
        self.0
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirmed {
    pub aid: String,
    pub mid: Option<String>,
    pub cookies: Cookies,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrStatus {
    Created,
    Scanned,
    Confirmed(Confirmed),
    Expired,
    Cancelled,
}

/// 绝区零角色（MHY-P4）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct GameRole {
    pub game_uid: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub level: i32,
    #[serde(default)]
    pub region: String,
    #[serde(default)]
    pub region_name: String,
    #[serde(default)]
    pub is_chosen: bool,
}

/// 主角色：优先 `is_chosen`，否则等级最高的（MHY-P5）。
pub fn main_role(roles: Vec<GameRole>) -> Option<GameRole> {
    if let Some(chosen) = roles.iter().find(|r| r.is_chosen) {
        return Some(chosen.clone());
    }
    roles.into_iter().max_by_key(|r| r.level)
}

#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("请求米游社失败：{0}")]
    Http(#[from] reqwest::Error),
    #[error("米游社返回错误 {retcode}：{message}")]
    Retcode { retcode: i64, message: String },
    #[error("米游社返回了无法识别的数据：{0}")]
    Malformed(&'static str),
}

#[async_trait]
pub trait MihoyoApi: Send + Sync + 'static {
    async fn create_qr(&self, device: &Device) -> Result<QrCode, UpstreamError>;
    async fn query_qr(&self, ticket: &str, device: &Device) -> Result<QrStatus, UpstreamError>;
    async fn game_roles(&self, cookies: &Cookies) -> Result<Vec<GameRole>, UpstreamError>;
}

/// 真实实现。
pub struct HttpApi {
    client: reqwest::Client,
    passport: String,
    takumi: String,
}

impl HttpApi {
    pub fn new(passport_base: &str, takumi_base: &str, timeout: Duration) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .pool_max_idle_per_host(10)
            .build()?;
        Ok(Self {
            client,
            passport: passport_base.trim_end_matches('/').to_owned(),
            takumi: takumi_base.trim_end_matches('/').to_owned(),
        })
    }

    fn passport_post(&self, path: &str, device: &Device) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.passport))
            .header("x-rpc-app_id", QR_APP_ID)
            .header("x-rpc-device_id", &device.device_id)
            .header("x-rpc-client_type", QR_CLIENT_TYPE)
            .header("x-rpc-device_name", encode_component(DEVICE_NAME))
            .header("x-rpc-device_model", encode_component(&device.model))
            .header("x-rpc-device_os", encode_component(&device.os))
            .header("User-Agent", &device.user_agent)
            .header("Referer", "https://user.mihoyo.com/")
            .header("Origin", "https://user.mihoyo.com")
    }
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    retcode: i64,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

impl<T> Envelope<T> {
    fn ok(self) -> Result<Option<T>, UpstreamError> {
        if self.retcode == 0 {
            Ok(self.data)
        } else {
            Err(UpstreamError::Retcode {
                retcode: self.retcode,
                message: self.message,
            })
        }
    }
}

#[derive(Debug, Deserialize)]
struct CreateData {
    url: Option<String>,
    ticket: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StatusData {
    status: Option<String>,
    user_info: Option<UserInfo>,
}

#[derive(Debug, Deserialize)]
struct UserInfo {
    aid: Option<String>,
    mid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RolesData {
    list: Option<Vec<GameRole>>,
}

#[async_trait]
impl MihoyoApi for HttpApi {
    async fn create_qr(&self, device: &Device) -> Result<QrCode, UpstreamError> {
        let envelope: Envelope<CreateData> = self
            .passport_post("/account/ma-cn-passport/web/createQRLogin", device)
            .json(&serde_json::json!({}))
            .send()
            .await?
            .json()
            .await?;
        let data = envelope
            .ok()?
            .ok_or(UpstreamError::Malformed("createQRLogin 缺少 data"))?;
        let (Some(url), Some(ticket)) = (data.url, data.ticket) else {
            return Err(UpstreamError::Malformed("createQRLogin 缺少 url 或 ticket"));
        };
        Ok(QrCode {
            expires_at: expire_from_url(&url),
            url,
            ticket,
        })
    }

    async fn query_qr(&self, ticket: &str, device: &Device) -> Result<QrStatus, UpstreamError> {
        let response = self
            .passport_post("/account/ma-cn-passport/web/queryQRLoginStatus", device)
            .json(&serde_json::json!({ "ticket": ticket }))
            .send()
            .await?;
        let cookies = Cookies(
            response
                .headers()
                .get_all(http::header::SET_COOKIE)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .filter_map(parse_set_cookie)
                .collect(),
        );
        let envelope: Envelope<StatusData> = response.json().await?;
        match envelope.retcode {
            RETCODE_EXPIRED => return Ok(QrStatus::Expired),
            RETCODE_CANCELLED => return Ok(QrStatus::Cancelled),
            _ => {}
        }
        let data = envelope
            .ok()?
            .ok_or(UpstreamError::Malformed("queryQRLoginStatus 缺少 data"))?;
        match data.status.as_deref() {
            Some("Created") => Ok(QrStatus::Created),
            Some("Scanned") => Ok(QrStatus::Scanned),
            Some("Expired") => Ok(QrStatus::Expired),
            Some("Cancelled") => Ok(QrStatus::Cancelled),
            Some("Confirmed") => {
                let info = data
                    .user_info
                    .ok_or(UpstreamError::Malformed("确认结果缺少 user_info"))?;
                let aid = info
                    .aid
                    .map(|a| a.trim().to_owned())
                    .filter(|a| !a.is_empty())
                    .ok_or(UpstreamError::Malformed("确认结果缺少 aid"))?;
                Ok(QrStatus::Confirmed(Confirmed {
                    aid,
                    mid: info
                        .mid
                        .map(|m| m.trim().to_owned())
                        .filter(|m| !m.is_empty()),
                    cookies,
                }))
            }
            _ => Err(UpstreamError::Malformed("queryQRLoginStatus 状态未知")),
        }
    }

    async fn game_roles(&self, cookies: &Cookies) -> Result<Vec<GameRole>, UpstreamError> {
        let envelope: Envelope<RolesData> = self
            .client
            .get(format!(
                "{}/binding/api/getUserGameRolesByCookie?game_biz={ZZZ_GAME_BIZ}",
                self.takumi
            ))
            .header("Cookie", cookies.header())
            .header("User-Agent", FALLBACK_USER_AGENT)
            .header("Referer", "https://user.mihoyo.com/")
            .send()
            .await?
            .json()
            .await?;
        Ok(envelope.ok()?.and_then(|d| d.list).unwrap_or_default())
    }
}

/// `expire=<unix 秒>` 查询参数。
fn expire_from_url(url: &str) -> Option<DateTime<Utc>> {
    let query = url.split_once('?')?.1;
    let value = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("expire="))?;
    DateTime::from_timestamp(value.split('#').next()?.parse().ok()?, 0)
}

/// `Set-Cookie: name=value; Path=/; ...` 的 `name=value`。
fn parse_set_cookie(header: &str) -> Option<(String, String)> {
    let pair = header.split(';').next()?;
    let (name, value) = pair.split_once('=')?;
    let name = name.trim();
    (!name.is_empty()).then(|| (name.to_owned(), value.trim().to_owned()))
}

/// 等价于 JS 的 `encodeURIComponent`（请求头里的中文与空格）。
fn encode_component(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expire_cookies_and_encodes_headers() {
        assert_eq!(
            expire_from_url(
                "https://user.mihoyo.com/qr_code_in_game.html?app_id=x&ticket=t&expire=1760000000"
            ),
            DateTime::from_timestamp(1_760_000_000, 0)
        );
        assert_eq!(expire_from_url("https://x/?ticket=t"), None);
        assert_eq!(expire_from_url("https://x/?expire=soon"), None);
        assert_eq!(
            parse_set_cookie("ltoken_v2=v2_abc; Path=/; Domain=.mihoyo.com; HttpOnly"),
            Some(("ltoken_v2".to_owned(), "v2_abc".to_owned()))
        );
        assert_eq!(parse_set_cookie("=x"), None);
        assert_eq!(
            encode_component("绳网论坛"),
            "%E7%BB%B3%E7%BD%91%E8%AE%BA%E5%9D%9B"
        );
        assert_eq!(encode_component("Chrome 124"), "Chrome%20124");
        assert_eq!(
            format!("{:?}", Cookies(vec![("a".into(), "secret".into())])),
            "Cookies([1 REDACTED])"
        );
    }

    #[test]
    fn main_role_prefers_chosen_then_level() {
        let role = |uid: &str, level, chosen| GameRole {
            game_uid: uid.into(),
            nickname: String::new(),
            level,
            region: String::new(),
            region_name: String::new(),
            is_chosen: chosen,
        };
        assert_eq!(
            main_role(vec![role("a", 10, false), role("b", 50, false)])
                .unwrap()
                .game_uid,
            "b"
        );
        assert_eq!(
            main_role(vec![role("a", 10, true), role("b", 50, false)])
                .unwrap()
                .game_uid,
            "a"
        );
        assert!(main_role(Vec::new()).is_none());
    }
}
