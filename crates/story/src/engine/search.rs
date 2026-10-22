//! ik_search：全文检索。
//!
//! 多个关键词用空格分隔，按「同一文档内全部出现」求交（AND）；没有文档同时包含全部关键词时
//! 默认明确标注并返回部分匹配，allowPartial=false 可要求严格 AND。
//! 正文/逐句署名与仅元数据命中独立呈现；matchIn=body 只允许正文参与覆盖。
//! 指定 docId 时变为文档内检索，逐行列出命中与行号。

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use super::{ToolResult, err, parse_scope, resolve_doc, resolve_group};
use crate::corpus::{Corpus, DocKind};
use crate::index::{Candidates, intersect, union_sorted};
use crate::text::{find_folded, fold};
mod output;
use output::{apply_windows, render_typed, typed_result};

const MAX_TERMS: usize = 8;
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.5;
const TITLE_BOOST: f64 = 1.5;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchArgs {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default)]
    pub match_unit: Option<String>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub window_lines: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub offset: Option<usize>,
    #[serde(default)]
    pub page_part: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub doc_id: Option<String>,
    #[serde(default)]
    pub speaker: Option<String>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub max_results: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub hits_per_doc: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_bool")]
    pub allow_partial: Option<bool>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub context_lines: Option<usize>,
    #[serde(default)]
    pub match_in: Option<String>,
    #[serde(default, deserialize_with = "crate::de::opt_bool")]
    pub include_evidence: Option<bool>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub directory_offset: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub max_directory_results: Option<usize>,
}

/// 检索范围：文档种类 / 分组 / 单篇 / 说话人。
struct Filter {
    kind: Option<DocKind>,
    group: Option<u32>,
    doc: Option<usize>,
    speakers: Option<Vec<u32>>,
    list_group: bool,
    body_only: bool,
}

impl Filter {
    fn accepts_doc(&self, corpus: &Corpus, doc: usize) -> bool {
        let d = &corpus.docs[doc];
        self.doc.is_none_or(|x| x == doc)
            && self.kind.is_none_or(|k| k == d.kind)
            && self.group.is_none_or(|g| g == d.group)
    }

    fn accepts_line(&self, corpus: &Corpus, line: u32) -> bool {
        match &self.speakers {
            Some(ids) => ids.contains(&corpus.lines[line as usize].speaker),
            None => true,
        }
    }
}

impl Filter {
    /// 没有任何范围限定（全库、全部说话人）时，范围内零命中即全库零命中。
    fn is_unrestricted(&self) -> bool {
        self.kind.is_none()
            && self.group.is_none()
            && self.doc.is_none()
            && self.speakers.is_none()
            && !self.list_group
    }
}

/// 不带范围筛选、按同一 matchIn 统计一个词在全库命中的篇数。
fn corpus_doc_count(corpus: &Corpus, term: &str, body_only: bool) -> usize {
    let everything = Filter {
        kind: None,
        group: None,
        doc: None,
        speakers: None,
        list_group: false,
        body_only,
    };
    collect_term_hits(corpus, &[term.to_owned()], &everything)
        .first()
        .map_or(0, |t| t.docs.len())
}

fn push_missing_terms(out: &mut String, missing: &[super::response::TermCount], body_only: bool) {
    let elsewhere: Vec<String> = missing
        .iter()
        .filter(|m| m.corpus_documents > 0)
        .map(|m| format!("{}（全库 {} 篇）", m.term, m.corpus_documents))
        .collect();
    if !elsewhere.is_empty() {
        let _ = writeln!(
            out,
            "当前范围内无命中：{}；范围外仍有，不代表资料库没有。",
            elsewhere.join("、")
        );
    }
    let nowhere: Vec<&str> = missing
        .iter()
        .filter(|m| m.corpus_documents == 0)
        .map(|m| m.term.as_str())
        .collect();
    if !nowhere.is_empty() {
        let _ = writeln!(
            out,
            "全库无此词：{}（当前语料快照，matchIn={}）。",
            nowhere.join("、"),
            if body_only { "body" } else { "all" }
        );
    }
}

