//! Typed tool results are the source for both text and MCP structuredContent.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt::Write as _;
use utoipa::ToSchema;

use super::coverage::{CoverageResult, Manifest, MissingRange, PendingDocument, RangeScope};
use super::{ToolError, err};
use crate::corpus::{Corpus, DocKind, LineKind};
pub use crate::corpus::{DocumentOrder, EvidenceSummary, OrderCount, SpeakerCoverage};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct NextCall {
    pub tool: String,
    pub arguments: Value,
}
impl NextCall {
    pub fn new(tool: &str, mut arguments: Value) -> Self {
        if let Some(map) = arguments.as_object_mut() {
            map.retain(|_, v| !v.is_null());
        }
        Self {
            tool: tool.into(),
            arguments,
        }
    }
    pub fn text(&self) -> String {
        let preferred = [
            "docId",
            "offset",
            "follow",
            "limit",
            "maxChars",
            "lineNumbers",
            "includeSources",
            "includeEvidence",
        ];
        let fields = self
            .arguments
            .as_object()
            .map(|a| {
                let mut keys = a.keys().collect::<Vec<_>>();
                keys.sort_by_key(|k| {
                    (
                        preferred
                            .iter()
                            .position(|p| p == k)
                            .unwrap_or(preferred.len()),
                        *k,
                    )
                });
                keys.into_iter()
                    .map(|k| format!("{k}={}", a[k]))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        format!("{}({fields})", self.tool)
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub schema_version: u32,
    pub corpus_revision: String,
    pub snapshot_version: String,
    pub extraction_mode: String,
}
impl Snapshot {
    pub fn text(&self) -> String {
        format!(
            "语料={} | 模式={} | corpusRevision={}\n",
            if self.snapshot_version.is_empty() {
                "未声明"
            } else {
                &self.snapshot_version
            },
            self.extraction_mode,
            self.corpus_revision
        )
    }
    pub fn new(c: &Corpus) -> Self {
        Self {
            schema_version: 1,
            corpus_revision: c.revision.clone(),
            snapshot_version: c.meta.source_version.clone(),
            extraction_mode: match c.meta.accuracy_first {
                Some(true) => "integrity",
                Some(false) => "default",
                None => "legacy",
            }
            .into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    pub doc_id: String,
    pub title: String,
    pub path: String,
    pub group_id: String,
    pub content_type: String,
    pub line_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence_hints: Vec<String>,
    pub next_read: NextCall,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub evidence_preview: Vec<SourceLine>,
    pub evidence_preview_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<DocumentOrder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_summary: Option<EvidenceSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_summary: Option<SpeakerSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_coverage: Option<SpeakerCoverage>,
    #[serde(skip)]
    compact_summary: std::sync::Arc<str>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerSummary {
    pub names: Vec<String>,
    pub total_speakers: usize,
    pub names_truncated: bool,
    pub inferred_names: Vec<String>,
    pub total_inferred_speakers: usize,
    pub inferred_names_truncated: bool,
    pub legacy_evidence_missing: bool,
}
impl Document {
    pub fn new(c: &Corpus, d: usize) -> Self {
        let doc = &c.docs[d];
        let n = &doc.evidence_counts;
        let mut hints = Vec::new();
        for (label, count) in [
            ("占位", n.placeholders),
            ("署名未知", n.unknown),
            ("未关联候选", n.unlinked),
            ("推断署名", n.inferred),
            ("未提供署名核验证据", n.missing),
        ] {
            if count > 0 {
                hints.push(format!("{label} {count}"));
            }
        }
        Self {
            doc_id: c.doc_id(d),
            title: doc.title.clone(),
            path: doc.path.clone(),
            group_id: c.groups[doc.group as usize].id.clone(),
            content_type: match doc.kind {
                DocKind::Story => "story",
                DocKind::Supplement => "supplement",
                DocKind::Lore => "lore",
            }
            .into(),
            line_count: doc.line_count as usize,
            evidence_hints: hints,
            next_read: NextCall::new("ik_read", json!({"docId":c.doc_id(d)})),
            evidence_preview: Vec::new(),
            evidence_preview_truncated: false,
            order: Some(c.doc_summaries[d].order.clone()),
            evidence_summary: Some(c.doc_summaries[d].evidence.clone()),
            speaker_summary: Some(Self::speakers(c, d, 6)),
            speaker_coverage: Some(c.doc_summaries[d].coverage.clone()),
            compact_summary: c.doc_summaries[d].compact_text.clone(),
        }
    }
    fn speakers(c: &Corpus, d: usize, max: usize) -> SpeakerSummary {
        let doc = &c.docs[d];
        let speakers = c.doc_speakers(doc);
        let names = |ids: &[u32]| {
            ids.iter()
                .take(max)
                .filter_map(|&s| c.speaker_name(s).map(str::to_owned))
                .collect()
        };
        SpeakerSummary {
            names: names(speakers),
            total_speakers: speakers.len(),
            names_truncated: speakers.len() > max,
            inferred_names: names(&doc.inferred_speakers),
            total_inferred_speakers: doc.inferred_speakers.len(),
            inferred_names_truncated: doc.inferred_speakers.len() > max,
            legacy_evidence_missing: c.doc_summaries[d].coverage.legacy_named_lines > 0,
        }
    }
    pub fn with_summary_detail(mut self, c: &Corpus, d: usize, full: bool) -> Self {
        if full {
            self.speaker_summary = Some(Self::speakers(c, d, usize::MAX));
        }
        self
    }
    /// Summary describes the entire document; it must not imply playback or definite appearances.
    pub fn push_summary(&self, out: &mut String, full: bool) {
        if !full {
            out.push_str(&self.compact_summary);
            return;
        }
        if let Some(order) = &self.order {
            if !order.applicable {
                out.push_str("顺序与署名覆盖：不适用（设定）。\n");
                return;
            }
            let counts = |values: &[OrderCount]| {
                values
                    .iter()
                    .map(|v| {
                        format!(
                            "{}:{}",
                            if v.value == "notProvided" {
                                "未提供"
                            } else {
                                &v.value
                            },
                            v.line_count
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("/")
            };
            let _ = writeln!(
                out,
                "整篇：篇序={} 行序={} 局部={} 剧情先后={} 上下文={}",
                order.doc_order_source,
                counts(&order.line_order_sources),
                counts(&order.line_order_statuses),
                order.chronology_status,
                order.context_status
            );
        }
        if let Some(e) = &self.evidence_summary {
            let hints = [
                ("占位", e.placeholders),
                ("未知署名", e.unknown),
                ("推断", e.inferred),
                ("缺署名证据", e.missing),
                ("未关联", e.unlinked),
                ("无配音关联", e.no_voice),
            ]
            .into_iter()
            .filter(|(_, n)| *n > 0)
            .map(|(label, n)| format!("{label}{n}"))
            .collect::<Vec<_>>();
            if !hints.is_empty() {
                let _ = writeln!(out, "缺口可交叉：{}", hints.join("/"));
            }
        }
        if let Some(c) = &self.speaker_coverage {
            let _ = writeln!(out, "整篇{}", c.text());
        }
        if let Some(s) = &self.speaker_summary {
            if !s.names.is_empty() {
                let _ = writeln!(out, "逐句署名：{}", s.names.join("、"));
            }
            if !s.inferred_names.is_empty() {
                let _ = writeln!(out, "推断署名：{}", s.inferred_names.join("、"));
            }
        }
    }
    pub fn with_evidence(mut self, c: &Corpus, d: usize, include: bool) -> Self {
        if include {
            self.evidence_preview_truncated = c.docs[d].line_count > 20;
            self.evidence_preview = c.docs[d]
                .lines()
                .take(20)
                .map(|l| SourceLine::new(c, l, true))
                .collect();
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceLine {
    pub number: usize,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
    pub matched_terms: Vec<String>,
    pub is_context: bool,
    pub complete: bool,
}
impl SourceLine {
    pub fn new(c: &Corpus, line: usize, include_evidence: bool) -> Self {
        let d = &c.docs[c.doc_of_line(line as u32)];
        let l = &c.lines[line];
        let evidence = c.line_evidence(line);
        Self {
            number: line - d.first_line as usize + 1,
            text: c.line_text(line).into(),
            speaker: c.speaker_name(l.speaker).map(str::to_owned),
            kind: match l.kind {
                LineKind::Dialogue => "dialogue",
                LineKind::Option => "option",
                LineKind::Narration => "narration",
                LineKind::Unknown => "unknown",
                LineKind::Text => "text",
                LineKind::Placeholder => "placeholder",
            }
            .into(),
            source_key: c.line_sources[line].source_key.clone(),
            line_id: c.line_sources[line].line_id.clone(),
            status: evidence.speaker_status.as_deref().map(str::to_owned),
            evidence: if include_evidence {
                serde_json::to_value(evidence).ok()
            } else {
                None
            },
            matched_terms: Vec::new(),
            is_context: false,
            complete: true,
        }
    }
    pub fn push_sources(&self, out: &mut String) {
        if let Some(k) = &self.source_key {
            let _ = write!(out, " [sourceKey={k}]");
        }
        if let Some(k) = &self.line_id {
            let _ = write!(out, " [lineId={k}]");
        }
    }
    pub fn display(&self) -> String {
        let prefix = match self.kind.as_str() {
            "placeholder" => "【占位说明】",
            "option" => "【选项】",
            "unknown" => "【署名未知】",
            _ if self.status.as_deref() == Some("inferred") => "【推断署名】",
            _ if self.speaker.is_some()
                && self.status.as_deref().is_some_and(|s| s != "resolved") =>
            {
                "【署名未核验】"
            }
            _ => "",
        };
        let speaker = if matches!(self.kind.as_str(), "placeholder" | "option" | "unknown") {
            String::new()
        } else {
            self.speaker
                .as_ref()
                .map(|s| format!("{s}："))
                .unwrap_or_default()
        };
        format!("{prefix}{speaker}{}", self.text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LineRange {
    pub corpus_revision: String,
    pub doc_id: String,
    pub start: usize,
    pub end: usize,
    pub complete: bool,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermMatch {
    pub term: String,
    pub fields: Vec<String>,
    pub lines: Vec<usize>,
    pub lines_truncated: bool,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub start: usize,
    pub end: usize,
    pub order_source: String,
    pub order_status: String,
    pub local_order_status: Vec<String>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchRange {
    pub scope: String,
    pub group_id: Option<String>,
    pub doc_id: Option<String>,
    pub speaker: Option<String>,
    pub match_in: String,
    pub match_unit: String,
    pub window_lines: usize,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RangeInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    pub group_ids: Vec<String>,
    pub selection_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_version: Option<String>,
}
impl RangeInfo {
    pub(crate) fn new(
        selector: Option<&str>,
        group_ids: Vec<String>,
        release: Option<&super::releases::Release>,
        mode: &str,
    ) -> Self {
        Self {
            selector: selector.filter(|s| !s.is_empty()).map(str::to_owned),
            group_ids,
            selection_mode: if release.is_some() {
                "addedMainStory"
            } else {
                mode
            }
            .into(),
            release_version: release.map(|r| r.version.into()),
        }
    }
    pub fn push_text(&self, out: &mut String) {
        if let Some(release) = &self.release_version {
            let _ = writeln!(
                out,
                "范围={release}新增主线 → {}",
                self.group_ids.join(", ")
            );
        } else if !self.group_ids.is_empty() {
            let _ = writeln!(
                out,
                "范围={}（{}）",
                self.group_ids.join(", "),
                self.selection_mode
            );
        }
    }
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAlias {
    pub release_version: String,
    pub group_id: String,
    pub available: bool,
    pub next_catalog: NextCall,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchItem {
    pub document: Document,
    pub result_type: String,
    pub matches: Vec<TermMatch>,
    pub missing_terms: Vec<String>,
    pub body_missing_terms: Vec<String>,
    pub lines: Vec<SourceLine>,
    pub hit_count: usize,
    pub windows: Vec<Window>,
    pub windows_truncated: bool,
    pub next_matches: Option<NextCall>,
    pub related_status: String,
}
/// 多词检索时各词在当前范围内的命中篇数。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermDocuments {
    pub term: String,
    pub documents: usize,
}
/// 当前范围内零命中的检索词在全库（不带范围筛选、同一 matchIn）的命中篇数。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TermCount {
    pub term: String,
    pub corpus_documents: usize,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_info: Option<RangeInfo>,
    pub query: String,
    pub resolved_range: SearchRange,
    pub partial: bool,
    /// 默认未合并为部分匹配（有词零命中或没有任何两词同篇），原因与建议调用见页眉与 suggestions。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub partial_withheld: bool,
    pub candidate_count: usize,
    pub body_result_count: usize,
    pub directory_count: usize,
    pub hit_count: usize,
    pub missing_terms: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub missing_term_counts: Vec<TermCount>,
    /// 多词检索时各词在当前范围内的命中篇数（单词检索时省略）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub term_documents: Vec<TermDocuments>,
    pub results: Vec<SearchItem>,
    pub directory_suggestions: Vec<SearchItem>,
    pub next_call: Option<NextCall>,
    pub next_directory_call: Option<NextCall>,
    pub suggestions: Vec<NextCall>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadBlock {
    pub document: Document,
    pub start: usize,
    pub end: usize,
    pub lines: Vec<SourceLine>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NextDocument {
    pub doc_id: String,
    pub title: String,
    pub group_id: String,
    pub relation: String,
    pub next_read: NextCall,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReadResult {
    pub blocks: Vec<ReadBlock>,
    pub coverage_delta: Vec<LineRange>,
    pub next_call: Option<NextCall>,
    pub complete: bool,
    /// 批量读取中本次一行都没读到的篇目（预算耗尽），需要用 nextCall 继续。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pending_documents: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_document: Option<NextDocument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_info: Option<RangeInfo>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupCard {
    pub group_id: String,
    pub title: String,
    pub document_count: usize,
    pub line_count: usize,
    pub next_catalog: NextCall,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogResult {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub chapter_relations: Vec<super::chapter_relations::ChapterRelation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range_info: Option<RangeInfo>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub release_aliases: Vec<ReleaseAlias>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_coverage: Option<SpeakerCoverage>,
    pub groups: Vec<GroupCard>,
    pub documents: Vec<Document>,
    pub total_documents: usize,
    pub total_lines: usize,
    pub next_call: Option<NextCall>,
    pub manifest: Option<Manifest>,
    pub resolved_range: RangeScope,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CharacterResult {
    pub name: String,
    pub resolved_names: Vec<String>,
    pub scenes: usize,
    pub spoken_lines: usize,
    pub name_table_scenes: usize,
    pub inferred_lines: usize,
    pub mentioned_lines: usize,
    pub chapters: Vec<CharacterChapter>,
    pub chapter_count: usize,
    pub chapters_truncated: bool,
    pub profiles: Vec<Document>,
    pub next_calls: Vec<NextCall>,
    pub evidence_documents: Vec<Document>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CharacterChapter {
    pub group_id: String,
    pub title: String,
    pub scenes: usize,
    pub spoken_lines: usize,
    pub mentioned_lines: usize,
    pub name_table_scenes: usize,
    pub inferred_lines: usize,
    pub first_directory_appearance: Option<Document>,
    pub first_directory_mention: Option<Document>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonaResult {
    /// 固定为 "roleplay"。
    pub purpose: String,
    pub name: String,
    pub resolved_names: Vec<String>,
    pub related_names: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub until: Option<PersonaCutoff>,
    /// 范围内逐句署名已解析的对白数与场景数。
    pub spoken_lines: usize,
    pub scenes: usize,
    /// 去重并满足字数范围、可被抽为样本的台词数。
    pub candidate_lines: usize,
    pub profiles: Vec<PersonaProfile>,
    /// 设定标题含名字、但不在代理人分类里的篇数。
    pub other_titled_lore: usize,
    pub partners: Vec<PersonaPartner>,
    pub samples: Vec<PersonaSample>,
    pub next_calls: Vec<NextCall>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonaCutoff {
    pub group_id: String,
    pub title: String,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonaProfile {
    pub doc_id: String,
    pub title: String,
    pub excerpt: String,
    pub excerpt_lines: usize,
    pub line_count: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_read: Option<NextCall>,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonaPartner {
    pub name: String,
    /// 与该角色相邻的署名对白往来次数。
    pub exchanges: usize,
    pub scenes: usize,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersonaSample {
    pub doc_id: String,
    pub title: String,
    pub group_id: String,
    pub group_title: String,
    pub line: SourceLine,
    /// 文档内的前一句（占位说明除外）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<SourceLine>,
    pub read: NextCall,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GuideResult {
    pub mode: String,
    pub text: String,
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(untagged)]
pub enum ToolData {
    Search(SearchResult),
    Read(ReadResult),
    Catalog(CatalogResult),
    Character(CharacterResult),
    Persona(PersonaResult),
    Guide(GuideResult),
    Coverage(CoverageResult),
}
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ToolResponse {
    #[serde(flatten)]
    pub snapshot: Snapshot,
    pub data: ToolData,
}
pub struct ToolOutput {
    pub text: String,
    pub response: ToolResponse,
}
impl ToolData {
    /// 文本里带逐篇 compact 摘要的文档（清单模式只给范围摘要，不算）。
    fn summarized_documents(&self) -> Vec<&Document> {
        match self {
            ToolData::Search(r) => r
                .results
                .iter()
                .chain(&r.directory_suggestions)
                .map(|item| &item.document)
                .collect(),
            ToolData::Read(r) => r.blocks.iter().map(|b| &b.document).collect(),
            ToolData::Catalog(r) if r.manifest.is_none() => r.documents.iter().collect(),
            _ => Vec::new(),
        }
    }
}

/// compact 逐篇摘要只列已知的顺序字段，整份输出说明一次，单篇层面「未知」与「已核验」才可区分。
pub(crate) const UNLISTED_ORDER_NOTE: &str = "逐篇摘要只列已知的篇序/行序/局部/剧情先后/上下文；未列出即 unknown（未核验），不代表没有问题。\n";

impl ToolOutput {
    pub fn new(c: &Corpus, mut text: String, data: ToolData) -> Self {
        let snapshot = Snapshot::new(c);
        if !text.contains('\n') {
            text.push('\n');
        }
        let at = text.find('\n').map_or(0, |i| i + 1);
        let mut header = snapshot.text();
        if data
            .summarized_documents()
            .iter()
            .any(|d| d.order.as_ref().is_some_and(|o| o.has_unlisted_unknown()))
        {
            header.push_str(UNLISTED_ORDER_NOTE);
        }
        text.insert_str(at, &header);
        Self {
            text,
            response: ToolResponse { snapshot, data },
        }
    }

    /// Internal renderers use this only when they already emitted the actual snapshot header.
    /// Query and corpus text cannot decide whether provenance is present.
    pub(crate) fn with_rendered_snapshot(c: &Corpus, text: String, data: ToolData) -> Self {
        Self {
            text,
            response: ToolResponse {
                snapshot: Snapshot::new(c),
                data,
            },
        }
    }
}
pub type OutputResult = Result<ToolOutput, ToolError>;

pub fn detail(value: Option<&str>) -> Result<bool, ToolError> {
    match value.unwrap_or("compact") {
        "compact" => Ok(false),
        "full" => Ok(true),
        _ => err("detail 只能是 compact / full"),
    }
}

/// Schemas come from Rust types; convert local OpenAPI references to JSON Schema $defs.
pub fn output_schema(tool: &str) -> Result<Value, serde_json::Error> {
    #[derive(utoipa::OpenApi)]
    #[openapi(components(schemas(
        ToolResponse,
        Snapshot,
        ToolData,
        SearchResult,
        SearchItem,
        TermMatch,
        Window,
        SearchRange,
        RangeInfo,
        ReleaseAlias,
        super::chapter_relations::ChapterRelation,
        super::chapter_relations::RelationSource,
        SourceLine,
        Document,
        DocumentOrder,
        OrderCount,
        EvidenceSummary,
        SpeakerSummary,
        SpeakerCoverage,
        NextCall,
        ReadResult,
        ReadBlock,
        NextDocument,
        LineRange,
        CatalogResult,
        GroupCard,
        CharacterResult,
        CharacterChapter,
        PersonaResult,
        PersonaCutoff,
        PersonaProfile,
        PersonaPartner,
        PersonaSample,
        GuideResult,
        Manifest,
        RangeScope,
        CoverageResult,
        PendingDocument,
        MissingRange
    )))]
    struct Schemas;
    use utoipa::OpenApi;
    let mut document = serde_json::to_value(Schemas::openapi())?;
    let mut defs = document["components"]["schemas"].take();
    fn refs(v: &mut Value) {
        match v {
            Value::Object(m) => {
                if let Some(Value::String(r)) = m.get_mut("$ref") {
                    *r = r.replace("#/components/schemas/", "#/$defs/");
                }
                for value in m.values_mut() {
                    refs(value);
                }
            }
            Value::Array(a) => {
                for value in a {
                    refs(value);
                }
            }
            _ => {}
        }
    }
    refs(&mut defs);
    let mut root = defs["ToolResponse"].clone();
    let one = |name: &str| json!({"$ref":format!("#/$defs/{name}")});
    let data = match tool {
        "ik_search" => one("SearchResult"),
        "ik_read" => one("ReadResult"),
        "ik_catalog" => one("CatalogResult"),
        // purpose=roleplay 返回 PersonaResult，其余返回 CharacterResult。
        "ik_character" => json!({"oneOf":[one("CharacterResult"), one("PersonaResult")]}),
        "ik_guide" => one("GuideResult"),
        "ik_coverage" => one("CoverageResult"),
        _ => one("ToolData"),
    };
    // flatten produces allOf; the envelope remains generated and only its data ref is specialized.
    fn specialize(v: &mut Value, data: &Value) {
        match v {
            Value::Object(m) => {
                if let Some(properties) = m.get_mut("properties")
                    && properties.get("data").is_some()
                {
                    properties["data"] = data.clone();
                }
                for value in m.values_mut() {
                    specialize(value, data);
                }
            }
            Value::Array(a) => {
                for value in a {
                    specialize(value, data);
                }
            }
            _ => {}
        }
    }
    specialize(&mut root, &data);
    fn referenced(v: &Value, names: &mut std::collections::BTreeSet<String>) {
        match v {
            Value::Object(m) => {
                if let Some(r) = m
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|s| s.strip_prefix("#/$defs/"))
                {
                    names.insert(r.into());
                }
                for v in m.values() {
                    referenced(v, names);
                }
            }
            Value::Array(a) => {
                for v in a {
                    referenced(v, names);
                }
            }
            _ => {}
        }
    }
    let mut needed = std::collections::BTreeSet::new();
    referenced(&root, &mut needed);
    loop {
        let previous = needed.len();
        for name in needed.clone() {
            referenced(&defs[&name], &mut needed);
        }
        if needed.len() == previous {
            break;
        }
    }
    if let Some(map) = defs.as_object_mut() {
        map.retain(|k, _| needed.contains(k));
    }
    root["type"] = json!("object");
    root["$defs"] = defs;
    Ok(root)
}
