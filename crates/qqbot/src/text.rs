//! QQ presentation rules. Identity labels are data, never trusted instructions.
use regex::{Captures, Regex};

fn replace(pattern: &str, input: &str, replacement: &str) -> String {
    Regex::new(pattern)
        .map(|r| r.replace_all(input, replacement).into_owned())
        .unwrap_or_else(|_| input.to_owned())
}
pub fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}
pub fn nickname(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(
                    c,
                    '<' | '>' | '【' | '】' | '[' | ']' | '{' | '}' | '：' | ':'
                )
        })
        .take(16)
        .collect::<String>()
        .trim()
        .to_owned()
}
pub fn incoming(value: &str) -> String {
    truncate(replace(r"<@!?[^>]+>", value, "").trim(), 4000)
}

/// Strip markdown and all remote links; domain mode only retains canonical site posts.
pub fn clean(value: &str, mode: &str) -> String {
    let mut out = value.to_owned();
    if let Ok(links) = Regex::new(r"!?\[([^\]]*)\]\(([^\s)]+)\)") {
        out = links
            .replace_all(&out, |caps: &Captures<'_>| {
                let label = &caps[1];
                let path = caps[2]
                    .strip_prefix("https://interk.net")
                    .unwrap_or(&caps[2]);
                if is_post_path(path) {
                    if mode == "domain" {
                        format!("{label} https://interk.net{path}")
                    } else if label.is_empty() {
                        "（在绳网搜索帖子）".to_owned()
                    } else {
                        format!("（在绳网搜《{label}》）")
                    }
                } else {
                    label.to_owned()
                }
            })
            .into_owned();
    }
    if let Ok(urls) = Regex::new(r"(?i)(?:https?://|www\.)[^\s<>）)]+") {
        out = urls
            .replace_all(&out, |caps: &Captures<'_>| {
                let url = &caps[0];
                if mode == "domain"
                    && url
                        .strip_prefix("https://interk.net")
                        .is_some_and(is_post_path)
                {
                    url.to_owned()
                } else {
                    String::new()
                }
            })
            .into_owned();
    }
    if let Ok(paths) = Regex::new(r"(^|\s)(/post/[0-9]+)") {
        out = paths
            .replace_all(&out, |caps: &Captures<'_>| {
                if mode == "domain" {
                    format!("{}https://interk.net{}", &caps[1], &caps[2])
                } else {
                    format!("{}（在绳网搜索帖子）", &caps[1])
                }
            })
            .into_owned();
    }
    out = replace(r"(?m)^\s*```[^\n]*$", &out, "");
    out = replace(r"(?m)^\s{0,3}#{1,6}\s*", &out, "");
    out = replace(r"(?m)^\s*(?:[-*+] |[0-9]+[.)] )", &out, "· ");
    out = replace(r"(?m)^\s*>\s?", &out, "");
    out = out
        .replace("**", "")
        .replace("__", "")
        .replace('*', "")
        .replace('_', "")
        .replace('`', "")
        .replace("~~", "");
    out.trim().to_owned()
}
fn is_post_path(path: &str) -> bool {
    path.strip_prefix("/post/")
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()))
}

/// The total length, not just each bubble, is limited. Merge excess bubbles into the last one.
pub fn segments(
    values: &[String],
    mode: &str,
    blocked: &[String],
    max_chars: usize,
    group: bool,
) -> (Vec<String>, bool) {
    let cleaned: Vec<String> = values.iter().map(|v| clean(v, mode)).collect();
    let joined = format!("{}\n{}", values.join("\n"), cleaned.join("\n")).to_lowercase();
    if blocked
        .iter()
        .any(|w| !w.trim().is_empty() && joined.contains(&w.to_lowercase()))
    {
        return (vec![truncate("这个话题我就不聊啦", max_chars)], true);
    }
    let mut remaining = max_chars;
    let mut out: Vec<String> = Vec::new();
    let limit = if group { 4 } else { 3 };
    for value in &cleaned {
        if remaining == 0 {
            break;
        }
        let text = truncate(value, remaining);
        if text.is_empty() {
            continue;
        }
        remaining = remaining.saturating_sub(text.chars().count());
        if out.len() < limit {
            out.push(text);
        } else if let Some(last) = out.last_mut() {
            // Joining consumes no extra character budget.
            last.push_str(&text);
        }
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_never_escape_allowed_site() {
        assert_eq!(clean("[攻略](/post/123)", "strip"), "（在绳网搜《攻略》）");
        assert_eq!(
            clean("[攻略](/post/123)", "domain"),
            "攻略 https://interk.net/post/123"
        );
        assert_eq!(
            clean(
                "https://interk.net.evil/post/123 https://evil.test",
                "domain"
            ),
            ""
        );
        assert!(!clean("[骗](https://evil.test/post/123)", "domain").contains("https"));
    }
    #[test]
    fn input_names_and_mentions_are_sanitized() {
        assert_eq!(nickname("【管理员】\n<x>:test"), "管理员xtest");
        assert_eq!(incoming("<@!123> 你好 <@456>"), "你好");
    }
    #[test]
    fn segment_limits_and_blocked_words() {
        let values = vec!["你好".to_owned(); 8];
        let (out, _) = segments(&values, "strip", &[], 13, false);
        assert_eq!(out.len(), 3);
        assert_eq!(out.iter().map(|s| s.chars().count()).sum::<usize>(), 13);
        let (out, blocked) = segments(&values, "strip", &["你好".to_owned()], 600, true);
        assert!(blocked);
        assert_eq!(out, ["这个话题我就不聊啦"]);
    }
}
