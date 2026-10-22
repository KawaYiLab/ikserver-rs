//! ik_read：读原文。
//!
//! 按行分页，并受字数预算约束；`follow=true` 时读完一篇自动接着读同组下一篇，
//! 结果包含当前调用完整返回行的覆盖记录及续读游标；目录顺序不表示剧情先后。
//! docId 也可以直接写分组 id（如 `ch:Chapter110`），等价于从该组第一篇开始 follow。

use std::fmt::Write as _;

use super::{cursor, response::*};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ToolResult, err, push_line, resolve_doc, resolve_group};
use crate::corpus::Corpus;
use crate::text::thousands;

/// 默认行数上限给得宽，实际由字数预算控制：整章顺读时短场景很多，不该先撞上行数。
const DEFAULT_LIMIT: usize = 2000;
const MAX_LIMIT: usize = 5000;
const DEFAULT_CHARS: usize = 16_000;
const MAX_CHARS: usize = 60_000;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadArgs {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    /// Cursor-only presentation context; resolved and checked against the actual document's group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_selector: Option<String>,
    #[serde(default)]
    pub doc_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub batch_index: Option<usize>,
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
    #[serde(default, deserialize_with = "crate::de::opt_bool")]
    pub include_evidence: Option<bool>,
}

fn continuation(doc: &str, offset: usize, args: &ReadArgs, follow: bool) -> String {
    let mut call = String::from("ik_read(");
    if let Some(detail) = &args.detail {
        let _ = write!(call, "detail={detail:?}, ");
    }
    let _ = write!(call, "docId={doc:?}");
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
    if args.include_evidence == Some(true) {
        call.push_str(", includeEvidence=true");
    }
    call.push(')');
    call
}

pub fn run(corpus: &Corpus, args: &ReadArgs) -> ToolResult {
    execute(corpus, args).map(|r| r.text)
}

