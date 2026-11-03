//! ik_search：全文检索。
//!
//! 多个关键词用空格分隔，按「同一文档内全部出现」求交（AND）；同时包含全部关键词的文档不到
//! 3 篇时，默认在其后附上明确标注的部分匹配，allowPartial=false 可要求严格 AND。
//! 整句零命中时找字面最接近的原句，并按别称表或拆词重搜。
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
/// 完整命中（全部词同篇）少于这么多篇时，把部分匹配接在完整命中后面，免得词选得严一点就只剩一两篇。
const TOP_UP_BELOW: usize = 3;

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

/// 给定的词里是否有任意两个在同一篇出现（范围内）。
fn any_pair_cooccurs(term_hits: &[TermHits], terms: &[usize]) -> bool {
    terms.iter().enumerate().any(|(a, &i)| {
        terms[a + 1..]
            .iter()
            .any(|&j| !intersect(&term_hits[i].docs, &term_hits[j].docs).is_empty())
    })
}

/// 不合并时的建议调用：有命中的词若彼此同篇出现，给一个检索这些词的调用（去掉零命中词）；
/// 否则每个有命中的词各给一个调用。其余参数与版本选择器原样保留，建议不扩大范围。
fn split_suggestions(
    args: &SearchArgs,
    terms: &[String],
    term_hits: &[TermHits],
) -> Result<Vec<super::response::NextCall>, super::ToolError> {
    let hitting: Vec<usize> = (0..terms.len())
        .filter(|&i| !term_hits[i].docs.is_empty())
        .collect();
    let groups: Vec<Vec<usize>> = if hitting.len() > 1 && any_pair_cooccurs(term_hits, &hitting) {
        vec![hitting]
    } else {
        hitting.into_iter().map(|i| vec![i]).collect()
    };
    let (_, release) = super::releases::query(args.keyword.as_deref().unwrap_or(""))?;
    groups
        .into_iter()
        .map(|group| {
            let mut keyword = group
                .iter()
                .map(|&i| terms[i].as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(release) = release {
                keyword = format!("{} {keyword}", release.version);
            }
            let mut retry = serde_json::to_value(args)
                .map_err(|_| super::ToolError("无法生成建议参数".into()))?;
            retry["keyword"] = serde_json::json!(keyword);
            retry["offset"] = serde_json::json!(0);
            retry["directoryOffset"] = serde_json::json!(0);
            retry["cursor"] = serde_json::Value::Null;
            retry["pagePart"] = serde_json::Value::Null;
            Ok(super::response::NextCall::new("ik_search", retry))
        })
        .collect()
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
            "全库无此词：{}{}。",
            nowhere.join("、"),
            if body_only { "（只查正文）" } else { "" }
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
    /// 全部词同篇出现的文档数；部分匹配接在它们后面。
    complete: usize,
}

pub fn run(corpus: &Corpus, args: &SearchArgs) -> ToolResult {
    execute(corpus, args).map(|r| r.text)
}

/// 检索入口。整句提问（如「安比为什么加入狡兔屋」）常常整句零命中：这时自动拆成语料里存在的词再搜一次，
/// 结果里写明实际用的检索词。显式翻页（cursor）与 purpose=verify（查原话要求原样匹配）不改写。
pub fn execute(corpus: &Corpus, args: &SearchArgs) -> super::response::OutputResult {
    let first = execute_once(corpus, args)?;
    let Some((rewritten, aliases)) = rewrite_for_retry(corpus, args, &first) else {
        return Ok(first);
    };
    let mut retry = args.clone();
    retry.keyword = Some(rewritten.clone());
    let Ok(mut second) = execute_once(corpus, &retry) else {
        return Ok(first);
    };
    let super::response::ToolData::Search(result) = &mut second.response.data else {
        return Ok(first);
    };
    if result.candidate_count == 0 {
        return Ok(first);
    }
    let original = args.keyword.as_deref().unwrap_or("").trim().to_owned();
    result.query = original.clone();
    result.rewritten_keyword = Some(rewritten.clone());
    // 拆词前找到的近似原句一并带上：记错一两个字的台词，最该看的是它们。
    let near = match &first.response.data {
        super::response::ToolData::Search(f) => f.near_lines.clone(),
        _ => Vec::new(),
    };
    let near_text = render_near(&near);
    result.near_lines = near;
    let note = if aliases.is_empty() {
        format!(
            "「{original}」没有命中，已拆成「{rewritten}」检索。结果不理想时，换用角色名、地名、事件名等实体词重搜。\n"
        )
    } else {
        let why = aliases
            .iter()
            .map(|(from, to, kind)| match kind {
                super::aliases::Kind::English => format!("「{from}」是「{to}」的英文名"),
                super::aliases::Kind::Nickname => {
                    format!("「{from}」是玩家对「{to}」的叫法，不是官方称呼")
                }
            })
            .collect::<Vec<_>>()
            .join("；");
        format!("「{original}」没有命中，已改用「{rewritten}」检索：{why}。\n")
    };
    second.text.insert_str(0, &format!("{near_text}{note}"));
    Ok(second)
}

/// 整句至少这么多字才找近似原句；短词零命中多半是换个叫法的问题，不是记错台词。
const NEAR_MIN_CHARS: usize = 6;
/// 近似原句最多列几行。
const NEAR_LIMIT: usize = 3;

/// 整句没有原样出现时，按「相邻两字」的重合比例找最像的几行（至少一半重合）。
/// 用的是现成的二元组倒排，不需要另建索引；结果只说明字面相近，不代表是同一句。
fn near_lines(corpus: &Corpus, term: &str, filter: &Filter) -> Vec<super::response::NearLine> {
    let chars: Vec<char> = term.chars().collect();
    let mut pairs: Vec<(char, char)> = chars.windows(2).map(|w| (w[0], w[1])).collect();
    pairs.sort_unstable();
    pairs.dedup();
    let need = pairs.len().div_ceil(2);
    let mut counts = vec![0u16; corpus.lines.len()];
    let mut touched: Vec<u32> = Vec::new();
    for &(a, b) in &pairs {
        for &line in corpus.index.lines_with_pair(a, b) {
            let slot = &mut counts[line as usize];
            if *slot == 0 {
                touched.push(line);
            }
            *slot += 1;
        }
    }
    let mut best: Vec<(u16, usize, u32)> = touched
        .into_iter()
        .filter(|&line| usize::from(counts[line as usize]) >= need)
        .filter(|&line| {
            filter.accepts_doc(corpus, corpus.doc_of_line(line))
                && filter.accepts_line(corpus, line)
        })
        .map(|line| {
            let len = corpus.line_text(line as usize).chars().count();
            // 同样的重合数，长度越接近检索句越像同一句。
            (counts[line as usize], len.abs_diff(chars.len()), line)
        })
        .collect();
    best.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    best.into_iter()
        .take(NEAR_LIMIT)
        .map(|(count, _, line)| {
            let d = corpus.doc_of_line(line);
            let doc = &corpus.docs[d];
            let source = super::response::SourceLine::new(corpus, line as usize, false);
            let offset = source.number.saturating_sub(5).max(1);
            let doc_id = corpus.doc_id(d);
            super::response::NearLine {
                next_read: super::response::NextCall::new(
                    "ik_read",
                    serde_json::json!({"docId": doc_id, "offset": offset, "limit": 20}),
                ),
                doc_id,
                title: doc.title.clone(),
                path: doc.path.clone(),
                line: source,
                overlap_percent: (usize::from(count) * 100 / pairs.len()).min(100) as u8,
            }
        })
        .collect()
}

fn render_near(near: &[super::response::NearLine]) -> String {
    if near.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "原句没有原样出现。字面最接近的原文（只说明字面相近，引用前请读原文核对）：\n",
    );
    for n in near {
        let speaker = n
            .line
            .speaker
            .as_deref()
            .map_or_else(String::new, |s| format!("{s}："));
        let _ = writeln!(
            out,
            "- {} 「{}」 L{} {speaker}{}（重合 {}%）→ {}",
            n.doc_id,
            n.title,
            n.line.number,
            crate::text::truncate_chars(&n.line.text, 120),
            n.overlap_percent,
            n.next_read.text()
        );
    }
    out
}

