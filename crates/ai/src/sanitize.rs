//! 回复正文清洗（GEN-031、GEN-040～044）。纯函数。

use std::sync::LazyLock;

use regex::Regex;

use crate::catalog::OutputMode;

/// 私信回复上限（GEN-031）。
pub const DM_MAX_CHARS: usize = 2500;
/// 评论回复上限（GEN-031）。
pub const COMMENT_MAX_CHARS: usize = 1500;

struct Rule {
    re: Option<Regex>,
    with: &'static str,
}

fn rule(pattern: &str, with: &'static str) -> Rule {
    Rule {
        re: Regex::new(pattern).ok(),
        with,
    }
}

fn apply(rules: &[Rule], text: &str) -> String {
    let mut out = text.to_owned();
    for r in rules {
        if let Some(re) = &r.re {
            out = re.replace_all(&out, r.with).into_owned();
        }
    }
    out
}

/// 两种模式都做的安全清洗：引用角标、`doc_` 引用、HTML 标签、提及注入（GEN-041/042）。
static SAFE: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        // 模型照着历史写出的工具记录行（GEN-045、CTX-012）。
        rule(r"(?m)^[ \t]*【工具记录】[^\n]*(?:\n|$)", ""),
        rule(r"(?i)\[(?:reference|citation|ref|source)[:\s]*\d+\]", ""),
        rule(r"\s*doc_[A-Za-z0-9_-]+\b", ""),
        rule(r"</?[a-zA-Z][^>]*>", ""),
        rule(
            r"@\[([^\[\]\n]{1,40})\]\((?:user:|author:)?[A-Za-z0-9]{1,32}\)",
            "@$1",
        ),
    ]
});

/// plain 模式额外去掉的 Markdown 标记（GEN-041）。
static PLAIN: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        rule(r"\*\*([^*\n]+)\*\*", "$1"),
        rule(r"__([^_\n]+)__", "$1"),
        rule(r"\*([^*\n]+)\*", "$1"),
        rule(r"(^|[^\w])_([^_\n]+)_([^\w]|$)", "$1$2$3"),
        rule(r"`([^`\n]+)`", "$1"),
        rule(r"(?m)^#{1,6}\s+", ""),
        rule(r"(?m)^[ \t]*[-*+][ \t]+", "· "),
        rule(r"(?m)^[ \t]*\d+\.[ \t]+", ""),
    ]
});

static TRAILING_SPACE: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"(?m)[ \t]+$").ok());
static BLANK_LINES: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"\n{3,}").ok());
static TOOL_ID: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^call_[A-Za-z0-9_-]+$").ok());

fn compress(text: &str) -> String {
    let mut out = text.replace("\r\n", "\n");
    if let Some(re) = &*TRAILING_SPACE {
        out = re.replace_all(&out, "").into_owned();
    }
    if let Some(re) = &*BLANK_LINES {
        out = re.replace_all(&out, "\n\n").into_owned();
    }
    out.trim().to_owned()
}

/// 形似工具调用的对象（GEN-040）：`id` 形如 `call_…`，且有 name+参数 / name+result / name+ts，或带 result。
fn looks_like_tool_call(value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let id_ok = object
        .get("id")
        .and_then(|v| v.as_str())
        .is_some_and(|id| TOOL_ID.as_ref().is_some_and(|re| re.is_match(id)));
    if !id_ok {
        return false;
    }
    let has = |k: &str| object.contains_key(k);
    (has("name") && (has("args") || has("arguments") || has("result") || has("ts")))
        || has("result")
}