pub fn execute(corpus: &Corpus, args: &ReadArgs) -> OutputResult {
    let full = detail(args.detail.as_deref())?;
    let mut batch = args.batch_index.unwrap_or(0);
    let batch_docs = if let Some(ids) = &args.doc_ids {
        if !args.doc_id.is_empty() || args.follow == Some(true) || ids.is_empty() || ids.len() > 16
        {
            return err("docIds 需为1～16个文档ID，不能与 docId/follow=true 同用");
        }
        let resolved = ids
            .iter()
            .map(|id| resolve_doc(corpus, id))
            .collect::<Result<Vec<_>, _>>()?;
        if batch >= resolved.len() {
            return err("batchIndex 超出批量范围；请重新读取");
        }
        Some(resolved)
    } else {
        None
    };
    let batch_id = batch_docs.as_ref().map(|ds| corpus.doc_id(ds[batch]));
    let id = batch_id.as_deref().unwrap_or(&args.doc_id).trim();
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

    let selector = if let Some(origin) = args.origin_selector.as_deref() {
        if batch_docs.is_some() {
            return err("originSelector 不适用于批量读取；请重新读取");
        }
        let origin = origin.trim();
        let origin_doc = if origin.starts_with("doc_") {
            resolve_doc(corpus, origin)?
        } else {
            let g = resolve_group(corpus, origin)?;
            match corpus.groups[g].docs.first() {
                Some(&d) => d as usize,
                None => return err(format!("分组「{origin}」是空的")),
            }
        };
        if corpus.docs[origin_doc].group != corpus.docs[doc].group
            || (!follow && origin_doc != doc)
            || corpus.docs[origin_doc].group_pos > corpus.docs[doc].group_pos
        {
            return err("续读原始选择器与当前文档范围不一致；请重新读取");
        }
        origin
    } else {
        id
    };
    let release = super::releases::lookup(selector)?;
    let mut group_ids = if let Some(ds) = &batch_docs {
        ds.iter()
            .map(|&d| corpus.groups[corpus.docs[d].group as usize].id.clone())
            .collect::<Vec<_>>()
    } else {
        vec![corpus.groups[corpus.docs[doc].group as usize].id.clone()]
    };
    group_ids.sort();
    group_ids.dedup();
    let range_info = RangeInfo::new(
        if batch_docs.is_some() {
            None
        } else {
            Some(selector)
        },
        group_ids,
        release,
        if batch_docs.is_some() {
            "batch"
        } else if selector.starts_with("doc_") {
            "document"
        } else {
            "catalog"
        },
    );
    let mut snapshot = Snapshot::new(corpus).text();
    // compact 逐篇摘要省略 unknown 的顺序字段时，在快照头说明一次。头部在读取前就要定稿（计入字数预算），
    // 所以按事先已知的篇目判断：批量读取看整批，其余看起始篇（同组顺读的提取状态一致）。
    let known_docs: Vec<usize> = batch_docs.clone().unwrap_or_else(|| vec![doc]);
    if !full
        && known_docs
            .iter()
            .any(|&d| corpus.doc_summaries[d].order.has_unlisted_unknown())
    {
        snapshot.push_str(super::response::UNLISTED_ORDER_NOTE);
    }
    let mut blocks = Vec::new();
    let mut continuation_at = None;
    let mut next_document = None;
    let mut exhausted_before_doc = false;
    let mut out = String::with_capacity(char_budget * 3 + 1024);
    let mut chars_used = 0usize;
    let mut lines_used = 0usize;
    let mut first_doc = true;
    let mut line_buf = String::new();
    loop {
        let d = &corpus.docs[doc];
        let total = d.line_count as usize;
        let document = Document::new(corpus, doc).with_summary_detail(corpus, doc, full);
        let start = offset;
        let mut end = start - 1;
        let mut source_lines = Vec::new();
        let mut body = String::new();
        let mut body_chars = 0usize;
        let base_header = block_header(
            &document,
            start,
            start,
            first_doc,
            full,
            &snapshot,
            &range_info,
            corpus,
            doc,
            release,
        );
        let header_chars = base_header.chars().count();
        while end < total && lines_used < line_budget {
            let line = d.first_line as usize + end;
            line_buf.clear();
            if numbers {
                let _ = write!(line_buf, "L{} ", end + 1);
            }
            push_line(&mut line_buf, corpus, line, corpus.line_text(line));
            if args.include_sources.unwrap_or(false) || args.include_evidence.unwrap_or(false) {
                super::push_source(&mut line_buf, corpus, line);
            }
            if args.include_evidence.unwrap_or(false) {
                super::push_evidence(&mut line_buf, corpus, line);
            }
            line_buf.push('\n');
            let cost = line_buf.chars().count();
            // The end line appears in the heading and complete-returned-range note.
            // Reuse the static header rather than rebuilding its summaries for every line.
            let end_digits = (end + 1).ilog10() as usize + 1;
            let start_digits = start.ilog10() as usize + 1;
            let candidate_chars =
                chars_used + header_chars + (end_digits - start_digits) * 2 + body_chars + cost;
            // Return at least one complete line per invocation, even if its header/line exceeds the budget.
            if candidate_chars > char_budget && lines_used > 0 {
                break;
            }
            source_lines.push(SourceLine::new(
                corpus,
                line,
                args.include_evidence == Some(true),
            ));
            body.push_str(&line_buf);
            body_chars += cost;
            lines_used += 1;
            end += 1;
        }
        if end < start {
            // The next header and first line did not fit; expose neither an empty block nor a "1-0" range.
            continuation_at = Some((doc, start, batch));
            exhausted_before_doc = true;
            break;
        }
        let header = if end == start {
            base_header
        } else {
            block_header(
                &document,
                start,
                end,
                first_doc,
                full,
                &snapshot,
                &range_info,
                corpus,
                doc,
                release,
            )
        };
        chars_used += header.chars().count() + body_chars;
        out.push_str(&header);
        out.push_str(&body);
        blocks.push(ReadBlock {
            document,
            start,
            end,
            lines: source_lines,
        });
        if end < total {
            continuation_at = Some((doc, end + 1, batch));
            break;
        }
        let next = if let Some(ds) = &batch_docs {
            ds.get(batch + 1).copied()
        } else {
            corpus.next_in_group(doc)
        };
        match next {
            Some(n) if follow || batch_docs.is_some() => {
                if lines_used >= line_budget {
                    continuation_at = Some((n, 1, batch + usize::from(batch_docs.is_some())));
                    exhausted_before_doc = true;
                    break;
                }
                if batch_docs.is_some() {
                    batch += 1;
                }
                doc = n;
                offset = 1;
                first_doc = false;
            }
            Some(n) => {
                let mut value = serde_json::to_value(args)
                    .map_err(|_| super::ToolError("无法生成下一篇参数".into()))?;
                value["cursor"] = json!(null);
                value["docIds"] = json!(null);
                value["batchIndex"] = json!(null);
                value["originSelector"] = json!(null);
                value["docId"] = json!(corpus.doc_id(n));
                value["offset"] = json!(1);
                value["follow"] = json!(false);
                next_document = Some(NextDocument {
                    doc_id: corpus.doc_id(n),
                    title: corpus.docs[n].title.clone(),
                    group_id: corpus.groups[corpus.docs[n].group as usize].id.clone(),
                    relation: "catalogAdjacent".into(),
                    next_read: NextCall::new("ik_read", value),
                });
                break;
            }
            None => break,
        }
    }
    let next_call = if let Some((d, start, index)) = continuation_at {
        let mut value =
            serde_json::to_value(args).map_err(|_| super::ToolError("无法生成续读参数".into()))?;
        value["cursor"] = json!(null);
        value["offset"] = json!(start);
        if batch_docs.is_some() {
            value["batchIndex"] = json!(index);
        } else {
            value["originSelector"] = json!(selector);
            value["docId"] = json!(corpus.doc_id(d));
            value["follow"] = json!(follow);
        }
        Some(cursor::next(corpus, "ik_read", value)?)
    } else {
        None
    };
    let coverage_delta = blocks
        .iter()
        .map(|b| LineRange {
            corpus_revision: corpus.revision.clone(),
            doc_id: b.document.doc_id.clone(),
            start: b.start,
            end: b.end,
            complete: true,
        })
        .collect();
    // 批量读取预算耗尽时，点名本次一行都没读到的篇目，避免调用方误以为整批已读。
    let pending_documents: Vec<String> = match (&batch_docs, continuation_at) {
        (Some(ds), Some((_, start, index))) => {
            let first_unstarted = if start == 1 { index } else { index + 1 };
            ds.iter()
                .skip(first_unstarted)
                .map(|&d| corpus.doc_id(d))
                .collect()
        }
        _ => Vec::new(),
    };
    let result = ReadResult {
        blocks,
        coverage_delta,
        pending_documents,
        complete: next_call.is_none(),
        next_call,
        next_document,
        range_info: Some(range_info),
    };
    if let Some((d, start, _)) = continuation_at {
        if exhausted_before_doc {
            if batch_docs.is_some() {
                let _ = write!(
                    out,
                    "\n\n— 本次批量下一篇：{} 「{}」；",
                    corpus.doc_id(d),
                    title_preview(&corpus.docs[d].title)
                );
            } else {
                let _ = write!(
                    out,
                    "\n\n— 本篇完。下一篇（目录相邻，非剧情后继）：{} 「{}」；",
                    corpus.doc_id(d),
                    title_preview(&corpus.docs[d].title)
                );
            }
        } else {
            let _ = write!(
                out,
                "\n\n— 已达本次{}上限。",
                if lines_used >= line_budget {
                    "行数"
                } else {
                    "字数"
                }
            );
        }
        if let Some(ds) = &batch_docs
            && !result.pending_documents.is_empty()
        {
            let shown: Vec<&str> = result
                .pending_documents
                .iter()
                .take(5)
                .map(String::as_str)
                .collect();
            let more = result.pending_documents.len() - shown.len();
            let _ = write!(
                out,
                "本批 {} 篇已开始 {} 篇，剩余 {} 篇未开始（{}{}）；",
                ds.len(),
                ds.len() - result.pending_documents.len(),
                result.pending_documents.len(),
                shown.join("、"),
                if more > 0 {
                    format!(" 等另 {more} 篇")
                } else {
                    String::new()
                }
            );
        }
        if full && batch_docs.is_none() {
            let _ = write!(
                out,
                "继续：{}",
                continuation(&corpus.doc_id(d), start, args, follow)
            );
        } else if let Some(call) = &result.next_call {
            let _ = write!(out, "继续：{}", call.text());
        }
    } else if let Some(next) = &result.next_document {
        let _ = write!(
            out,
            "\n\n— 本篇完。下一篇（目录相邻，非剧情后继）：{} 「{}」 → {}",
            next.doc_id,
            title_preview(&next.title),
            next.next_read.text()
        );
    } else if batch_docs.is_some() {
        out.push_str("\n\n— 本次批量目标已到末尾；coverageDelta 仅记录本次完整返回行。");
    } else {
        let g = corpus.docs[doc].group as usize;
        let _ = write!(
            out,
            "\n\n— 已到「{}」目录末尾；coverageDelta 仅记录本次完整返回行。",
            corpus.groups[g].label
        );
        // Preserve the detailed presentation's optional navigation without crossing into another chapter automatically.
        if full
            && let Some(ng) = corpus
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
    }
    Ok(ToolOutput::with_rendered_snapshot(
        corpus,
        out,
        ToolData::Read(result),
    ))
}

