//! Coverage accounts for caller-supplied returned rows, never model understanding.
use super::{cursor, err, parse_scope, resolve_doc, resolve_group, response::*};
use crate::corpus::Corpus;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use utoipa::ToSchema;

#[derive(Debug, Clone, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct RangeScope {
    pub path: Option<String>,
    pub scope: Option<String>,
    #[serde(default)]
    pub exclude_placeholders: bool,
    #[serde(default)]
    pub exclude_unlinked: bool,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub manifest_id: String,
    pub resolved_range: RangeScope,
    pub total_documents: usize,
    pub total_lines: usize,
    pub excluded_documents: usize,
    pub excluded_lines: usize,
    pub placeholder_lines: usize,
    pub unlinked_lines: usize,
    pub counts_may_overlap: bool,
}
pub(crate) fn members(
    c: &Corpus,
    scope: &RangeScope,
) -> Result<(Vec<usize>, Vec<usize>), super::ToolError> {
    let kind = parse_scope(scope.scope.as_deref())?;
    let path = scope.path.as_deref().unwrap_or("").trim();
    let selected = if path.starts_with("doc_") {
        vec![resolve_doc(c, path)?]
    } else if path.is_empty() || path == "/" {
        (0..c.docs.len()).collect()
    } else {
        c.groups[resolve_group(c, path)?]
            .docs
            .iter()
            .map(|d| *d as usize)
            .collect()
    };
    Ok(selected
        .into_iter()
        .filter(|&d| kind.is_none_or(|k| k == c.docs[d].kind))
        .partition(|&d| {
            let doc = &c.docs[d];
            !(scope.exclude_placeholders
                && doc.evidence_counts.placeholders == doc.line_count as usize
                || scope.exclude_unlinked
                    && doc.evidence_counts.unlinked == doc.line_count as usize)
        }))
}
pub(crate) fn manifest(c: &Corpus, scope: RangeScope) -> Result<Manifest, super::ToolError> {
    let (docs, excluded) = members(c, &scope)?;
    let scope = RangeScope {
        path: if let Some(&d) = docs.first().or(excluded.first()) {
            if scope.path.as_deref().is_some_and(|p| p.starts_with("doc_")) {
                Some(c.doc_id(d))
            } else if scope
                .path
                .as_deref()
                .is_some_and(|p| !p.is_empty() && p != "/")
            {
                Some(c.groups[c.docs[d].group as usize].id.clone())
            } else {
                None
            }
        } else {
            scope.path.clone()
        },
        ..scope
    };
    let call = cursor::next(
        c,
        "ik_coverage",
        serde_json::to_value(&scope).map_err(|_| super::ToolError("无法生成范围清单".into()))?,
    )?;
    Ok(Manifest {
        manifest_id: call.arguments["cursor"].as_str().unwrap_or_default().into(),
        resolved_range: scope,
        total_documents: docs.len(),
        total_lines: docs.iter().map(|&d| c.docs[d].line_count as usize).sum(),
        excluded_documents: excluded.len(),
        excluded_lines: excluded
            .iter()
            .map(|&d| c.docs[d].line_count as usize)
            .sum(),
        placeholder_lines: docs
            .iter()
            .map(|&d| c.docs[d].evidence_counts.placeholders)
            .sum(),
        unlinked_lines: docs
            .iter()
            .map(|&d| c.docs[d].evidence_counts.unlinked)
            .sum(),
        counts_may_overlap: true,
    })
}

