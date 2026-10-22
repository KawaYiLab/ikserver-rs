//! Reviewed release aliases for newly added main-story chapters.
//!
//! These are release labels, not the version of every line in a snapshot.
//! Keep the mapping explicit: season/chapter numbers do not determine releases.

use std::fmt::Write as _;

use super::{ToolError, err};
use crate::corpus::Corpus;
use crate::text::{fold, query_terms};

pub(crate) struct Release {
    pub version: &'static str,
    pub group: &'static str,
}

// Verified by the extractor's 3.2/3.3 snapshot comparison; see
// docs/story-search.md. Do not extrapolate unreviewed versions.
const RELEASES: &[Release] = &[
    Release {
        version: "3.2",
        group: "ch:Chapter230",
    },
    Release {
        version: "3.3",
        group: "ch:Chapter240",
    },
];

pub(crate) fn revision_data() -> Vec<(&'static str, &'static str)> {
    RELEASES.iter().map(|r| (r.version, r.group)).collect()
}

/// Recognize only a whole version expression. Percentages, decimal values in
/// phrases and arbitrary substrings remain literal search terms.
fn version_expression(query: &str) -> Option<String> {
    parse_version(query).map(|(version, _)| version)
}

/// 解析版本表达式，并返回是否带有显式版本标记（版本 / 版 / version / v）。
/// 无标记的裸小数（如「1.5」）在自由文本里可能只是普通数字。
fn parse_version(query: &str) -> Option<(String, bool)> {
    let folded = fold(query);
    let mut value = folded.trim();
    let mut marked = false;
    for prefix in ["版本", "version "] {
        if let Some(rest) = value.strip_prefix(prefix) {
            value = rest.trim();
            marked = true;
            break;
        }
    }
    for suffix in ["版本", "版", " version"] {
        if let Some(rest) = value.strip_suffix(suffix) {
            value = rest.trim();
            marked = true;
            break;
        }
    }
    if let Some(rest) = value.strip_prefix('v') {
        value = rest;
        marked = true;
    }
    let parts: Vec<&str> = value.split('.').collect();
    if !(2..=3).contains(&parts.len())
        || parts
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    // Only the reviewed x.y / x.y.0 expressions are aliases. An unknown patch
    // retains its full version so it produces an explicit unmapped result.
    let version = if parts.len() == 3 && parts[2] == "0" {
        format!("{}.{}", parts[0], parts[1])
    } else {
        value.to_owned()
    };
    Some((version, marked))
}

pub(crate) fn lookup(query: &str) -> Result<Option<&'static Release>, ToolError> {
    let Some(version) = version_expression(query) else {
        return Ok(None);
    };
    RELEASES.iter().find(|r| r.version == version).map(Some).ok_or_else(|| {
        ToolError(format!("版本「{query}」没有已确认的主线章节映射。请用 ik_catalog() 查看章节，并传入章节名或精确 ch:… id；不会按版本周期猜测。"))
    })
}

/// 自由文本（检索关键词、目录标题筛选）里的版本识别：已核验版本照常作为选择器；
/// 未核验的裸小数当作普通文本返回 None，显式标注的未核验版本仍然报错。
pub(crate) fn lookup_in_text(query: &str) -> Result<Option<&'static Release>, ToolError> {
    match parse_version(query) {
        Some((version, false)) if !RELEASES.iter().any(|r| r.version == version) => Ok(None),
        _ => lookup(query),
    }
}

pub(crate) fn for_group(group: &str) -> Option<&'static Release> {
    RELEASES.iter().find(|r| r.group == group)
}

pub(crate) fn resolve(corpus: &Corpus, release: &Release) -> Result<usize, ToolError> {
    corpus.find_group(release.group).ok_or_else(|| {
        ToolError(format!("版本 {} 对应的新增主线 {} 在当前语料中不可用。当前语料版本：{}；请用 ik_catalog() 查看已收录章节。", release.version, release.group, corpus.meta.source_version))
    })
}

pub(crate) fn push_note(out: &mut String, corpus: &Corpus, release: &Release) {
    let label = corpus
        .find_group(release.group)
        .map(|g| corpus.groups[g].label.as_str())
        .unwrap_or("");
    let _ = writeln!(
        out,
        "版本别名：{} → {} {}（该版新增主线；语料快照版本 {} 包含历史章节）。",
        release.version, release.group, label, corpus.meta.source_version
    );
}

/// Extract version selectors before applying the text-term limit. A version
/// remains a hard filter even when partial text matching is enabled.
pub(crate) fn query(keyword: &str) -> Result<(Vec<String>, Option<&'static Release>), ToolError> {
    let input = query_terms(keyword);
    let mut terms = Vec::new();
    let mut selected: Option<&Release> = None;
    let mut i = 0;
    while i < input.len() {
        let mut consumed = 1;
        let mut expression = input[i].as_str();
        let combined;
        if matches!(expression, "版本" | "version") && i + 1 < input.len() {
            combined = format!("{expression} {}", input[i + 1]);
            if version_expression(&combined).is_some() {
                expression = &combined;
                consumed = 2;
            }
        } else if i + 1 < input.len() && matches!(input[i + 1].as_str(), "版本" | "版" | "version")
        {
            combined = format!("{} {}", expression, input[i + 1]);
            if version_expression(&combined).is_some() {
                expression = &combined;
                consumed = 2;
            }
        }
        if let Some(release) = lookup_in_text(expression)? {
            if selected.is_some_and(|r| r.group != release.group) {
                return err(
                    "一次检索只能指定一个版本章节范围；请分别查询，或使用 ik_catalog() 浏览。",
                );
            }
            selected = Some(release);
        } else {
            terms.push(input[i].clone());
        }
        i += consumed;
    }
    Ok((terms, selected))
}
