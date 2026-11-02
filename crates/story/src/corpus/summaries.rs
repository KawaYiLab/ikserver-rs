//! Derived presentation statistics are cached separately from the revision-bearing documents.
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use utoipa::ToSchema;

use super::{Corpus, DocKind, EvidenceCounts, NO_SPEAKER};

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrderCount {
    pub value: String,
    pub line_count: usize,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentOrder {
    pub applicable: bool,
    pub doc_order_source: String,
    pub line_order_sources: Vec<OrderCount>,
    pub line_order_statuses: Vec<OrderCount>,
    pub chronology_status: String,
    pub context_status: String,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceSummary {
    pub placeholders: usize,
    pub unknown: usize,
    pub inferred: usize,
    pub missing: usize,
    pub unlinked: usize,
    pub no_voice: usize,
    pub counts_may_overlap: bool,
}
impl From<&EvidenceCounts> for EvidenceSummary {
    fn from(n: &EvidenceCounts) -> Self {
        Self {
            placeholders: n.placeholders,
            unknown: n.unknown,
            inferred: n.inferred,
            missing: n.missing,
            unlinked: n.unlinked,
            no_voice: n.no_voice,
            counts_may_overlap: true,
        }
    }
}

/// Mutually exclusive attribution categories, using every selected source line as denominator.
#[derive(Debug, Clone, Default, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerCoverage {
    pub applicable: bool,
    pub total_lines: usize,
    pub named_lines: usize,
    pub resolved_lines: usize,
    pub inferred_lines: usize,
    pub legacy_named_lines: usize,
    pub other_named_lines: usize,
    pub unnamed_lines: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub named_rate: Option<f64>,
    pub content_types: Vec<String>,
    pub excluded_not_applicable_lines: usize,
}
impl SpeakerCoverage {
    pub fn merge(&mut self, other: &Self) {
        if !other.applicable {
            self.excluded_not_applicable_lines +=
                other.total_lines + other.excluded_not_applicable_lines;
            for kind in &other.content_types {
                if !self.content_types.contains(kind) {
                    self.content_types.push(kind.clone());
                }
            }
            return;
        }
        self.applicable |= other.applicable;
        self.total_lines += other.total_lines;
        self.named_lines += other.named_lines;
        self.resolved_lines += other.resolved_lines;
        self.inferred_lines += other.inferred_lines;
        self.legacy_named_lines += other.legacy_named_lines;
        self.other_named_lines += other.other_named_lines;
        self.unnamed_lines += other.unnamed_lines;
        self.excluded_not_applicable_lines += other.excluded_not_applicable_lines;
        for kind in &other.content_types {
            if !self.content_types.contains(kind) {
                self.content_types.push(kind.clone());
            }
        }
        self.named_rate = if self.applicable && self.total_lines > 0 {
            Some(self.named_lines as f64 / self.total_lines as f64)
        } else {
            None
        };
    }

    pub fn text(&self) -> String {
        if !self.applicable || self.total_lines == 0 {
            return "全行署名率=不适用".into();
        }
        format!(
            "全行署名率={}/{}（{:.1}%）；解析{}/推断{}/旧署名{}/待核验{}/无名{}",
            self.named_lines,
            self.total_lines,
            self.named_rate.unwrap_or(0.0) * 100.0,
            self.resolved_lines,
            self.inferred_lines,
            self.legacy_named_lines,
            self.other_named_lines,
            self.unnamed_lines
        )
    }
}

#[derive(Debug, Clone)]
pub struct DocumentSummary {
    pub order: DocumentOrder,
    pub evidence: EvidenceSummary,
    pub coverage: SpeakerCoverage,
    pub compact_text: Arc<str>,
}

impl DocumentSummary {
    /// compact 逐篇摘要：只列出场说话人（最多 6 个）。顺序、署名覆盖与缺口计数留在 structuredContent 与 detail=full，
    /// 逐篇重复这些状态只会占模型的注意力。
    pub fn render_compact(&self, names: &[&str], total_speakers: usize) -> String {
        if !self.order.applicable || names.is_empty() {
            return String::new();
        }
        let shown = names
            .iter()
            .take(6)
            .map(|n| {
                if n.chars().count() <= 16 {
                    (*n).to_owned()
                } else {
                    format!("{}…", n.chars().take(16).collect::<String>())
                }
            })
            .collect::<Vec<_>>();
        let mut text = format!("出场：{}", shown.join("、"));
        if shown.len() < total_speakers {
            let _ = write!(text, " 等 {total_speakers} 人");
        }
        text.push('\n');
        text
    }
}

pub(super) fn build(c: &Corpus) -> Vec<DocumentSummary> {
    c.docs
        .iter()
        .map(|doc| {
            let applicable = doc.kind != DocKind::Lore;
            let mut sources: BTreeMap<&str, usize> = BTreeMap::new();
            let mut statuses: BTreeMap<&str, usize> = BTreeMap::new();
            let mut coverage = SpeakerCoverage {
                applicable,
                total_lines: doc.line_count as usize,
                content_types: vec![doc.kind.scope_name().into()],
                ..SpeakerCoverage::default()
            };
            for line in doc.lines() {
                let evidence = c.line_evidence(line);
                if applicable {
                    *sources
                        .entry(
                            evidence
                                .line_order_source
                                .as_deref()
                                .unwrap_or("notProvided"),
                        )
                        .or_default() += 1;
                    *statuses
                        .entry(
                            evidence
                                .line_order_status
                                .as_deref()
                                .unwrap_or("notProvided"),
                        )
                        .or_default() += 1;
                }
                if c.lines[line].speaker == NO_SPEAKER {
                    coverage.unnamed_lines += 1;
                } else {
                    coverage.named_lines += 1;
                    match evidence.speaker_status.as_deref() {
                        Some("resolved") => coverage.resolved_lines += 1,
                        Some("inferred") => coverage.inferred_lines += 1,
                        None => coverage.legacy_named_lines += 1,
                        Some(_) => coverage.other_named_lines += 1,
                    }
                }
            }
            if applicable && coverage.total_lines > 0 {
                coverage.named_rate =
                    Some(coverage.named_lines as f64 / coverage.total_lines as f64);
            }
            let counts = |values: BTreeMap<&str, usize>| {
                values
                    .into_iter()
                    .map(|(value, line_count)| OrderCount {
                        value: value.into(),
                        line_count,
                    })
                    .collect()
            };
            let value = |value: &Option<String>| {
                if applicable {
                    value.as_deref().unwrap_or("unknown").to_owned()
                } else {
                    "notApplicable".into()
                }
            };
            let mut summary = DocumentSummary {
                order: DocumentOrder {
                    applicable,
                    doc_order_source: value(&doc.order_source),
                    line_order_sources: counts(sources),
                    line_order_statuses: counts(statuses),
                    chronology_status: value(&doc.chronology_status),
                    context_status: value(&doc.context_status),
                },
                evidence: EvidenceSummary::from(&doc.evidence_counts),
                coverage,
                compact_text: Arc::from(""),
            };
            let speakers = c.doc_speakers(doc);
            let names = speakers
                .iter()
                .filter_map(|&id| c.speaker_name(id))
                .collect::<Vec<_>>();
            summary.compact_text = Arc::from(summary.render_compact(&names, speakers.len()));
            summary
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn presentation_cache_is_excluded_from_content_revision() {
        let path =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sample");
        let mut c = super::super::build_from_dir(&path).unwrap();
        let before = super::super::revision::revision(&c).unwrap();
        c.doc_summaries[0].coverage.total_lines += 100;
        c.doc_summaries[0].order.context_status = "different presentation".into();
        assert_eq!(super::super::revision::revision(&c).unwrap(), before);
    }
}
