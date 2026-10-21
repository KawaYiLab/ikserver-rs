//! Typed tool results are the source for both text and MCP structuredContent.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt::Write as _;
use utoipa::ToSchema;

use super::coverage::{CoverageResult, Manifest, MissingRange, PendingDocument, RangeScope};
use super::{ToolError, err};
use crate::corpus::{Corpus, DocKind, LineKind};

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
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub query: String,
    pub resolved_range: SearchRange,
    pub partial: bool,
    pub candidate_count: usize,
    pub body_result_count: usize,
    pub directory_count: usize,
    pub hit_count: usize,
    pub missing_terms: Vec<String>,
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
pub struct ReadResult {
    pub blocks: Vec<ReadBlock>,
    pub coverage_delta: Vec<LineRange>,
    pub next_call: Option<NextCall>,
    pub complete: bool,
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
impl ToolOutput {
    pub fn new(c: &Corpus, text: String, data: ToolData) -> Self {
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
        SourceLine,
        Document,
        NextCall,
        ReadResult,
        ReadBlock,
        LineRange,
        CatalogResult,
        GroupCard,
        CharacterResult,
        CharacterChapter,
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
    let data = match tool {
        "ik_search" => "SearchResult",
        "ik_read" => "ReadResult",
        "ik_catalog" => "CatalogResult",
        "ik_character" => "CharacterResult",
        "ik_guide" => "GuideResult",
        "ik_coverage" => "CoverageResult",
        _ => "ToolData",
    };
    // flatten produces allOf; the envelope remains generated and only its data ref is specialized.
    fn specialize(v: &mut Value, data: &str) {
        match v {
            Value::Object(m) => {
                if let Some(properties) = m.get_mut("properties") {
                    if properties.get("data").is_some() {
                        properties["data"] = json!({"$ref":format!("#/$defs/{data}")});
                    }
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
    specialize(&mut root, data);
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