/// 句中常见的虚词与疑问词：拆词时去掉，避免它们把同篇 AND 卡死。
const FILLERS: &[&str] = &[
    "为什么",
    "怎么样",
    "是什么",
    "什么样",
    "有没有",
    "是不是",
    "怎么",
    "怎样",
    "如何",
    "什么",
    "哪些",
    "哪个",
    "哪里",
    "哪儿",
    "是谁",
    "请问",
    "一下",
    "关于",
    "之间",
    "为何",
    "吗",
    "呢",
    "吧",
    "啊",
    "了",
    "的",
    "和",
    "与",
    "跟",
    "及",
    "是",
    "在",
    "被",
    "把",
    "对",
    "给",
    "谁",
];

/// 改写时用到的别称：（原词, 换成的词, 来历）。
type AliasUse = (String, &'static str, super::aliases::Kind);

fn rewrite_for_retry(
    corpus: &Corpus,
    args: &SearchArgs,
    first: &super::response::ToolOutput,
) -> Option<(String, Vec<AliasUse>)> {
    if args.cursor.is_some() || args.purpose.as_deref() == Some("verify") {
        return None;
    }
    let super::response::ToolData::Search(result) = &first.response.data else {
        return None;
    };
    if result.candidate_count > 0 && !result.partial_withheld {
        return None;
    }
    let body_only = args.match_in.as_deref() == Some("body");
    let keyword = args.keyword.as_deref()?.trim();
    // 「Ellen Joe」这类带空格的英文名先整体对照。
    if let Some((to, kind)) = super::aliases::lookup(&fold(keyword)) {
        return Some((to.to_owned(), vec![(keyword.to_owned(), to, kind)]));
    }
    let mut terms: Vec<String> = Vec::new();
    let mut aliases: Vec<AliasUse> = Vec::new();
    let mut changed = false;
    for term in keyword.split_whitespace() {
        let absent = result
            .missing_term_counts
            .iter()
            .any(|m| m.term == term && m.corpus_documents == 0)
            || (result.candidate_count == 0 && corpus_doc_count(corpus, term, body_only) == 0);
        if absent && let Some((to, kind)) = super::aliases::lookup(&fold(term)) {
            changed = true;
            aliases.push((term.to_owned(), to, kind));
            if !terms.iter().any(|t| t == to) {
                terms.push(to.to_owned());
            }
            continue;
        }
        // 拆不出任何语料里有的词时保留原词，不悄悄丢掉用户写的检索词。
        let pieces = Some(term)
            .filter(|t| absent && t.chars().count() >= 3)
            .map(|t| segment(corpus, t, body_only))
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| vec![term.to_owned()]);
        if pieces.len() != 1 || pieces[0] != term {
            changed = true;
        }
        for piece in pieces {
            if !terms.contains(&piece) {
                terms.push(piece);
            }
        }
    }
    (changed && !terms.is_empty()).then(|| (terms.join(" "), aliases))
}

