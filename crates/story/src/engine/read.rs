//! ik_read：读原文。
//!
//! 按行分页，并受字数预算约束；`follow=true` 时读完一篇自动接着读同组下一篇，
//! 总结型 AI 可以用一串 ik_read 调用把整章按阅读顺序读完，每次结尾都给出精确的续读游标。
//! docId 也可以直接写分组 id（如 `ch:Chapter110`），等价于从该组第一篇开始 follow。

use std::fmt::Write as _;

use serde::Deserialize;

use super::{ToolResult, err, push_line, resolve_doc, resolve_group, speakers_preview};
use crate::corpus::Corpus;
use crate::text::thousands;

/// 默认行数上限给得宽，实际由字数预算控制：整章顺读时短场景很多，不该先撞上行数。
const DEFAULT_LIMIT: usize = 2000;
const MAX_LIMIT: usize = 5000;
const DEFAULT_CHARS: usize = 16_000;
const MAX_CHARS: usize = 60_000;
/// 剩余预算不足这么多字时不再接续下一篇，留给下一次调用。
const FOLLOW_MIN_CHARS: usize = 400;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadArgs {
    #[serde(default)]
    pub doc_id: String,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub offset: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub limit: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub max_chars: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_bool")]
    pub follow: Option<bool>,
    #[serde(default, deserialize_with = "crate::de::opt_bool")]
    pub line_numbers: Option<bool>,
    #[serde(default, deserialize_with = "crate::de::opt_bool")]
    pub include_sources: Option<bool>,
}

fn continuation(doc: &str, offset: usize, args: &ReadArgs, follow: bool) -> String {
    let mut call = format!("ik_read(docId=\"{doc}\"");
    if offset > 1 {
        let _ = write!(call, ", offset={offset}");
    }
    let _ = write!(call, ", follow={follow}");
    if let Some(limit) = args.limit {
        let _ = write!(call, ", limit={}", limit.clamp(1, MAX_LIMIT));
    }
    if let Some(chars) = args.max_chars {
        let _ = write!(call, ", maxChars={}", chars.clamp(500, MAX_CHARS));
    }
    if args.line_numbers == Some(false) {
        call.push_str(", lineNumbers=false");
    }
    if args.include_sources == Some(true) {
        call.push_str(", includeSources=true");
    }
    call.push(')');
    call
}

pub fn run(corpus: &Corpus, args: &ReadArgs) -> ToolResult {
    let id = args.doc_id.trim();
    if id.is_empty() {
        return err(
            "docId 不能为空：传 ik_search / ik_catalog 返回的 doc_…，或分组 id（如 ch:Chapter110）",
        );
    }
    let (mut doc, follow) = if id.starts_with("doc_") {
        (resolve_doc(corpus, id)?, args.follow.unwrap_or(false))
    } else {
        let g = resolve_group(corpus, id)?;
        match corpus.groups[g].docs.first() {
            Some(&d) => (d as usize, args.follow.unwrap_or(true)),
            None => return err(format!("分组「{id}」是空的")),
        }
    };
    let line_budget = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let char_budget = args
        .max_chars
        .unwrap_or(DEFAULT_CHARS)
        .clamp(500, MAX_CHARS);
    let numbers = args.line_numbers.unwrap_or(true);

    let mut offset = args.offset.unwrap_or(1).max(1);
    let total = corpus.docs[doc].line_count as usize;
    if offset > total {
        return err(format!("offset={offset} 超出范围：该篇共 {total} 行"));
    }

    let mut out = String::with_capacity(char_budget * 3 + 1024);
    let mut lines_used = 0usize;
    let mut chars_used = 0usize;
    let mut first_doc = true;
    let mut line_buf = String::new();
    loop {
        let header_start = out.len();
        let d = &corpus.docs[doc];
        let total = d.line_count as usize;
        if first_doc {
            let _ = write!(
                out,
                "[ik_read] {} 「{}」\n{}",
                corpus.doc_id(doc),
                d.title,
                d.path
            );
        } else {
            let _ = write!(
                out,
                "\n\n## {} 「{}」\n{}",
                corpus.doc_id(doc),
                d.title,
                d.path
            );
        }
        let speakers = speakers_preview(corpus, doc, 12);
        if !speakers.is_empty() {
            let _ = write!(out, " | 出场：{speakers}");
        }
        out.push('\n');
        if first_doc {
            super::push_version(&mut out, corpus);
        }
        super::push_order_note(&mut out, corpus, doc);
        chars_used += out[header_start..].chars().count();

        let start = offset;
        let mut end = start - 1; // 已输出的最后一行（1 基）
        let mut body = String::new();
        while end < total && lines_used < line_budget {
            let line = d.first_line as usize + end;
            line_buf.clear();
            if numbers {
                let _ = write!(line_buf, "L{} ", end + 1);
            }
            push_line(&mut line_buf, corpus, line, corpus.line_text(line));
            if args.include_sources.unwrap_or(false) {
                super::push_source(&mut line_buf, corpus, line);
            }
            let cost = line_buf.chars().count() + 1;
            // 预算用尽就停，但每次调用至少给出一行，避免死循环。
            if chars_used + cost > char_budget && lines_used > 0 {
                break;
            }
            body.push_str(&line_buf);
            body.push('\n');
            chars_used += cost;
            lines_used += 1;
            end += 1;
        }
        if end < start && !first_doc {
            // Don't expose an empty "1-0" range when the next header/line
            // cannot fit. Resume that document on the next invocation.
            out.truncate(header_start);
            let _ = write!(
                out,
                "\n\n— 已达本次字数上限。继续：{}",
                continuation(&corpus.doc_id(doc), start, args, follow)
            );
            break;
        }
        let range_start = out.len();
        let _ = writeln!(out, " | 第 {start}-{end} 行 / 共 {} 行", thousands(total));
        chars_used += out[range_start..].chars().count();
        out.push_str(body.trim_end_matches('\n'));

        if end < total {
            let _ = write!(
                out,
                "\n\n— 已达本次{}上限。继续：{}",
                if lines_used >= line_budget {
                    "行数"
                } else {
                    "字数"
                },
                continuation(&corpus.doc_id(doc), end + 1, args, follow)
            );
            break;
        }
        let next = corpus.next_in_group(doc);
        match next {
            Some(n)
                if follow
                    && lines_used < line_budget
                    && chars_used + FOLLOW_MIN_CHARS < char_budget =>
            {
                doc = n;
                offset = 1;
                first_doc = false;
            }
            Some(n) => {
                let call = continuation(&corpus.doc_id(n), 1, args, follow);
                let _ = write!(
                    out,
                    "\n\n— 本篇完。下一篇（目录相邻，非剧情后继）：{} 「{}」 → {call}",
                    corpus.doc_id(n),
                    corpus.docs[n].title
                );
                break;
            }
            None => {
                let g = d.group as usize;
                let _ = write!(out, "\n\n— 「{}」已全部读完。", corpus.groups[g].label);
                if let Some(ng) = corpus
                    .groups
                    .get(g + 1)
                    .filter(|ng| ng.kind == corpus.groups[g].kind)
                {
                    let _ = write!(
                        out,
                        "下一组：{} {} → {}",
                        ng.id,
                        ng.label,
                        continuation(&ng.id, 1, args, follow)
                    );
                }
                break;
            }
        }
    }
    Ok(out)
}