#[allow(clippy::too_many_arguments)]
fn block_header(
    document: &Document,
    start: usize,
    end: usize,
    first: bool,
    full: bool,
    snapshot: &str,
    range: &RangeInfo,
    corpus: &Corpus,
    doc: usize,
    release: Option<&super::releases::Release>,
) -> String {
    let mut out = String::new();
    if !first {
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "{} {} 「{}」 | 第 {start}-{end} 行 / 共 {} 行",
        if first || !full { "[ik_read]" } else { "##" },
        document.doc_id,
        document.title,
        thousands(document.line_count)
    );
    if first {
        out.push_str(snapshot);
        range.push_text(&mut out);
        if full && let Some(release) = release {
            super::releases::push_note(&mut out, corpus, release);
        }
    }
    let _ = writeln!(out, "{}", document.path);
    if full && corpus.docs[doc].kind != crate::corpus::DocKind::Lore {
        let d = &corpus.docs[doc];
        let _ = writeln!(
            out,
            "顺序来源：{} | 剧情先后：{}。目录位置不代表剧情后继或故事内时间。",
            d.order_source.as_deref().unwrap_or("旧语料未记录"),
            d.chronology_status.as_deref().unwrap_or("unknown")
        );
    }
    document.push_summary(&mut out, full);
    let _ = writeln!(out, "完整返回行：{} {start}-{end}", document.doc_id);
    out
}

fn title_preview(title: &str) -> String {
    let mut value = title.chars().take(80).collect::<String>();
    if title.chars().count() > 80 {
        value.push('…');
    }
    value
}
