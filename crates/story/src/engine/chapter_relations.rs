//! Directory hints from current labels and reviewed extractor mappings.
//! These relations do not change selectors, merge groups, or establish playback order.
use serde::Serialize;
use std::collections::BTreeMap;
use utoipa::ToSchema;

use crate::corpus::Corpus;
use crate::text::fold;

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChapterRelation {
    pub member_group_ids: Vec<String>,
    pub related_group_ids: Vec<String>,
    pub relation: String,
    pub status: String,
    pub source: RelationSource,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RelationSource {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    pub note: String,
}

pub fn relations(c: &Corpus) -> Vec<ChapterRelation> {
    let mut result = Vec::new();
    let mut labels = BTreeMap::<String, Vec<String>>::new();
    for group in &c.groups {
        if group.id.starts_with("ch:") {
            labels
                .entry(fold(&group.label))
                .or_default()
                .push(group.id.clone());
        }
    }
    for mut members in labels.into_values().filter(|ids| ids.len() > 1) {
        members.sort();
        result.push(ChapterRelation {
            member_group_ids: members,
            related_group_ids: Vec::new(),
            relation: "sharedLabel".into(),
            status: "labelOnly".into(),
            source: RelationSource {
                kind: "catalogLabel".into(),
                repository: None,
                commit: None,
                path: None,
                start_line: None,
                end_line: None,
                note: "当前语料标签相同；章节等价与播放顺序待核验，不合并原始分组。".into(),
            },
        });
    }
    // Verified against the extractor's explicit first-season CS prefix table.
    // This is directory attribution, not a runtime chapter-equivalence claim.
    for mapping in [
        Mapping {
            members: &["ch:Chapter028"],
            related: &["ch:Chapter02"],
            member_label: "第一季 第二章",
            related_label: "第一季 第二章",
            relation: "supplementOf",
            start_line: 127,
            end_line: 129,
        },
        Mapping {
            members: &["ch:Chapter052", "ch:Chapter058"],
            related: &["ch:Chapter05", "ch:Chapter050"],
            member_label: "第一季 第五章",
            related_label: "第一季 第五章",
            relation: "supplementOf",
            start_line: 134,
            end_line: 137,
        },
        Mapping {
            members: &["ch:Chapter025"],
            related: &["ch:Chapter02"],
            member_label: "第一季 第二章间章",
            related_label: "第一季 第二章",
            relation: "interludeOf",
            start_line: 127,
            end_line: 128,
        },
        Mapping {
            members: &["ch:Chapter055"],
            related: &["ch:Chapter05", "ch:Chapter050"],
            member_label: "第一季 第五章间章",
            related_label: "第一季 第五章",
            relation: "interludeOf",
            start_line: 134,
            end_line: 137,
        },
    ] {
        let members = present(c, mapping.members, mapping.member_label);
        let related = present(c, mapping.related, mapping.related_label);
        if members.is_empty() || related.is_empty() {
            continue;
        }
        result.push(ChapterRelation {
            member_group_ids: members,
            related_group_ids: related,
            relation: mapping.relation.into(),
            status: "reviewedExtractorMapping".into(),
            source: RelationSource {
                kind: "extractorMapping".into(),
                repository: Some("https://github.com/KawaYiLab/zzz-story-extractor".into()),
                commit: Some("f1b85abfb4efc19018526ca72107d8c09d2c84a0".into()),
                path: Some("zzz_story/extract.py".into()),
                start_line: Some(mapping.start_line),
                end_line: Some(mapping.end_line),
                note: "2026-10-22核对第一季CS前缀映射及zzz_story/chapters.py:L35–41标签；只说明正章、间章或补充归属，成员保留原始ID。当前标签不匹配时省略。".into(),
            },
        });
    }
    result.sort_by(|a, b| {
        a.relation
            .cmp(&b.relation)
            .then_with(|| a.member_group_ids.cmp(&b.member_group_ids))
    });
    result
}

struct Mapping {
    members: &'static [&'static str],
    related: &'static [&'static str],
    member_label: &'static str,
    related_label: &'static str,
    relation: &'static str,
    start_line: usize,
    end_line: usize,
}

fn present(c: &Corpus, ids: &[&str], label: &str) -> Vec<String> {
    let label = fold(label);
    ids.iter()
        .filter(|id| {
            c.groups
                .iter()
                .any(|g| g.id == **id && fold(&g.label) == label)
        })
        .map(|id| (*id).into())
        .collect()
}