/// 单字的人名一般会被当成虚字丢掉；主角兄妹太常被问到，单独保留。
const SINGLE_CHAR_NAMES: &[&str] = &["哲", "铃"];

/// 把一段没有空格的整句拆成语料里存在的词：先认出说话人名字，再去掉虚词与疑问词，
/// 剩下的部分从左到右取语料中出现过的最长片段（2～8 字），单字丢弃。
fn segment(corpus: &Corpus, text: &str, body_only: bool) -> Vec<String> {
    let mut names: Vec<&str> = corpus
        .speakers
        .iter()
        .map(String::as_str)
        .filter(|n| n.chars().count() >= 2 && text.contains(*n))
        .chain(
            SINGLE_CHAR_NAMES
                .iter()
                .copied()
                .filter(|n| text.contains(*n)),
        )
        .collect();
    names.sort_unstable();
    names.dedup();
    names.sort_by_key(|n| std::cmp::Reverse(n.chars().count()));
    // 用占位符切开：名字整体保留，其余片段再拆。
    let mut parts: Vec<(String, bool)> = vec![(text.to_owned(), false)];
    for name in names {
        parts = parts
            .into_iter()
            .flat_map(|(part, fixed)| {
                if fixed || !part.contains(name) {
                    return vec![(part, fixed)];
                }
                let mut out = Vec::new();
                for (i, piece) in part.split(name).enumerate() {
                    if i > 0 {
                        out.push((name.to_owned(), true));
                    }
                    out.push((piece.to_owned(), false));
                }
                out
            })
            .collect();
    }
    let mut words = Vec::new();
    for (part, fixed) in parts {
        if fixed {
            words.push(part);
            continue;
        }
        let mut cleaned = part;
        for filler in FILLERS {
            cleaned = cleaned.replace(filler, " ");
        }
        for chunk in cleaned.split(|c: char| {
            c.is_whitespace()
                || c.is_ascii_punctuation()
                || "，。？！、；：“”‘’（）《》".contains(c)
        }) {
            let chars: Vec<char> = chunk.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let longest = (2..=8.min(chars.len() - i)).rev().find(|&len| {
                    let piece: String = chars[i..i + len].iter().collect();
                    corpus_doc_count(corpus, &piece, body_only) > 0
                });
                match longest {
                    Some(len) => {
                        words.push(chars[i..i + len].iter().collect());
                        i += len;
                    }
                    None => i += 1,
                }
            }
        }
    }
    words
}

