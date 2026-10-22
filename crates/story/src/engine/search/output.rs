use super::*;
use crate::engine::{cursor, response::*};
use crate::text::snippet;
use serde_json::json;

/// Minimal covers in document row order. They make no playback/branch claim.
pub(super) fn apply_windows(c: &Corpus, hits: &mut Hits, terms: &[String], args: &SearchArgs) {
    let width = args.window_lines.unwrap_or(12).clamp(1, 100) as u32;
    let all = (1u32 << terms.len()) - 1;
    for h in &mut hits.docs {
        let rows = &hits.lines[h.start as usize..h.end as usize];
        let mut counts = [0usize; MAX_TERMS];
        let mut left = 0;
        let mut mask = 0;
        for right in 0..rows.len() {
            for (t, n) in counts.iter_mut().enumerate().take(terms.len()) {
                if rows[right].1 & (1 << t) != 0 {
                    *n += 1;
                    mask |= 1 << t;
                }
            }
            while left <= right && rows[right].0 - rows[left].0 + 1 > width {
                for (t, n) in counts.iter_mut().enumerate().take(terms.len()) {
                    if rows[left].1 & (1 << t) != 0 {
                        *n -= 1;
                        if *n == 0 {
                            mask &= !(1 << t);
                        }
                    }
                }
                left += 1;
            }
            if mask == all && left <= right {
                while left < right
                    && counts
                        .iter()
                        .enumerate()
                        .take(terms.len())
                        .all(|(t, n)| rows[left].1 & (1 << t) == 0 || *n > 1)
                {
                    for (t, n) in counts.iter_mut().enumerate().take(terms.len()) {
                        if rows[left].1 & (1 << t) != 0 {
                            *n -= 1;
                        }
                    }
                    left += 1;
                }
                if h.windows.last().is_none_or(|&(a, _)| a < rows[left].0) {
                    h.windows.push((rows[left].0, rows[right].0));
                }
            }
        }
    }
    let complete = hits.docs.iter().any(|h| !h.windows.is_empty());
    hits.partial = !complete && args.allow_partial.unwrap_or(true);
    hits.docs.retain(|h| {
        if complete {
            !h.windows.is_empty()
        } else {
            args.allow_partial.unwrap_or(true) && h.line_count() > 0
        }
    });
    if !complete && args.allow_partial.unwrap_or(true) {
        for h in &mut hits.docs {
            let rows = &hits.lines[h.start as usize..h.end as usize];
            let mut best = (0, 0, 0u32);
            for (i, &(start, _)) in rows.iter().enumerate() {
                let mut mask = 0;
                let mut end = start;
                for &(l, m) in &rows[i..] {
                    if l - start + 1 > width {
                        break;
                    }
                    if m & !mask != 0 {
                        end = l;
                    }
                    mask |= m;
                }
                if mask.count_ones() > best.2.count_ones()
                    || mask.count_ones() == best.2.count_ones() && end - start < best.1 - best.0
                {
                    best = (start, end, mask);
                }
            }
            h.windows.push((best.0, best.1));
        }
    }
    // Rows outside accepted windows cannot contribute to reported window matches.
    let mut lines = Vec::new();
    for h in &mut hits.docs {
        let rows = &hits.lines[h.start as usize..h.end as usize];
        h.start = lines.len() as u32;
        h.line_mask = 0;
        h.tf = [0; MAX_TERMS];
        h.fields = [0; 7];
        h.title_mask = 0;
        for &(l, m) in rows {
            if h.windows.iter().any(|&(a, b)| a <= l && l <= b) {
                lines.push((l, m));
                h.line_mask |= m;
                for (t, term) in terms.iter().enumerate() {
                    if m & (1 << t) != 0 {
                        h.tf[t] += 1;
                        let mut scratch = String::new();
                        if find_folded(c.line_text(l as usize), term, &mut scratch).is_some() {
                            h.fields[0] |= 1 << t;
                        }
                        let sp = c.lines[l as usize].speaker as usize;
                        let named = c
                            .speakers_folded
                            .get(sp)
                            .is_some_and(|name| name.contains(term.as_str()))
                            || c.speaker_identities
                                .get(sp)
                                .is_some_and(|name| name.contains(term.as_str()));
                        if args.match_in.as_deref() == Some("body") {
                            continue;
                        }
                        if c.is_inferred(l as usize) && named {
                            h.fields[6] |= 1 << t;
                        } else if c.is_attributed(l as usize) {
                            if c.speakers_folded
                                .get(sp)
                                .is_some_and(|name| name.contains(term.as_str()))
                            {
                                h.fields[1] |= 1 << t;
                            }
                            if c.speaker_identities
                                .get(sp)
                                .is_some_and(|name| name.contains(term.as_str()))
                            {
                                h.fields[2] |= 1 << t;
                            }
                        }
                    }
                }
            }
        }
        h.end = lines.len() as u32;
    }
    hits.lines = lines;
}

