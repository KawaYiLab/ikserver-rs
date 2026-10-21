//! 语料的内存模型。
//!
//! 所有台词文本拼成一整块 `text`，行、文档、分组都是指向它的下标区间；
//! 一个文档的行在 `lines` 里连续，一个分组按阅读顺序列出文档。
//! 这种扁平布局让检索时几乎不产生指针追逐。

mod build;
mod bundle;
mod evidence;
mod revision;

pub use build::{build_from_bundle, build_from_dir, family_label};
pub use bundle::{BUNDLE_FORMAT, Bundle};
pub use evidence::{EvidenceCounts, LineEvidence, ResourceEvidence};

use serde::Serialize;
use std::ops::Range;

use crate::index::BigramIndex;

/// 没有说话人。
pub const NO_SPEAKER: u32 = u32::MAX;

#[derive(Debug, Clone, Default)]
pub struct Meta {
    /// 数据版本（ZenlessData 版本号，如 3.2.0）。
    pub source_version: String,
    pub language: String,
    /// 提取器生成时间。
    pub generated_at: String,
    /// 本机游戏版本（提取说话人时才有）。
    pub local_game_version: String,
    /// 快照构建时间（Unix 秒）。
    pub built_at: u64,
    /// 主线台词里带说话人的比例（0..=1）。
    pub speaker_coverage: f32,
    /// None 表示旧包未声明筛选模式。
    pub accuracy_first: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum LineKind {
    /// 有说话人的台词。
    Dialogue,
    /// 玩家选项。
    Option,
    /// 旁白（场景有说话人数据，这一行没有）。
    Narration,
    /// 说话人未知。
    Unknown,
    /// 设定资料里的一行 Markdown。
    Text,
    /// 经来源核查的场景提纲/占位说明，不是角色对白。
    Placeholder,
}

#[derive(Debug, Clone, Copy)]
pub struct Line {
    pub start: u32,
    pub len: u32,
    pub speaker: u32,
    pub kind: LineKind,
}

/// 可选的原始行出处；与 lines 一一对应，兼容旧语料包。
#[derive(Debug, Clone, Default, Serialize)]
pub struct LineSource {
    pub source_key: Option<String>,
    pub line_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DocKind {
    /// 主线剧情（story.jsonl）。
    Story,
    /// 过场字幕 / 漫画（supplement.jsonl）。
    Supplement,
    /// 世界观设定（lore/**/*.md）。
    Lore,
}

impl DocKind {
    pub fn label(self) -> &'static str {
        match self {
            DocKind::Story => "主线",
            DocKind::Supplement => "补充",
            DocKind::Lore => "设定",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Doc {
    /// 稳定的 64 位标识，对外显示为 `doc_` + 12 位十六进制。
    pub hash: u64,
    pub kind: DocKind,
    pub group: u32,
    /// 在分组阅读顺序里的位置。
    pub group_pos: u32,
    /// 内容类型：GalGame / Chat / CS … ；设定资料为所在文件名。
    pub section: String,
    pub title: String,
    /// 面包屑路径（不含标题）。
    pub path: String,
    /// 官方任务名（仅剧情演出）。
    pub quest: Option<String>,
    pub order_source: Option<String>,
    pub chronology_status: Option<String>,
    pub context_status: Option<String>,
    pub first_line: u32,
    pub line_count: u32,
    pub char_count: u32,
    /// `doc_speakers` 中的区间：逐句已解析署名（含旧包署名），按文档行序。
    pub speakers_start: u32,
    pub speakers_len: u32,
    /// TextMap 姓名表；与实际署名独立，不能据此确认出场。
    pub name_table: Vec<u32>,
    pub inferred_speakers: Vec<u32>,
    pub evidence_counts: EvidenceCounts,
}

impl Doc {
    pub fn lines(&self) -> Range<usize> {
        self.first_line as usize..(self.first_line + self.line_count) as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Chapter,
    Lore,
}

#[derive(Debug, Clone)]
pub struct Group {
    /// 对外标识：`ch:Chapter110`、`lore:agents`。
    pub id: String,
    pub kind: GroupKind,
    pub label: String,
    /// 阅读顺序排列的文档下标。
    pub docs: Vec<u32>,
}

#[derive(Debug)]
pub struct Corpus {
    /// 规范化内容、影响结果的配置及查询契约的摘要；不含构建/提取时间。
    pub revision: String,
    pub meta: Meta,
    pub text: String,
    pub lines: Vec<Line>,
    pub line_sources: Vec<LineSource>,
    /// 与行对应的证据表下标，相同证据只保存一份。
    pub line_evidence_ids: Vec<u32>,
    pub evidence: Vec<LineEvidence>,
    pub docs: Vec<Doc>,
    pub groups: Vec<Group>,
    pub speakers: Vec<String>,
    /// 折叠后的说话人名，与 `speakers` 一一对应。
    pub speakers_folded: Vec<String>,
    /// 角色身份的折叠名；不同形态仍保留各自的显示名。
    pub speaker_identities: Vec<String>,
    /// 归一角色名的原始写法，用于聚合后的显示。
    pub speaker_identity_names: Vec<String>,
    pub doc_speakers: Vec<u32>,
    /// CSR：说话人 → 出场文档（升序），`speaker_docs[speaker_doc_offsets[s]..speaker_doc_offsets[s+1]]`。
    pub speaker_doc_offsets: Vec<u32>,
    pub speaker_docs: Vec<u32>,
    /// 折叠后的「标题 + 路径」，检索时用来给标题命中加分、目录按标题筛选。
    pub doc_keys: DocKeys,
    /// (hash, doc) 按 hash 排序，用于 docId 查找。
    pub doc_lookup: Vec<(u64, u32)>,
    /// 行号 → 文档号（O(1) 归属判断，检索热路径上每个命中行都要用）。
    pub line_doc: Vec<u32>,
    pub index: BigramIndex,
}

impl Corpus {
    #[inline]
    pub fn line_text(&self, line: usize) -> &str {
        let l = &self.lines[line];
        &self.text[l.start as usize..(l.start + l.len) as usize]
    }

    pub fn line_evidence(&self, line: usize) -> &LineEvidence {
        &self.evidence[self.line_evidence_ids[line] as usize]
    }

    pub fn is_inferred(&self, line: usize) -> bool {
        self.line_evidence(line).speaker_status.as_deref() == Some("inferred")
    }

    /// 旧包有署名但未提供证据时仍保留署名；新包的推断/未解析状态不当作已解析。
    pub fn is_attributed(&self, line: usize) -> bool {
        let l = &self.lines[line];
        l.speaker != NO_SPEAKER
            && l.kind == LineKind::Dialogue
            && self
                .line_evidence(line)
                .speaker_status
                .as_deref()
                .is_none_or(|s| s == "resolved")
    }

    #[inline]
    pub fn speaker_name(&self, id: u32) -> Option<&str> {
        if id == NO_SPEAKER {
            None
        } else {
            self.speakers.get(id as usize).map(String::as_str)
        }
    }

    /// 某说话人出场（场景名单或实际台词）的文档。
    pub fn docs_with_speaker(&self, speaker: u32) -> &[u32] {
        let s = speaker as usize;
        &self.speaker_docs
            [self.speaker_doc_offsets[s] as usize..self.speaker_doc_offsets[s + 1] as usize]
    }

    pub fn doc_speakers(&self, doc: &Doc) -> &[u32] {
        let start = doc.speakers_start as usize;
        &self.doc_speakers[start..start + doc.speakers_len as usize]
    }

    /// 行号 → 所在文档。
    #[inline]
    pub fn doc_of_line(&self, line: u32) -> usize {
        self.line_doc[line as usize] as usize
    }

    pub fn doc_id(&self, doc: usize) -> String {
        format_doc_id(self.docs[doc].hash)
    }

    pub fn find_doc(&self, id: &str) -> Option<usize> {
        let hash = parse_doc_id(id)?;
        self.doc_lookup
            .binary_search_by_key(&hash, |&(h, _)| h)
            .ok()
            .map(|i| self.doc_lookup[i].1 as usize)
    }

    /// 解析分组：精确 id、章节 token（`Chapter110` / `110`）、标签包含关系依次尝试。
    pub fn find_group(&self, query: &str) -> Option<usize> {
        let q = query.trim();
        if q.is_empty() {
            return None;
        }
        if let Some(i) = self.groups.iter().position(|g| g.id == q) {
            return Some(i);
        }
        let folded = crate::text::fold(q);
        let token = folded.trim_start_matches("ch:").trim_start_matches("lore:");
        let by_token = self.groups.iter().position(|g| {
            let gid = g.id.to_ascii_lowercase();
            let tail = gid.split(':').nth(1).unwrap_or("");
            tail == token || tail.trim_start_matches("chapter") == token
        });
        if by_token.is_some() {
            return by_token;
        }
        let exact: Vec<usize> = self
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| crate::text::fold(&g.label) == folded)
            .map(|(i, _)| i)
            .collect();
        if !exact.is_empty() {
            return (exact.len() == 1).then(|| exact[0]);
        }
        let partial: Vec<usize> = self
            .groups
            .iter()
            .enumerate()
            .filter(|(_, g)| crate::text::fold(&g.label).contains(&folded))
            .map(|(i, _)| i)
            .collect();
        (partial.len() == 1).then(|| partial[0])
    }

    /// 阅读顺序中的下一篇（同一分组内）。
    pub fn next_in_group(&self, doc: usize) -> Option<usize> {
        let d = &self.docs[doc];
        self.groups[d.group as usize]
            .docs
            .get(d.group_pos as usize + 1)
            .map(|&i| i as usize)
    }

    pub fn total_chars(&self, kind: DocKind) -> usize {
        self.docs
            .iter()
            .filter(|d| d.kind == kind)
            .map(|d| d.char_count as usize)
            .sum()
    }
}

/// 标题/路径检索用的折叠键。
///
/// 标题拼成一整块 `标题\0` × N，一个检索词在整块上做一次 SIMD 子串扫描；
/// 路径由整组/整个文件共享，只按去重后的几百条路径各比一次，再展开成文档。
#[derive(Debug, Default)]
pub struct DocKeys {
    titles: String,
    /// 第 i 篇标题的起点；末尾多一个哨兵。
    title_offsets: Vec<u32>,
    /// 去重后的折叠路径。
    paths: Vec<String>,
    /// CSR：`path_docs[path_offsets[p]..path_offsets[p+1]]` 是路径 p 下的文档（升序）。
    path_offsets: Vec<u32>,
    path_docs: Vec<u32>,
}

impl DocKeys {
    pub fn build(docs: &[Doc]) -> Self {
        let mut keys = DocKeys::default();
        let mut path_ids: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let mut members: Vec<Vec<u32>> = Vec::new();
        for (i, d) in docs.iter().enumerate() {
            keys.title_offsets.push(keys.titles.len() as u32);
            keys.titles
                .extend(d.title.chars().map(crate::text::fold_char));
            keys.titles.push('\0');
            let p = *path_ids.entry(d.path.as_str()).or_insert_with(|| {
                keys.paths.push(crate::text::fold(&d.path));
                members.push(Vec::new());
                members.len() - 1
            });
            members[p].push(i as u32);
        }
        keys.title_offsets.push(keys.titles.len() as u32);
        for m in members {
            keys.path_offsets.push(keys.path_docs.len() as u32);
            keys.path_docs.extend(m);
        }
        keys.path_offsets.push(keys.path_docs.len() as u32);
        keys
    }

    /// 标题（`title_only`）或标题/路径包含已折叠 `needle` 的文档，升序去重。
    pub fn matching(&self, needle: &str, title_only: bool) -> Vec<u32> {
        let mut out: Vec<u32> = Vec::new();
        if needle.is_empty() || needle.contains('\0') {
            return out;
        }
        let finder = memchr::memmem::Finder::new(needle.as_bytes());
        let mut next_start = 0usize;
        for pos in finder.find_iter(self.titles.as_bytes()) {
            if pos < next_start {
                continue;
            }
            let d = self.title_offsets.partition_point(|&o| o as usize <= pos) - 1;
            out.push(d as u32);
            next_start = self.title_offsets[d + 1] as usize;
        }
        if title_only {
            return out;
        }
        for (p, path) in self.paths.iter().enumerate() {
            if finder.find(path.as_bytes()).is_some() {
                let members = &self.path_docs
                    [self.path_offsets[p] as usize..self.path_offsets[p + 1] as usize];
                out = crate::index::union_sorted(&out, members);
            }
        }
        out
    }

    pub fn matching_path(&self, needle: &str) -> Vec<u32> {
        let mut out = Vec::new();
        if needle.is_empty() || needle.contains('\0') {
            return out;
        }
        for (p, path) in self.paths.iter().enumerate() {
            if path.contains(needle) {
                out = crate::index::union_sorted(
                    &out,
                    &self.path_docs
                        [self.path_offsets[p] as usize..self.path_offsets[p + 1] as usize],
                );
            }
        }
        out
    }
}

pub fn format_doc_id(hash: u64) -> String {
    format!("doc_{:012x}", hash & DOC_HASH_MASK)
}

pub fn parse_doc_id(id: &str) -> Option<u64> {
    let hex = id.trim().strip_prefix("doc_")?;
    if hex.is_empty() || hex.len() > 12 {
        return None;
    }
    u64::from_str_radix(hex, 16).ok()
}

/// 对外只用 48 位：12 位十六进制足够短，两万篇文档的碰撞概率可以忽略（构建时仍会检查）。
pub const DOC_HASH_MASK: u64 = (1 << 48) - 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_id_round_trip() {
        let id = format_doc_id(0xdead_beef_cafe_f00d);
        assert_eq!(id, "doc_beefcafef00d");
        assert_eq!(parse_doc_id(&id), Some(0xbeef_cafe_f00d));
        assert_eq!(parse_doc_id("doc_"), None);
        assert_eq!(parse_doc_id("xyz"), None);
        assert_eq!(parse_doc_id("doc_zz"), None);
    }
}
