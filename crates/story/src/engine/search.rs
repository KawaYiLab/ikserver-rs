//! ik_search：全文检索。
//!
//! 多个关键词用空格分隔，按「同一文档内全部出现」求交（AND）；没有文档同时包含全部关键词时
//! 默认明确标注并返回部分匹配，allowPartial=false 可要求严格 AND。文档打分用行级 BM25，标题/路径命中额外加分。
//! 指定 docId 时变为文档内检索，逐行列出命中与行号。

use std::fmt::Write as _;

use serde::Deserialize;

use super::{
    ToolResult, clamp, err, parse_scope, push_doc_summary, push_line, resolve_doc, resolve_group,
};
use crate::corpus::{Corpus, DocKind};
use crate::index::{Candidates, intersect, union_sorted};
use crate::text::{find_folded, fold, snippet, thousands};

const MAX_TERMS: usize = 8;
const SNIPPET_WIDTH: usize = 90;
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.5;
const TITLE_BOOST: f64 = 1.5;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchArgs {
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
}

/// 检索范围：文档种类 / 分组 / 单篇 / 说话人。
struct Filter {
    kind: Option<DocKind>,
    group: Option<u32>,
    doc: Option<usize>,
    speakers: Option<Vec<u32>>,
    list_group: bool,
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
    };

    let term_hits = collect_term_hits(corpus, &terms, &filter);
    let df: Vec<usize> = term_hits.iter().map(|t| t.docs.len()).collect();
    let hits = assemble(corpus, &term_hits, args.allow_partial.unwrap_or(true));

    let rendered = match doc {
        Some(d) => render_in_doc(
            corpus,
            d,
            keyword,
            speaker_name,
            &terms,
            &hits,
            truncated,
            args.max_results,
        ),
        None => render_ranked(
            corpus,
            keyword,
            speaker_name,
            &terms,
            truncated,
            &df,
            &filter,
            hits,
            args,
        ),
    };
    let mut out = String::new();
    if let Some(release) = release {
        super::releases::push_note(&mut out, corpus, release);
    }
    out.push_str(&rendered);
    Ok(out)
}

