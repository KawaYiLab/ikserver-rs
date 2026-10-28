//! QQ presentation: parse first, then apply link policy and a budget on the actual wire text.
use crate::message::MessageFormat;
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag};
use regex::Regex;
use std::sync::LazyLock;

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

#[derive(Debug)]
enum Node<'a> {
    Branch(Tag<'a>, Vec<Node<'a>>),
    Text(CowStr<'a>),
    Break,
    Rule,
}
fn nodes<'a>(events: &mut impl Iterator<Item = Event<'a>>) -> Vec<Node<'a>> {
    let mut out = Vec::new();
    while let Some(event) = events.next() {
        match event {
            Event::Start(tag) => out.push(Node::Branch(tag, nodes(events))),
            Event::End(_) => break,
            Event::Text(s) | Event::Code(s) => out.push(Node::Text(s)),
            Event::SoftBreak | Event::HardBreak => out.push(Node::Break),
            Event::Rule => out.push(Node::Rule),
            // Images are parsed as containers, so their alt text survives; raw HTML does not.
            _ => {}
        }
    }
    out
}

#[derive(Debug, Clone)]
enum Piece {
    Text(String),
    Syntax(String),
    Heading(String, Vec<Piece>),
    Wrap(String, Vec<Piece>, String),
    Link(Vec<Piece>, String, bool),
}
/// 链接策略（后台「QQ 机器人」的 `link_mode`）与站点地址（`IK_QQBOT__SITE_URL`）。
/// 站内帖子的相对路径 `/post/<id>` 按站点地址补全；站点地址为空时不补全，按无法直达的链接处理。
#[derive(Debug, Clone, Copy)]
pub struct Links<'a> {
    pub mode: &'a str,
    pub site: &'a str,
}

impl Links<'_> {
    const STRIP: Links<'static> = Links {
        mode: "strip",
        site: "",
    };
}

fn syntax(s: &str) -> Piece {
    Piece::Syntax(s.to_owned())
}
fn is_post_path(path: &str) -> bool {
    path.strip_prefix("/post/")
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()))
}
fn post_path<'a>(url: &'a str, site: &str) -> Option<&'a str> {
    let path = (!site.is_empty())
        .then(|| url.strip_prefix(site))
        .flatten()
        .unwrap_or(url);
    is_post_path(path).then_some(path)
}
fn link(label: Vec<Piece>, url: &str, automatic: bool, links: Links<'_>) -> Vec<Piece> {
    let site = links.site.trim_end_matches('/');
    let post = post_path(url, site);
    let resolved = post.map_or_else(|| url.to_owned(), |p| format!("{site}{p}"));
    let web =
        reqwest13::Url::parse(&resolved).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
    let plain_label = render(&label, MessageFormat::Plain, usize::MAX);
    let automatic = automatic || plain_label == url || plain_label == resolved;
    if web && (links.mode == "allow" || (links.mode == "domain" && post.is_some())) {
        // Parentheses and angle brackets must not terminate a Markdown destination.
        let url = resolved
            .replace('(', "%28")
            .replace(')', "%29")
            .replace('<', "%3C")
            .replace('>', "%3E")
            .replace(' ', "%20");
        return vec![Piece::Link(label, url, automatic)];
    }
    if post.is_some() {
        if automatic && url.contains("://") {
            return vec![];
        }
        if automatic || label.is_empty() {
            return vec![Piece::Text("（在绳网搜索帖子）".into())];
        }
        let mut out = vec![Piece::Text("（在绳网搜《".into())];
        out.extend(label);
        out.push(Piece::Text("》）".into()));
        return out;
    }
    if automatic { vec![] } else { label }
}

