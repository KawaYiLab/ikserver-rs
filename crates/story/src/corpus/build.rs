//! 从 zzz-story-extractor 的产出（[`Bundle`]）构建语料。
//!
//! 读取：
//! - `story.jsonl`：主线台词（一行一句，按章节 → 场景 → 行排序）
//! - `supplement.jsonl`：过场字幕 / 漫画（可选）
//! - `lore/**/*.md`：世界观设定（可选），按标题切成独立文档
//! - `manifest.json`：数据版本（可选）

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::sync::Arc;
use xxhash_rust::xxh3::{xxh3_64, xxh3_128};

use super::bundle::Bundle;
use super::{
    Corpus, DOC_HASH_MASK, Doc, DocKeys, DocKind, Group, GroupKind, Line, LineEvidence, LineKind,
    LineSource, Meta, NO_SPEAKER,
};
use crate::index::BigramIndex;
use crate::text::fold;

#[derive(Deserialize)]
struct Row<'a> {
    content_status: Option<Arc<str>>,
    content_evidence: Option<Box<serde_json::Value>>,
    source_attribution: Option<Box<serde_json::Value>>,
    speaker_status: Option<Arc<str>>,
    speaker_gap_reason: Option<Arc<str>>,
    speaker_source: Option<Arc<str>>,
    speaker_slot: Option<Box<serde_json::Value>>,
    has_voice: Option<bool>,
    line_order_source: Option<Arc<str>>,
    line_order_status: Option<Arc<str>>,
    perform: Option<Box<super::ResourceEvidence>>,
    timeline: Option<Box<super::ResourceEvidence>>,
    #[serde(borrow)]
    chapter: Cow<'a, str>,
    #[serde(borrow, default)]
    chapter_label: Option<Cow<'a, str>>,
    #[serde(default = "zero_order")]
    chapter_order: serde_json::Number,
    #[serde(borrow)]
    scene: Cow<'a, str>,
    #[serde(borrow, default)]
    scene_title: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    quest_name: Option<Cow<'a, str>>,
    #[serde(borrow)]
    family: Cow<'a, str>,
    #[serde(borrow, default)]
    speaker: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    speaker_id: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    source_key: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    line_id: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    order_source: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    chronology_status: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    context_status: Option<Cow<'a, str>>,
    #[serde(borrow)]
    text: Cow<'a, str>,
    #[serde(borrow, default)]
    line_type: Option<Cow<'a, str>>,
    #[serde(borrow, default)]
    scene_speakers: Option<Vec<Cow<'a, str>>>,
    #[serde(default)]
    order: i64,
}

impl Row<'_> {
    fn take_evidence(&mut self) -> LineEvidence {
        LineEvidence {
            content_status: self.content_status.take(),
            content_evidence: self.content_evidence.take(),
            source_attribution: self.source_attribution.take(),
            speaker_status: self.speaker_status.take(),
            speaker_gap_reason: self.speaker_gap_reason.take(),
            speaker_source: self.speaker_source.take(),
            speaker_slot: self.speaker_slot.take(),
            has_voice: self.has_voice,
            line_order_source: self.line_order_source.take(),
            line_order_status: self.line_order_status.take(),
            perform: self.perform.take(),
            timeline: self.timeline.take(),
        }
    }
}

fn zero_order() -> serde_json::Number {
    0.into()
}

#[derive(Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    source_version: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    generated_at: String,
    #[serde(default)]
    local_game_version: String,
    #[serde(default)]
    accuracy_first: Option<bool>,
}

/// 构建过程中的文档（行先挂在文档上，最后统一落进扁平数组）。
struct DraftDoc {
    key: String,
    kind: DocKind,
    group: usize,
    section: String,
    title: String,
    path: String,
    quest: Option<String>,
    order_source: Option<String>,
    chronology_status: Option<String>,
    context_status: Option<String>,
    lines: Vec<DraftLine>,
    speakers: Vec<String>,
    name_table: Vec<String>,
    inferred_speakers: Vec<String>,
}

struct DraftLine {
    text: String,
    speaker: Option<String>,
    kind: LineKind,
    source: LineSource,
    evidence: LineEvidence,
}

