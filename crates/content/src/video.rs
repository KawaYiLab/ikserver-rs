//! 外部视频（B 站）链接解析（POST-015/016）。纯函数，不访问网络。

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const MAX_EXTERNAL_VIDEOS: usize = 1;
const MAX_INPUT_LEN: usize = 512;

/// 存进 `posts.external_videos` 的规范形态。`source` 是作者原始输入，编辑时原样回填。
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
    pub embed_url: String,
    pub source: String,
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
    if bvid.is_none() && aid.is_none_or(|a| a == 0) {
        return None;
    }

    let mut embed_url = String::from("https://player.bilibili.com/player.html?");
    match (&bvid, aid) {
        (Some(bvid), _) => embed_url.push_str(&format!("bvid={bvid}")),
        (None, Some(aid)) => embed_url.push_str(&format!("aid={aid}")),
        (None, None) => return None,
    }
    if let Some(page) = page {
        embed_url.push_str(&format!("&p={page}"));
    }
    embed_url.push_str("&autoplay=0&danmaku=0&poster=1");

    Some(ExternalVideo {
        provider: "bilibili".to_owned(),
        bvid,
        aid,
        page,
        embed_url,
        source: source.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
