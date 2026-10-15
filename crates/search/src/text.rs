//! 纯函数：Markdown 转纯文本（SRCH-003）、查询词规范化、高亮转义。

use std::sync::OnceLock;

use regex::Regex;

/// 索引里正文的最大字数（SRCH-003：`ARTICLE_SEARCH_TEXT_LIMIT`，评论也用这个值）。
pub const INDEX_TEXT_MAX_CHARS: usize = 3000;
/// 高亮占位符：Meilisearch 用它们包住命中词，转义之后再换成 `<mark>`。
/// 取私用区字符，正常文本里不会出现。
pub(crate) const MARK_START: &str = "\u{E000}";
pub(crate) const MARK_END: &str = "\u{E001}";

struct Patterns {
    html: Regex,
    image: Regex,
    link: Regex,
    marks: Regex,
    space: Regex,
}

fn patterns() -> Option<&'static Patterns> {
    static P: OnceLock<Option<Patterns>> = OnceLock::new();
    P.get_or_init(|| {
        Some(Patterns {
            html: Regex::new(r"<[^>]+>").ok()?,
            image: Regex::new(r"!\[[^\]]*\]\([^)]*\)").ok()?,
            link: Regex::new(r"\[([^\]]*)\]\([^)]*\)").ok()?,
            marks: Regex::new(r"[#>*_~`]+").ok()?,
            space: Regex::new(r"\s+").ok()?,
        })
    })
    .as_ref()
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// Markdown → 纯文本（SRCH-003）：去 HTML 标签与图片，链接留文字，去强调符号，空白折叠，截 3000 字。
pub fn markdown_to_plain(markdown: &str) -> String {
    let Some(p) = patterns() else {
        return truncate(markdown, INDEX_TEXT_MAX_CHARS);
    };
    let text = p.html.replace_all(markdown, " ");
    let text = p.image.replace_all(&text, " ");
    let text = p.link.replace_all(&text, "$1");
    let text = p.marks.replace_all(&text, " ");
    let text = p.space.replace_all(&text, " ");
    truncate(text.trim(), INDEX_TEXT_MAX_CHARS)
}

/// 查询词：去首尾空白、折叠内部空白、去掉控制字符和高亮占位符，最多 `max_chars` 字（D-042 方案 A）。
pub fn normalize_query(q: Option<&str>, max_chars: usize) -> String {
    let cleaned: String = q
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .filter(|c| !matches!(*c, '\u{E000}' | '\u{E001}'))
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate(&collapsed, max_chars)
}

pub(crate) fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// 带占位符的高亮文本 → 转义后的 HTML，命中词包在 `<mark>` 里。
pub(crate) fn highlight_html(marked: &str) -> String {
    escape_html(marked)
        .replace(MARK_START, "<mark>")
        .replace(MARK_END, "</mark>")
}

/// 去掉高亮占位符，得到原文。
pub(crate) fn strip_marks(marked: &str) -> String {
    marked.replace(MARK_START, "").replace(MARK_END, "")
}

/// Meilisearch 过滤表达式里的字符串字面量（双引号内转义 `\` 与 `"`）。
pub(crate) fn filter_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_becomes_plain_text() {
        let md = "# 标题\n\n**加粗** 和 _斜体_ ![图](https://x/y.png) [链接文字](https://x) <b>html</b>\n> 引用 `code`";
        assert_eq!(
            markdown_to_plain(md),
            "标题 加粗 和 斜体 链接文字 html 引用 code"
        );
        assert_eq!(markdown_to_plain(&"字".repeat(5000)).chars().count(), 3000);
        assert_eq!(markdown_to_plain("   "), "");
    }

    #[test]
    fn queries_are_normalized_and_bounded() {
        assert_eq!(normalize_query(Some("  绳网\t 委托\n"), 64), "绳网 委托");
        assert_eq!(normalize_query(None, 64), "");
        assert_eq!(normalize_query(Some("a\u{E000}b"), 64), "ab");
        assert_eq!(
            normalize_query(Some(&"字".repeat(100)), 64).chars().count(),
            64
        );
    }

    #[test]
    fn highlights_are_escaped() {
        let marked = format!("<script>{MARK_START}绳网{MARK_END}&");
        assert_eq!(
            highlight_html(&marked),
            "&lt;script&gt;<mark>绳网</mark>&amp;"
        );
        assert_eq!(strip_marks(&marked), "<script>绳网&");
    }

    #[test]
    fn filter_strings_are_quoted() {
        assert_eq!(filter_string(r#"a"b\c"#), r#""a\"b\\c""#);
    }
}