struct DraftGroup {
    id: String,
    kind: GroupKind,
    label: String,
    order: i64,
    docs: Vec<usize>,
}

#[derive(Default)]
struct Builder {
    docs: Vec<DraftDoc>,
    groups: Vec<DraftGroup>,
    group_by_id: HashMap<String, usize>,
    speaker_identities: HashMap<String, String>,
}

impl Builder {
    fn group(&mut self, id: String, kind: GroupKind, label: &str, order: i64) -> usize {
        if let Some(&g) = self.group_by_id.get(&id) {
            return g;
        }
        let g = self.groups.len();
        self.group_by_id.insert(id.clone(), g);
        self.groups.push(DraftGroup {
            id,
            kind,
            label: label.to_owned(),
            order,
            docs: Vec::new(),
        });
        g
    }

    fn push_doc(&mut self, doc: DraftDoc) {
        let g = doc.group;
        self.groups[g].docs.push(self.docs.len());
        self.docs.push(doc);
    }
}

pub fn build_from_dir(dir: &Path) -> Result<Corpus> {
    build_from_bundle(&Bundle::from_dir(dir)?)
}

/// 从语料包构建（上传、启动加载、本地目录都走这里）。
pub fn build_from_bundle(bundle: &Bundle) -> Result<Corpus> {
    let manifest: Manifest = match &bundle.manifest {
        Some(value) => serde_json::from_value(value.clone()).context("manifest 解析失败")?,
        None => Manifest::default(),
    };

    let legacy_orders = legacy_chapter_orders(bundle)?;
    let mut builder = Builder::default();
    let story_stats = ingest_jsonl(
        &mut builder,
        "story.jsonl",
        &bundle.story,
        DocKind::Story,
        legacy_orders,
    )?;
    if let Some(supplement) = &bundle.supplement {
        ingest_jsonl(
            &mut builder,
            "supplement.jsonl",
            supplement,
            DocKind::Supplement,
            legacy_orders,
        )?;
    }
    ingest_lore(&mut builder, &bundle.lore);
    if builder.docs.is_empty() {
        bail!("语料包里没有任何可用的文本");
    }

    let built_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let meta = Meta {
        source_version: manifest.source_version,
        language: if manifest.language.is_empty() {
            "CHS".into()
        } else {
            manifest.language
        },
        generated_at: manifest.generated_at,
        local_game_version: manifest.local_game_version,
        accuracy_first: manifest.accuracy_first,
        built_at,
        speaker_coverage: if story_stats.lines == 0 {
            0.0
        } else {
            story_stats.with_speaker as f32 / story_stats.lines as f32
        },
    };
    finish(builder, meta)
}

#[derive(Default)]
struct JsonlStats {
    lines: usize,
    with_speaker: usize,
}

pub fn family_label(family: &str) -> &str {
    match family {
        "GalGame" => "剧情演出",
        "MainChat" => "主线聊天",
        "Chat" => "聊天",
        "ChatAudio" => "语音聊天",
        "ChatPlus" => "聊天（补充）",
        "BubbleStory" => "气泡剧情",
        "Bubble" => "气泡对话",
        "Ongoing" => "关卡对话",
        "OngoingLevel" => "关卡内对话",
        "OngoingMainCity" => "城区对话",
        "OngoingAntique" => "古董店对话",
        "CS" => "过场字幕",
        "Comic" => "漫画",
        other => other,
    }
}

