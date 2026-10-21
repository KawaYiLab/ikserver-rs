//! ik_catalog：浏览目录。根目录列出全部章节与设定分类；指定分组时按阅读顺序列出文档。

use std::fmt::Write as _;

use super::{coverage, cursor, response::*};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{ToolResult, clamp, push_doc_summary, resolve_doc, resolve_group, speakers_preview};
use crate::corpus::{Corpus, DocKind, GroupKind, family_label};
use crate::text::{char_amount, fold, thousands};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogArgs {
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub manifest: Option<bool>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub exclude_placeholders: Option<bool>,
    #[serde(default)]
    pub exclude_unlinked: Option<bool>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub offset: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub max_results: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_bool")]
    pub include_evidence: Option<bool>,
}

fn legacy(corpus: &Corpus, args: &CatalogArgs) -> ToolResult {
    let path = args.path.as_deref().map(str::trim).unwrap_or("");
    let keyword = args.keyword.as_deref().map(str::trim).unwrap_or("");
    let max = clamp(args.max_results, 100, 500);
    let offset = args.offset.unwrap_or(0);
    if path.is_empty() || path == "/" {
        if keyword.is_empty() {
            return Ok(root(corpus));
        }
        if let Some(release) = super::releases::lookup(keyword)? {
            let group = super::releases::resolve(corpus, release)?;
            let mut out = String::new();
            super::releases::push_note(&mut out, corpus, release);
            out.push_str(&group_listing(
                corpus,
                group,
                "",
                offset,
                max,
                args.include_evidence.unwrap_or(false),
            ));
            return Ok(out);
        }
        return Ok(titles_everywhere(
            corpus,
            keyword,
            offset,
            max,
            args.include_evidence.unwrap_or(false),
        ));
    }
    if path.starts_with("doc_") {
        let doc = resolve_doc(corpus, path)?;
        let mut out = doc_card(corpus, doc);
        if args.include_evidence == Some(true) {
            out.push('\n');
            super::push_doc_evidence(&mut out, corpus, doc);
        }
        return Ok(out);
    }
    let group = resolve_group(corpus, path)?;
    let mut out = String::new();
    if let Some(release) = super::releases::lookup(path)? {
        super::releases::push_note(&mut out, corpus, release);
    }
    out.push_str(&group_listing(
        corpus,
        group,
        keyword,
        offset,
        max,
        args.include_evidence.unwrap_or(false),
    ));
    Ok(out)
}

fn root(corpus: &Corpus) -> String {
    let mut out = String::with_capacity(4096);
    let story_docs = corpus
        .docs
        .iter()
        .filter(|d| d.kind == DocKind::Story)
        .count();
    let story_lines: usize = corpus
        .docs
        .iter()
        .filter(|d| d.kind == DocKind::Story)
        .map(|d| d.line_count as usize)
        .sum();
    let meta = &corpus.meta;
    let _ = writeln!(
        out,
        "[ik_catalog] 绝区零剧情库 | 数据版本 {} {} | 主线 {} 场景 · {} 句 · {} | 说话人覆盖 {}",
        if meta.source_version.is_empty() {
            "?"
        } else {
            &meta.source_version
        },
        meta.language,
        thousands(story_docs),
        thousands(story_lines),
        char_amount(corpus.total_chars(DocKind::Story)),
        if meta.speaker_coverage > 0.0 {
            format!("{:.0}%", meta.speaker_coverage * 100.0)
        } else {
            "无（提取时未接入游戏资源，台词不带说话人）".to_owned()
        }
    );
    super::push_version(&mut out, corpus);
    out.push_str("目录顺序不代表故事内时间或实际播放顺序。\n");
    out.push_str("\n## 主线章节（章节目录顺序）\n");
    let mut supplement_only = Vec::new();
    for g in corpus
        .groups
        .iter()
        .filter(|g| g.kind == GroupKind::Chapter)
    {
        let (mut scenes, mut lines, mut chars, mut sup, mut sup_chars) =
            (0usize, 0usize, 0usize, 0usize, 0usize);
        for &d in &g.docs {
            let doc = &corpus.docs[d as usize];
            if doc.kind == DocKind::Story {
                scenes += 1;
                lines += doc.line_count as usize;
                chars += doc.char_count as usize;
            } else {
                sup += 1;
                sup_chars += doc.char_count as usize;
            }
        }
        if scenes == 0 {
            supplement_only.push(format!(
                "- {} {} | {} 篇 · {}",
                g.id,
                g.label,
                sup,
                char_amount(sup_chars)
            ));
            continue;
        }
        let _ = write!(
            out,
            "- {} {} | {} 场景 · {} 句 · {}",
            g.id,
            g.label,
            scenes,
            thousands(lines),
            char_amount(chars)
        );
        if sup > 0 {
            let _ = write!(out, " | 补充 {sup} 篇");
        }
        if let Some(release) = super::releases::for_group(&g.id) {
            let _ = write!(out, " | 版本别名：{}（该版新增主线）", release.version);
        }
        out.push('\n');
    }
    if !supplement_only.is_empty() {
        out.push_str("\n## 补充（过场字幕 / 动态漫画 / 角色过场）\n");
        for line in supplement_only {
            out.push_str(&line);
            out.push('\n');
        }
    }
    let lore: Vec<_> = corpus
        .groups
        .iter()
        .filter(|g| g.kind == GroupKind::Lore)
        .collect();
    if !lore.is_empty() {
        out.push_str("\n## 设定资料\n");
        for g in lore {
            let chars: usize = g
                .docs
                .iter()
                .map(|&d| corpus.docs[d as usize].char_count as usize)
                .sum();
            let _ = writeln!(
                out,
                "- {} {} | {} 篇 · {}",
                g.id,
                g.label,
                thousands(g.docs.len()),
                char_amount(chars)
            );
        }
    }
    out.push_str(
        "\n相关入口：ik_catalog(path=\"ch:…\") 为章节目录；ik_read(docId=\"ch:…\") 为章节原文；\
         ik_search(keyword=…) 为全文检索。",
    );
    out
}

