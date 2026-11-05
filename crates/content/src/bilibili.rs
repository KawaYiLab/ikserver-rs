//! B 站视频信息：标题、封面、时长、分 P。发帖编辑器预览与保存帖子时用。
//!
//! 上游用 trait 抽象，测试换成假实现；真实实现见 [`HttpBilibili`]。查询结果在进程内缓存 10 分钟
//! （同一个视频的并发查询合并成一次），编辑器预览之后紧接着保存帖子不会再访问上游。
//! 调用都在数据库事务之外。
//!
//! B 站的风控会拦下机房 IP 上不带 `buvid3` Cookie 的请求（HTTP 412 + HTML 页面），
//! 所以真实实现先向 `finger/spi` 要一对 buvid，之后的请求都带上；被拦时换一对重试一次。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use moka::future::Cache;
use reqwest::StatusCode;
use reqwest::header::{COOKIE, HeaderValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use utoipa::ToSchema;

const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_CAPACITY: u64 = 10_000;
const MAX_PAGES: usize = 200;
/// 这些返回码表示视频不存在、被删除或不可见（不是上游故障）。
const GONE_CODES: [i64; 5] = [-404, -403, -400, 62002, 62004];
/// 返回码 -412：请求被风控拦截，和 HTTP 412 一样处理。
const BLOCKED_CODE: i64 = -412;
const MAX_BUVID_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VideoKey {
    Bvid(String),
    Aid(u64),
}

/// 分 P。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct VideoPage {
    pub cid: u64,
    /// 从 1 开始。
    pub page: u32,
    /// 分 P 标题。
    pub part: String,
    /// 秒。
    pub duration: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoMeta {
    pub bvid: String,
    pub aid: u64,
    pub title: String,
    /// 上游给的封面原始地址（可能是 http），由 [`crate::video`] 校验。
    pub cover_url: Option<String>,
    /// 全部分 P 的总时长（秒）。
    pub duration: u32,
    /// 第一个分 P 的 cid。
    pub cid: Option<u64>,
    pub owner: Option<String>,
    pub pages: Vec<VideoPage>,
}

#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    #[error("请求 B 站失败：{0}")]
    Http(#[from] reqwest::Error),
    #[error("B 站风控拦截了请求（412）")]
    Blocked,
    #[error("B 站返回 HTTP {0}")]
    Status(StatusCode),
    #[error("B 站返回的不是预期的 JSON：{0}")]
    Json(#[from] serde_json::Error),
    #[error("B 站返回错误 {code}：{message}")]
    Code { code: i64, message: String },
    #[error("B 站返回了无法识别的数据：{0}")]
    Malformed(&'static str),
    #[error("没有配置 B 站接口")]
    Disabled,
}

#[async_trait]
pub trait VideoApi: Send + Sync + 'static {
    /// 视频不存在、被删除或不可见时返回 `Ok(None)`。
    async fn bilibili(&self, key: &VideoKey) -> Result<Option<VideoMeta>, UpstreamError>;
}

/// 不访问网络的默认实现（测试与 CLI）：一律视为上游不可用，保存帖子时照常保存、只是没有封面。
pub struct Disabled;

#[async_trait]
impl VideoApi for Disabled {
    async fn bilibili(&self, _key: &VideoKey) -> Result<Option<VideoMeta>, UpstreamError> {
        Err(UpstreamError::Disabled)
    }
}

/// 带缓存的查询入口。
#[derive(Clone)]
pub(crate) struct Videos {
    api: Arc<dyn VideoApi>,
    cache: Cache<VideoKey, Option<Arc<VideoMeta>>>,
}

impl Videos {
    pub fn new(api: Arc<dyn VideoApi>) -> Self {
        Self {
            api,
            cache: Cache::builder()
                .max_capacity(CACHE_CAPACITY)
                .time_to_live(CACHE_TTL)
                .build(),
        }
    }

    /// 查询（命中缓存时不访问上游；「不存在」也缓存）。失败不缓存，下次重试。
    pub async fn get(&self, key: &VideoKey) -> Result<Option<Arc<VideoMeta>>, Arc<UpstreamError>> {
        let api = self.api.clone();
        let owned = key.clone();
        self.cache
            .try_get_with(key.clone(), async move {
                api.bilibili(&owned).await.map(|meta| meta.map(Arc::new))
            })
            .await
    }
}

/// 真实实现：`GET {base}/x/web-interface/view?bvid=|aid=`，带上 `GET {base}/x/frontend/finger/spi`
/// 给的 buvid Cookie（进程内共用一对，被风控拦截时换新的）。
pub struct HttpBilibili {
    client: reqwest::Client,
    base: String,
    /// 当前使用的 `buvid3=…; buvid4=…`。还没取过或刚被拦截时为空。
    cookie: Mutex<Option<HeaderValue>>,
}

impl HttpBilibili {
    pub fn new(base: &str, timeout: Duration) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            )
            .pool_max_idle_per_host(4)
            .build()?;
        Ok(Self {
            client,
            base: base.trim_end_matches('/').to_owned(),
            cookie: Mutex::new(None),
        })
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{path}", self.base))
            .header("Referer", "https://www.bilibili.com/")
    }

    /// 当前的 buvid Cookie，没有就向 `finger/spi` 要一对（并发的请求只要一次）。
    async fn cookie(&self) -> Result<HeaderValue, UpstreamError> {
        let mut slot = self.cookie.lock().await;
        if let Some(cookie) = slot.as_ref() {
            return Ok(cookie.clone());
        }
        let cookie = self.fetch_buvid().await?;
        *slot = Some(cookie.clone());
        Ok(cookie)
    }

    /// 这对 buvid 被拦截了：丢掉，下次重新要。别的请求已经换过新的就不动。
    async fn discard(&self, used: &HeaderValue) {
        let mut slot = self.cookie.lock().await;
        if slot.as_ref() == Some(used) {
            *slot = None;
        }
    }

    async fn fetch_buvid(&self) -> Result<HeaderValue, UpstreamError> {
        let envelope: Envelope<SpiData> =
            decode(self.get("/x/frontend/finger/spi").send().await?).await?;
        let data = envelope
            .ok()?
            .ok_or(UpstreamError::Malformed("finger/spi 缺少 data"))?;
        let b3 = data
            .b_3
            .filter(|v| valid_buvid(v))
            .ok_or(UpstreamError::Malformed("finger/spi 缺少 b_3"))?;
        let cookie = match data.b_4.filter(|v| valid_buvid(v)) {
            Some(b4) => format!("buvid3={b3}; buvid4={b4}"),
            None => format!("buvid3={b3}"),
        };
        HeaderValue::from_str(&cookie)
            .map_err(|_| UpstreamError::Malformed("finger/spi 的 buvid 无法放进 Cookie"))
    }

    /// 查一次 view。被拦截时返回 [`UpstreamError::Blocked`]，由调用方决定换 buvid 重试。
    async fn view(
        &self,
        key: &VideoKey,
        cookie: &HeaderValue,
    ) -> Result<Option<ViewData>, UpstreamError> {
        let request = self
            .get("/x/web-interface/view")
            .header(COOKIE, cookie.clone());
        let request = match key {
            VideoKey::Bvid(bvid) => request.query(&[("bvid", bvid)]),
            VideoKey::Aid(aid) => request.query(&[("aid", aid)]),
        };
        let envelope: Envelope<ViewData> = decode(request.send().await?).await?;
        if GONE_CODES.contains(&envelope.code) {
            return Ok(None);
        }
        Ok(Some(
            envelope
                .ok()?
                .ok_or(UpstreamError::Malformed("缺少 data"))?,
        ))
    }
}