/// ik_coverage 的输入区间。read.coverageDelta 的记录可以原样提交；也可以只填 docId/start/end：
/// 语料快照由 manifestId 决定，complete 缺省视为 true（coverageDelta 只记录完整返回行）。
/// 显式提交的 corpusRevision 与当前快照不一致时仍然报错。
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RangeInput {
    #[serde(default)]
    pub corpus_revision: Option<String>,
    pub doc_id: String,
    pub start: usize,
    pub end: usize,
    #[serde(default)]
    pub complete: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoverageArgs {
    #[serde(default)]
    pub manifest_id: String,
    #[serde(default)]
    pub ranges: Vec<RangeInput>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub offset: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub max_results: Option<usize>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PendingDocument {
    pub document: Document,
    pub covered_lines: usize,
    /// Missing intervals are not returned-row receipts.
    pub missing: Vec<MissingRange>,
    pub next_read: NextCall,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MissingRange {
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CoverageResult {
    pub manifest: Manifest,
    pub accounting_basis: String,
    pub covered_lines: usize,
    pub complete_documents: usize,
    pub partial_documents: usize,
    pub unread_documents: usize,
    pub ledger: Vec<LineRange>,
    pub pending: Vec<PendingDocument>,
    pub next_call: Option<NextCall>,
}
pub fn execute(c: &Corpus, args: &CoverageArgs) -> OutputResult {
    if args.manifest_id.is_empty() {
        return err("manifestId 不能为空；先调用 ik_catalog(path=…, manifest=true)");
    }
    if args.ranges.len() > 512 {
        return err("ranges 最多512个区间；请先合并重叠记录，或按章节拆分范围");
    }
    let scope: RangeScope = serde_json::from_value(cursor::resolve(
        c,
        "ik_coverage",
        json!({"cursor":args.manifest_id}),
    )?)
    .map_err(|_| super::ToolError("manifestId 无效；请重新获取目录清单".into()))?;
    let manifest = manifest(c, scope.clone())?;
    let (members, _) = members(c, &scope)?;
    let mut rows: BTreeMap<usize, Vec<(usize, usize)>> = BTreeMap::new();
    for r in &args.ranges {
        if r.corpus_revision
            .as_ref()
            .is_some_and(|rev| *rev != c.revision)
        {
            return err("记录与当前语料快照不一致；请重新获取清单并读取原文");
        }
        let d = resolve_doc(c, &r.doc_id)?;
        if !members.contains(&d) {
            return err(format!(
                "{} 不在清单的有效范围中；请使用对应范围的清单",
                r.doc_id
            ));
        }
        if r.start == 0 || r.start > r.end || r.end > c.docs[d].line_count as usize {
            return err(format!(
                "{} 的区间越界；需满足 1 ≤ start ≤ end ≤ 文档行数",
                r.doc_id
            ));
        }
        if r.complete.unwrap_or(true) {
            rows.entry(d).or_default().push((r.start, r.end));
        }
    }
    let mut ledger = Vec::new();
    let mut pending = Vec::new();
    let offset = args.offset.unwrap_or(0);
    let max = args.max_results.unwrap_or(20).clamp(1, 100);
    let mut pending_count = 0;
    let (mut covered, mut complete, mut partial, mut unread) = (0, 0, 0, 0);
    for &d in &members {
        let mut ranges = rows.remove(&d).unwrap_or_default();
        ranges.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (a, b) in ranges {
            if let Some(last) = merged
                .last_mut()
                .filter(|last| a <= last.1.saturating_add(1))
            {
                last.1 = last.1.max(b);
            } else {
                merged.push((a, b));
            }
        }
        let count: usize = merged.iter().map(|&(a, b)| b - a + 1).sum();
        covered += count;
        let range = |a, b| LineRange {
            corpus_revision: c.revision.clone(),
            doc_id: c.doc_id(d),
            start: a,
            end: b,
            complete: true,
        };
        ledger.extend(merged.iter().map(|&(a, b)| range(a, b)));
        let total = c.docs[d].line_count as usize;
        if count == total {
            complete += 1;
            continue;
        }
        if count == 0 {
            unread += 1;
        } else {
            partial += 1;
        }
        pending_count += 1;
        if pending_count <= offset || pending_count > offset.saturating_add(max) {
            continue;
        }
        let mut gaps = Vec::new();
        let mut start = 1;
        for (a, b) in merged {
            if start < a {
                gaps.push(MissingRange { start, end: a - 1 });
            }
            start = b + 1;
        }
        if start <= total {
            gaps.push(MissingRange { start, end: total });
        }
        let next_read = NextCall::new(
            "ik_read",
            json!({"docId":c.doc_id(d),"offset":gaps[0].start,"limit":(gaps[0].end-gaps[0].start+1).min(5000)}),
        );
        pending.push(PendingDocument {
            document: Document::new(c, d),
            covered_lines: count,
            missing: gaps,
            next_read,
        });
    }
    let more = offset.saturating_add(max) < pending_count;
    // 续页只需重交合并后的区间；用 docId/start/end 短格式，避免每段重复 corpusRevision。
    let next_call = more.then(|| {
        let ranges: Vec<_> = ledger
            .iter()
            .map(|r| json!({"docId": r.doc_id, "start": r.start, "end": r.end}))
            .collect();
        NextCall::new(
            "ik_coverage",
            json!({"manifestId": args.manifest_id, "ranges": ranges, "offset": offset.saturating_add(max), "maxResults": max}),
        )
    });
    let result = CoverageResult {
        manifest,
        accounting_basis: "调用者提交记录所描述的完整返回行；不证明模型已理解或服务器历史请求"
            .into(),
        covered_lines: covered,
        complete_documents: complete,
        partial_documents: partial,
        unread_documents: unread,
        ledger,
        pending,
        next_call,
    };
    let mut text = format!(
        "[ik_coverage] 已返回 {covered}/{} 行 | 完整 {complete} 篇 · 部分 {partial} 篇 · 未读 {unread} 篇 | 显式排除 {} 篇\n{}\n",
        result.manifest.total_lines, result.manifest.excluded_documents, result.accounting_basis
    );
    for p in &result.pending {
        let _ = writeln!(
            text,
            "{} 「{}」 已返回 {}/{} 行 → {}",
            p.document.doc_id,
            p.document.title,
            p.covered_lines,
            p.document.line_count,
            p.next_read.text()
        );
    }
    if let Some(call) = &result.next_call {
        let _ = writeln!(text, "继续核对：{}", call.text());
    }
    Ok(ToolOutput::new(c, text, ToolData::Coverage(result)))
}