fn titles_everywhere(
    corpus: &Corpus,
    keyword: &str,
    offset: usize,
    max: usize,
    include_evidence: bool,
) -> String {
    let hits: Vec<usize> = corpus
        .doc_keys
        .matching(&fold(keyword), false)
        .into_iter()
        .map(|d| d as usize)
        .collect();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "[ik_catalog] 标题/路径包含「{keyword}」的文档 {} 篇（显示 {}-{}）",
        thousands(hits.len()),
        (offset.saturating_add(1)).min(hits.len()),
        (offset.saturating_add(max)).min(hits.len())
    );
    super::push_version(&mut out, corpus);
    for &d in hits.iter().skip(offset).take(max) {
        out.push_str("- ");
        push_doc_summary(&mut out, corpus, d);
        out.push('\n');
        super::push_evidence_note(&mut out, corpus, d);
        if include_evidence {
            super::push_doc_evidence(&mut out, corpus, d);
        }
    }
    if offset.saturating_add(max) < hits.len() {
        let _ = write!(
            out,
            "还有更多：ik_catalog(keyword=\"{keyword}\", offset={})",
            offset.saturating_add(max)
        );
    } else if hits.is_empty() {
        out.push_str("没有标题匹配。标题之外的正文请用 ik_search 检索。");
    }
    out
}

