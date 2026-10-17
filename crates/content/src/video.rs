//! 外部视频（B 站）链接解析与存储形态（POST-015/016）。纯函数，不访问网络；
//! 视频的标题、封面、时长由 [`crate::bilibili`] 从上游取得后经 [`ExternalVideo::apply`] 补上。

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::bilibili::{VideoKey, VideoMeta};

pub const MAX_EXTERNAL_VIDEOS: usize = 1;
const MAX_INPUT_LEN: usize = 512;
const MAX_TITLE_CHARS: usize = 200;
const MAX_COVER_URL_LEN: usize = 512;

/// 存进 `posts.external_videos` 的规范形态。`source` 是作者原始输入，编辑时原样回填。
///
/// 标题、封面、时长、`cid` 由服务端在保存时向 B 站查询后写入（不接受客户端提交）；
/// 查询失败时省略，播放器照常可用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExternalVideo {
    /// 目前只有 `bilibili`。
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bvid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aid: Option<u64>,
    /// 分 P（从 1 开始）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>,
    /// 该分 P 的 cid（播放器据此直接定位分 P）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<u64>,
    pub embed_url: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// 封面（B 站图床 `*.hdslb.com` 的 https 地址；其他来源一律丢弃）。B 站图床校验 Referer，
    /// 客户端加载时用 `referrerpolicy="no-referrer"`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_url: Option<String>,
    /// 时长（秒）。有分 P 时是该分 P 的时长。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<u32>,
}

struct Patterns {
    bvid: Regex,
    avid: Regex,
    url: Regex,
    page: Regex,
}

static PATTERNS: LazyLock<Option<Patterns>> = LazyLock::new(|| {
    Some(Patterns {
        bvid: Regex::new(r"^BV[0-9A-Za-z]{10}$").ok()?,
        avid: Regex::new(r"(?i)^(?:av)?(\d{1,19})$").ok()?,
        url: Regex::new(r"(?i)bilibili\.com/video/(?:(BV[0-9A-Za-z]{10})|(?:av)?(\d{1,19}))")
            .ok()?,
        page: Regex::new(r"[?&]p=(\d{1,5})").ok()?,
    })
});

fn valid_bvid(raw: &str) -> bool {
    PATTERNS.as_ref().is_some_and(|p| p.bvid.is_match(raw))
}

/// 播放器地址。有 BV 号时优先用 BV 号。
fn embed_url(bvid: Option<&str>, aid: Option<u64>, cid: Option<u64>, page: Option<u32>) -> String {
    let mut url = String::from("https://player.bilibili.com/player.html?");
    match (bvid, aid) {
        (Some(bvid), _) => url.push_str(&format!("bvid={bvid}")),
        (None, Some(aid)) => url.push_str(&format!("aid={aid}")),
        (None, None) => {}
    }
    if let Some(cid) = cid {
        url.push_str(&format!("&cid={cid}"));
    }
    if let Some(page) = page {
        url.push_str(&format!("&p={page}"));
    }
    url.push_str("&autoplay=0&danmaku=0&poster=1");
    url
}

/// 解析一条 B 站视频输入：完整链接、BV 号或 av 号。无法识别时返回 `None`。
pub fn parse(input: &str) -> Option<ExternalVideo> {
    let p = PATTERNS.as_ref()?;
    let source = input.trim();
    if source.is_empty() || source.len() > MAX_INPUT_LEN {
        return None;
    }

    let (bvid, aid, page) = if let Some(caps) = p.url.captures(source) {
        let page = p
            .page
            .captures(source)
            .and_then(|c| c.get(1)?.as_str().parse::<u32>().ok())
            .filter(|&n| n > 0);
        // URL 里的 BV 号前缀大小写不敏感匹配到了，这里规范成大写 BV。
        let bvid = caps.get(1).map(|m| format!("BV{}", &m.as_str()[2..]));
        let aid = caps.get(2).and_then(|m| m.as_str().parse::<u64>().ok());
        (bvid, aid, page)
    } else if p.bvid.is_match(source) {
        (Some(source.to_owned()), None, None)
    } else {
        let caps = p.avid.captures(source)?;
        (None, caps.get(1)?.as_str().parse::<u64>().ok(), None)
    };
    let aid = aid.filter(|&a| a > 0);
    if bvid.is_none() && aid.is_none() {
        return None;
    }

    Some(ExternalVideo {
        provider: "bilibili".to_owned(),
        embed_url: embed_url(bvid.as_deref(), aid, None, page),
        bvid,
        aid,
        page,
        cid: None,
        source: source.to_owned(),
        title: None,
        cover_url: None,
        duration: None,
    })
}