fn legacy_chapter_orders(bundle: &Bundle) -> Result<bool> {
    let scale = bundle
        .manifest
        .as_ref()
        .and_then(|m| m.get("chapter_order_scale"));
    if let Some(scale) = scale {
        return match scale.as_u64() {
            Some(1) => Ok(true),
            Some(10) => Ok(false),
            _ => bail!("chapter_order_scale 必须为 1 或 10"),
        };
    }
    // Legacy outputs had integral story orders and fractional supplements.
    // Detect once across both files so every group uses the same scale.
    #[derive(Deserialize)]
    struct OrderProbe {
        #[serde(default)]
        chapter_order: Option<serde_json::Number>,
    }
    for (name, raw) in std::iter::once(("story.jsonl", bundle.story.as_str())).chain(
        bundle
            .supplement
            .as_deref()
            .map(|s| ("supplement.jsonl", s)),
    ) {
        for (i, line) in raw
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
        {
            let row: OrderProbe = serde_json::from_str(line)
                .with_context(|| format!("{name} 第 {} 行解析失败", i + 1))?;
            if row.chapter_order.is_some_and(|n| n.is_f64()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn chapter_order(number: &serde_json::Number, legacy: bool) -> Result<i64> {
    if let Some(n) = number.as_i64() {
        return n
            .checked_mul(if legacy { 10 } else { 1 })
            .context("chapter_order 超出 64 位整数范围");
    }
    if !legacy {
        bail!("chapter_order 必须为 64 位整数");
    }
    // Old JSON decimal orders have at most one non-zero fractional digit.
    // Parse their decimal spelling without a lossy float-to-int cast.
    let spelling = number.to_string();
    let (whole, fraction) = spelling
        .split_once('.')
        .context("chapter_order 无法无损转换")?;
    let fraction = fraction.trim_end_matches('0');
    if fraction.len() > 1 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        bail!("chapter_order 无法无损转换");
    }
    let whole: i64 = whole.parse().context("chapter_order 超出范围")?;
    let digit = fraction.parse::<i64>().unwrap_or(0);
    whole
        .checked_mul(10)
        .and_then(|n| {
            if spelling.starts_with('-') {
                n.checked_sub(digit)
            } else {
                n.checked_add(digit)
            }
        })
        .context("chapter_order 超出 64 位整数范围")
}

fn ingest_jsonl(
    builder: &mut Builder,
    name: &str,
    raw: &str,
    kind: DocKind,
    legacy: bool,
) -> Result<JsonlStats> {
    let mut evidence_strings = HashMap::new();
    let mut rows: Vec<(usize, Row<'_>, LineEvidence)> = raw
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str::<Row<'_>>(l)
                .map(|mut r| {
                    let mut evidence = r.take_evidence();
                    evidence.intern(&mut evidence_strings);
                    (i, r, evidence)
                })
                .with_context(|| format!("{name} 第 {} 行解析失败", i + 1))
        })
        .collect::<Result<_>>()?;
    rows.sort_by_key(|(i, r, _)| (r.order, *i));

    let mut stats = JsonlStats::default();
    let mut doc_by_key: HashMap<String, usize> = HashMap::new();
    for (_, row, mut evidence) in rows {
        let text = row.text.trim();
        if text.is_empty() {
            continue;
        }
        let chapter_label = row
            .chapter_label
            .as_deref()
            .unwrap_or(&row.chapter)
            .to_owned();
        let group = builder.group(
            format!("ch:{}", row.chapter),
            GroupKind::Chapter,
            &chapter_label,
            chapter_order(&row.chapter_order, legacy)
                .with_context(|| format!("{name} chapter_order 无效"))?,
        );
        let prefix = if kind == DocKind::Story {
            "story"
        } else {
            "sup"
        };
        let key = format!("{prefix}|{}|{}|{}", row.chapter, row.family, row.scene);
        let doc = match doc_by_key.get(&key) {
            Some(&d) => d,
            None => {
                let family = family_label(&row.family);
                let scene_speakers: Vec<String> = row
                    .scene_speakers
                    .as_ref()
                    .map(|v| {
                        v.iter()
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                let title = match (&row.scene_title, &row.quest_name) {
                    (Some(t), _) if !t.trim().is_empty() => t.trim().to_owned(),
                    (_, Some(q)) if !q.trim().is_empty() => q.trim().to_owned(),
                    _ if !scene_speakers.is_empty() => {
                        format!(
                            "{family}·{}",
                            scene_speakers
                                .iter()
                                .take(3)
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("/")
                        )
                    }
                    _ => format!("{family} {}", row.scene),
                };
                let quest = row
                    .quest_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|q| !q.is_empty())
                    .map(str::to_owned);
                let mut path = format!("{} › {} › {}", kind.label(), chapter_label, family);
                if let Some(q) = &quest
                    && *q != title
                {
                    path.push_str(" › ");
                    path.push_str(q);
                }
                let d = builder.docs.len();
                builder.push_doc(DraftDoc {
                    key: key.clone(),
                    kind,
                    group,
                    section: row.family.to_string(),
                    title,
                    path,
                    quest,
                    order_source: row.order_source.as_deref().map(str::to_owned),
                    chronology_status: row.chronology_status.as_deref().map(str::to_owned),
                    context_status: row.context_status.as_deref().map(str::to_owned),
                    lines: Vec::new(),
                    speakers: Vec::new(),
                    name_table: scene_speakers,
                    inferred_speakers: Vec::new(),
                });
                doc_by_key.insert(key, d);
                d
            }
        };
        let placeholder = row.line_type.as_deref() == Some("placeholder")
            || evidence.content_status.as_deref() == Some("reviewed_placeholder");
        let raw_speaker = row
            .speaker
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        if placeholder && evidence.source_attribution.is_none() && raw_speaker.is_some() {
            evidence.source_attribution = Some(Box::new(
                serde_json::json!({"speaker":raw_speaker,"speaker_id":row.speaker_id,"line_type":row.line_type}),
            ));
        }
        let speaker = if placeholder { None } else { raw_speaker };
        let line_kind = match (row.line_type.as_deref(), &speaker) {
            _ if placeholder => LineKind::Placeholder,
            (Some("option"), _) => LineKind::Option,
            (_, Some(_)) => LineKind::Dialogue,
            (Some("narration"), None) => LineKind::Narration,
            _ => LineKind::Unknown,
        };
        if let (Some(display), Some(identity)) = (&speaker, row.speaker_id.as_deref())
            && !identity.trim().is_empty()
        {
            let identity = identity.trim().to_owned();
            if let Some(previous) = builder
                .speaker_identities
                .insert(display.clone(), identity.clone())
                && previous != identity
            {
                bail!("说话人 {display} 对应多个角色身份：{previous} / {identity}");
            }
        }
        if kind == DocKind::Story {
            stats.lines += 1;
            stats.with_speaker += usize::from(speaker.is_some());
        }
        let draft = &mut builder.docs[doc];
        if let Some(names) = row.scene_speakers {
            for name in names {
                let name = name.trim();
                if !name.is_empty() && !draft.name_table.iter().any(|s| s == name) {
                    draft.name_table.push(name.to_owned());
                }
            }
        }
        let names = if evidence.speaker_status.as_deref() == Some("inferred") {
            &mut draft.inferred_speakers
        } else {
            &mut draft.speakers
        };
        if let Some(s) = &speaker
            && (evidence
                .speaker_status
                .as_deref()
                .is_none_or(|s| matches!(s, "resolved" | "inferred")))
            && line_kind == LineKind::Dialogue
            && !names.contains(s)
        {
            names.push(s.clone());
        }
        draft.lines.push(DraftLine {
            text: text.to_owned(),
            speaker,
            kind: line_kind,
            source: LineSource {
                source_key: row.source_key.as_deref().map(str::to_owned),
                line_id: row.line_id.as_deref().map(str::to_owned),
            },
            evidence,
        });
    }
    Ok(stats)
}

fn lore_category_label(dir: &str) -> String {
    match dir {
        "agents" => "代理人档案".into(),
        "equipment" => "音擎·驱动盘·邦布·收藏".into(),
        "interknot" => "绳网委托与私信".into(),
        "cinema" => "录像店影片".into(),
        "glossary" => "世界观百科".into(),
        "street" => "街区闲聊".into(),
        other => other.into(),
    }
}

fn lore_category_order(dir: &str) -> i64 {
    match dir {
        "agents" => 0,
        "glossary" => 1,
        "equipment" => 2,
        "interknot" => 3,
        "cinema" => 4,
        "street" => 5,
        _ => 9,
    }
}

/// 去掉文件名的数字前缀：`01_狡兔屋` → `狡兔屋`。
fn strip_order_prefix(stem: &str) -> &str {
    let trimmed = stem.trim_start_matches(|c: char| c.is_ascii_digit());
    let trimmed = trimmed.strip_prefix('_').unwrap_or(trimmed);
    if trimmed.is_empty() { stem } else { trimmed }
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|&b| b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = line[level..].strip_prefix(' ')?;
    Some((level, rest.trim()))
}

/// 去掉标题里的 Markdown 强调符号。
fn plain_heading(text: &str) -> String {
    text.replace("**", "").replace('`', "").trim().to_owned()
}

pub(crate) struct LoreSection {
    pub title: String,
    pub lines: Vec<String>,
}

/// 把一个 Markdown 文件切成若干节。
///
/// 默认按二级标题切；若二级标题很少而三级标题更多（如代理人档案：一个「阵营」下挂多位代理人），
/// 改按三级标题切。没有可切的标题时整个文件是一节。
pub(crate) fn split_markdown(content: &str, fallback_title: &str) -> Vec<LoreSection> {
    let h2 = content
        .lines()
        .filter(|l| heading(l).is_some_and(|(lv, _)| lv == 2))
        .count();
    let h3 = content
        .lines()
        .filter(|l| heading(l).is_some_and(|(lv, _)| lv == 3))
        .count();
    let split_level = if h2 <= 2 && h3 > h2 { 3 } else { 2 };

    let mut file_title = fallback_title.to_owned();
    let mut sections: Vec<LoreSection> = Vec::new();
    let mut current = LoreSection {
        title: String::new(),
        lines: Vec::new(),
    };
    for raw in content.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim() == "---" {
            continue;
        }
        if let Some((level, text)) = heading(line) {
            if level == 1 {
                file_title = plain_heading(text);
                continue;
            }
            if level == split_level {
                if !current.lines.is_empty() {
                    sections.push(std::mem::replace(
                        &mut current,
                        LoreSection {
                            title: String::new(),
                            lines: Vec::new(),
                        },
                    ));
                }
                current.title = plain_heading(text);
                continue;
            }
            if level < split_level {
                // 更高层的标题（如「阵营：狡兔屋」）：作为后续节的上下文行保留到下一节之前。
                if !current.lines.is_empty() {
                    sections.push(std::mem::replace(
                        &mut current,
                        LoreSection {
                            title: String::new(),
                            lines: Vec::new(),
                        },
                    ));
                }
                current.title = plain_heading(text);
                continue;
            }
        }
        current.lines.push(line.to_owned());
    }
    if !current.lines.is_empty() {
        sections.push(current);
    }
    if file_title.is_empty() {
        file_title = fallback_title.to_owned();
    }
    for s in &mut sections {
        if s.title.is_empty() {
            s.title.clone_from(&file_title);
        }
    }
    sections
}

/// `files`：lore 目录下的相对路径（`/` 分隔）→ Markdown。第一层目录是分类；
/// lore 根目录下的文件（README.md 是目录索引）不收录。
fn ingest_lore(builder: &mut Builder, files: &BTreeMap<String, String>) {
    let mut entries: Vec<(String, &String, &String)> = files
        .iter()
        .filter(|(rel, _)| rel.ends_with(".md"))
        .filter_map(|(rel, content)| {
            let (category, _) = rel.split_once('/')?;
            Some((category.to_owned(), rel, content))
        })
        .collect();
    entries.sort_by(|a, b| (lore_category_order(&a.0), a.1).cmp(&(lore_category_order(&b.0), b.1)));

    let parsed: Vec<(String, String, Vec<LoreSection>)> = entries
        .into_iter()
        .map(|(category, rel_key, content)| {
            let stem = Path::new(rel_key)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let sections = split_markdown(content, strip_order_prefix(&stem));
            (category, rel_key.clone(), sections)
        })
        .collect();

    for (category, rel_key, sections) in parsed {
        let label = lore_category_label(&category);
        let group = builder.group(
            format!("lore:{category}"),
            GroupKind::Lore,
            &label,
            lore_category_order(&category),
        );
        let file_stem = Path::new(&rel_key)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file_label = strip_order_prefix(&file_stem).to_owned();
        let mut seen_content: HashMap<u64, ()> = HashMap::new();
        let mut title_count: HashMap<String, usize> = HashMap::new();
        for section in sections {
            let content_hash =
                xxh3_64(section.lines.join("\n").as_bytes()) ^ xxh3_64(section.title.as_bytes());
            if seen_content.insert(content_hash, ()).is_some() {
                // 同一文件里完全重复的条目（例如同名音擎的两份相同记录）只保留一份。
                continue;
            }
            let n = title_count.entry(section.title.clone()).or_insert(0);
            *n += 1;
            let key = format!("lore|{rel_key}|{}|{}", section.title, n);
            builder.push_doc(DraftDoc {
                key,
                kind: DocKind::Lore,
                group,
                section: file_label.clone(),
                title: section.title,
                path: format!("设定 › {label} › {file_label}"),
                quest: None,
                order_source: None,
                chronology_status: None,
                context_status: None,
                lines: section
                    .lines
                    .into_iter()
                    .map(|text| DraftLine {
                        text,
                        speaker: None,
                        kind: LineKind::Text,
                        source: LineSource::default(),
                        evidence: LineEvidence::default(),
                    })
                    .collect(),
                speakers: Vec::new(),
                name_table: Vec::new(),
                inferred_speakers: Vec::new(),
            });
        }
    }
}

/// 把草稿落成扁平结构并建索引。
fn finish(builder: Builder, meta: Meta) -> Result<Corpus> {
    let Builder {
        docs: drafts,
        groups: mut draft_groups,
        speaker_identities: identities,
        ..
    } = builder;

    // 分组顺序：章节按游戏顺序，设定在后；文档按「分组顺序 → 组内阅读顺序」重新排列，
    // 让同一章的场景在行数组里也相邻（顺读时缓存友好）。
    let mut group_order: Vec<usize> = (0..draft_groups.len()).collect();
    group_order.sort_by_key(|&g| {
        let dg = &draft_groups[g];
        (matches!(dg.kind, GroupKind::Lore), dg.order, g)
    });
    let mut doc_order: Vec<usize> = Vec::with_capacity(drafts.len());
    for &g in &group_order {
        let dg = &mut draft_groups[g];
        // 组内：主线在前，补充在后，各自保持出现顺序。
        dg.docs
            .sort_by_key(|&d| (drafts[d].kind == DocKind::Supplement, d));
        doc_order.extend(dg.docs.iter().copied());
    }
    let mut new_group_index = vec![0u32; draft_groups.len()];
    for (new, &old) in group_order.iter().enumerate() {
        new_group_index[old] = new as u32;
    }

    let mut speakers: Vec<String> = Vec::new();
    let mut speaker_ids: HashMap<String, u32> = HashMap::new();
    let mut intern = |name: &str, speakers: &mut Vec<String>| -> u32 {
        if let Some(&id) = speaker_ids.get(name) {
            return id;
        }
        let id = speakers.len() as u32;
        speakers.push(name.to_owned());
        speaker_ids.insert(name.to_owned(), id);
        id
    };

    let total_lines: usize = drafts.iter().map(|d| d.lines.len()).sum();
    let total_bytes: usize = drafts
        .iter()
        .flat_map(|d| d.lines.iter())
        .map(|l| l.text.len())
        .sum();
    let mut text = String::with_capacity(total_bytes);
    let mut lines = Vec::with_capacity(total_lines);
    let mut line_sources = Vec::with_capacity(total_lines);
    let mut line_evidence_ids = Vec::with_capacity(total_lines);
    let mut evidence = Vec::new();
    let mut evidence_lookup = HashMap::new();
    let mut evidence_strings: HashMap<Arc<str>, Arc<str>> = HashMap::new();
    let mut docs = Vec::with_capacity(drafts.len());
    let mut doc_speakers = Vec::new();
    let mut new_doc_index = vec![0u32; drafts.len()];
    let mut seen_hashes: HashMap<u64, String> = HashMap::with_capacity(drafts.len());

    for (new, &old) in doc_order.iter().enumerate() {
        new_doc_index[old] = new as u32;
        let d = &drafts[old];
        let hash = xxh3_64(d.key.as_bytes()) & DOC_HASH_MASK;
        if let Some(prev) = seen_hashes.insert(hash, d.key.clone()) {
            bail!("文档标识碰撞：{prev} 与 {}", d.key);
        }
        let first_line = lines.len() as u32;
        let mut char_count = 0usize;
        for l in &d.lines {
            let start = text.len() as u32;
            text.push_str(&l.text);
            char_count += l.text.chars().count();
            let speaker = l
                .speaker
                .as_deref()
                .map_or(NO_SPEAKER, |s| intern(s, &mut speakers));
            lines.push(Line {
                start,
                len: l.text.len() as u32,
                speaker,
                kind: l.kind,
            });
            line_sources.push(l.source.clone());
            let mut entry = l.evidence.clone();
            entry.intern(&mut evidence_strings);
            let key = xxh3_128(&serde_json::to_vec(&entry)?);
            if let Some(&id) = evidence_lookup.get(&key)
                && evidence[id as usize] != entry
            {
                bail!("提取证据摘要碰撞，请重新生成语料包");
            }
            let id = *evidence_lookup.entry(key).or_insert_with(|| {
                let id = evidence.len() as u32;
                evidence.push(entry);
                id
            });
            line_evidence_ids.push(id);
        }
        let speakers_start = doc_speakers.len() as u32;
        for s in &d.speakers {
            doc_speakers.push(intern(s, &mut speakers));
        }
        docs.push(Doc {
            hash,
            kind: d.kind,
            group: new_group_index[d.group],
            group_pos: 0,
            section: d.section.clone(),
            title: d.title.clone(),
            path: d.path.clone(),
            quest: d.quest.clone(),
            order_source: d.order_source.clone(),
            chronology_status: d.chronology_status.clone(),
            context_status: d.context_status.clone(),
            first_line,
            line_count: d.lines.len() as u32,
            char_count: char_count as u32,
            speakers_start,
            speakers_len: d.speakers.len() as u32,
            name_table: d
                .name_table
                .iter()
                .map(|s| intern(s, &mut speakers))
                .collect(),
            inferred_speakers: d
                .inferred_speakers
                .iter()
                .map(|s| intern(s, &mut speakers))
                .collect(),
            evidence_counts: {
                let mut counts = super::EvidenceCounts::default();
                for line in &d.lines {
                    counts.placeholders += usize::from(line.kind == LineKind::Placeholder);
                    counts.unknown += usize::from(
                        line.kind == LineKind::Unknown
                            || (line.kind == LineKind::Dialogue
                                && line
                                    .evidence
                                    .speaker_status
                                    .as_deref()
                                    .is_some_and(|s| !matches!(s, "resolved" | "inferred"))),
                    );
                    counts.inferred +=
                        usize::from(line.evidence.speaker_status.as_deref() == Some("inferred"));
                    counts.missing += usize::from(line.evidence.speaker_status.is_none());
                    counts.unlinked += usize::from(line.evidence.unlinked_candidate());
                    counts.no_voice += usize::from(line.evidence.has_voice == Some(false));
                }
                counts
            },
        });
    }
    if text.len() > u32::MAX as usize {
        bail!("语料超过 4 GiB，超出索引寻址范围");
    }
    // 原文、来源与去重证据均已落成；构建倒排索引前释放草稿和临时字典，降低峰值驻留内存。
    drop(drafts);
    drop(evidence_lookup);
    drop(evidence_strings);

    let mut groups: Vec<Group> = Vec::with_capacity(group_order.len());
    for &old in &group_order {
        let dg = &draft_groups[old];
        let members: Vec<u32> = dg.docs.iter().map(|&d| new_doc_index[d]).collect();
        for (pos, &d) in members.iter().enumerate() {
            docs[d as usize].group_pos = pos as u32;
        }
        groups.push(Group {
            id: dg.id.clone(),
            kind: dg.kind,
            label: dg.label.clone(),
            docs: members,
        });
    }

    let folded_lines: Vec<String> = lines
        .iter()
        .map(|l: &Line| fold(&text[l.start as usize..(l.start + l.len) as usize]))
        .collect();
    let index = BigramIndex::build(&folded_lines);
    drop(folded_lines);

    let speakers_folded = speakers.iter().map(|s| fold(s)).collect();
    let speaker_identity_names: Vec<String> = speakers
        .iter()
        .map(|s| identities.get(s).unwrap_or(s).clone())
        .collect();
    let speaker_identities = speaker_identity_names.iter().map(|s| fold(s)).collect();
    let mut by_speaker: Vec<Vec<u32>> = vec![Vec::new(); speakers.len()];
    for (i, d) in docs.iter().enumerate() {
        let start = d.speakers_start as usize;
        for &s in &doc_speakers[start..start + d.speakers_len as usize] {
            by_speaker[s as usize].push(i as u32);
        }
        for &s in d.name_table.iter().chain(&d.inferred_speakers) {
            let list = &mut by_speaker[s as usize];
            if list.last() != Some(&(i as u32)) {
                list.push(i as u32);
            }
        }
    }
    let mut speaker_doc_offsets = Vec::with_capacity(speakers.len() + 1);
    let mut speaker_docs = Vec::new();
    for list in by_speaker {
        speaker_doc_offsets.push(speaker_docs.len() as u32);
        speaker_docs.extend(list);
    }
    speaker_doc_offsets.push(speaker_docs.len() as u32);
    let doc_keys = DocKeys::build(&docs);
    let mut doc_lookup: Vec<(u64, u32)> = docs
        .iter()
        .enumerate()
        .map(|(i, d)| (d.hash, i as u32))
        .collect();
    doc_lookup.sort_unstable();
    let mut line_doc = Vec::with_capacity(lines.len());
    for (i, d) in docs.iter().enumerate() {
        line_doc.extend(std::iter::repeat_n(i as u32, d.line_count as usize));
    }

    let mut corpus = Corpus {
        revision: String::new(),
        meta,
        text,
        lines,
        line_sources,
        line_evidence_ids,
        evidence,
        docs,
        groups,
        speakers,
        speakers_folded,
        speaker_identities,
        speaker_identity_names,
        doc_speakers,
        speaker_doc_offsets,
        speaker_docs,
        doc_keys,
        doc_lookup,
        line_doc,
        index,
    };
    corpus.revision = super::revision::revision(&corpus)?;
    Ok(corpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_agents_by_h3_and_quests_by_h2() {
        let agents = "# 阵营档案\n\n## 阵营：狡兔屋\n\n### 安比\n\n| a | b |\n\n#### 子节\n\n正文一\n\n### 妮可\n\n正文二\n";
        let s = split_markdown(agents, "狡兔屋");
        let titles: Vec<_> = s.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, vec!["安比", "妮可"]);
        assert_eq!(s[0].lines, vec!["| a | b |", "#### 子节", "正文一"]);

        let quests = "# 委托\n\n> 引言\n\n## 【委托甲】\n\n### 详情\n\n内容甲\n\n---\n\n## 【委托乙】\n\n### 详情\n\n内容乙\n\n## 【委托丙】\n\n内容丙\n";
        let s = split_markdown(quests, "委托档案");
        let titles: Vec<_> = s.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["委托", "【委托甲】", "【委托乙】", "【委托丙】"]
        );
        assert_eq!(s[0].lines, vec!["> 引言"]);
        assert_eq!(s[1].lines, vec!["### 详情", "内容甲"]);
    }

    #[test]
    fn file_without_headings_is_one_section() {
        let s = split_markdown("- 甲\n- 乙\n", "街区闲聊");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].title, "街区闲聊");
        assert_eq!(s[0].lines.len(), 2);
    }

    #[test]
    fn strips_numeric_prefix() {
        assert_eq!(strip_order_prefix("01_狡兔屋"), "狡兔屋");
        assert_eq!(strip_order_prefix("README"), "README");
        assert_eq!(strip_order_prefix("2024"), "2024");
    }
}