fn group_listing(
    corpus: &Corpus,
    group: usize,
    keyword: &str,
    offset: usize,
    max: usize,
    include_evidence: bool,
) -> String {
    let g = &corpus.groups[group];
    let members: Vec<usize> = if keyword.is_empty() {
        g.docs.iter().map(|&d| d as usize).collect()
    } else {
        let matched = corpus.doc_keys.matching(&fold(keyword), false);
        g.docs
            .iter()
            .map(|&d| d as usize)
            .filter(|&d| matched.binary_search(&(d as u32)).is_ok())
            .collect()
    };
    let total_lines: usize = members
        .iter()
        .map(|&d| corpus.docs[d].line_count as usize)
        .sum();
    let total_chars: usize = members
        .iter()
        .map(|&d| corpus.docs[d].char_count as usize)
        .sum();

    let mut out = String::with_capacity(8192);
    let _ = write!(
        out,
        "[ik_catalog] {} {} | {} 篇 · {} 行 · {}",
        g.id,
        g.label,
        thousands(members.len()),
        thousands(total_lines),
        char_amount(total_chars)
    );
    if !keyword.is_empty() {
        let _ = write!(out, " | 标题含「{keyword}」");
    }
    let _ = writeln!(
        out,
        " | 显示 {}-{}",
        (offset.saturating_add(1)).min(members.len()),
        (offset.saturating_add(max)).min(members.len())
    );

    super::push_version(&mut out, corpus);
    if g.kind == GroupKind::Chapter {
        out.push_str("以下为目录顺序；场景剧情先后未核验。\n");
        let scenes: Vec<_> = members
            .iter()
            .map(|&d| &corpus.docs[d])
            .filter(|d| d.kind == DocKind::Story && d.section == "GalGame")
            .collect();
        if !scenes.is_empty() {
            let linked = scenes.iter().filter(|d| d.quest.is_some()).count();
            let _ = writeln!(
                out,
                "剧情演出任务关联：{linked}/{} 场景；未关联场景按标题或 docId 定位。",
                scenes.len()
            );
        }
    }

    let mut last_section: Option<(&str, DocKind)> = None;
    let mut last_quest: Option<&str> = None;
    for &d in members.iter().skip(offset).take(max) {
        let doc = &corpus.docs[d];
        let section = (doc.section.as_str(), doc.kind);
        if last_section != Some(section) {
            let label = if doc.kind == DocKind::Lore {
                doc.section.as_str()
            } else {
                family_label(&doc.section)
            };
            let _ = writeln!(
                out,
                "\n### {}{}",
                if doc.kind == DocKind::Supplement {
                    "补充 · "
                } else {
                    ""
                },
                label
            );
            last_section = Some(section);
            last_quest = None;
        }
        if let Some(q) = doc.quest.as_deref() {
            if last_quest != Some(q) && q != doc.title {
                let _ = writeln!(out, "#### 任务：{q}");
            }
            last_quest = Some(q);
        }
        let unit = if doc.kind == DocKind::Lore {
            "行"
        } else {
            "句"
        };
        let _ = write!(
            out,
            "{}. {} {} · {}{}",
            doc.group_pos + 1,
            corpus.doc_id(d),
            doc.title,
            doc.line_count,
            unit
        );
        let speakers = speakers_preview(corpus, d, 6);
        if !speakers.is_empty() {
            let _ = write!(out, " · 逐句署名：{speakers}");
        }
        out.push('\n');
        super::push_evidence_note(&mut out, corpus, d);
        if include_evidence {
            super::push_doc_evidence(&mut out, corpus, d);
        }
    }
    if offset.saturating_add(max) < members.len() {
        let _ = write!(
            out,
            "\n还有 {} 篇：ik_catalog(path=\"{}\", offset={})",
            members.len() - offset - max,
            g.id,
            offset.saturating_add(max)
        );
        if !keyword.is_empty() {
            out.pop();
            let _ = write!(out, ", keyword=\"{keyword}\")");
        }
    } else {
        let next = corpus.groups.get(group + 1).filter(|n| n.kind == g.kind);
        if let Some(n) = next {
            let _ = write!(out, "\n下一组：{} {}", n.id, n.label);
        }
    }
    out
}

fn doc_card(corpus: &Corpus, doc: usize) -> String {
    let d = &corpus.docs[doc];
    let g = &corpus.groups[d.group as usize];
    let mut out = String::new();
    out.push_str("[ik_catalog] ");
    push_doc_summary(&mut out, corpus, doc);
    out.push('\n');
    super::push_version(&mut out, corpus);
    super::push_order_note(&mut out, corpus, doc);
    let _ = write!(
        out,
        "\n所属：{} {}（第 {}/{} 篇）",
        g.id,
        g.label,
        d.group_pos + 1,
        g.docs.len()
    );
    let speakers = speakers_preview(corpus, doc, 30);
    if !speakers.is_empty() {
        let _ = write!(out, "\n逐句署名：{speakers}");
    }
    if d.group_pos > 0 {
        let prev = g.docs[d.group_pos as usize - 1] as usize;
        let _ = write!(
            out,
            "\n上一篇（目录相邻）：{} 「{}」",
            corpus.doc_id(prev),
            corpus.docs[prev].title
        );
    }
    if let Some(next) = corpus.next_in_group(doc) {
        let _ = write!(
            out,
            "\n下一篇（目录相邻）：{} 「{}」",
            corpus.doc_id(next),
            corpus.docs[next].title
        );
    }
    out
}