/// 一个检索词（或「只按说话人」时的一个虚拟词）的命中。
struct TermHits {
    /// 命中行，升序，已过滤、已核对。
    lines: Vec<u32>,
    /// 元数据（标题 / 路径 / 出场名单）命中的文档，升序。
    title_docs: Vec<u32>,
    /// 行命中 ∪ 标题命中的文档，升序。即该词的文档频率。
    docs: Vec<u32>,
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
        // 说话人名也算行内容：搜「Fairy」要能找到 Fairy 说的话，即使台词里没有这个词。
        let named: Vec<u32> = corpus
            .speakers_folded
            .iter()
            .enumerate()
            .filter(|(i, n)| {
                n.contains(term.as_str()) || corpus.speaker_identities[*i].contains(term.as_str())
            })
            .map(|(i, _)| i as u32)
            .collect();
        let mut cast_docs: Vec<u32> = Vec::new();
        for &sp in &named {
            cast_docs = union_sorted(&cast_docs, corpus.docs_with_speaker(sp));
        }
        if !named.is_empty() {
            let mut spoken: Vec<u32> = Vec::new();
            for &d in &cast_docs {
                if !filter.accepts_doc(corpus, d as usize) {
                    continue;
                }
                for l in corpus.docs[d as usize].lines() {
                    let line = l as u32;
                    if named.contains(&corpus.lines[l].speaker) && filter.accepts_line(corpus, line)
                    {
                        spoken.push(line);
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
        let title_docs: Vec<u32> = {
            union_sorted(&corpus.doc_keys.matching(term, false), &cast_docs)
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
fn assemble(corpus: &Corpus, terms: &[TermHits], allow_partial: bool) -> Hits {
    let mut sets: Vec<&[u32]> = terms.iter().map(|t| t.docs.as_slice()).collect();
    sets.sort_by_key(|s| s.len());
    let mut selected: Vec<u32> = sets.first().map(|s| s.to_vec()).unwrap_or_default();
    for s in sets.iter().skip(1) {
        selected = intersect(&selected, s);
    }
    let mut partial = false;
    if selected.is_empty() && terms.len() > 1 && allow_partial {
        partial = true;
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
        });
    }
    for (i, t) in terms.iter().enumerate() {
        for &d in &t.title_docs {
            if let Ok(slot) = selected.binary_search(&d) {
                docs[slot].title_mask |= 1 << i;
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

#[allow(clippy::too_many_arguments)]
fn render_ranked(
    corpus: &Corpus,
    keyword: &str,
    speaker: Option<&str>,
    terms: &[String],
    truncated: bool,
    df: &[usize],
    filter: &Filter,
    hits: Hits,
    args: &SearchArgs,
) -> String {
    let Hits {
        docs: mut hits,
        lines: hit_lines,
        partial,
    } = hits;
    let max = clamp(args.max_results, 10, 50);
    let per_doc = clamp(args.hits_per_doc, 3, 20);
    let n_terms = terms.len().max(1);
    let all_mask: u32 = if terms.is_empty() {
        0
    } else {
        (1u32 << terms.len()) - 1
    };

    // 平均文档长度（行）与逆文档频率。
    let n_docs = corpus.docs.len().max(1) as f64;
    let avg_len = corpus.lines.len() as f64 / n_docs;
    let idf: Vec<f64> = (0..n_terms)
        .map(|i| {
            let df = df.get(i).copied().unwrap_or(0) as f64;
            (1.0 + (n_docs - df + 0.5) / (df + 0.5)).ln()
        })
        .collect();
    for h in &mut hits {
        let len = f64::from(corpus.docs[h.doc].line_count.max(1));
        let norm = BM25_K1 * (1.0 - BM25_B + BM25_B * len / avg_len);
        let mut score = 0.0;
        if terms.is_empty() {
            score = h.line_count() as f64;
        }
        for (i, idf) in idf.iter().enumerate().take(terms.len()) {
            let tf = f64::from(h.tf[i]);
            score += idf * tf * (BM25_K1 + 1.0) / (tf + norm);
            if (h.title_mask >> i) & 1 == 1 {
                score += idf * TITLE_BOOST;
            }
        }
        h.score = score;
    }

    let coverage = |h: &DocHits| ((h.line_mask | h.title_mask) & all_mask).count_ones();
    let rank = |a: &DocHits, b: &DocHits| {
        coverage(b)
            .cmp(&coverage(a))
            .then(b.score.total_cmp(&a.score))
            .then(a.doc.cmp(&b.doc))
    };
    let total_docs = hits.len();
    let total_lines = hit_lines.len();
    // 只需要前 max 篇：先线性选出前 max，再对这一小段排序。
    if hits.len() > max {
        hits.select_nth_unstable_by(max - 1, rank);
        hits.truncate(max);
    }
    hits.sort_by(rank);

    let mut out = String::with_capacity(4096);
    let _ = write!(out, "[ik_search] 「{keyword}」");
    if let Some(s) = speaker {
        let _ = write!(out, " | 说话人: {s}");
    }
    if let Some(k) = filter.kind {
        let _ = write!(out, " | 范围: {}", k.label());
    }
    if let Some(g) = filter.group {
        let g = &corpus.groups[g as usize];
        let _ = write!(out, " | 分组: {} {}", g.id, g.label);
    }
    let _ = writeln!(
        out,
        " | 命中 {} 篇 · {} 行（显示前 {}）",
        thousands(total_docs),
        thousands(total_lines),
        hits.len()
    );
    super::push_version(&mut out, corpus);
    if filter.list_group {
        out.push_str("仅按版本范围列出候选场景，未执行正文关键词匹配。\n");
    }
    if truncated {
        let _ = writeln!(out, "注意：最多使用前 {MAX_TERMS} 个关键词。");
    }
    if partial {
        out.push_str("没有文档同时包含全部关键词，以下按覆盖的关键词数排序（部分匹配）。部分匹配不证明全部关键词共现；allowPartial=false 可要求严格匹配。\n");
    }
    if hits.is_empty() {
        if terms.len() > 1 && !partial {
            out.push_str("AND 查询没有共同命中；可显式设置 allowPartial=true 查看部分匹配。\n");
        }
        out.push_str(
            "没有命中。建议：换用角色名/地名/组织名等实体词；减少关键词；去掉 scope/path/speaker 限制；\
             或用 ik_catalog(keyword=…) 按标题查找。",
        );
        return out;
    }

    let mut scratch = String::new();
    for (rank, h) in hits.iter().enumerate() {
        let _ = write!(out, "{}. ", rank + 1);
        push_doc_summary(&mut out, corpus, h.doc);
        let _ = write!(out, " | 命中 {} 行", h.line_count());
        if h.title_mask != 0 {
            out.push_str("（标题/出场命中）");
        }
        out.push('\n');
        super::push_order_note(&mut out, corpus, h.doc);
        if partial {
            let missing: Vec<&str> = terms
                .iter()
                .enumerate()
                .filter(|(i, _)| ((h.line_mask | h.title_mask) >> i) & 1 == 0)
                .map(|(_, term)| term.as_str())
                .collect();
            let _ = writeln!(out, "   未命中关键词：{}", missing.join("、"));
        }
        // 先挑覆盖词最多的行，再按行号展示。
        let mut picked: Vec<(u32, u32)> = hit_lines[h.start as usize..h.end as usize].to_vec();
        picked.sort_by(|a, b| b.1.count_ones().cmp(&a.1.count_ones()).then(a.0.cmp(&b.0)));
        picked.truncate(per_doc);
        picked.sort_unstable_by_key(|&(l, _)| l);
        let first = corpus.docs[h.doc].first_line;
        let anchor = picked
            .first()
            .map(|&(line, _)| (line - first + 1).saturating_sub(5).max(1))
            .or_else(|| (corpus.docs[h.doc].line_count > 0).then_some(1));
        let context = args
            .context_lines
            .unwrap_or(usize::from(speaker.is_none()))
            .min(3);
        let doc = &corpus.docs[h.doc];
        let mut shown: Vec<u32> = picked
            .iter()
            .flat_map(|&(line, _)| {
                line.saturating_sub(context as u32).max(first)
                    ..(line + context as u32 + 1).min(first + doc.line_count)
            })
            .collect();
        shown.sort_unstable();
        shown.dedup();
        for line in shown {
            let mask = picked
                .binary_search_by_key(&line, |&(l, _)| l)
                .ok()
                .map(|i| picked[i].1);
            let text = corpus.line_text(line as usize);
            let term = mask.and_then(|m| terms.get(m.trailing_zeros() as usize));
            let at = term.and_then(|t| find_folded(text, t, &mut scratch));
            let _ = write!(
                out,
                "   {}L{} ",
                if mask.is_some() { "" } else { "上下文 " },
                line - first + 1
            );
            push_line(
                &mut out,
                corpus,
                line as usize,
                &snippet(text, at, SNIPPET_WIDTH),
            );
            if let Some(mask) = mask {
                let names: Vec<&str> = terms
                    .iter()
                    .enumerate()
                    .filter(|(i, t)| {
                        mask & (1 << i) != 0 && find_folded(text, t, &mut scratch).is_none()
                    })
                    .map(|(_, t)| t.as_str())
                    .collect();
                if !names.is_empty() {
                    let _ = write!(out, " [说话人名/身份匹配：{}]", names.join("、"));
                }
            }
            out.push('\n');
        }
        if let Some(offset) = anchor {
            let _ = writeln!(
                out,
                "   读上下文：ik_read(docId=\"{}\", offset={offset}, limit=20)",
                corpus.doc_id(h.doc)
            );
        }
    }
    if total_docs > hits.len() {
        let _ = writeln!(
            out,
            "…另有 {} 篇未显示，可加 path/scope/speaker 缩小范围或调大 maxResults。",
            total_docs - hits.len()
        );
    }
    out.push_str("下一步：使用各篇的精确 ik_read 调用读上下文；ik_search(docId=…, keyword=…) 列出该篇全部命中行。");
    out
}

fn render_in_doc(
    corpus: &Corpus,
    doc: usize,
    keyword: &str,
    speaker: Option<&str>,
    terms: &[String],
    hits: &Hits,
    truncated: bool,
    max_results: Option<usize>,
) -> String {
    let max = clamp(max_results, 50, 500);
    let mut out = String::with_capacity(4096);
    out.push_str("[ik_search] 篇内检索 ");
    push_doc_summary(&mut out, corpus, doc);
    let _ = write!(out, "\n关键词「{keyword}」");
    if let Some(s) = speaker {
        let _ = write!(out, " | 说话人: {s}");
    }
    let shown = &hits.lines;
    let mode = if hits.partial {
        "部分匹配"
    } else {
        "同篇 AND"
    };
    let _ = writeln!(
        out,
        " | 命中 {} 行（{mode}，列出各关键词的命中行）",
        shown.len()
    );
    super::push_version(&mut out, corpus);
    if truncated {
        out.push_str("关键词过多，仅检索前 8 个。\n");
    }
    super::push_order_note(&mut out, corpus, doc);
    if hits.docs.iter().any(|h| h.title_mask != 0) {
        out.push_str("标题/出场命中计入同篇 AND；正文命中行单独列出。\n");
    }
    if hits.partial {
        let mask = hits
            .docs
            .iter()
            .fold(0u32, |acc, h| acc | h.line_mask | h.title_mask);
        let missing: Vec<&str> = terms
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) == 0)
            .map(|(_, t)| t.as_str())
            .collect();
        let _ = writeln!(
            out,
            "部分匹配；未命中关键词：{}。不证明全部关键词共现；allowPartial=false 可要求严格匹配。",
            missing.join("、")
        );
    } else if hits.docs.is_empty() && terms.len() > 1 {
        out.push_str("AND 查询没有共同命中；可显式设置 allowPartial=true 查看部分匹配。\n");
    }
    let first = corpus.docs[doc].first_line;
    let mut scratch = String::new();
    for &(line, mask) in shown.iter().take(max) {
        let text = corpus.line_text(line as usize);
        let at = terms
            .get(mask.trailing_zeros() as usize)
            .and_then(|t| find_folded(text, t, &mut scratch));
        let _ = write!(out, "L{} ", line - first + 1);
        push_line(
            &mut out,
            corpus,
            line as usize,
            &snippet(text, at, SNIPPET_WIDTH * 2),
        );
        let names: Vec<&str> = terms
            .iter()
            .enumerate()
            .filter(|(i, t)| mask & (1 << i) != 0 && find_folded(text, t, &mut scratch).is_none())
            .map(|(_, t)| t.as_str())
            .collect();
        if !names.is_empty() {
            let _ = write!(out, " [说话人名/身份匹配：{}]", names.join("、"));
        }
        out.push('\n');
    }
    if shown.len() > max {
        let _ = writeln!(
            out,
            "…另有 {} 行未显示（调大 maxResults）。",
            shown.len() - max
        );
    }
    out
}
