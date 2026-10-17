//! B 站视频信息：标题、封面、时长、分 P。发帖编辑器预览与保存帖子时用。
//!
//! 上游用 trait 抽象，测试换成假实现；真实实现见 [`HttpBilibili`]。查询结果在进程内缓存 10 分钟
//! （同一个视频的并发查询合并成一次），编辑器预览之后紧接着保存帖子不会再访问上游。
//! 调用都在数据库事务之外。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const CACHE_CAPACITY: u64 = 10_000;
const MAX_PAGES: usize = 200;
/// 这些返回码表示视频不存在、被删除或不可见（不是上游故障）。
const GONE_CODES: [i64; 5] = [-404, -403, -400, 62002, 62004];

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

/// 真实实现：`GET {base}/x/web-interface/view?bvid=|aid=`。
pub struct HttpBilibili {
    client: reqwest::Client,
    base: String,
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
        })
    }
}

#[derive(Deserialize)]
struct Envelope {
    code: i64,
    #[serde(default)]
    message: String,
    data: Option<ViewData>,
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
        let request = self
            .client
            .get(format!("{}/x/web-interface/view", self.base))
            .header("Referer", "https://www.bilibili.com/");
        let request = match key {
            VideoKey::Bvid(bvid) => request.query(&[("bvid", bvid)]),
            VideoKey::Aid(aid) => request.query(&[("aid", aid)]),
        };
        let envelope: Envelope = request.send().await?.json().await?;
        if GONE_CODES.contains(&envelope.code) {
            return Ok(None);
        }
        if envelope.code != 0 {
            return Err(UpstreamError::Code {
                code: envelope.code,
                message: envelope.message,
            });
        }
        let data = envelope.data.ok_or(UpstreamError::Malformed("缺少 data"))?;
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    /// 本地桩服务器模拟 B 站 view 接口。
    async fn stub() -> String {
        use axum::extract::Query;
        use axum::routing::get;
        use std::collections::HashMap;

        let app = axum::Router::new().route(
            "/x/web-interface/view",
            get(|Query(q): Query<HashMap<String, String>>| async move {
                let body = match (q.get("bvid").map(String::as_str), q.get("aid").map(String::as_str)) {
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
                    _ => serde_json::json!({ "code": -412, "message": "请求被拦截" }),
                };
                axum::Json(body)
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn http_client_parses_view_and_maps_codes() {
        let api = HttpBilibili::new(&stub().await, Duration::from_secs(5)).unwrap();
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
            Err(UpstreamError::Code { code: -412, .. })
        ));
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