/// 只接受 B 站图床（`hdslb.com` 及其子域）的地址，统一成 https。
/// 旧系统直接保存客户端提交的任意地址，导入的数据也要经过这里。
pub(crate) fn cover_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.len() > MAX_COVER_URL_LEN {
        return None;
    }
    let lower = raw.to_ascii_lowercase();
    let rest = ["https://", "http://", "//"]
        .iter()
        .find_map(|scheme| lower.starts_with(scheme).then(|| &raw[scheme.len()..]))?;
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = rest[..host_end].to_ascii_lowercase();
    let host_ok = (host == "hdslb.com" || host.ends_with(".hdslb.com"))
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    if !host_ok || rest.contains(char::is_whitespace) {
        return None;
    }
    Some(format!("https://{rest}"))
}

fn clean_title(raw: &str) -> Option<String> {
    let title: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TITLE_CHARS)
        .collect();
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

impl ExternalVideo {
    /// 向上游查询用的键。BV 号优先。
    pub(crate) fn key(&self) -> Option<VideoKey> {
        match (&self.bvid, self.aid) {
            (Some(bvid), _) => Some(VideoKey::Bvid(bvid.clone())),
            (None, Some(aid)) => Some(VideoKey::Aid(aid)),
            (None, None) => None,
        }
    }

    /// 是否是同一个视频的同一个分 P（判断已存的标题、封面能否沿用）。
    pub(crate) fn same_video(&self, other: &Self) -> bool {
        let id_matches = match (&self.bvid, &other.bvid) {
            (Some(a), Some(b)) => a == b,
            _ => self.aid.is_some() && self.aid == other.aid,
        };
        id_matches && self.page.unwrap_or(1) == other.page.unwrap_or(1)
    }

    /// 补上从 B 站查到的信息：两种编号、标题、封面、该分 P 的 cid 与时长。
    pub(crate) fn apply(&mut self, meta: &VideoMeta) {
        self.bvid = Some(meta.bvid.clone())
            .filter(|b| valid_bvid(b))
            .or(self.bvid.take());
        self.aid = Some(meta.aid).filter(|&a| a > 0).or(self.aid);
        let page = meta.pages.iter().find(|p| p.page == self.page.unwrap_or(1));
        self.cid = page.map(|p| p.cid).or(meta.cid);
        self.duration = page.map(|p| p.duration).or(Some(meta.duration));
        self.title = clean_title(&meta.title);
        self.cover_url = meta.cover_url.as_deref().and_then(cover_url);
        self.rebuild_embed();
    }

    /// 沿用之前保存过的同一视频的信息（这次查询失败时）。
    pub(crate) fn inherit(&mut self, previous: &Self) {
        self.bvid = self.bvid.take().or_else(|| previous.bvid.clone());
        self.aid = self.aid.or(previous.aid);
        self.cid = previous.cid;
        self.title.clone_from(&previous.title);
        self.cover_url.clone_from(&previous.cover_url);
        self.duration = previous.duration;
        self.rebuild_embed();
    }

    fn rebuild_embed(&mut self) {
        self.embed_url = embed_url(self.bvid.as_deref(), self.aid, self.cid, self.page);
    }
}

