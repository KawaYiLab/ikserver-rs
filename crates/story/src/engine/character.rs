//! ik_character：角色速览。设定档案、按章节的出场与台词统计、首次登场、常同场角色、被提及次数，
//! 给「角色弧线」类总结一个可直接按图索骥的读书清单。

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use super::response::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::search::match_speakers;
use super::{ToolResult, clamp, err, push_doc_summary};
use crate::corpus::{Corpus, DocKind, GroupKind};
use crate::index::Candidates;
use crate::text::{find_folded, fold, thousands};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterArgs {
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub max_results: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_bool")]
    pub include_evidence: Option<bool>,
}

/// 场景名单里的非角色项。
fn is_placeholder(name: &str) -> bool {
    matches!(name, "？？？" | "???" | "旁白" | "选项" | "系统" | "广播")
        || name.chars().all(|c| c == '？' || c == '?')
}

#[derive(Default)]
struct ChapterStat {
    scenes: usize,
    lines: usize,
    mentions: usize,
    first_appear: Option<usize>,
    first_mention: Option<usize>,
    name_table: usize,
    inferred: usize,
}

pub fn run(corpus: &Corpus, args: &CharacterArgs) -> ToolResult {
    execute(corpus, args).map(|r| r.text)
}

pub fn execute(corpus: &Corpus, args: &CharacterArgs) -> OutputResult {
    let full = detail(args.detail.as_deref())?;
    let name = args.name.trim();
    if name.is_empty() {
        return err("name 不能为空");
    }
    let max = clamp(args.max_results, 40, 200);
    let folded = fold(name);
    let ids = match_speakers(corpus, name);
    let exact = ids.iter().any(|&i| {
        corpus.speakers_folded[i as usize] == folded
            || corpus.speaker_identities[i as usize] == folded
    });
    // 精确命中时，把「包含该名字」的其他说话人（如「零号·安比」之于「安比」）单独列为相关名称。
    let related: Vec<u32> = if exact {
        corpus
            .speakers_folded
            .iter()
            .enumerate()
            .filter(|(i, s)| s.contains(&folded) && !ids.contains(&(*i as u32)))
            .map(|(i, _)| i as u32)
            .collect()
    } else {
        Vec::new()
    };

    // 被提及：正文里出现名字的行（不限说话人）。
    let mention_lines: Vec<u32> = match corpus.index.candidates(&folded) {
        Candidates::Exact(v) => v,
        Candidates::Verify(v) => {
            let mut scratch = String::new();
            v.into_iter()
                .filter(|&l| {
                    find_folded(corpus.line_text(l as usize), &folded, &mut scratch).is_some()
                })
                .collect()
        }
    };

    let mut per_group: HashMap<u32, ChapterStat> = HashMap::new();
    let mut co: HashMap<&str, usize> = HashMap::new();
    let (mut scenes, mut spoken) = (0usize, 0usize);
    let (mut table_scenes, mut inferred_lines) = (0usize, 0usize);
    let mut association_docs = Vec::new();
    let mut attributed_docs = Vec::new();
    let mut related_lines: HashMap<u32, usize> = HashMap::new();
    for (d, doc) in corpus.docs.iter().enumerate() {
        if doc.kind == DocKind::Lore {
            continue;
        }
        let names = corpus.doc_speakers(doc);
        let appears = names.iter().any(|s| ids.contains(s));
        let table = doc.name_table.iter().any(|s| ids.contains(s));
        let inferred_here = doc
            .lines()
            .filter(|&l| ids.contains(&corpus.lines[l].speaker) && corpus.is_inferred(l))
            .count();
        if table || inferred_here > 0 {
            association_docs.push(d);
            let stat = per_group.entry(doc.group).or_default();
            stat.name_table += usize::from(table);
            stat.inferred += inferred_here;
            table_scenes += usize::from(table);
            inferred_lines += inferred_here;
        }
        let mut lines_here = 0usize;
        if appears || !related.is_empty() {
            for l in doc.lines() {
                let sp = corpus.lines[l].speaker;
                if ids.contains(&sp) && corpus.is_attributed(l) {
                    lines_here += 1;
                } else if related.contains(&sp) {
                    *related_lines.entry(sp).or_default() += 1;
                }
            }
        }
        if !appears {
            continue;
        }
        attributed_docs.push(d);
        scenes += 1;
        spoken += lines_here;
        let stat = per_group.entry(doc.group).or_default();
        stat.scenes += 1;
        stat.lines += lines_here;
        if stat.first_appear.is_none() {
            stat.first_appear = Some(d);
        }
        let mut seen = HashSet::new();
        for &s in names {
            if !ids.contains(&s) && !related.contains(&s) {
                let identity = corpus.speaker_identity_names[s as usize].as_str();
                if seen.insert(identity) {
                    *co.entry(identity).or_default() += 1;
                }
            }
        }
    }
    let mut mention_docs = 0usize;
    let mut last_doc = usize::MAX;
    let mut lore_mentions = 0usize;
    for &l in &mention_lines {
        let d = corpus.doc_of_line(l);
        let doc = &corpus.docs[d];
        if d != last_doc {
            mention_docs += 1;
            last_doc = d;
            if doc.kind == DocKind::Lore {
                lore_mentions += 1;
            }
        }
        if doc.kind != DocKind::Lore {
            let stat = per_group.entry(doc.group).or_default();
            stat.mentions += 1;
            if stat.first_mention.is_none() {
                stat.first_mention = Some(d);
            }
        }
    }

    // 设定档案：标题里含名字的设定文档，代理人档案优先。
    let titled: Vec<usize> = corpus
        .doc_keys
        .matching(&folded, true)
        .into_iter()
        .map(|d| d as usize)
        .filter(|&d| corpus.docs[d].kind == DocKind::Lore)
        .collect();
    let is_agent = |d: usize| corpus.groups[corpus.docs[d].group as usize].id == "lore:agents";
    // 代理人档案是正式的人物资料；其余标题含名字的设定（多为委托标题）只计数，避免淹没档案。
    let mut profiles: Vec<usize> = titled.iter().copied().filter(|&d| is_agent(d)).collect();
    if profiles.is_empty() {
        profiles = titled.iter().copied().take(5).collect();
    }
    let other_titled = titled.len() - profiles.len();

    if ids.is_empty() && mention_lines.is_empty() && titled.is_empty() {
        return err(format!(
            "语料中没有找到「{name}」。试试全名、简称或别的写法（如「零号·安比」「11号」）"
        ));
    }

    let mut out = String::with_capacity(4096);
    let _ = write!(out, "[ik_character] {name}");
    if !ids.is_empty() {
        let names: Vec<&str> = ids.iter().filter_map(|&i| corpus.speaker_name(i)).collect();
        let _ = write!(out, " | 匹配说话人：{}", names.join("、"));
    } else {
        out.push_str(" | 不是已知说话人（以下仅统计被提及与设定资料）");
    }
    out.push('\n');
    super::push_version(&mut out, corpus);
    out.push_str("目录首见仅指当前语料排列中的最早位置，实际首次登场需读原文确认。\n");
    if !related.is_empty() {
        let mut rel: Vec<(u32, usize)> = related
            .iter()
            .map(|&r| (r, related_lines.get(&r).copied().unwrap_or(0)))
            .collect();
        rel.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
        let list: Vec<String> = rel
            .iter()
            .take(8)
            .filter_map(|&(r, n)| corpus.speaker_name(r).map(|s| format!("{s}（{n} 句）")))
            .collect();
        let _ = writeln!(out, "相关名称（未合并统计）：{}", list.join("、"));
    }

    if !profiles.is_empty() {
        out.push_str("\n## 设定档案\n");
        for &d in profiles.iter().take(8) {
            out.push_str("- ");
            push_doc_summary(&mut out, corpus, d);
            out.push('\n');
        }
    }
    if other_titled > 0 {
        let _ = writeln!(
            out,
            "另有 {other_titled} 篇设定标题含「{name}」（多为委托/物品）：ik_catalog(keyword=\"{name}\")"
        );
    }

    let _ = writeln!(
        out,
        "\n## 剧情出场\n出场 {} 场景 · 台词 {} 句 · 被提及 {} 行（{} 篇，其中设定 {} 篇）",
        thousands(scenes),
        thousands(spoken),
        thousands(mention_lines.len()),
        thousands(mention_docs),
        thousands(lore_mentions)
    );
    let _ = writeln!(
        out,
        "姓名表关联 {table_scenes} 场景 · 推断署名 {inferred_lines} 句（独立统计，不计入实际署名台词或目录首见出场）。"
    );
    if args.include_evidence == Some(true) {
        out.push_str("逐句署名证据入口（按目录序）：\n");
        for &d in attributed_docs.iter().take(max) {
            push_doc_summary(&mut out, corpus, d);
            out.push('\n');
            super::push_evidence_note(&mut out, corpus, d);
            super::push_doc_evidence(&mut out, corpus, d);
        }
        if attributed_docs.len() > max {
            let _ = writeln!(
                out,
                "另有 {} 篇逐句署名证据未展开；用 ik_search(speaker=\"{name}\", includeEvidence=true) 缩小范围。",
                attributed_docs.len() - max
            );
        }
    }
    if !association_docs.is_empty() {
        out.push_str("姓名表/推断关联入口：\n");
        for &d in association_docs.iter().take(max) {
            push_doc_summary(&mut out, corpus, d);
            out.push('\n');
            super::push_evidence_note(&mut out, corpus, d);
            if args.include_evidence == Some(true) {
                super::push_doc_evidence(&mut out, corpus, d);
            }
        }
        if association_docs.len() > max {
            let _ = writeln!(
                out,
                "另有 {} 篇关联未显示；用 ik_search(keyword=\"{name}\") 查看。",
                association_docs.len() - max
            );
        }
    }
    let mut groups: Vec<(&u32, &ChapterStat)> = per_group.iter().collect();
    groups.sort_by_key(|(g, _)| **g);
    let chapter_rows = groups
        .iter()
        .filter(|(g, _)| corpus.groups[**g as usize].kind == GroupKind::Chapter);
    for (g, stat) in chapter_rows.clone().take(max) {
        let group = &corpus.groups[**g as usize];
        let _ = write!(
            out,
            "- {} {} | 场景 {} · 台词 {} · 提及 {} · 姓名表 {} · 推断 {}",
            group.id,
            group.label,
            stat.scenes,
            stat.lines,
            stat.mentions,
            stat.name_table,
            stat.inferred
        );
        let first = match (stat.first_appear, stat.first_mention) {
            (Some(d), _) => Some(("目录首见出场", d)),
            (None, Some(d)) => Some(("目录首见提及", d)),
            _ => None,
        };
        if let Some((label, d)) = first {
            let _ = write!(
                out,
                " | {label} {} 「{}」",
                corpus.doc_id(d),
                corpus.docs[d].title
            );
        }
        out.push('\n');
    }
    let rows = chapter_rows.count();
    if rows > max {
        let _ = writeln!(out, "…另有 {} 章未显示", rows - max);
    }

    let mut co: Vec<(&str, usize)> = co
        .into_iter()
        .filter(|(name, _)| !is_placeholder(name))
        .collect();
    co.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    if !co.is_empty() {
        let list: Vec<String> = co
            .iter()
            .take(15)
            .map(|&(name, n)| format!("{name}({n})"))
            .collect();
        let _ = writeln!(out, "\n常同场角色（共同出场场景数）：{}", list.join("、"));
    }

    out.push_str("\n下一步：");
    if !ids.is_empty() {
        let _ = write!(out, "ik_search(keyword=…, speaker=\"{name}\") 查本人台词；");
    }
    let _ = write!(
        out,
        "ik_search(keyword=\"{name}\") 查被提及处；ik_read(docId=目录首见场景) 读原文确认登场经过。"
    );
    let chapters = groups
        .iter()
        .filter(|(g, _)| corpus.groups[**g as usize].kind == GroupKind::Chapter)
        .take(max)
        .map(|(g, s)| CharacterChapter {
            group_id: corpus.groups[**g as usize].id.clone(),
            title: corpus.groups[**g as usize].label.clone(),
            scenes: s.scenes,
            spoken_lines: s.lines,
            mentioned_lines: s.mentions,
            name_table_scenes: s.name_table,
            inferred_lines: s.inferred,
            first_directory_appearance: s
                .first_appear
                .map(|d| Document::new(corpus, d).with_summary_detail(corpus, d, full)),
            first_directory_mention: s
                .first_mention
                .map(|d| Document::new(corpus, d).with_summary_detail(corpus, d, full)),
        })
        .collect();
    let result = CharacterResult {
        evidence_documents: if args.include_evidence == Some(true) {
            attributed_docs
                .iter()
                .take(max)
                .chain(association_docs.iter().take(max))
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|d| {
                    Document::new(corpus, d)
                        .with_summary_detail(corpus, d, full)
                        .with_evidence(corpus, d, true)
                })
                .collect()
        } else {
            Vec::new()
        },
        name: name.into(),
        resolved_names: ids
            .iter()
            .filter_map(|&s| corpus.speaker_name(s).map(str::to_owned))
            .collect(),
        scenes,
        spoken_lines: spoken,
        name_table_scenes: table_scenes,
        inferred_lines,
        mentioned_lines: mention_lines.len(),
        chapters,
        chapter_count: rows,
        chapters_truncated: rows > max,
        profiles: profiles
            .iter()
            .take(8)
            .map(|&d| Document::new(corpus, d).with_summary_detail(corpus, d, full))
            .collect(),
        next_calls: {
            let mut calls = Vec::new();
            if !ids.is_empty() {
                calls.push(NextCall::new(
                    "ik_search",
                    json!({"speaker":name,"includeEvidence":args.include_evidence}),
                ));
            }
            calls.push(NextCall::new(
                "ik_search",
                json!({"keyword":name,"matchIn":"body"}),
            ));
            calls
        },
    };
    if !full {
        out = format!(
            "[ik_character] {name} | 逐句署名 {scenes} 场景 · {spoken} 句 · 被提及 {} 行\n姓名表关联 {table_scenes} 场景 · 推断署名 {inferred_lines} 句（独立统计）。\n",
            result.mentioned_lines
        );
        for p in &result.profiles {
            let _ = writeln!(
                out,
                "档案：{} 「{}」 → {}",
                p.doc_id,
                p.title,
                p.next_read.text()
            );
            p.push_summary(&mut out, full);
        }
        for chapter in &result.chapters {
            let _ = writeln!(
                out,
                "{} {} | 台词 {} · 提及 {}",
                chapter.group_id, chapter.title, chapter.spoken_lines, chapter.mentioned_lines
            );
            if let Some(d) = &chapter.first_directory_appearance {
                let _ = writeln!(
                    out,
                    "目录首见：{} 「{}」 → {}",
                    d.doc_id,
                    d.title,
                    d.next_read.text()
                );
                d.push_summary(&mut out, full);
            }
        }
        for d in &result.evidence_documents {
            let _ = writeln!(out, "{} 「{}」", d.doc_id, d.title);
            d.push_summary(&mut out, full);
            for l in &d.evidence_preview {
                let _ = write!(out, "L{}", l.number);
                l.push_sources(&mut out);
                if let Some(e) = &l.evidence {
                    let _ = write!(out, " [提取证据：{e}]");
                }
                out.push('\n');
            }
        }
        for call in &result.next_calls {
            let _ = writeln!(out, "下一步：{}", call.text());
        }
    }
    if full {
        for d in result
            .profiles
            .iter()
            .chain(
                result
                    .chapters
                    .iter()
                    .filter_map(|ch| ch.first_directory_appearance.as_ref()),
            )
            .chain(result.evidence_documents.iter())
        {
            let _ = writeln!(out, "文档摘要：{}", d.doc_id);
            d.push_summary(&mut out, true);
        }
    }
    Ok(if full {
        ToolOutput::with_rendered_snapshot(corpus, out, ToolData::Character(result))
    } else {
        ToolOutput::new(corpus, out, ToolData::Character(result))
    })
}
