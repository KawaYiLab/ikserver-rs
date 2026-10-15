//! 评论正文里的 token：表情 `:ik-xxx:` 与提及 `@[名字](用户id)`（CMT-007/008/014/015）。纯函数。

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

pub const MAX_BODY_CHARS: usize = 2000;
pub const MAX_EMOTES: usize = 20;
/// 一条评论最多给前 10 个不同的被提及者发通知（CMT-018）。
pub const MAX_MENTION_TARGETS: usize = 10;

struct Patterns {
    emote: Regex,
    mention: Regex,
    bare: Regex,
}

static PATTERNS: LazyLock<Option<Patterns>> = LazyLock::new(|| {
    Some(Patterns {
        // 与前端一字不差（CMT-008）。
        emote: Regex::new(r":(ik-[a-z0-9-]{1,32}):").ok()?,
        mention: Regex::new(r"@\[([^\[\]\n]{1,40})\]\((\d{1,20})\)").ok()?,
        bare: Regex::new(r"@([\p{L}\p{N}_\-]+)").ok()?,
    })
});

fn patterns() -> Option<&'static Patterns> {
    PATTERNS.as_ref()
}

/// 表情 token 出现次数（不去重）。
pub fn emote_count(body: &str) -> usize {
    patterns().map_or(0, |p| p.emote.find_iter(body).count())
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 正文里可以展开的「裸 @」：不在已有提及 token 内部，且 `@` 前不是字母数字
/// （避免把邮箱 `a@b.com` 里的 `b` 当成用户名，旧系统会，见 CMT-143）。返回 (起止字节, 用户名)。
fn bare_mentions(body: &str) -> Vec<(usize, usize, &str)> {
    let Some(p) = patterns() else {
        return Vec::new();
    };
    let tokens: Vec<(usize, usize)> = p
        .mention
        .find_iter(body)
        .map(|m| (m.start(), m.end()))
        .collect();
    p.bare
        .captures_iter(body)
        .filter_map(|caps| {
            let whole = caps.get(0)?;
            let name = caps.get(1)?;
            let inside_token = tokens
                .iter()
                .any(|&(s, e)| whole.start() >= s && whole.start() < e);
            let glued = body[..whole.start()]
                .chars()
                .next_back()
                .is_some_and(is_word_char);
            (!inside_token && !glued).then(|| (whole.start(), whole.end(), name.as_str()))
        })
        .collect()
}

/// 需要查库解析的裸 @ 用户名（去重）。
pub fn bare_mention_names(body: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (_, _, name) in bare_mentions(body) {
        if !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// 把能解析到用户的裸 @ 改写成 `@[用户名](id)`；解析不到的原样保留（CMT-016）。
pub fn expand_bare_mentions(body: &str, resolved: &HashMap<String, i64>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut last = 0;
    for (start, end, name) in bare_mentions(body) {
        if let Some(id) = resolved.get(name) {
            out.push_str(&body[last..start]);
            out.push_str(&format!("@[{name}]({id})"));
            last = end;
        }
    }
    out.push_str(&body[last..]);
    out
}

/// 正文中提及的用户 id，按首次出现顺序去重，最多 [`MAX_MENTION_TARGETS`] 个。
pub fn mentioned_user_ids(body: &str) -> Vec<i64> {
    let Some(p) = patterns() else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for caps in p.mention.captures_iter(body) {
        let Some(id) = caps.get(2).and_then(|m| m.as_str().parse::<i64>().ok()) else {
            continue;
        };
        if !ids.contains(&id) {
            ids.push(id);
            if ids.len() == MAX_MENTION_TARGETS {
                break;
            }
        }
    }
    ids
}

/// 提及 token `@[名字](用户id)`（TRIG-017）：名字去掉 `[`、`]` 与换行，截 40 字，空则为「用户」。
pub fn mention_token(name: &str, user_id: i64) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '\n' | '\r'))
        .take(40)
        .collect();
    let cleaned = if cleaned.trim().is_empty() {
        "用户".to_owned()
    } else {
        cleaned
    };
    format!("@[{cleaned}]({user_id})")
}

/// 通知预览用的纯文本：提及 token 变成 `@名字`，表情 token 变成 `[表情:名字]`（NOTIF 触发点表）。
pub fn plain_text(body: &str) -> String {
    let Some(p) = patterns() else {
        return body.to_owned();
    };
    let mentions = p.mention.replace_all(body, "@$1");
    p.emote
        .replace_all(&mentions, |caps: &regex::Captures<'_>| {
            let name = caps.get(1).map_or("", |m| m.as_str());
            format!("[表情:{}]", name.trim_start_matches("ik-"))
        })
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_renders_tokens() {
        assert_eq!(
            plain_text("@[小明](42) 你好 :ik-smile::ik-cry-2:"),
            "@小明 你好 [表情:smile][表情:cry-2]"
        );
    }

    #[test]
    fn patterns_compile() {
        assert!(patterns().is_some());
    }

    #[test]
    fn counts_emotes_including_repeats() {
        assert_eq!(
            emote_count(":ik-smile::ik-smile: :ik-cry: :smile: :IK-x:"),
            3
        );
    }

    #[test]
    fn finds_bare_mentions_outside_tokens() {
        let body = "你好 @alice，@[Bob](123) 和 @绳网用户ab12 还有 a@b.com @alice";
        assert_eq!(bare_mention_names(body), vec!["alice", "绳网用户ab12"]);
    }

    #[test]
    fn expands_only_resolved_names() {
        let resolved = HashMap::from([("alice".to_owned(), 7_i64)]);
        assert_eq!(
            expand_bare_mentions("@alice 和 @nobody @alice", &resolved),
            "@[alice](7) 和 @nobody @[alice](7)"
        );
    }

    #[test]
    fn mention_ids_are_distinct_and_capped() {
        let body: String = (1..=15)
            .map(|i| format!("@[u{i}]({i}) @[again]({i}) "))
            .collect();
        let ids = mentioned_user_ids(&body);
        assert_eq!(ids, (1..=10).collect::<Vec<i64>>());
    }

    #[test]
    fn mention_token_is_parseable() {
        let token = mention_token("a[b]\nc", 7);
        assert_eq!(token, "@[abc](7)");
        assert_eq!(mentioned_user_ids(&token), vec![7]);
        assert_eq!(mention_token("[]", 8), "@[用户](8)");
        let long = mention_token(&"名".repeat(50), 9);
        assert_eq!(mentioned_user_ids(&long), vec![9]);
    }

    #[test]
    fn malformed_tokens_are_ignored() {
        assert!(mentioned_user_ids("@[名字](abc) @[](1) @[a\nb](2)").is_empty());
    }
}