/// 说话人解析：先精确匹配（折叠后相等），没有再用包含匹配。
pub(crate) fn match_speakers(corpus: &Corpus, name: &str) -> Vec<u32> {
    let folded = fold(name.trim());
    if folded.is_empty() {
        return Vec::new();
    }
    let exact: Vec<u32> = corpus
        .speakers_folded
        .iter()
        .enumerate()
        .filter(|(i, s)| **s == folded || corpus.speaker_identities[*i] == folded)
        .map(|(i, _)| i as u32)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    corpus
        .speakers_folded
        .iter()
        .enumerate()
        .filter(|(_, s)| s.contains(&folded))
        .map(|(i, _)| i as u32)
        .collect()
}

#[derive(Clone)]
struct DocHits {
    doc: usize,
    /// 每个检索词命中的行数。
    tf: [u32; MAX_TERMS],
    line_mask: u32,
    title_mask: u32,
    /// 本篇命中行在 `Hits::lines` 中的区间。
    start: u32,
    end: u32,
    score: f64,
    fields: [u32; 7],
    windows: Vec<(u32, u32)>,
}

impl DocHits {
    fn line_count(&self) -> usize {
        (self.end - self.start) as usize
    }
}

/// 一次检索的全部命中：文档表 + 扁平的（行号, 命中词位图）数组，避免每篇一次分配。
struct Hits {
    docs: Vec<DocHits>,
    lines: Vec<(u32, u32)>,
    partial: bool,
}

pub fn run(corpus: &Corpus, args: &SearchArgs) -> ToolResult {
    execute(corpus, args).map(|r| r.text)
}

pub fn execute(corpus: &Corpus, args: &SearchArgs) -> super::response::OutputResult {
    let full = super::response::detail(args.detail.as_deref())?;
    let mut args = args.clone();
    match args.purpose.as_deref().unwrap_or("browse") {
        "browse" => {}
        "locate" => {
            args.hits_per_doc.get_or_insert(2);
            args.context_lines.get_or_insert(1);
        }
        "verify" => {
            args.match_in.get_or_insert("body".into());
            args.allow_partial.get_or_insert(false);
            args.context_lines.get_or_insert(2);
        }
        _ => return err("purpose 只能是 browse / locate / verify"),
    }
    let args = &args;
    let body_only = match args.match_in.as_deref().unwrap_or("all") {
        "all" => false,
        "body" => true,
        _ => return err("matchIn 只能是 all / body"),
    };
    let keyword = args.keyword.as_deref().unwrap_or("").trim();
    let (mut terms, keyword_release) = super::releases::query(keyword)?;
    let truncated = terms.len() > MAX_TERMS;
    terms.truncate(MAX_TERMS);

    let doc = args
        .doc_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let doc = doc.map(|id| resolve_doc(corpus, id)).transpose()?;
    let path = args
        .path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let path_release = path.map(super::releases::lookup).transpose()?.flatten();
    let release = keyword_release.or(path_release);
    let mut group = path
        .map(|p| resolve_group(corpus, p))
        .transpose()?
        .map(|g| g as u32);
    if let Some(release) = release {
        let target = super::releases::resolve(corpus, release)? as u32;
        if group.is_some_and(|g| g != target) || doc.is_some_and(|d| corpus.docs[d].group != target)
        {
            return err("版本别名与 path/docId 指定的章节范围冲突；请使用一致的章节范围。");
        }
        group = Some(target);
    }
    let speaker_name = args
        .speaker
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let speakers = match speaker_name {
        Some(name) => {
            let ids = match_speakers(corpus, name);
            if ids.is_empty() {
                return err(format!(
                    "没有名为「{name}」的说话人。说话人只覆盖有配音/有头像的台词；想找提及该角色的文本，请把名字放进 keyword"
                ));
            }
            Some(ids)
        }
        None => None,
    };
    let list_group = terms.is_empty() && speakers.is_none() && release.is_some();
    if terms.is_empty() && speakers.is_none() && !list_group {
        return err("keyword 不能为空（或至少提供 speaker）");
    }
    let filter = Filter {
        kind: parse_scope(args.scope.as_deref())?,
        group,
        doc,
        speakers,
        list_group,
        body_only,
    };

    let term_hits = collect_term_hits(corpus, &terms, &filter);
    let df: Vec<usize> = term_hits.iter().map(|t| t.docs.len()).collect();
    let window_mode = args.match_unit.as_deref() == Some("window");
    let mut hits = assemble(
        corpus,
        &term_hits,
        args.allow_partial.unwrap_or(true),
        window_mode,
    );
    match args.match_unit.as_deref().unwrap_or("doc") {
        "doc" => {}
        "window" => {
            if terms.is_empty() {
                return err("window 模式需要正文关键词；仅按 speaker 查询请用 matchUnit=doc");
            }
            apply_windows(corpus, &mut hits, &terms, args);
        }
        _ => return err("matchUnit 只能是 doc / window"),
    }
    // 范围内零命中的词：再按同一 matchIn、不带范围筛选查一次全库，
    // 区分「范围外仍有」与「全库无此词」，避免模型把前者误读为资料库里没有。
    let missing_counts: Vec<super::response::TermCount> = terms
        .iter()
        .zip(&term_hits)
        .filter(|(_, h)| h.docs.is_empty())
        .map(|(t, _)| super::response::TermCount {
            term: t.clone(),
            corpus_documents: if filter.is_unrestricted() {
                0
            } else {
                corpus_doc_count(corpus, t, filter.body_only)
            },
        })
        .collect();

    let mut result = typed_result(corpus, keyword, &terms, &df, &filter, &hits, args)?;
    result.missing_terms = missing_counts.iter().map(|m| m.term.clone()).collect();
    if terms.len() == 1
        && missing_counts
            .first()
            .is_some_and(|m| m.corpus_documents > 0)
    {
        // 单个词只是不在当前范围：给一个去掉范围筛选的调用（建议是调用，不替换结果）。
        let mut widen =
            serde_json::to_value(args).map_err(|_| super::ToolError("无法生成建议参数".into()))?;
        for key in ["cursor", "speaker", "path", "docId", "scope", "pagePart"] {
            widen[key] = serde_json::Value::Null;
        }
        widen["offset"] = serde_json::json!(0);
        widen["directoryOffset"] = serde_json::json!(0);
        result
            .suggestions
            .push(super::response::NextCall::new("ik_search", widen));
    }
    result.missing_term_counts = missing_counts;
    let rendered = render_typed(corpus, &result, doc.is_some(), full);
    let mut out = String::new();
    if let Some(release) = release {
        super::releases::push_note(&mut out, corpus, release);
    }
    if truncated {
        out.push_str("关键词过多，仅检索前 8 个。\n");
    }
    push_missing_terms(&mut out, &result.missing_term_counts, filter.body_only);
    out.push_str(&rendered);
    Ok(if full {
        super::response::ToolOutput::with_rendered_snapshot(
            corpus,
            out,
            super::response::ToolData::Search(result),
        )
    } else {
        super::response::ToolOutput::new(corpus, out, super::response::ToolData::Search(result))
    })
}