/// 从 `start`（一个 `{`）找配对的 `}`，跳过字符串里的括号。返回 `}` 之后的字节位置。
fn balanced_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, c) in text[start..].char_indices() {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(start + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// 剔除泄漏进正文的工具调用 JSON（GEN-040）。
pub fn strip_tool_json(text: &str) -> String {
    if !text.contains('{') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(found) = text[cursor..].find('{') {
        let start = cursor + found;
        out.push_str(&text[cursor..start]);
        match balanced_end(text, start) {
            Some(end)
                if serde_json::from_str::<serde_json::Value>(&text[start..end])
                    .is_ok_and(|v| looks_like_tool_call(&v)) =>
            {
                cursor = end;
            }
            _ => {
                out.push('{');
                cursor = start + 1;
            }
        }
    }
    out.push_str(&text[cursor..]);
    out
}

/// 超长截到 `max − 1` 再加 `…`（GEN-031）。
fn truncate_plain(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// plain 模式（GEN-041）。
pub fn plain(text: &str, max: usize) -> String {
    let text = apply(&SAFE, &strip_tool_json(text));
    let text = apply(&PLAIN, &text);
    truncate_plain(&compress(&text), max)
}

/// markdown 模式（GEN-042）：只在代码块外做安全清洗；截断留 5 字余量，截在未闭合的代码块里时补上围栏。
pub fn markdown(text: &str, max: usize) -> String {
    let text = strip_tool_json(text);
    let mut out = String::with_capacity(text.len());
    for (index, segment) in text.split("```").enumerate() {
        if index > 0 {
            out.push_str("```");
        }
        if index % 2 == 0 {
            out.push_str(&apply(&SAFE, segment));
        } else {
            out.push_str(segment);
        }
    }
    let out = compress(&out);
    if out.chars().count() <= max {
        return out;
    }
    let mut cut: String = out.chars().take(max.saturating_sub(5)).collect();
    cut.push('…');
    if cut.matches("```").count() % 2 == 1 {
        cut.push_str("\n```");
    }
    cut
}

/// 按输出模式清洗；结果为空时按 plain 再洗一次（GEN-044）。仍为空返回空串，由调用方按错误处理。
pub fn sanitize(text: &str, mode: OutputMode, max: usize) -> String {
    let cleaned = match mode {
        OutputMode::Plain => plain(text, max),
        OutputMode::Markdown => markdown(text, max),
    };
    if cleaned.is_empty() {
        plain(text, max)
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_strips_markdown_and_injections() {
        let raw = "## 标题\n**重点** 和 *斜体* 与 `code`\n- 一\n1. 二\n<b>粗</b> [ref:1] doc_abc12\n\
                   @[绳匠](author:abc123)\n\n\n\n尾巴   ";
        assert_eq!(
            plain(raw, 2500),
            "标题\n重点 和 斜体 与 code\n· 一\n二\n粗\n@绳匠\n\n尾巴"
        );
        assert_eq!(plain("snake_case_name", 100), "snake_case_name");
    }

    #[test]
    fn imitated_tool_digest_lines_are_removed() {
        let raw = "【工具记录】检索「攻略」1 条\n\n找到了。";
        assert_eq!(plain(raw, 2500), "找到了。");
        assert_eq!(markdown(raw, 2500), "找到了。");
        assert_eq!(
            plain("正文里提到【工具记录】不算", 2500),
            "正文里提到【工具记录】不算"
        );
    }

    #[test]
    fn plain_truncates_with_ellipsis() {
        let out = plain(&"字".repeat(30), 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn markdown_keeps_syntax_and_code() {
        let raw = "# 标题\n- 项\n```html\n<b>保留</b>\n```\n<i>去掉</i>";
        assert_eq!(
            markdown(raw, 2500),
            "# 标题\n- 项\n```html\n<b>保留</b>\n```\n去掉"
        );
        let long = format!("```\n{}", "a".repeat(100));
        let cut = markdown(&long, 20);
        assert!(cut.ends_with("\n```"));
        assert!(cut.chars().count() <= 20);
    }

    #[test]
    fn tool_json_is_removed() {
        let raw = r#"结果如下{"id":"call_1","name":"search_posts","arguments":"{}"}。{"a":1}"#;
        assert_eq!(strip_tool_json(raw), r#"结果如下。{"a":1}"#);
        assert_eq!(strip_tool_json("{未闭合"), "{未闭合");
    }

    #[test]
    fn empty_after_markdown_falls_back() {
        assert_eq!(sanitize("<br>", OutputMode::Markdown, 100), "");
        assert_eq!(sanitize("**好**", OutputMode::Plain, 100), "好");
    }
}