#[allow(clippy::too_many_arguments)]
pub(super) fn typed_result(
    c: &Corpus,
    keyword: &str,
    terms: &[String],
    df: &[usize],
    f: &Filter,
    hits: &Hits,
    args: &SearchArgs,
) -> Result<SearchResult, super::super::ToolError> {
    let all = (1u32 << terms.len()) - 1;
    let n = c.docs.len().max(1) as f64;
    let avg = c.lines.len().max(1) as f64 / n;
    let mut docs = hits.docs.clone();
    for h in &mut docs {
        let norm =
            BM25_K1 * (1.0 - BM25_B + BM25_B * f64::from(c.docs[h.doc].line_count.max(1)) / avg);
        for (i, _) in terms.iter().enumerate() {
            let idf = (1.0
                + (n - df.get(i).copied().unwrap_or(0) as f64 + 0.5)
                    / (df.get(i).copied().unwrap_or(0) as f64 + 0.5))
                .ln();
            let tf = f64::from(h.tf[i]);
            h.score += idf * tf * (BM25_K1 + 1.0) / (tf + norm);
            if h.title_mask & (1 << i) != 0 {
                h.score += idf * TITLE_BOOST;
            }
        }
        if terms.is_empty() {
            h.score = h.line_count() as f64;
        }
    }
    let coverage = |h: &DocHits| ((h.line_mask | h.title_mask) & all).count_ones();
    docs.sort_by(|a, b| {
        (coverage(b) == terms.len() as u32)
            .cmp(&(coverage(a) == terms.len() as u32))
            .then(b.fields[0].count_ones().cmp(&a.fields[0].count_ones()))
            .then(
                (b.fields[1] | b.fields[2])
                    .count_ones()
                    .cmp(&(a.fields[1] | a.fields[2]).count_ones()),
            )
            .then(coverage(b).cmp(&coverage(a)))
            .then(b.score.total_cmp(&a.score))
            .then(c.docs[a.doc].hash.cmp(&c.docs[b.doc].hash))
    });
    let (body, directory): (Vec<_>, Vec<_>) = docs.iter().partition(|h| h.line_count() > 0);
    let max = super::super::clamp(
        args.max_results,
        if f.doc.is_some() { 50 } else { 10 },
        if f.doc.is_some() { 500 } else { 50 },
    );
    let offset = args.offset.unwrap_or(0);
    let dm = super::super::clamp(args.max_directory_results, 10, 50);
    let off = args.directory_offset.unwrap_or(0);
    let part = args.page_part.as_deref().unwrap_or("both");
    if !matches!(part, "both" | "body" | "directory") {
        return err("pagePart 只能是 both / body / directory");
    }
    let per = if f.doc.is_some() {
        max
    } else {
        super::super::clamp(args.hits_per_doc, 3, 20)
    };
    let original_args = serde_json::to_value(args)
        .map_err(|_| super::super::ToolError("无法生成查询参数".into()))?;
    let item = |h: &&DocHits| {
        let d = &c.docs[h.doc];
        let rows = &hits.lines[h.start as usize..h.end as usize];
        let mut picked = rows.to_vec();
        if f.doc.is_some() {
            picked = picked.into_iter().skip(offset).take(per).collect();
        } else {
            picked.sort_by(|a, b| b.1.count_ones().cmp(&a.1.count_ones()).then(a.0.cmp(&b.0)));
            picked.truncate(per);
            picked.sort_unstable();
        }
        let context = args
            .context_lines
            .unwrap_or(usize::from(f.speakers.is_none()))
            .min(3) as u32;
        let windows = if f.doc.is_some() {
            h.windows
                .iter()
                .skip(offset)
                .take(per)
                .copied()
                .collect::<Vec<_>>()
        } else {
            h.windows
                .iter()
                .take(per.min(20))
                .copied()
                .collect::<Vec<_>>()
        };
        let mut shown: Vec<u32> = if !windows.is_empty() {
            windows
                .iter()
                .flat_map(|&(a, b)| {
                    a.saturating_sub(context).max(d.first_line)
                        ..(b + context + 1).min(d.first_line + d.line_count)
                })
                .collect()
        } else {
            picked
                .iter()
                .flat_map(|&(l, _)| {
                    l.saturating_sub(context).max(d.first_line)
                        ..(l + context + 1).min(d.first_line + d.line_count)
                })
                .collect()
        };
        shown.sort_unstable();
        shown.dedup();
        let lines = shown
            .into_iter()
            .map(|l| {
                let mut line = SourceLine::new(c, l as usize, args.include_evidence == Some(true));
                let m = if windows.is_empty() {
                    picked
                        .binary_search_by_key(&l, |r| r.0)
                        .ok()
                        .map(|i| picked[i].1)
                } else {
                    rows.binary_search_by_key(&l, |r| r.0)
                        .ok()
                        .filter(|_| windows.iter().any(|&(a, b)| a <= l && l <= b))
                        .map(|i| rows[i].1)
                };
                line.is_context = m.is_none();
                line.matched_terms = terms
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| m.is_some_and(|m| m & (1 << i) != 0))
                    .map(|(_, t)| t.clone())
                    .collect();
                let width = 180;
                if line.text.chars().count() > width {
                    let mut scratch = String::new();
                    let at = line
                        .matched_terms
                        .iter()
                        .find_map(|t| find_folded(&line.text, t, &mut scratch));
                    line.text = snippet(&line.text, at, width);
                    line.complete = false;
                }
                line
            })
            .collect();
        let labels = [
            "body",
            "speaker",
            "identity",
            "title",
            "path",
            "nameTable",
            "inferred",
        ];
        let matching_rows = if f.doc.is_some() {
            if windows.is_empty() {
                picked.clone()
            } else {
                rows.iter()
                    .filter(|r| windows.iter().any(|&(a, b)| a <= r.0 && r.0 <= b))
                    .copied()
                    .collect()
            }
        } else {
            rows.to_vec()
        };
        let matches = terms
            .iter()
            .enumerate()
            .map(|(i, t)| TermMatch {
                term: t.clone(),
                fields: labels
                    .iter()
                    .enumerate()
                    .filter(|(f, _)| h.fields[*f] & (1 << i) != 0)
                    .map(|(_, s)| (*s).into())
                    .collect(),
                lines: matching_rows
                    .iter()
                    .filter(|r| r.1 & (1 << i) != 0)
                    .map(|r| (r.0 - d.first_line + 1) as usize)
                    .take(500)
                    .collect(),
                lines_truncated: matching_rows.iter().filter(|r| r.1 & (1 << i) != 0).count() > 500,
            })
            .collect();
        let mut document = Document::new(c, h.doc).with_evidence(
            c,
            h.doc,
            h.line_count() == 0 && args.include_evidence == Some(true),
        );
        document = document.with_summary_detail(c, h.doc, args.detail.as_deref() == Some("full"));
        let anchor = windows
            .first()
            .map(|w| w.0)
            .or_else(|| picked.first().map(|r| r.0))
            .map(|line| (line - d.first_line + 1).saturating_sub(5).max(1))
            .unwrap_or(1);
        document.next_read = NextCall::new(
            "ik_read",
            json!({"docId":document.doc_id,"offset":anchor,"limit":windows.first().map(|&(a,b)|(b-a+6+context*2).max(20)).unwrap_or(20),"includeEvidence":if args.include_evidence==Some(true){Some(true)}else{None}}),
        );
        SearchItem {
            document,
            result_type: if h.line_count() > 0 {
                "body"
            } else {
                "directory"
            }
            .into(),
            matches,
            missing_terms: terms
                .iter()
                .enumerate()
                .filter(|(i, _)| (h.line_mask | h.title_mask) & (1 << i) == 0)
                .map(|(_, t)| t.clone())
                .collect(),
            body_missing_terms: terms
                .iter()
                .enumerate()
                .filter(|(i, _)| h.fields[0] & (1 << i) == 0)
                .map(|(_, t)| t.clone())
                .collect(),
            lines,
            hit_count: h.line_count(),
            windows_truncated: h.windows.len() > windows.len(),
            next_matches: if f.doc.is_none()
                && (if h.windows.is_empty() {
                    h.line_count() > picked.len()
                } else {
                    h.windows.len() > windows.len()
                }) {
                let mut value = original_args.clone();
                value["docId"] = json!(c.doc_id(h.doc));
                value["offset"] = json!(if h.windows.is_empty() {
                    0
                } else {
                    windows.len()
                });
                value["pagePart"] = json!("body");
                value["maxResults"] = json!(50);
                Some(NextCall::new("ik_search", value))
            } else {
                None
            },
            windows: windows
                .iter()
                .map(|&(a, b)| Window {
                    start: (a - d.first_line + 1) as usize,
                    end: (b - d.first_line + 1) as usize,
                    order_source: {
                        let sources = (a..=b)
                            .map(|l| {
                                c.line_evidence(l as usize)
                                    .line_order_source
                                    .as_deref()
                                    .unwrap_or("legacy_unrecorded")
                            })
                            .collect::<std::collections::BTreeSet<_>>();
                        sources.into_iter().collect::<Vec<_>>().join(" + ")
                    },
                    order_status: "playback_unverified".into(),
                    local_order_status: (a..=b)
                        .map(|l| {
                            c.line_evidence(l as usize)
                                .line_order_status
                                .as_deref()
                                .unwrap_or("unknown")
                                .to_owned()
                        })
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                })
                .collect(),
            related_status: "unknown".into(),
        }
    };
    let results = if part == "directory" {
        Vec::new()
    } else if f.doc.is_some() {
        body.iter().map(item).collect()
    } else {
        body.iter().skip(offset).take(max).map(item).collect()
    };
    let directory_suggestions = if part == "body" {
        Vec::new()
    } else {
        directory.iter().skip(off).take(dm).map(item).collect()
    };
    let mut base = original_args;
    base["cursor"] = json!(null);
    let more = if f.doc.is_some() {
        body.first()
            .map(|h| {
                if h.windows.is_empty() {
                    h.line_count()
                } else {
                    h.windows.len()
                }
            })
            .unwrap_or(0)
            > offset.saturating_add(max)
    } else {
        body.len() > offset.saturating_add(max)
    };
    let next_call = if part != "directory" && more {
        let mut v = base.clone();
        v["offset"] = json!(offset.saturating_add(max));
        v["pagePart"] = json!("body");
        Some(cursor::next(c, "ik_search", v)?)
    } else {
        None
    };
    let next_directory_call = if part != "body" && directory.len() > off.saturating_add(dm) {
        let mut v = base.clone();
        v["directoryOffset"] = json!(off.saturating_add(dm));
        v["pagePart"] = json!("directory");
        Some(cursor::next(c, "ik_search", v)?)
    } else {
        None
    };
    let mut suggestions = Vec::new();
    if docs.is_empty() {
        if args.allow_partial == Some(false) {
            base["allowPartial"] = json!(true);
            suggestions.push(NextCall::new("ik_search", base));
        }
        // 有零命中词时去掉这些词重查；否则退回只搜第一个词。单个词时这就是原调用本身，
        // 不给这种原地重试的建议。版本选择器原样保留，建议不能悄悄扩大范围。
        let hitting: Vec<&str> = terms
            .iter()
            .zip(df)
            .filter(|(_, n)| **n > 0)
            .map(|(t, _)| t.as_str())
            .collect();
        let retry_terms: Vec<&str> = if !hitting.is_empty() && hitting.len() < terms.len() {
            hitting
        } else {
            terms.iter().take(1).map(String::as_str).collect()
        };
        if terms.len() > 1 && !retry_terms.is_empty() {
            let mut keyword = retry_terms.join(" ");
            if let (_, Some(release)) =
                super::super::releases::query(args.keyword.as_deref().unwrap_or(""))?
            {
                keyword = format!("{} {keyword}", release.version);
            }
            let mut retry = serde_json::to_value(args)
                .map_err(|_| super::super::ToolError("无法生成建议参数".into()))?;
            retry["keyword"] = json!(keyword);
            retry["offset"] = json!(0);
            retry["directoryOffset"] = json!(0);
            retry["cursor"] = json!(null);
            suggestions.push(NextCall::new("ik_search", retry));
        }
    }
    Ok(SearchResult {
        range_info: Some(RangeInfo::new(
            args.path.as_deref().or(args.keyword.as_deref()),
            f.group
                .map(|g| vec![c.groups[g as usize].id.clone()])
                .or_else(|| {
                    f.doc
                        .map(|d| vec![c.groups[c.docs[d].group as usize].id.clone()])
                })
                .unwrap_or_default(),
            super::super::releases::query(keyword)?
                .1
                .or(super::super::releases::lookup(
                    args.path.as_deref().unwrap_or(""),
                )?),
            if f.doc.is_some() {
                "document"
            } else {
                "search"
            },
        )),
        query: keyword.into(),
        resolved_range: SearchRange {
            scope: match f.kind {
                Some(k) => match k {
                    crate::corpus::DocKind::Story => "story",
                    crate::corpus::DocKind::Supplement => "supplement",
                    crate::corpus::DocKind::Lore => "lore",
                },
                None => "all",
            }
            .into(),
            group_id: f.group.map(|g| c.groups[g as usize].id.clone()),
            doc_id: f.doc.map(|d| c.doc_id(d)),
            speaker: args.speaker.clone(),
            match_in: if f.body_only { "body" } else { "all" }.into(),
            match_unit: args.match_unit.as_deref().unwrap_or("doc").into(),
            window_lines: args.window_lines.unwrap_or(12).clamp(1, 100),
        },
        partial: hits.partial,
        candidate_count: docs.len(),
        body_result_count: body.len(),
        directory_count: directory.len(),
        hit_count: hits.docs.iter().map(DocHits::line_count).sum(),
        missing_terms: Vec::new(),
        missing_term_counts: Vec::new(),
        results,
        directory_suggestions,
        next_call,
        next_directory_call,
        suggestions,
    })
}