/// 解码 `posts.external_videos` / `post_drafts.external_videos`。
///
/// 逐项宽松解析：旧系统导入的数据字段不同（`p` 而不是 `page`、没有 `source`、`embedUrl` 与 `coverUrl`
/// 是客户端提交的），整体反序列化会把整个数组丢掉。这里只取能校验的字段，播放器地址按编号重建，
/// 封面只认 B 站图床；无法识别的项跳过。
pub(crate) fn decode_stored(value: &serde_json::Value) -> Vec<ExternalVideo> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items.iter().filter_map(decode_one).collect()
}

fn decode_one(item: &serde_json::Value) -> Option<ExternalVideo> {
    let obj = item.as_object()?;
    let positive = |key: &str| -> Option<u64> {
        let v = obj.get(key)?;
        v.as_u64()
            .or_else(|| v.as_str()?.trim().parse().ok())
            .filter(|&n| n > 0)
    };
    let bvid = obj
        .get("bvid")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|b| valid_bvid(b))
        .map(str::to_owned);
    let aid = positive("aid");
    if bvid.is_none() && aid.is_none() {
        return None;
    }
    let page = positive("page")
        .or_else(|| positive("p"))
        .and_then(|p| u32::try_from(p).ok());
    let cid = positive("cid");
    let source = obj
        .get("source")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= MAX_INPUT_LEN)
        .map_or_else(
            || {
                let id = bvid
                    .clone()
                    .unwrap_or_else(|| format!("av{}", aid.unwrap_or_default()));
                match page {
                    Some(p) => format!("https://www.bilibili.com/video/{id}?p={p}"),
                    None => format!("https://www.bilibili.com/video/{id}"),
                }
            },
            str::to_owned,
        );
    Some(ExternalVideo {
        provider: "bilibili".to_owned(),
        embed_url: embed_url(bvid.as_deref(), aid, cid, page),
        bvid,
        aid,
        page,
        cid,
        source,
        title: obj
            .get("title")
            .and_then(|v| v.as_str())
            .and_then(clean_title),
        cover_url: obj
            .get("coverUrl")
            .and_then(|v| v.as_str())
            .and_then(cover_url),
        duration: positive("duration").and_then(|d| u32::try_from(d).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bilibili::VideoPage;

    #[test]
    fn parses_url_with_page() {
        let v = parse("https://www.bilibili.com/video/BV1xx411c7mD/?p=2&t=10").unwrap();
        assert_eq!(v.bvid.as_deref(), Some("BV1xx411c7mD"));
        assert_eq!(v.page, Some(2));
        assert_eq!(
            v.embed_url,
            "https://player.bilibili.com/player.html?bvid=BV1xx411c7mD&p=2&autoplay=0&danmaku=0&poster=1"
        );
    }

    #[test]
    fn parses_bare_ids() {
        assert_eq!(
            parse("BV1xx411c7mD").unwrap().bvid.as_deref(),
            Some("BV1xx411c7mD")
        );
        assert_eq!(parse("av170001").unwrap().aid, Some(170_001));
        assert_eq!(parse("170001").unwrap().aid, Some(170_001));
        assert_eq!(
            parse("https://bilibili.com/video/av170001").unwrap().aid,
            Some(170_001)
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("").is_none());
        assert!(parse("https://youtube.com/watch?v=x").is_none());
        assert!(parse("BV123").is_none());
        assert!(parse("av0").is_none());
        assert!(
            parse("https://www.bilibili.com/video/BV1xx411c7mD?p=0")
                .unwrap()
                .page
                .is_none()
        );
    }

    #[test]
    fn cover_only_from_bilibili_image_host() {
        assert_eq!(
            cover_url("http://i0.hdslb.com/bfs/archive/a.jpg").as_deref(),
            Some("https://i0.hdslb.com/bfs/archive/a.jpg")
        );
        assert_eq!(
            cover_url("//i2.HDSLB.com/bfs/a.png?x=1").as_deref(),
            Some("https://i2.HDSLB.com/bfs/a.png?x=1")
        );
        assert!(cover_url("https://evil.com/i0.hdslb.com/a.jpg").is_none());
        assert!(cover_url("https://hdslb.com.evil.com/a.jpg").is_none());
        assert!(cover_url("https://user@i0.hdslb.com/a.jpg").is_none());
        assert!(cover_url("javascript:alert(1)").is_none());
        assert!(cover_url("https://i0.hdslb.com/a b.jpg").is_none());
    }

    #[test]
    fn applies_upstream_meta_for_the_requested_page() {
        let mut v = parse("https://www.bilibili.com/video/av170001?p=2").unwrap();
        v.apply(&VideoMeta {
            bvid: "BV1xx411c7mD".into(),
            aid: 170_001,
            title: " 标题\n ".into(),
            cover_url: Some("http://i0.hdslb.com/bfs/a.jpg".into()),
            duration: 300,
            cid: Some(1),
            owner: None,
            pages: vec![
                VideoPage {
                    cid: 1,
                    page: 1,
                    part: "上".into(),
                    duration: 100,
                },
                VideoPage {
                    cid: 2,
                    page: 2,
                    part: "下".into(),
                    duration: 200,
                },
            ],
        });
        assert_eq!(v.bvid.as_deref(), Some("BV1xx411c7mD"));
        assert_eq!(v.cid, Some(2));
        assert_eq!(v.duration, Some(200));
        assert_eq!(v.title.as_deref(), Some("标题"));
        assert_eq!(
            v.cover_url.as_deref(),
            Some("https://i0.hdslb.com/bfs/a.jpg")
        );
        assert_eq!(
            v.embed_url,
            "https://player.bilibili.com/player.html?bvid=BV1xx411c7mD&cid=2&p=2&autoplay=0&danmaku=0&poster=1"
        );
        // 原始输入保持不变（编辑时回填）。
        assert_eq!(v.source, "https://www.bilibili.com/video/av170001?p=2");
    }

    #[test]
    fn decodes_legacy_rows() {
        let stored = serde_json::json!([
            {
                "provider": "bilibili", "bvid": "BV1xx411c7mD", "aid": null, "cid": 42, "p": 3,
                "embedUrl": "javascript:alert(1)", "coverUrl": "https://evil.com/x.jpg",
                "title": "旧标题", "duration": 61
            },
            { "provider": "bilibili", "bvid": "bad" },
            "not an object"
        ]);
        let videos = decode_stored(&stored);
        assert_eq!(videos.len(), 1);
        let v = &videos[0];
        assert_eq!(v.page, Some(3));
        assert_eq!(v.cid, Some(42));
        assert_eq!(v.cover_url, None);
        assert_eq!(v.title.as_deref(), Some("旧标题"));
        assert_eq!(v.duration, Some(61));
        assert_eq!(v.source, "https://www.bilibili.com/video/BV1xx411c7mD?p=3");
        assert!(v.embed_url.starts_with("https://player.bilibili.com/"));
    }

    #[test]
    fn new_rows_round_trip() {
        let mut v = parse("BV1xx411c7mD").unwrap();
        v.title = Some("t".into());
        v.cover_url = Some("https://i0.hdslb.com/a.jpg".into());
        let stored = serde_json::to_value(vec![v.clone()]).unwrap();
        assert_eq!(decode_stored(&stored), vec![v]);
    }

    #[test]
    fn same_video_compares_id_and_page() {
        let a = parse("https://www.bilibili.com/video/BV1xx411c7mD?p=1").unwrap();
        let b = parse("BV1xx411c7mD").unwrap();
        let c = parse("https://www.bilibili.com/video/BV1xx411c7mD?p=2").unwrap();
        assert!(a.same_video(&b));
        assert!(!a.same_video(&c));
        assert!(!b.same_video(&parse("av1").unwrap()));
    }
}