pub fn run(c: &Corpus, args: &CatalogArgs) -> ToolResult {
    execute(c, args).map(|r| r.text)
}
pub fn execute(c: &Corpus, args: &CatalogArgs) -> OutputResult {
    let full = detail(args.detail.as_deref())?;
    let path = args.path.as_deref().unwrap_or("").trim();
    let keyword = args.keyword.as_deref().unwrap_or("").trim();
    let release = if path.is_empty() {
        super::releases::lookup(keyword)?
    } else {
        None
    };
    let resolved_path = if let Some(r) = release {
        Some(c.groups[super::releases::resolve(c, r)?].id.clone())
    } else {
        args.path.clone()
    };
    let range = coverage::RangeScope {
        path: resolved_path,
        scope: args.scope.clone(),
        exclude_placeholders: args.exclude_placeholders.unwrap_or(false),
        exclude_unlinked: args.exclude_unlinked.unwrap_or(false),
    };
    if args.manifest == Some(true) && !keyword.is_empty() && release.is_none() {
        return super::err("生成清单时请用 path/scope 限定范围；keyword 仅用于目录标题检索");
    }
    let (mut docs, _) = coverage::members(c, &range)?;
    if !keyword.is_empty() && release.is_none() && !path.starts_with("doc_") {
        let matches = c.doc_keys.matching(&fold(keyword), false);
        docs.retain(|d| matches.binary_search(&(*d as u32)).is_ok());
    }
    let root = (path.is_empty() || path == "/") && keyword.is_empty();
    let total_documents = docs.len();
    let total_lines = docs.iter().map(|&d| c.docs[d].line_count as usize).sum();
    let offset = args.offset.unwrap_or(0);
    let max = clamp(args.max_results, 100, 500);
    let groups = if root {
        let mut counts = std::collections::BTreeMap::new();
        for &d in &docs {
            let n = counts.entry(c.docs[d].group).or_insert((0, 0));
            n.0 += 1;
            n.1 += c.docs[d].line_count as usize;
        }
        counts
            .into_iter()
            .map(|(index, (document_count, line_count))| {
                let g = &c.groups[index as usize];
                GroupCard {
                    group_id: g.id.clone(),
                    title: g.label.clone(),
                    document_count,
                    line_count,
                    next_catalog: NextCall::new(
                        "ik_catalog",
                        json!({"path":g.id,"scope":args.scope,"excludePlaceholders":args.exclude_placeholders,"excludeUnlinked":args.exclude_unlinked}),
                    ),
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let documents = if root {
        Vec::new()
    } else {
        docs.iter()
            .skip(offset)
            .take(max)
            .map(|&d| {
                Document::new(c, d)
                    .with_summary_detail(c, d, full)
                    .with_evidence(c, d, args.include_evidence == Some(true))
            })
            .collect()
    };
    let next_call = if !root && offset.saturating_add(max) < docs.len() {
        let mut value = serde_json::to_value(args)
            .map_err(|_| super::ToolError("无法生成目录续接参数".into()))?;
        value["cursor"] = json!(null);
        value["offset"] = json!(offset.saturating_add(max));
        Some(cursor::next(c, "ik_catalog", value)?)
    } else {
        None
    };
    let manifest = if args.manifest == Some(true) {
        Some(coverage::manifest(c, range.clone())?)
    } else {
        None
    };
    let root_main = root && super::parse_scope(args.scope.as_deref())?.is_none();
    let coverage_docs = docs
        .iter()
        .copied()
        .filter(|&d| !root_main || c.docs[d].kind == DocKind::Story)
        .collect::<Vec<_>>();
    let release = release.or(super::releases::lookup(path)?);
    let group_ids = if root {
        Vec::new()
    } else if let Some(release) = release {
        vec![release.group.into()]
    } else if !path.is_empty() && !path.starts_with("doc_") {
        vec![c.groups[resolve_group(c, path)?].id.clone()]
    } else {
        docs.iter()
            .map(|&d| c.groups[c.docs[d].group as usize].id.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    let range_info = Some(RangeInfo::new(
        if path.is_empty() {
            Some(keyword)
        } else {
            Some(path)
        },
        group_ids,
        release,
        if path.starts_with("doc_") {
            "document"
        } else {
            "catalog"
        },
    ));
    let release_aliases = if root {
        super::releases::revision_data()
            .into_iter()
            .map(|(version, group)| ReleaseAlias {
                release_version: version.into(),
                group_id: group.into(),
                available: c.find_group(group).is_some(),
                next_catalog: NextCall::new("ik_catalog", json!({"path":group})),
            })
            .collect()
    } else {
        Vec::new()
    };
    let chapter_relations = if root && super::parse_scope(args.scope.as_deref())?.is_none() {
        let visible = docs
            .iter()
            .map(|&d| c.groups[c.docs[d].group as usize].id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        super::chapter_relations::relations(c)
            .into_iter()
            .filter_map(|mut relation| {
                relation
                    .member_group_ids
                    .retain(|id| visible.contains(id.as_str()));
                relation
                    .related_group_ids
                    .retain(|id| visible.contains(id.as_str()));
                (if relation.relation == "sharedLabel" {
                    relation.member_group_ids.len() > 1
                } else {
                    !relation.member_group_ids.is_empty() && !relation.related_group_ids.is_empty()
                })
                .then_some(relation)
            })
            .collect()
    } else {
        Vec::new()
    };
    let result = CatalogResult {
        chapter_relations,
        range_info,
        release_aliases,
        speaker_coverage: Some(c.speaker_coverage_for_docs(&coverage_docs)),
        groups,
        documents,
        total_documents,
        total_lines,
        next_call,
        manifest,
        resolved_range: range,
    };
    let legacy_text = full
        && args.scope.is_none()
        && args.exclude_placeholders != Some(true)
        && args.exclude_unlinked != Some(true);
    let mut text = if legacy_text {
        legacy(c, args)?
    } else {
        let mut out = format!(
            "[ik_catalog] {total_documents} 篇 · {total_lines} 行\n目录相邻不证明剧情先后。\n"
        );
        for g in &result.groups {
            let _ = writeln!(
                out,
                "{} {} | {} 篇 · {} 行 → {}",
                g.group_id,
                g.title,
                g.document_count,
                g.line_count,
                g.next_catalog.text()
            );
        }
        for d in &result.documents {
            let _ = writeln!(
                out,
                "{} 「{}」 {} | {} 行 → {}",
                d.doc_id,
                d.title,
                d.path,
                d.line_count,
                d.next_read.text()
            );
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
        out
    };
    if legacy_text {
        for d in &result.documents {
            let _ = writeln!(text, "文档摘要：{}", d.doc_id);
            d.push_summary(&mut text, true);
        }
    }
    if let Some(info) = &result.range_info {
        info.push_text(&mut text);
    }
    if let Some(counts) = &result.speaker_coverage {
        let _ = writeln!(
            text,
            "{}{}（姓名解析不确认播放，设定排除{}行）",
            if root_main { "主线" } else { "范围" },
            counts.text(),
            counts.excluded_not_applicable_lines
        );
    }
    for alias in &result.release_aliases {
        let _ = writeln!(
            text,
            "版本别名={} → {}（{}） → {}",
            alias.release_version,
            alias.group_id,
            if alias.available {
                "已收录"
            } else {
                "当前未收录"
            },
            alias.next_catalog.text()
        );
    }
    for relation in &result.chapter_relations {
        let label = match relation.relation.as_str() {
            "sharedLabel" => "同名分组，等价待核验",
            "supplementOf" => "补充归属",
            "interludeOf" => "正章与间章",
            _ => &relation.relation,
        };
        let _ = write!(
            text,
            "章节关联（{label}）：{}",
            relation.member_group_ids.join(",")
        );
        if !relation.related_group_ids.is_empty() {
            let _ = write!(text, " → {}", relation.related_group_ids.join(","));
        }
        let _ = writeln!(text, "（{}；只作目录导航）", relation.status);
    }
    if result
        .chapter_relations
        .iter()
        .any(|r| r.source.kind == "extractorMapping")
    {
        text.push_str(
            "关联依据：提取器 f1b85ab zzz_story/extract.py:L127–137；归属不确认播放顺序。\n",
        );
    }
    if let Some(call) = &result.next_call {
        let _ = writeln!(text, "\n续页：{}", call.text());
    }
    if let Some(manifest) = &result.manifest {
        let _ = writeln!(
            text,
            "manifestId={} | 清单={}篇/{}行",
            manifest.manifest_id, manifest.total_documents, manifest.total_lines
        );
        text.push_str("\n范围清单已生成；ik_coverage 的 manifestId 对应该清单，ranges 接收 ik_read.coverageDelta 的完整返回行记录。过滤以整篇为单位，仅排除全部为占位/未关联行的文档，混合文档保留。\n");
    }
    Ok(if legacy_text {
        ToolOutput::with_rendered_snapshot(c, text, ToolData::Catalog(result))
    } else {
        ToolOutput::new(c, text, ToolData::Catalog(result))
    })
}
