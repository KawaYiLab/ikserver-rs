//! 工具的业务逻辑：输入是语料与已解析的参数，输出是给模型读的紧凑文本。
//!
//! 与协议层完全解耦，命令行 `call` 子命令与测试直接调用这里。

pub mod catalog;
pub mod character;
pub mod guide;
pub mod read;
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
        _ => {
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

/// 出场名单（最多 `max` 个）。
pub(crate) fn speakers_preview(corpus: &Corpus, doc: usize, max: usize) -> String {
    let ids = corpus.doc_speakers(&corpus.docs[doc]);
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
    corpus.find_group(path).ok_or_else(|| {
        ToolError(format!(
            "找不到分组「{path}」。用 ik_catalog() 查看全部章节（如 ch:Chapter110）与设定分类（如 lore:agents）"
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
