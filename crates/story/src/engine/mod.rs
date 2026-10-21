//! 工具的业务逻辑：输入是语料与已解析的参数，输出类型化结果与兼容文本。
//!
//! 与协议层完全解耦，命令行 `call` 子命令与测试直接调用这里。

pub mod catalog;
pub mod character;
pub mod coverage;
pub(crate) mod cursor;
pub mod guide;
pub mod read;
pub(crate) mod releases;
pub mod response;
pub mod search;

use std::fmt::Write as _;

use crate::corpus::{Corpus, DocKind, LineKind};
use crate::text::{char_amount, thousands};

/// 工具执行失败：文本会以 `isError: true` 返回给模型，让它据此修正参数。
#[derive(Debug)]
pub struct ToolError(pub String);

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type ToolResult = Result<String, ToolError>;

pub(crate) fn err<T>(msg: impl Into<String>) -> Result<T, ToolError> {
    Err(ToolError(msg.into()))
}

/// 一行的展示形式：`说话人：台词`、`【选项】…`、旁白原样。
pub(crate) fn push_line(out: &mut String, corpus: &Corpus, line: usize, text: &str) {
    let l = &corpus.lines[line];
    match l.kind {
        LineKind::Option => {
            out.push_str("【选项】");
        }
        LineKind::Placeholder => out.push_str("【占位说明】"),
        LineKind::Unknown => out.push_str("【署名未知】"),
        _ => {
            if corpus.is_inferred(line) {
                out.push_str("【推断署名】");
            } else if corpus.speaker_name(l.speaker).is_some()
                && corpus
                    .line_evidence(line)
                    .speaker_status
                    .as_deref()
                    .is_some_and(|s| s != "resolved")
            {
                out.push_str("【署名未核验】");
            }
            if let Some(name) = corpus.speaker_name(l.speaker) {
                out.push_str(name);
                out.push('：');
            }
        }
    }
    out.push_str(text);
}

/// 文档摘要行：`doc_xxx 「标题」 路径 | N句 · 字数`。
pub(crate) fn push_doc_summary(out: &mut String, corpus: &Corpus, doc: usize) {
    let d = &corpus.docs[doc];
    let unit = if d.kind == DocKind::Lore {
        "行"
    } else {
        "句"
    };
    let _ = write!(
        out,
        "{} 「{}」 {} | {}{} · {}",
        corpus.doc_id(doc),
        d.title,
        d.path,
        thousands(d.line_count as usize),
        unit,
        char_amount(d.char_count as usize)
    );
}

pub(crate) fn push_version(out: &mut String, corpus: &Corpus) {
    let _ = writeln!(
        out,
        "语料版本：{} {} | 提取时间：{}",
        corpus.meta.source_version, corpus.meta.language, corpus.meta.generated_at
    );
    let mode = match corpus.meta.accuracy_first {
        Some(true) => "完整性校对（双方分支候选保留）",
        Some(false) => "默认哲视角；不确定分支保留，非严格单一路线",
        None => "旧语料未声明",
    };
    let _ = writeln!(out, "提取模式：{mode}");
    let _ = writeln!(out, "corpusRevision：{}", corpus.revision);
    out.push_str("证据边界：姓名解析不确认播放；未找到配音不代表旁白。局部行序与跨场景顺序分别核验，目录相邻不证明执行关系。\n");
}

pub(crate) fn push_order_note(out: &mut String, corpus: &Corpus, doc: usize) {
    let d = &corpus.docs[doc];
    if d.kind == DocKind::Lore {
        return;
    }
    let _ = writeln!(
        out,
        "顺序来源：{} | 剧情先后：{}。目录位置不代表剧情后继或故事内时间。",
        d.order_source.as_deref().unwrap_or("旧语料未记录"),
        d.chronology_status.as_deref().unwrap_or("unknown")
    );
    if d.section == "Comic" && d.context_status.as_deref() != Some("linked") {
        out.push_str("漫画片段尚未关联完整场景；没有说话人和时间信息时不要补写，目录前后片段也不构成上下文。\n");
    }
    push_evidence_note(out, corpus, doc);
}