/// buvid 只接受 B 站实际会给的字符，免得上游的怪值改写 Cookie 头。
fn valid_buvid(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_BUVID_LEN
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'=' | b'+' | b'/'))
}

/// 先看状态码再解析 JSON：风控拦截时 B 站回的是 HTTP 412 和一个 HTML 页面。
async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, UpstreamError> {
    let status = response.status();
    if status == StatusCode::PRECONDITION_FAILED {
        return Err(UpstreamError::Blocked);
    }
    if !status.is_success() {
        return Err(UpstreamError::Status(status));
    }
    Ok(serde_json::from_slice(&response.bytes().await?)?)
}

#[derive(Deserialize)]
struct Envelope<T> {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

impl<T> Envelope<T> {
    fn ok(self) -> Result<Option<T>, UpstreamError> {
        match self.code {
            0 => Ok(self.data),
            BLOCKED_CODE => Err(UpstreamError::Blocked),
            code => Err(UpstreamError::Code {
                code,
                message: self.message,
            }),
        }
    }
}

#[derive(Deserialize)]
struct SpiData {
    b_3: Option<String>,
    b_4: Option<String>,
}

#[derive(Deserialize)]
struct ViewData {
    bvid: Option<String>,
    aid: Option<u64>,
    title: Option<String>,
    pic: Option<String>,
    duration: Option<u32>,
    cid: Option<u64>,
    #[serde(default)]
    pages: Vec<PageData>,
    owner: Option<OwnerData>,
}

#[derive(Deserialize)]
struct PageData {
    cid: u64,
    page: u32,
    #[serde(default)]
    part: String,
    #[serde(default)]
    duration: u32,
}

#[derive(Deserialize)]
struct OwnerData {
    name: Option<String>,
}

#[async_trait]
impl VideoApi for HttpBilibili {
    async fn bilibili(&self, key: &VideoKey) -> Result<Option<VideoMeta>, UpstreamError> {
        let cookie = self.cookie().await?;
        let data = match self.view(key, &cookie).await {
            Err(UpstreamError::Blocked) => {
                // buvid 可能失效了：换一对再试一次，还被拦就报错。
                self.discard(&cookie).await;
                let cookie = self.cookie().await?;
                self.view(key, &cookie).await?
            }
            other => other?,
        };
        let Some(data) = data else {
            return Ok(None);
        };
        let (Some(bvid), Some(aid)) = (data.bvid, data.aid) else {
            return Err(UpstreamError::Malformed("缺少 bvid 或 aid"));
        };
        Ok(Some(VideoMeta {
            bvid,
            aid,
            title: data.title.unwrap_or_default(),
            cover_url: data.pic,
            duration: data.duration.unwrap_or_default(),
            cid: data.cid,
            owner: data.owner.and_then(|o| o.name),
            pages: data
                .pages
                .into_iter()
                .take(MAX_PAGES)
                .map(|p| VideoPage {
                    cid: p.cid,
                    page: p.page,
                    part: p.part.chars().take(200).collect(),
                    duration: p.duration,
                })
                .collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::{Query, State};
    use axum::http::HeaderMap;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;

    use super::*;

    struct Counting(AtomicUsize);

    #[async_trait]
    impl VideoApi for Counting {
        async fn bilibili(&self, key: &VideoKey) -> Result<Option<VideoMeta>, UpstreamError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            match key {
                VideoKey::Aid(1) => Ok(None),
                VideoKey::Aid(2) => Err(UpstreamError::Disabled),
                _ => Ok(Some(VideoMeta {
                    bvid: "BV1xx411c7mD".into(),
                    aid: 3,
                    title: "t".into(),
                    cover_url: None,
                    duration: 1,
                    cid: None,
                    owner: None,
                    pages: Vec::new(),
                })),
            }
        }
    }

    /// 本地桩服务器，按 B 站的真实行为：`finger/spi` 发 buvid；view 不带当前有效的 buvid3 时
    /// 回 HTTP 412 和 HTML 风控页。
    #[derive(Default)]
    struct Stub {
        /// 发出去的 buvid 对数。
        issued: AtomicUsize,
        /// 当前有效的 buvid3。设成 `None` 模拟 buvid 失效。
        valid: std::sync::Mutex<Option<String>>,
    }

    async fn stub() -> (String, Arc<Stub>) {
        let state = Arc::new(Stub::default());
        let app = axum::Router::new()
            .route("/x/frontend/finger/spi", get(stub_spi))
            .route("/x/web-interface/view", get(stub_view))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), state)
    }

    async fn stub_spi(State(stub): State<Arc<Stub>>) -> axum::Json<serde_json::Value> {
        let n = stub.issued.fetch_add(1, Ordering::SeqCst) + 1;
        let b3 = format!("TEST-{n}infoc");
        *stub.valid.lock().unwrap() = Some(b3.clone());
        axum::Json(serde_json::json!({
            "code": 0, "message": "ok",
            "data": { "b_3": b3, "b_4": format!("TEST-{n}-b4==") }
        }))
    }

    async fn stub_view(
        State(stub): State<Arc<Stub>>,
        headers: HeaderMap,
        Query(q): Query<HashMap<String, String>>,
    ) -> Response {
        let valid = stub.valid.lock().unwrap().clone();
        let cookie = headers
            .get(COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let has_buvid = valid.is_some_and(|b3| {
            cookie
                .split("; ")
                .any(|c| c.strip_prefix("buvid3=") == Some(b3.as_str()))
        });
        let aid = q.get("aid").map(String::as_str);
        if !has_buvid || aid == Some("412") {
            let page =
                "<!DOCTYPE html><html><title>出错啦! - bilibili.com</title>错误号: 412</html>";
            return (
                StatusCode::PRECONDITION_FAILED,
                [("content-type", "text/html")],
                page,
            )
                .into_response();
        }
        let body = match (q.get("bvid").map(String::as_str), aid) {
            (Some("BV1xx411c7mD"), _) => serde_json::json!({
                "code": 0, "message": "0",
                "data": {
                    "bvid": "BV1xx411c7mD", "aid": 170001, "title": "测试视频",
                    "pic": "http://i0.hdslb.com/bfs/archive/a.jpg", "duration": 300, "cid": 11,
                    "owner": { "name": "UP 主", "mid": 1 },
                    "pages": [
                        { "cid": 11, "page": 1, "part": "上", "duration": 100 },
                        { "cid": 12, "page": 2, "part": "下", "duration": 200 }
                    ]
                }
            }),
            (_, Some("404")) => serde_json::json!({ "code": -404, "message": "啥都木有" }),
            (_, Some("4120")) => serde_json::json!({ "code": -412, "message": "请求被拦截" }),
            (_, Some("503")) => {
                return (StatusCode::SERVICE_UNAVAILABLE, "upstream down").into_response();
            }
            (_, Some("999")) => return "not json".into_response(),
            _ => serde_json::json!({ "code": -799, "message": "请求过于频繁" }),
        };
        axum::Json(body).into_response()
    }

    #[tokio::test]
    async fn http_client_parses_view_and_maps_codes() {
        let (base, server) = stub().await;
        let api = HttpBilibili::new(&base, Duration::from_secs(5)).unwrap();
        let meta = api
            .bilibili(&VideoKey::Bvid("BV1xx411c7mD".into()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(meta.aid, 170_001);
        assert_eq!(meta.title, "测试视频");
        assert_eq!(meta.owner.as_deref(), Some("UP 主"));
        assert_eq!(meta.pages.len(), 2);
        assert_eq!(meta.pages[1].cid, 12);
        assert!(api.bilibili(&VideoKey::Aid(404)).await.unwrap().is_none());
        assert!(matches!(
            api.bilibili(&VideoKey::Aid(5)).await,
            Err(UpstreamError::Code { code: -799, .. })
        ));
        assert!(matches!(
            api.bilibili(&VideoKey::Aid(503)).await,
            Err(UpstreamError::Status(s)) if s == StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(matches!(
            api.bilibili(&VideoKey::Aid(999)).await,
            Err(UpstreamError::Json(_))
        ));
        // buvid 只要了一次，之后的请求一直带着。
        assert_eq!(server.issued.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn http_client_renews_buvid_when_blocked() {
        let (base, server) = stub().await;
        let api = HttpBilibili::new(&base, Duration::from_secs(5)).unwrap();
        let key = VideoKey::Bvid("BV1xx411c7mD".into());
        assert!(api.bilibili(&key).await.unwrap().is_some());

        // buvid 失效：被拦一次，换一对后成功。
        *server.valid.lock().unwrap() = None;
        assert!(api.bilibili(&key).await.unwrap().is_some());
        assert_eq!(server.issued.load(Ordering::SeqCst), 2);

        // 换了 buvid 还被拦（HTML 页面或返回码 -412）：报 Blocked，只重试一次。
        for aid in [412, 4120] {
            let error = api.bilibili(&VideoKey::Aid(aid)).await.unwrap_err();
            assert!(matches!(error, UpstreamError::Blocked), "{error}");
            assert!(error.to_string().contains("412"));
        }
        assert_eq!(server.issued.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn caches_hits_and_misses_but_not_failures() {
        let api = Arc::new(Counting(AtomicUsize::new(0)));
        let videos = Videos::new(api.clone());
        let key = VideoKey::Bvid("BV1xx411c7mD".into());
        // 并发的同一查询合并成一次。
        let (a, b) = tokio::join!(videos.get(&key), videos.get(&key));
        assert!(a.unwrap().is_some() && b.unwrap().is_some());
        assert_eq!(api.0.load(Ordering::SeqCst), 1);

        assert!(videos.get(&VideoKey::Aid(1)).await.unwrap().is_none());
        assert!(videos.get(&VideoKey::Aid(1)).await.unwrap().is_none());
        assert_eq!(api.0.load(Ordering::SeqCst), 2);

        assert!(videos.get(&VideoKey::Aid(2)).await.is_err());
        assert!(videos.get(&VideoKey::Aid(2)).await.is_err());
        assert_eq!(api.0.load(Ordering::SeqCst), 4);
    }
}