pub(super) fn render_typed(c: &Corpus, r: &SearchResult, local: bool, full: bool) -> String {
    let mut out = format!(
        "[ik_search] 「{}」 | 命中 {} 篇 · 命中 {} 行 | 正文结果 {} 篇 · 目录建议 {} 篇\n",
        r.query, r.candidate_count, r.hit_count, r.body_result_count, r.directory_count
    );
    if local {
        out.insert_str(12, "篇内检索 ");
    }
    if full {
        super::super::push_version(&mut out, c);
    }
    if let Some(info) = &r.range_info {
        info.push_text(&mut out);
    }
    if r.partial {
        out.push_str("部分匹配，不证明全部关键词共现。\n");
    }
    if r.candidate_count == 0 {
        // 去掉范围筛选的建议只在「范围外仍有该词」时出现，必须如实说明它会扩大范围。
        let widens = r.suggestions.iter().any(|s| {
            s.arguments.get("speaker").is_none() && r.resolved_range.speaker.is_some()
                || s.arguments.get("path").is_none() && r.resolved_range.group_id.is_some()
                || s.arguments.get("docId").is_none() && r.resolved_range.doc_id.is_some()
                || s.arguments.get("scope").is_none() && r.resolved_range.scope != "all"
        });
        out.push_str(if widens {
            "没有命中。可使用 suggestions 中的调用调整条件；其中去掉范围筛选的调用会扩大检索范围。\n"
        } else {
            "没有命中。可使用 suggestions 中的调用调整条件，范围保持不变。\n"
        });
    }
    let labels = [
        ("body", "正文"),
        ("speaker", "逐句说话人"),
        ("identity", "角色身份"),
        ("title", "标题"),
        ("path", "章节/路径"),
        ("nameTable", "TextMap 姓名表"),
        ("inferred", "推断署名"),
    ];
    for item in r.results.iter().chain(&r.directory_suggestions) {
        if item.result_type == "directory"
            && r.directory_suggestions
                .first()
                .is_some_and(|first| first.document.doc_id == item.document.doc_id)
        {
            out.push_str("目录建议（仅元数据；不计为正文命中）：\n");
        }
        let d = &item.document;
        if item.result_type == "body" {
            let _ = write!(
                out,
                "{}. ",
                r.results
                    .iter()
                    .position(|p| p.document.doc_id == d.doc_id)
                    .unwrap_or(0)
                    + 1
            );
        }
        let _ = writeln!(
            out,
            "{} 「{}」 {} | 命中 {} 行{}",
            d.doc_id,
            d.title,
            d.path,
            item.hit_count,
            if item.result_type == "directory" {
                "（仅元数据）"
            } else {
                ""
            }
        );
        d.push_summary(&mut out, full);
        if full {
            let words = |body: bool| {
                item.matches
                    .iter()
                    .filter(|m| m.fields.iter().any(|f| (f == "body") == body))
                    .map(|m| m.term.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            };
            let _ = writeln!(
                out,
                "正文匹配词：{} | 元数据匹配词：{}",
                words(true),
                words(false)
            );
            for m in &item.matches {
                let names = m
                    .fields
                    .iter()
                    .filter_map(|f| labels.iter().find(|p| p.0 == f).map(|p| p.1))
                    .collect::<Vec<_>>();
                let _ = writeln!(out, "{}：{}", m.term, names.join("、"));
            }
            if !item.body_missing_terms.is_empty() {
                let _ = writeln!(out, "正文缺失词：{}", item.body_missing_terms.join("、"));
            }
            if item
                .matches
                .iter()
                .any(|m| m.fields.contains(&"title".into()))
            {
                out.push_str("标题/出场命中计入同篇 AND；正文命中行单独列出。\n");
            }
        }
        if !item.missing_terms.is_empty() {
            let _ = writeln!(out, "未命中关键词：{}", item.missing_terms.join("、"));
        }
        for l in &item.lines {
            let _ = write!(
                out,
                "{}L{} {}",
                if l.is_context { "上下文 " } else { "" },
                l.number,
                l.display()
            );
            if full || l.evidence.is_some() {
                l.push_sources(&mut out);
            }
            if let Some(e) = &l.evidence {
                let _ = write!(out, " [提取证据：{e}]");
            }
            let mut scratch = String::new();
            let names = l
                .matched_terms
                .iter()
                .filter(|t| find_folded(&l.text, t, &mut scratch).is_none())
                .cloned()
                .collect::<Vec<_>>();
            if !names.is_empty() {
                let _ = write!(out, " [说话人名/身份匹配：{}]", names.join("、"));
            }
            out.push('\n');
        }
        for l in &d.evidence_preview {
            let _ = write!(out, "L{}", l.number);
            l.push_sources(&mut out);
            if let Some(e) = &l.evidence {
                let _ = write!(out, " [提取证据：{e}]");
            }
            out.push('\n');
        }
        let _ = writeln!(out, "原文入口：{}", d.next_read.text());
        if let Some(call) = &item.next_matches {
            let _ = writeln!(out, "篇内更多命中：{}", call.text());
        }
    }
    if local && r.results.is_empty() {
        out.push_str("命中 0 行\n");
    }
    for (label, call) in [
        ("继续正文", &r.next_call),
        ("继续目录", &r.next_directory_call),
    ] {
        if let Some(call) = call {
            let _ = writeln!(out, "{label}：{}", call.text());
        }
    }
    for call in &r.suggestions {
        let _ = writeln!(out, "相关查询：{}", call.text());
    }
    out
}