fn execute_once(corpus: &Corpus, args: &SearchArgs) -> super::response::OutputResult {
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
    // 部分匹配只有在「至少两个词同篇出现」时才有信息量。有词零命中、或没有任何两个词同篇时，
    // 并集只是各词单独结果的拼接：默认不合并，页眉一次报各词篇数，并在 suggestions 里给出
    // 一跳可用的检索调用。显式 allowPartial 仍按调用方要求（显式值优先）。
    let all_terms: Vec<usize> = (0..terms.len()).collect();
    let zero_term = term_hits.iter().any(|t| t.docs.is_empty());
    let skip_union = args.allow_partial.is_none()
        && terms.len() > 1
        && (zero_term || !any_pair_cooccurs(&term_hits, &all_terms));
    let mut hits = assemble(
        corpus,
        &term_hits,
        args.allow_partial.unwrap_or(true) && !skip_union,
        window_mode && !skip_union,
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
    if terms.len() > 1 {
        result.term_documents = terms
            .iter()
            .zip(&df)
            .map(|(t, &n)| super::response::TermDocuments {
                term: t.clone(),
                documents: n,
            })
            .collect();
    }
    // 所有词都零命中时没有可合并的东西，也没有可建议的拆分调用：按普通「没有命中」处理。
    let withheld = skip_union && term_hits.iter().any(|t| !t.docs.is_empty());
    if withheld {
        result.partial_withheld = true;
        result.suggestions = split_suggestions(args, &terms, &term_hits)?;
    }
    if result.candidate_count == 0
        && args.cursor.is_none()
        && terms.len() == 1
        && terms[0].chars().count() >= NEAR_MIN_CHARS
    {
        result.near_lines = near_lines(corpus, &terms[0], &filter);
    }
    let rendered = render_typed(corpus, &result, doc.is_some(), full);
    let mut out = String::new();
    if let Some(release) = release {
        super::releases::push_note(&mut out, corpus, release);
    }
    if truncated {
        out.push_str("关键词过多，仅检索前 8 个。\n");
    }
    push_missing_terms(&mut out, &result.missing_term_counts, filter.body_only);
    out.push_str(&render_near(&result.near_lines));
    if withheld {
        let counts = output::term_document_counts(&result.term_documents);
        let _ = writeln!(
            out,
            "{}",
            if zero_term {
                format!(
                    "没有全部词都出现的文档，其中有词一次都没出现（{counts}），所以不返回部分匹配；请用下面的「相关查询」。"
                )
            } else {
                format!(
                    "没有全部词都出现的文档，而且任意两个词都没在同一篇出现过（{counts}），所以不返回部分匹配；请用下面的「相关查询」分别检索。"
                )
            }
        );
    }
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
    let complete = selected.len();
    if force_union || complete < TOP_UP_BELOW && terms.len() > 1 && allow_partial {
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
        complete,
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