/// 一个检索词（或「只按说话人」时的一个虚拟词）的命中。
struct TermHits {
    /// 命中行，升序，已过滤、已核对。
    lines: Vec<u32>,
    /// 元数据（标题 / 路径 / 出场名单）命中的文档，升序。
    title_docs: Vec<u32>,
    /// 行命中 ∪ 标题命中的文档，升序。即该词的文档频率。
    docs: Vec<u32>,
    /// 正文、逐句说话人、身份、标题、路径、姓名表、推断署名。
    fields: [Vec<u32>; 7],
}

/// 逐个检索词取候选行，过滤、核对，并求出各自命中的文档集合。
fn collect_term_hits(corpus: &Corpus, terms: &[String], filter: &Filter) -> Vec<TermHits> {
    let mut scratch = String::new();
    let line_range = filter.doc.map(|d| {
        let r = corpus.docs[d].lines();
        (r.start as u32, r.end as u32)
    });
    let mut out = Vec::with_capacity(terms.len().max(1));

    if terms.is_empty() {
        if filter.list_group {
            let docs = corpus
                .docs
                .iter()
                .enumerate()
                .filter(|(d, _)| filter.accepts_doc(corpus, *d))
                .map(|(d, _)| d as u32)
                .collect();
            out.push(TermHits {
                lines: Vec::new(),
                title_docs: Vec::new(),
                docs,
                fields: Default::default(),
            });
            return out;
        }
        // 只按说话人筛：遍历范围内全部行。
        let (lo, hi) = line_range.unwrap_or((0, corpus.lines.len() as u32));
        let mut cursor = DocCursor::new();
        let lines: Vec<u32> = (lo..hi)
            .filter(|&l| cursor.accepts(corpus, filter, l) && filter.accepts_line(corpus, l))
            .collect();
        let docs = docs_of_lines(corpus, &lines);
        out.push(TermHits {
            lines,
            title_docs: Vec::new(),
            docs,
            fields: Default::default(),
        });
        return out;
    }

    for term in terms {
        let (cands, verify) = match corpus.index.candidates(term) {
            Candidates::Exact(v) => (v, false),
            Candidates::Verify(v) => (v, true),
        };
        let cands = match line_range {
            Some((lo, hi)) => {
                let a = cands.partition_point(|&l| l < lo);
                let b = cands.partition_point(|&l| l < hi);
                &cands[a..b]
            }
            None => &cands[..],
        };
        let mut cursor = DocCursor::new();
        let mut lines: Vec<u32> = cands
            .iter()
            .copied()
            .filter(|&line| {
                cursor.accepts(corpus, filter, line)
                    && filter.accepts_line(corpus, line)
                    && (!verify
                        || find_folded(corpus.line_text(line as usize), term, &mut scratch)
                            .is_some())
            })
            .collect();
        let body_docs = docs_of_lines(corpus, &lines);
        // matchIn=all 保留逐句署名/身份参与同篇 AND；姓名表只生成目录建议。
        let named: Vec<u32> = corpus
            .speakers_folded
            .iter()
            .enumerate()
            .filter(|(i, n)| {
                !filter.body_only
                    && (n.contains(term.as_str())
                        || corpus.speaker_identities[*i].contains(term.as_str()))
            })
            .map(|(i, _)| i as u32)
            .collect();
        let mut cast_docs: Vec<u32> = Vec::new();
        for &sp in &named {
            cast_docs = union_sorted(&cast_docs, corpus.docs_with_speaker(sp));
        }
        let mut fields: [Vec<u32>; 7] = Default::default();
        fields[0] = body_docs;
        if !named.is_empty() && !filter.body_only {
            let mut spoken: Vec<u32> = Vec::new();
            for &d in &cast_docs {
                if !filter.accepts_doc(corpus, d as usize) {
                    continue;
                }
                for l in corpus.docs[d as usize].lines() {
                    let line = l as u32;
                    if named.contains(&corpus.lines[l].speaker)
                        && filter.accepts_line(corpus, line)
                        && (corpus.is_attributed(l) || corpus.is_inferred(l))
                    {
                        spoken.push(line);
                        let sp = corpus.lines[l].speaker as usize;
                        let field = if corpus.is_inferred(l) { 6 } else { 1 };
                        if corpus.speakers_folded[sp].contains(term.as_str())
                            && fields[field].last() != Some(&d)
                        {
                            fields[field].push(d);
                        }
                        let identity_field = if corpus.is_inferred(l) { 6 } else { 2 };
                        if corpus.speaker_identities[sp].contains(term.as_str())
                            && fields[identity_field].last() != Some(&d)
                        {
                            fields[identity_field].push(d);
                        }
                    }
                }
            }
            if !spoken.is_empty() {
                lines = union_sorted(&lines, &spoken);
            }
        }
        let line_docs = docs_of_lines(corpus, &lines);
        // 全库和篇内使用同一元数据条件；按说话人筛选时，
        // 元数据命中但没有该说话人台词的文档不算。
        // 出场名单也算元数据：「狡兔屋 妮可」应能找到妮可出场、台词里提到狡兔屋的场景。
        if !filter.body_only {
            fields[3] = corpus.doc_keys.matching(term, true);
            fields[4] = corpus.doc_keys.matching_path(term);
            fields[5] = cast_docs
                .iter()
                .copied()
                .filter(|&d| {
                    corpus.docs[d as usize]
                        .name_table
                        .iter()
                        .any(|s| named.contains(s))
                })
                .collect();
            for field in &mut fields[1..] {
                field.retain(|&d| {
                    filter.accepts_doc(corpus, d as usize)
                        && (filter.speakers.is_none() || line_docs.binary_search(&d).is_ok())
                });
                field.sort_unstable();
                field.dedup();
            }
        }
        let title_docs: Vec<u32> = {
            fields[3..]
                .iter()
                .fold(Vec::new(), |acc, field| union_sorted(&acc, field))
                .into_iter()
                .filter(|&d| {
                    filter.accepts_doc(corpus, d as usize)
                        && (filter.speakers.is_none() || line_docs.binary_search(&d).is_ok())
                })
                .collect()
        };
        let docs = union_sorted(&line_docs, &title_docs);
        out.push(TermHits {
            lines,
            title_docs,
            docs,
            fields,
        });
    }
    out
}