static URLS: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)(?:[a-z][a-z0-9+.-]*://|www\.)[^\s<>]+|/post/[0-9]+").ok());
fn text_pieces(value: &str, links: Links<'_>) -> Vec<Piece> {
    let Some(re) = &*URLS else {
        return vec![Piece::Text(value.to_owned())];
    };
    let mut out = Vec::new();
    let mut cursor = 0;
    for found in re.find_iter(value) {
        out.push(Piece::Text(value[cursor..found.start()].to_owned()));
        let url = found.as_str().trim_end_matches([
            '。', '，', '！', '？', '、', '；', '：', '）', ')', ']', '}', '.', ',', '!', '?', ';',
        ]);
        out.extend(link(vec![Piece::Text(url.to_owned())], url, true, links));
        cursor = found.start() + url.len();
    }
    out.push(Piece::Text(value[cursor..].to_owned()));
    out
}

fn join(parts: impl IntoIterator<Item = Vec<Piece>>, separator: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(syntax(separator));
        }
        out.extend(part);
    }
    out
}
fn children(nodes: &[Node<'_>], links: Links<'_>, format: MessageFormat) -> Vec<Piece> {
    nodes
        .iter()
        .flat_map(|n| pieces(n, links, format))
        .collect()
}
// Indentation belongs to each physical line, including nested lists and quotes.
fn indent(parts: Vec<Piece>, prefix: &str) -> Vec<Piece> {
    parts
        .into_iter()
        .map(|part| match part {
            Piece::Text(s) => Piece::Text(s.replace('\n', &format!("\n{prefix}"))),
            Piece::Syntax(s) => Piece::Syntax(s.replace('\n', &format!("\n{prefix}"))),
            Piece::Heading(open, inner) => Piece::Heading(open, indent(inner, prefix)),
            Piece::Wrap(a, inner, b) => Piece::Wrap(a, indent(inner, prefix), b),
            other => other,
        })
        .collect()
}
fn pieces(node: &Node<'_>, links: Links<'_>, format: MessageFormat) -> Vec<Piece> {
    let md = format == MessageFormat::Markdown;
    match node {
        Node::Text(s) => text_pieces(s, links),
        Node::Break => vec![syntax("\n")],
        Node::Rule => vec![syntax(if md { "---" } else { "——" })],
        Node::Branch(tag, inner) => match tag {
            Tag::Heading { level, .. } => vec![Piece::Heading(
                if md {
                    if *level == pulldown_cmark::HeadingLevel::H1 {
                        "# "
                    } else {
                        "## "
                    }
                } else {
                    ""
                }
                .into(),
                children(inner, links, format),
            )],
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough => {
                let marker = if !md {
                    ""
                } else {
                    match tag {
                        Tag::Emphasis => "*",
                        Tag::Strong => "**",
                        _ => "~~",
                    }
                };
                vec![Piece::Wrap(
                    marker.into(),
                    children(inner, links, format),
                    marker.into(),
                )]
            }
            Tag::Link {
                dest_url,
                link_type,
                ..
            } => {
                let automatic = matches!(
                    link_type,
                    pulldown_cmark::LinkType::Autolink | pulldown_cmark::LinkType::Email
                );
                // Link labels must not themselves be recognized as bare URLs.
                let label = label_pieces(inner, format);
                link(label, dest_url, automatic, links)
            }
            Tag::Image { .. } => label_pieces(inner, format),
            Tag::List(start) => join(
                inner.iter().enumerate().map(|(index, item)| {
                    let marker = start.map_or_else(
                        || {
                            if md {
                                "- ".to_owned()
                            } else {
                                "· ".to_owned()
                            }
                        },
                        |n| format!("{}. ", n + index as u64),
                    );
                    let continuation = " ".repeat(marker.chars().count());
                    vec![Piece::Wrap(
                        marker,
                        indent(pieces(item, links, format), &continuation),
                        String::new(),
                    )]
                }),
                "\n",
            ),
            Tag::Item => join(inner.iter().map(|n| pieces(n, links, format)), "\n"),
            Tag::BlockQuote(_) => {
                let body = join(inner.iter().map(|n| pieces(n, links, format)), "\n\n");
                if md {
                    vec![Piece::Wrap("> ".into(), indent(body, "> "), String::new())]
                } else {
                    body
                }
            }
            Tag::Table(_) => join(inner.iter().map(|n| pieces(n, links, format)), "\n"),
            Tag::TableHead | Tag::TableRow => {
                join(inner.iter().map(|n| pieces(n, links, format)), " ｜ ")
            }
            _ => children(inner, links, format),
        },
    }
}
fn label_pieces(nodes: &[Node<'_>], format: MessageFormat) -> Vec<Piece> {
    nodes
        .iter()
        .flat_map(|node| match node {
            Node::Text(s) => vec![Piece::Text(s.to_string())],
            Node::Break => vec![Piece::Text(" ".into())],
            Node::Branch(Tag::Emphasis | Tag::Strong | Tag::Strikethrough, _) => {
                pieces(node, Links::STRIP, format)
            }
            Node::Branch(_, inner) => label_pieces(inner, format),
            _ => vec![],
        })
        .collect()
}
fn escape(c: char, format: MessageFormat) -> String {
    if format == MessageFormat::Markdown
        && matches!(
            c,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '<'
                | '>'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
                | '~'
        )
    {
        format!("\\{c}")
    } else {
        c.to_string()
    }
}

/// Each escaped character and complete link is indivisible; wrappers reserve their closing markers.
struct Rendered {
    text: String,
    truncated: bool,
}

fn natural_boundary(c: char) -> bool {
    matches!(
        c,
        '\n' | '。' | '！' | '？' | '；' | '…' | '.' | '!' | '?' | ';'
    )
}

fn render_text(value: &str, format: MessageFormat, max: usize) -> Rendered {
    let mut out = String::new();
    let mut remaining = max;
    let mut last_boundary = None;
    for c in value.chars() {
        let atom = escape(c, format);
        let size = atom.chars().count();
        if size > remaining {
            if let Some(boundary) = last_boundary {
                out.truncate(boundary);
            }
            return Rendered {
                text: out,
                truncated: true,
            };
        }
        remaining -= size;
        out.push_str(&atom);
        if natural_boundary(c) {
            last_boundary = Some(out.len());
        }
    }
    Rendered {
        text: out,
        truncated: false,
    }
}

fn render_wrapped(
    open: &str,
    inner: &[Piece],
    close: &str,
    format: MessageFormat,
    max: usize,
) -> Rendered {
    let overhead = open.chars().count() + close.chars().count();
    if overhead >= max {
        return render_limited(inner, format, max);
    }
    let body = render_limited(inner, format, max - overhead);
    if body.text.trim().is_empty() {
        Rendered {
            text: String::new(),
            truncated: body.truncated,
        }
    } else {
        Rendered {
            text: format!("{open}{}{close}", body.text.trim_end()),
            truncated: body.truncated,
        }
    }
}

fn render_limited(parts: &[Piece], format: MessageFormat, max: usize) -> Rendered {
    let mut out = String::new();
    let mut remaining = max;
    for part in parts {
        let rendered = match part {
            Piece::Text(s) => render_text(s, format, remaining),
            Piece::Syntax(s) => {
                if s.chars().count() > remaining {
                    return Rendered {
                        text: out,
                        truncated: true,
                    };
                }
                Rendered {
                    text: s.clone(),
                    truncated: false,
                }
            }
            Piece::Heading(open, inner) => render_wrapped(open, inner, "", format, remaining),
            Piece::Wrap(open, inner, close) => {
                render_wrapped(open, inner, close, format, remaining)
            }
            Piece::Link(label, url, automatic) => {
                let title = render(label, format, usize::MAX);
                let full = if format == MessageFormat::Markdown {
                    format!("[{title}]({url})")
                } else if *automatic {
                    url.clone()
                } else {
                    format!("{title} {url}")
                };
                if full.chars().count() <= remaining {
                    Rendered {
                        text: full,
                        truncated: false,
                    }
                } else if *automatic {
                    render_limited(&[Piece::Text("网页链接".into())], format, remaining)
                } else {
                    render_limited(label, format, remaining)
                }
            }
        };
        let size = rendered.text.chars().count();
        if size > remaining {
            return Rendered {
                text: out,
                truncated: true,
            };
        }
        remaining -= size;
        out.push_str(&rendered.text);
        if rendered.truncated {
            return Rendered {
                text: out,
                truncated: true,
            };
        }
    }
    Rendered {
        text: out,
        truncated: false,
    }
}

fn render(parts: &[Piece], format: MessageFormat, max: usize) -> String {
    render_limited(parts, format, max).text
}

fn is_heading_chunk(chunk: &[Piece]) -> bool {
    matches!(chunk, [Piece::Heading(_, _)])
}

fn blocks(value: &str, links: Links<'_>, format: MessageFormat) -> Vec<Vec<Piece>> {
    let root = nodes(&mut Parser::new_ext(
        value,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
    ));
    root.iter()
        .flat_map(|node| {
            // Preserve the existing status-sentence bubbles, but never split inside Markdown syntax.
            if let Node::Branch(Tag::Paragraph, inner) = node
                && inner.iter().all(|n| matches!(n, Node::Text(_)))
            {
                let raw: String = inner
                    .iter()
                    .filter_map(|n| {
                        if let Node::Text(s) = n {
                            Some(s.as_ref())
                        } else {
                            None
                        }
                    })
                    .collect();
                if !raw.contains("://") {
                    return ik_ai::workflow::split_segments(&raw)
                        .iter()
                        .map(|s| text_pieces(s, links))
                        .collect();
                }
            }
            vec![pieces(node, links, format)]
        })
        .collect()
}

pub fn clean(value: &str, links: Links<'_>) -> String {
    format_content(value, links, MessageFormat::Plain, usize::MAX)
}
pub fn format_content(value: &str, links: Links<'_>, format: MessageFormat, max: usize) -> String {
    render(&join(blocks(value, links, format), "\n\n"), format, max)
        .trim()
        .to_owned()
}

/// A persisted plain payload has already been rendered; its literal symbols are not Markdown.
pub fn without_links(value: &str, format: MessageFormat) -> String {
    if format == MessageFormat::Markdown {
        format_content(value, Links::STRIP, format, value.chars().count())
    } else {
        render(
            &text_pieces(value, Links::STRIP),
            format,
            value.chars().count(),
        )
        .trim()
        .to_owned()
    }
}

/// The total includes every format marker, URL and newline, including merged bubbles.
/// The tuple is `(segments, blocked, truncated)`; truncation is reported for diagnostics
/// while the persisted payload remains the exact rendered prefix.
pub fn segments(
    values: &[String],
    links: Links<'_>,
    format: MessageFormat,
    blocked: &[String],
    max_chars: usize,
    group: bool,
) -> (Vec<String>, bool, bool) {
    let visible = values
        .iter()
        .map(|v| {
            clean(
                v,
                Links {
                    mode: "allow",
                    ..links
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let joined = format!("{}\n{visible}", values.join("\n")).to_lowercase();
    if blocked
        .iter()
        .any(|w| !w.trim().is_empty() && joined.contains(&w.to_lowercase()))
    {
        return (vec![truncate("这个话题我就不聊啦", max_chars)], true, false);
    }
    let mut remaining = max_chars;
    let mut out: Vec<String> = Vec::new();
    let mut truncated = false;
    let limit = if group { 4 } else { 3 };
    let chunks: Vec<_> = values
        .iter()
        .flat_map(|v| blocks(v, links, format))
        .collect();
    for chunk in &chunks {
        let separator = if out.len() >= limit { "\n\n" } else { "" };
        let budget = remaining.saturating_sub(separator.len());
        if budget == 0 {
            truncated = true;
            break;
        }
        let rendered = render_limited(chunk, format, budget);
        let text = rendered.text.trim().to_owned();
        if text.is_empty() {
            truncated |= rendered.truncated;
            continue;
        }
        if rendered.truncated && is_heading_chunk(chunk) {
            truncated = true;
            break;
        }
        remaining -= text.chars().count() + separator.len();
        truncated |= rendered.truncated;
        if out.len() < limit {
            out.push(text);
        } else if let Some(last) = out.last_mut() {
            last.push_str(separator);
            last.push_str(&text);
        }
        if rendered.truncated {
            break;
        }
    }
    (out, false, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use MessageFormat::{Markdown, Plain};
    fn l(mode: &str) -> Links<'_> {
        Links {
            mode,
            site: "https://interk.net",
        }
    }
    #[test]
    fn links_obey_all_three_policies() {
        assert_eq!(
            clean("[攻略](/post/123)", l("strip")),
            "（在绳网搜《攻略》）"
        );
        assert_eq!(
            clean("[攻略](/post/123)", l("domain")),
            "攻略 https://interk.net/post/123"
        );
        assert_eq!(
            clean(
                "https://interk.net.evil/post/123 https://evil.test",
                l("domain")
            ),
            ""
        );
        assert_eq!(
            format_content("[攻略](/post/123)", l("allow"), Markdown, 600),
            "[攻略](https://interk.net/post/123)"
        );
        assert_eq!(
            clean(
                "[外链](https://example.com) [邮件](mailto:a@b.com)",
                l("allow")
            ),
            "外链 https://example.com 邮件"
        );
        assert!(
            format_content("<https://example.com>", l("allow"), Markdown, 600)
                .contains("](https://example.com)")
        );
        assert_eq!(clean("<https://example.com>", l("strip")), "");
    }
    #[test]
    fn plain_text_symbols_survive_cleaning() {
        assert_eq!(
            clean("好耶 ^_^ 3*4=12 snake_case", l("strip")),
            "好耶 ^_^ 3*4=12 snake_case"
        );
        assert_eq!(clean("**重点** *强调* ~~旧~~", l("strip")), "重点 强调 旧");
        let md = format_content("好耶 ^_^ 3*4=12", l("allow"), Markdown, 600);
        assert_eq!(clean(&md, l("allow")), "好耶 ^_^ 3*4=12");
    }
    #[test]
    fn supported_structures_and_readable_fallbacks() {
        let raw = "### 标题\n\n**重点** *强调* ~~旧~~\n\n- 第一\n  - 第二\n    1. 第三\n\n> 引用\n> 下一行\n\n---\n\n```rs\nlet x = 1;\n```\n\n| 名字 | 值 |\n| --- | --- |\n| 甲 | 乙 |\n\n![图片说明](https://img.test/a.png)";
        let out = format_content(raw, l("allow"), Markdown, 600);
        assert!(out.starts_with("## 标题"));
        assert!(out.contains("**重点** *强调* ~~旧~~"));
        assert!(out.contains("- 第一\n  - 第二\n    1. 第三"), "{out}");
        assert!(out.contains("> 引用\n> 下一行"));
        assert!(out.contains("---"));
        assert!(out.contains("let x = 1;"));
        assert!(out.contains("甲 ｜ 乙"));
        assert!(out.ends_with("图片说明"));
        assert!(!out.contains("```"));
    }
    #[test]
    fn input_names_and_mentions_are_sanitized() {
        assert_eq!(nickname("【管理员】\n<x>:test"), "管理员xtest");
        assert_eq!(incoming("<@!123> 你好 <@456>"), "你好");
    }
    #[test]
    fn budgets_count_markers_links_and_merged_newlines() {
        for format in [Plain, Markdown] {
            for group in [false, true] {
                let values = vec!["**你好**".to_owned(); 8];
                let (out, _, truncated) = segments(&values, l("allow"), format, &[], 20, group);
                assert!(out.len() <= if group { 4 } else { 3 });
                assert!(out.iter().map(|s| s.chars().count()).sum::<usize>() <= 20);
                assert!(truncated);
                for part in out {
                    assert_eq!(part.matches("**").count() % 2, 0);
                }
            }
        }
        let long = format!(
            "**中文[攻略](https://example.com/{})继续**",
            "长".repeat(500)
        );
        for max in 1..80 {
            let out = format_content(&long, l("allow"), Markdown, max);
            assert!(out.chars().count() <= max);
            assert!(!out.contains("https"));
            assert_eq!(out.matches("**").count() % 2, 0, "{max}: {out}");
            assert!(!out.ends_with('\\'));
        }
        assert_eq!(
            format_content("[攻略](https://example.com/long)", l("allow"), Markdown, 2),
            "攻略"
        );
    }

    #[test]
    fn truncation_prefers_sentence_boundaries_and_skips_partial_headings() {
        let out = format_content("第一句完整。第二句还没有结束", l("allow"), Plain, 8);
        assert_eq!(out, "第一句完整。", "{out}");

        let (out, _, truncated) = segments(
            &[
                "第一句完整。第二句还没有结束".into(),
                "后续内容不应跳过前文".into(),
            ],
            l("allow"),
            Plain,
            &[],
            8,
            false,
        );
        assert!(truncated);
        assert_eq!(out, ["第一句完整。"]);

        let raw = "# 一、第一节标题很长\n\n正文很长。";
        let (out, blocked, truncated) =
            segments(&[raw.into()], l("allow"), Markdown, &[], 5, false);
        assert!(!blocked);
        assert!(truncated);
        assert!(out.is_empty(), "partial heading leaked: {out:?}");

        let raw = "**第一句完整。第二句还没有结束**";
        let out = format_content(raw, l("allow"), Markdown, 10);
        assert_eq!(out, "**第一句完整。**", "{out}");
    }
    #[test]
    fn blocked_words_match_visible_text_and_link_destinations() {
        for raw in ["敏**感**词", "[普通](https://example.com/敏感词)"] {
            let (out, blocked, truncated) = segments(
                &[raw.into()],
                l("strip"),
                Markdown,
                &["敏感词".into()],
                600,
                true,
            );
            assert!(blocked);
            assert!(!truncated);
            assert_eq!(out, ["这个话题我就不聊啦"]);
        }
    }

    #[test]
    fn automatic_links_never_leave_truncated_urls() {
        for raw in ["<https://example.com/a(b)c>", "/post/1234567890123456789"] {
            for format in [Plain, Markdown] {
                assert_eq!(format_content(raw, l("allow"), format, 8), "网页链接");
            }
        }
        assert_eq!(
            clean("/post/123", l("allow")),
            "https://interk.net/post/123"
        );
        let no_site = Links {
            mode: "allow",
            site: "",
        };
        assert_eq!(clean("/post/123", no_site), "（在绳网搜索帖子）");
        assert_eq!(
            clean("https://interk.net/post/123", no_site),
            "https://interk.net/post/123"
        );
    }

    #[test]
    fn url_correction_preserves_already_rendered_plain_symbols() {
        assert_eq!(
            without_links("*原样* _符号_ https://example.com", Plain),
            "*原样* _符号_"
        );
        assert_eq!(
            without_links("**重点** [网页](https://example.com)", Markdown),
            "**重点** 网页"
        );
    }
}
