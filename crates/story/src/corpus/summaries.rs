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
    /// Cached compact text exposes all decision-relevant axes; full counts remain in typed data.
    pub fn render_compact(&self, names: &[&str], total_speakers: usize) -> String {
        if !self.order.applicable {
            return "顺序与署名覆盖：不适用（设定）。\n".into();
        }
        let short = |value: &str, max: usize| {
            if value.chars().count() <= max {
                value.to_owned()
            } else {
                format!("{}…", value.chars().take(max).collect::<String>())
            }
        };
        let document_source = |value: &str| match value {
            "source_key" | "archive_table" => value.to_owned(),
            _ => short(value, 8),
        };
        let sources = |values: &[OrderCount], max: usize, width: usize, mixed_count: bool| {
            let mut selected = values
                .iter()
                .filter(|v| matches!(v.value.as_str(), "source_key" | "archive_table"))
                .collect::<Vec<_>>();
            selected.extend(
                values
                    .iter()
                    .filter(|v| !matches!(v.value.as_str(), "source_key" | "archive_table")),
            );
            let mut result = selected
                .iter()
                .take(max)
                .map(|v| match v.value.as_str() {
                    "source_key" | "archive_table" => v.value.clone(),
                    "notProvided" => "未提供".into(),
                    _ => short(&v.value, width),
                })
                .collect::<Vec<_>>()
                .join("/");
            if values.len() > max {
                if mixed_count {
                    let _ = write!(result, "/混合{}种(full)", values.len());
                } else {
                    result.push_str("/混合(full)");
                }
            }
            result
        };
        let o = &self.order;
        let e = &self.evidence;
        let c = &self.coverage;
        let hints = || {
            [
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
            .collect::<Vec<_>>()
            .join("/")
        };
        let mut text = format!(
            "整篇：篇序={} 行序={} 局部={} 剧情先后={} 上下文={}\n",
            short(&o.doc_order_source, 16),
            sources(&o.line_order_sources, 3, 16, true),
            sources(&o.line_order_statuses, 1, 16, true),
            short(&o.chronology_status, 16),
            short(&o.context_status, 16)
        );
        let gaps = hints();
        if !gaps.is_empty() {
            let _ = writeln!(text, "缺口可交叉：{gaps}");
        }
        let _ = writeln!(
            text,
            "署名{}/{}（含推断{}、旧未核验{}、待核验{}）",
            c.named_lines,
            c.total_lines,
            c.inferred_lines,
            c.legacy_named_lines,
            c.other_named_lines
        );
        if text.chars().count() > 155 {
            // Keep concrete known sources when shortening; a count of kinds alone cannot support evidence assessment.
            text = format!(
                "整篇：篇序={} 行序={} 剧情先后={} 上下文={}；局部{}种(full)\n",
                document_source(&o.doc_order_source),
                sources(&o.line_order_sources, 2, 16, true),
                short(&o.chronology_status, 8),
                short(&o.context_status, 8),
                o.line_order_statuses.len()
            );
            if !gaps.is_empty() {
                let _ = writeln!(text, "缺口可交叉：{gaps}");
            }
            let _ = writeln!(
                text,
                "署名{}/{} 推断{} 旧未核验{} 待核验{}；full详情",
                c.named_lines,
                c.total_lines,
                c.inferred_lines,
                c.legacy_named_lines,
                c.other_named_lines
            );
        }
        if text.chars().count() > 180 {
            let count = |n: usize| {
                if n < 100_000 {
                    n.to_string()
                } else {
                    ">=10万".into()
                }
            };
            text = format!(
                "整篇：篇序={} 行序={} 剧情先后={} 上下文={} 局部{}种\n",
                document_source(&o.doc_order_source),
                sources(&o.line_order_sources, 2, 16, true),
                short(&o.chronology_status, 8),
                short(&o.context_status, 8),
                o.line_order_statuses.len()
            );
            let _ = writeln!(
                text,
                "缺口可交叉：占位{} 未知{} 推断{} 缺证据{} 未关联{} 无配音{}",
                count(e.placeholders),
                count(e.unknown),
                count(e.inferred),
                count(e.missing),
                count(e.unlinked),
                count(e.no_voice)
            );
            let _ = writeln!(
                text,
                "署名{}/{} 推断{} 旧未核验{} 待核验{}；full精确计数",
                count(c.named_lines),
                count(c.total_lines),
                count(c.inferred_lines),
                count(c.legacy_named_lines),
                count(c.other_named_lines)
            );
            if text.chars().count() > 180 {
                // Bound every remaining variable-width field; typed/full data retains exact counts and statuses.
                text = format!(
                    "整篇：篇序={} 行序={} 剧情先后={} 上下文={}；局部见full\n",
                    document_source(&o.doc_order_source),
                    sources(&o.line_order_sources, 2, 8, false),
                    short(&o.chronology_status, 4),
                    short(&o.context_status, 4)
                );
                let _ = writeln!(
                    text,
                    "缺口可交叉：占位{} 未知{} 推断{} 缺证据{} 未关联{} 无配音{}",
                    count(e.placeholders),
                    count(e.unknown),
                    count(e.inferred),
                    count(e.missing),
                    count(e.unlinked),
                    count(e.no_voice)
                );
                let _ = writeln!(
                    text,
                    "署名{}/{} 推断/旧未核验/待核验见full；精确计数",
                    count(c.named_lines),
                    count(c.total_lines)
                );
            }
        }
        let suffix = format!("（共{total_speakers}人，full）");
        let available = 180usize.saturating_sub(text.chars().count() + 7 + suffix.chars().count());
        let mut shown = Vec::new();
        let mut used = 0;
        for name in names.iter().take(6) {
            let name = short(name, 16);
            let cost = name.chars().count() + usize::from(!shown.is_empty());
            if used + cost > available {
                break;
            }
            used += cost;
            shown.push(name);
        }
        if !shown.is_empty() {
            let _ = write!(text, "逐句署名：{}", shown.join("、"));
            if shown.len() < total_speakers {
                text.push_str(&suffix);
            }
            text.push('\n');
        } else if total_speakers > 0 && text.chars().count() + suffix.chars().count() + 5 <= 180 {
            let _ = writeln!(text, "署名名单{suffix}");
        }
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
                content_types: vec![
                    match doc.kind {
                        DocKind::Story => "story",
                        DocKind::Supplement => "supplement",
                        DocKind::Lore => "lore",
                    }
                    .into(),
                ],
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