/// 升序行号 → 升序去重的文档号。
fn docs_of_lines(corpus: &Corpus, lines: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let mut end = 0u32;
    for &line in lines {
        if out.is_empty() || line >= end {
            let d = corpus.doc_of_line(line);
            let doc = &corpus.docs[d];
            end = doc.first_line + doc.line_count;
            out.push(d as u32);
        }
    }
    out
}

/// 先在文档集合上求交（AND），允许部分匹配时退化为并集；筛选范围保持不变。
fn assemble(corpus: &Corpus, terms: &[TermHits], allow_partial: bool, force_union: bool) -> Hits {
    let mut sets: Vec<&[u32]> = terms.iter().map(|t| t.docs.as_slice()).collect();
    sets.sort_by_key(|s| s.len());
    let mut selected: Vec<u32> = sets.first().map(|s| s.to_vec()).unwrap_or_default();
    for s in sets.iter().skip(1) {
        selected = intersect(&selected, s);
    }
    let mut partial = false;
    if force_union || selected.is_empty() && terms.len() > 1 && allow_partial {
        partial = !force_union;
        for s in &sets {
            selected = union_sorted(&selected, s);
        }
    }

    // 只收入选文档的命中行。各词的行本身有序，单个词时无需再排序。
    let mut tagged: Vec<(u32, u32)> = Vec::with_capacity(terms.iter().map(|t| t.lines.len()).sum());
    for (i, t) in terms.iter().enumerate() {
        let bit = 1u32 << i;
        let mut end = 0u32;
        let mut keep = false;
        for &line in &t.lines {
            if line >= end {
                let d = corpus.doc_of_line(line);
                let doc = &corpus.docs[d];
                end = doc.first_line + doc.line_count;
                keep = selected.binary_search(&(d as u32)).is_ok();
            }
            if keep {
                tagged.push((line, bit));
            }
        }
    }
    let lines: Vec<(u32, u32)> = if terms.len() > 1 {
        tagged.sort_unstable_by_key(|&(line, _)| line);
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(tagged.len());
        for (line, bit) in tagged {
            match merged.last_mut() {
                Some((l, mask)) if *l == line => *mask |= bit,
                _ => merged.push((line, bit)),
            }
        }
        merged
    } else {
        tagged
    };

    // 入选文档与命中行都升序且文档行区间互不重叠：一次双指针归并建出文档表。
    let mut docs: Vec<DocHits> = Vec::with_capacity(selected.len());
    let mut p = 0usize;
    for &d in &selected {
        let doc = &corpus.docs[d as usize];
        let end_line = doc.first_line + doc.line_count;
        let start = p;
        let mut tf = [0u32; MAX_TERMS];
        let mut line_mask = 0u32;
        while p < lines.len() && lines[p].0 < end_line {
            let mask = lines[p].1;
            line_mask |= mask;
            for (i, slot) in tf.iter_mut().enumerate() {
                *slot += (mask >> i) & 1;
            }
            p += 1;
        }
        docs.push(DocHits {
            doc: d as usize,
            tf,
            line_mask,
            title_mask: 0,
            start: start as u32,
            end: p as u32,
            score: 0.0,
            fields: [0; 7],
            windows: Vec::new(),
        });
    }
    for (i, t) in terms.iter().enumerate() {
        for &d in &t.title_docs {
            if let Ok(slot) = selected.binary_search(&d) {
                docs[slot].title_mask |= 1 << i;
            }
        }
        for (field, members) in t.fields.iter().enumerate() {
            for &d in members {
                if let Ok(slot) = selected.binary_search(&d) {
                    docs[slot].fields[field] |= 1 << i;
                }
            }
        }
    }
    Hits {
        docs,
        lines,
        partial,
    }
}

/// 顺序扫描时的文档游标：行号单调递增，文档判断只在跨文档时做一次。
struct DocCursor {
    doc: usize,
    end: u32,
    ok: bool,
}

impl DocCursor {
    fn new() -> Self {
        Self {
            doc: usize::MAX,
            end: 0,
            ok: false,
        }
    }

    fn accepts(&mut self, corpus: &Corpus, filter: &Filter, line: u32) -> bool {
        if self.doc == usize::MAX || line >= self.end {
            self.doc = corpus.doc_of_line(line);
            let d = &corpus.docs[self.doc];
            self.end = d.first_line + d.line_count;
            self.ok = filter.accepts_doc(corpus, self.doc);
        }
        self.ok
    }
}