pub(crate) fn push_evidence_note(out: &mut String, corpus: &Corpus, doc: usize) {
    let d = &corpus.docs[doc];
    if d.kind == DocKind::Lore {
        return;
    }
    let counts = &d.evidence_counts;
    let mut notes = Vec::new();
    for (label, count) in [
        ("占位说明", counts.placeholders),
        ("署名未知", counts.unknown),
        ("未关联候选", counts.unlinked),
        ("推断署名", counts.inferred),
        ("未提供署名核验证据", counts.missing),
        ("未找到配音关联", counts.no_voice),
    ] {
        if count > 0 {
            notes.push(format!("{label} {count}"));
        }
    }
    if !notes.is_empty() {
        let _ = writeln!(out, "证据状态：{}（统计可交叉）。", notes.join(" · "));
    }
    let names = names_preview(corpus, &d.name_table, 12);
    if !names.is_empty() {
        let _ = writeln!(
            out,
            "TextMap 姓名表：{names}（只表示姓名关联，不是逐句署名或首次登场证据）。"
        );
    }
}

pub(crate) fn push_evidence(out: &mut String, corpus: &Corpus, line: usize) {
    let evidence = corpus.line_evidence(line);
    if !evidence.provided() {
        out.push_str(" [提取证据：未提供证据]");
    } else if let Ok(json) = serde_json::to_string(evidence) {
        let _ = write!(out, " [提取证据：{json}]");
    }
}

pub(crate) fn push_doc_evidence(out: &mut String, corpus: &Corpus, doc: usize) {
    for line in corpus.docs[doc].lines().take(20) {
        let _ = write!(out, "L{}", line - corpus.docs[doc].first_line as usize + 1);
        push_source(out, corpus, line);
        push_evidence(out, corpus, line);
        out.push('\n');
    }
    if corpus.docs[doc].line_count > 20 {
        let _ = writeln!(
            out,
            "证据仅展开前 20 行；其余用 ik_read(docId=\"{}\", offset=21, includeEvidence=true)。",
            corpus.doc_id(doc)
        );
    }
}

pub(crate) fn push_source(out: &mut String, corpus: &Corpus, line: usize) {
    let source = &corpus.line_sources[line];
    if let Some(key) = &source.source_key {
        let _ = write!(out, " [sourceKey={key}]");
    }
    if let Some(id) = &source.line_id {
        let _ = write!(out, " [lineId={id}]");
    }
}

/// 出场名单（最多 `max` 个）。
pub(crate) fn speakers_preview(corpus: &Corpus, doc: usize, max: usize) -> String {
    names_preview(corpus, corpus.doc_speakers(&corpus.docs[doc]), max)
}

fn names_preview(corpus: &Corpus, ids: &[u32], max: usize) -> String {
    let mut names: Vec<&str> = ids
        .iter()
        .take(max)
        .filter_map(|&s| corpus.speaker_name(s))
        .collect();
    if ids.len() > max {
        names.push("…");
    }
    names.join("、")
}

/// 解析检索范围参数。
pub(crate) fn parse_scope(scope: Option<&str>) -> Result<Option<DocKind>, ToolError> {
    match scope.map(str::trim).unwrap_or("") {
        "" | "all" | "全部" => Ok(None),
        "story" | "main" | "主线" | "剧情" => Ok(Some(DocKind::Story)),
        "supplement" | "补充" | "过场" | "漫画" => Ok(Some(DocKind::Supplement)),
        "lore" | "设定" | "世界观" => Ok(Some(DocKind::Lore)),
        other => err(format!(
            "scope 只能是 all / story / supplement / lore，收到「{other}」"
        )),
    }
}

/// 解析 `path`：分组 id / 章节 token / 章节名。
pub(crate) fn resolve_group(corpus: &Corpus, path: &str) -> Result<usize, ToolError> {
    if let Some(release) = releases::lookup(path)? {
        return releases::resolve(corpus, release);
    }
    corpus.find_group(path).ok_or_else(|| {
        ToolError(format!(
            "找不到分组「{path}」或名称有歧义。用 ik_catalog() 查看全部章节，并使用精确 id（如 ch:Chapter110 / lore:agents）"
        ))
    })
}

pub(crate) fn resolve_doc(corpus: &Corpus, id: &str) -> Result<usize, ToolError> {
    corpus.find_doc(id).ok_or_else(|| {
        ToolError(format!(
            "找不到文档「{id}」。docId 形如 doc_1a2b3c4d5e6f，必须原样使用 ik_search / ik_catalog 返回的值"
        ))
    })
}

/// 结果上限的统一钳制。
pub(crate) fn clamp(value: Option<usize>, default: usize, max: usize) -> usize {
    value.unwrap_or(default).clamp(1, max)
}
