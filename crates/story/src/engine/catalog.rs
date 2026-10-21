//! ik_catalog：浏览目录。根目录列出全部章节与设定分类；指定分组时按阅读顺序列出文档。

use std::fmt::Write as _;

use serde::Deserialize;

use super::{ToolResult, clamp, push_doc_summary, resolve_doc, resolve_group, speakers_preview};
use crate::corpus::{Corpus, DocKind, GroupKind, family_label};
use crate::text::{char_amount, fold, thousands};

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogArgs {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub offset: Option<usize>,
    #[serde(default, deserialize_with = "crate::de::opt_usize")]
    pub max_results: Option<usize>,
}

pub fn run(corpus: &Corpus, args: &CatalogArgs) -> ToolResult {
    let path = args.path.as_deref().map(str::trim).unwrap_or("");
    let keyword = args.keyword.as_deref().map(str::trim).unwrap_or("");
    let max = clamp(args.max_results, 100, 500);
    let offset = args.offset.unwrap_or(0);
    if path.is_empty() || path == "/" {
        if keyword.is_empty() {
            return Ok(root(corpus));
        }
        return Ok(titles_everywhere(corpus, keyword, offset, max));
    }
    if path.starts_with("doc_") {
        let doc = resolve_doc(corpus, path)?;
        return Ok(doc_card(corpus, doc));
    }
    let group = resolve_group(corpus, path)?;
    Ok(group_listing(corpus, group, keyword, offset, max))
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
        "\n下一步：ik_catalog(path=\"ch:…\") 列出一章的场景；ik_read(docId=\"ch:…\") 从章首顺读；\
         ik_search(keyword=…) 全文检索。",
    );
    out
}

fn titles_everywhere(corpus: &Corpus, keyword: &str, offset: usize, max: usize) -> String {
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
        out.push_str("以下为目录顺序，未确认的场景先后不能据此作为剧情时间线。\n");
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
            let _ = write!(out, " · {speakers}");
        }
        out.push('\n');
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
        let _ = write!(out, "\n出场：{speakers}");
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
