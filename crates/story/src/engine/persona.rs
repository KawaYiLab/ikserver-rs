//! ik_character(purpose="roleplay")：角色扮演参考资料。
//!
//! 主要素材来自代理人档案：身份与性格小节原样给出，信赖邀约、街区日常、随行气泡按幕抽取原文片段。
//! 档案对话只保留原文的【名字】标注，不逐句署名。主线逐句署名台词作为辅助（关系与语气）。
//! 性格、语气等归纳不是语料字段，不在这里生成。抽样是确定性的，同一快照同一参数结果相同。

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use serde_json::json;

use super::response::*;
use super::search::match_speakers;
use super::{clamp, err, push_line};
use crate::corpus::{Corpus, DocKind, GroupKind, LineKind};
use crate::text::{fold, thousands};

/// 主线台词样本的字数范围：太短的（「嗯。」「……」）信息量低，太长的多为大段说明。
const MIN_CHARS: usize = 6;
const MAX_CHARS: usize = 120;
/// 前一句在文本里的截断长度；structuredContent 保留全文。
const CONTEXT_CHARS: usize = 60;
const MAX_PARTNERS: usize = 8;
/// 先保证这么多位常对话对象各有一条往来样本，其余名额按章节分散。
const PARTNER_SAMPLES: usize = 5;
/// 档案身份/性格小节每节的字数上限（按整行截断）。
const SECTION_CHARS: usize = 1500;
/// 每幕档案对话最多列出的行数与字数。
const SCENE_LINES: usize = 30;
const SCENE_CHARS: usize = 1500;
/// 随行气泡每组只有两三句，整体占一个名额，最多取这么多组。
const BUBBLE_GROUPS: usize = 10;
/// 街区日常优先选不少于这么多行的幕。
const SCENE_MIN_LINES: usize = 4;

/// 档案小节的用途，按提取器输出的固定标题判断；不认识的小节只列入口，不丢弃。
#[derive(Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    Profile,
    Trust,
    Daily,
    Bubble,
    Other,
}

fn classify(heading: &str) -> SectionKind {
    const PROFILE: &[&str] = &["空洞调查协会", "深度身世", "录像店私密评语"];
    if heading.is_empty() || PROFILE.iter().any(|p| heading.starts_with(p)) {
        SectionKind::Profile
    } else if heading.starts_with("专属信赖邀约") {
        SectionKind::Trust
    } else if heading.starts_with("街区偶遇") {
        SectionKind::Daily
    } else if heading.starts_with("随行气泡") {
        SectionKind::Bubble
    } else {
        SectionKind::Other
    }
}

/// 档案文档内的小节（`####`）与其下的幕（`#####`）。行号为文档内 0 起下标。
struct Section {
    heading: String,
    kind: SectionKind,
    /// 标题行；开头部分没有标题行，等于 body.start。
    start: usize,
    body: std::ops::Range<usize>,
    scenes: Vec<Scene>,
}

struct Scene {
    heading: String,
    start: usize,
    body: std::ops::Range<usize>,
}

fn parse_sections(lines: &[&str]) -> Vec<Section> {
    let mut sections: Vec<Section> = vec![Section {
        heading: String::new(),
        kind: SectionKind::Profile,
        start: 0,
        body: 0..0,
        scenes: Vec::new(),
    }];
    for (i, line) in lines.iter().enumerate() {
        if let Some(h) = line.strip_prefix("#### ") {
            let h = h.trim().to_owned();
            sections.push(Section {
                kind: classify(&h),
                heading: h,
                start: i,
                body: i + 1..i + 1,
                scenes: Vec::new(),
            });
        } else if let Some(section) = sections.last_mut() {
            section.body.end = i + 1;
            if let Some(h) = line.strip_prefix("##### ") {
                section.scenes.push(Scene {
                    heading: h.trim().to_owned(),
                    start: i,
                    body: i + 1..i + 1,
                });
            } else if let Some(scene) = section.scenes.last_mut() {
                scene.body.end = i + 1;
            }
        }
    }
    sections.retain(|s| !(s.heading.is_empty() && s.body.is_empty()));
    sections
}

/// 去掉档案行尾的来源键「 (`Key`)」与列表前缀，只用于文本展示。
fn display_line(line: &str) -> &str {
    let mut s = line.trim();
    if s.ends_with("`)")
        && let Some(i) = s.rfind(" (`")
    {
        s = s[..i].trim_end();
    }
    s.strip_prefix("- ").unwrap_or(s)
}

/// 标题「青衣（青衣）」「「11号」（「11号」）」「雅（星见 雅）」→ 可比较的名字键。
fn title_keys(title: &str) -> (String, String) {
    let clean = |s: &str| {
        fold(
            &s.chars()
                .filter(|c| !matches!(c, '「' | '」' | ' ' | '\u{3000}'))
                .collect::<String>(),
        )
    };
    let (short, full) = match title.split_once('（') {
        Some((a, b)) => (a, b.trim_end_matches('）')),
        None => (title, ""),
    };
    (clean(short), clean(full))
}

/// 选一篇代理人档案：标题名或括号里的全名与查询精确一致优先；否则标题含名字且只有一篇时用它。
/// 返回（选中的档案，其余标题含名字的代理人档案）。
fn pick_profile(corpus: &Corpus, name: &str) -> (Option<usize>, Vec<usize>) {
    let key = title_keys(name).0;
    let agents: Vec<usize> = corpus
        .groups
        .iter()
        .filter(|g| g.id == "lore:agents")
        .flat_map(|g| g.docs.iter().map(|&d| d as usize))
        .collect();
    let exact: Vec<usize> = agents
        .iter()
        .copied()
        .filter(|&d| {
            let (short, full) = title_keys(&corpus.docs[d].title);
            short == key || (!full.is_empty() && full == key)
        })
        .collect();
    let containing: Vec<usize> = agents
        .iter()
        .copied()
        .filter(|&d| title_keys(&corpus.docs[d].title).0.contains(&key))
        .collect();
    match (exact.first(), containing.as_slice()) {
        (Some(&d), _) => (
            Some(d),
            containing.into_iter().filter(|&x| x != d).collect(),
        ),
        (None, [only]) => (Some(*only), Vec::new()),
        (None, _) => (None, containing),
    }
}

/// 等距取 `k` 个下标（含首个），保持原顺序。
fn spread(len: usize, k: usize) -> Vec<usize> {
    if k == 0 || len == 0 {
        return Vec::new();
    }
    if k >= len {
        return (0..len).collect();
    }
    (0..k).map(|i| i * len / k).collect()
}

struct Candidate {
    line: usize,
    doc: usize,
    chars: usize,
    partner: Option<String>,
}

pub(crate) struct RoleplayArgs<'a> {
    pub name: &'a str,
    pub until: Option<&'a str>,
    pub max_samples: Option<usize>,
    pub max_scenes: Option<usize>,
    pub include_evidence: Option<bool>,
}

pub(crate) fn execute(corpus: &Corpus, args: &RoleplayArgs) -> OutputResult {
    let name = args.name;
    if args.include_evidence == Some(true) {
        return err(format!(
            "purpose=roleplay 不展开逐行证据；逐句署名证据见 ik_search(speaker=\"{name}\", includeEvidence=true)"
        ));
    }
    let n_samples = clamp(args.max_samples, 8, 40);
    let n_scenes = clamp(args.max_scenes, 6, 20);
    let folded = fold(name);
    let ids = match_speakers(corpus, name);
    let exact = ids.iter().any(|&i| {
        corpus.speakers_folded[i as usize] == folded
            || corpus.speaker_identities[i as usize] == folded
    });
    // 包含匹配可能把几个不同角色混在一起；扮演资料必须是同一个人。
    let mut identities: Vec<&str> = Vec::new();
    for &i in &ids {
        let identity = corpus.speaker_identity_names[i as usize].as_str();
        if !identities.contains(&identity) {
            identities.push(identity);
        }
    }
    if !exact && identities.len() > 1 {
        let names: Vec<&str> = ids.iter().filter_map(|&i| corpus.speaker_name(i)).collect();
        return err(format!(
            "「{name}」不是精确的说话人名，包含匹配到多个角色：{}。扮演资料只针对一个角色，请传入其中一个名字",
            names.join("、")
        ));
    }
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

    let cutoff = match args.until.map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(path) => {
            let g = super::resolve_group(corpus, path)?;
            if corpus.groups[g].kind != GroupKind::Chapter {
                return err(format!(
                    "until 只接受主线章节或已核验版本别名，「{path}」是设定分类"
                ));
            }
            Some(g)
        }
    };

    let (profile_doc, other_profiles) = pick_profile(corpus, name);
    if ids.is_empty() && profile_doc.is_none() {
        let hint = if other_profiles.is_empty() {
            String::new()
        } else {
            format!(
                "；标题含此名字的代理人档案：{}",
                other_profiles
                    .iter()
                    .map(|&d| corpus.docs[d].title.as_str())
                    .collect::<Vec<_>>()
                    .join("、")
            )
        };
        return err(format!(
            "语料中没有找到说话人或代理人档案「{name}」{hint}。试试全名、简称或别的写法（如「零号·安比」「11号」）"
        ));
    }

    let (sections, scenes) = match profile_doc {
        Some(d) => profile_material(corpus, d, n_scenes),
        None => (Vec::new(), Vec::new()),
    };

    let story = story_material(corpus, &ids, &related, cutoff, n_samples);

    let result = PersonaResult {
        purpose: "roleplay".into(),
        name: name.into(),
        resolved_names: ids
            .iter()
            .filter_map(|&s| corpus.speaker_name(s).map(str::to_owned))
            .collect(),
        related_names: related
            .iter()
            .filter_map(|&s| corpus.speaker_name(s).map(str::to_owned))
            .collect(),
        until: cutoff.map(|g| PersonaCutoff {
            group_id: corpus.groups[g].id.clone(),
            title: corpus.groups[g].label.clone(),
        }),
        profile: profile_doc.map(|d| PersonaProfile {
            doc_id: corpus.doc_id(d),
            title: corpus.docs[d].title.clone(),
            line_count: corpus.docs[d].line_count as usize,
            sections,
        }),
        other_profiles: other_profiles
            .iter()
            .map(|&d| corpus.docs[d].title.clone())
            .collect(),
        dialogue_scenes: scenes,
        story_spoken_lines: story.spoken,
        story_scenes: story.scenes,
        story_candidate_lines: story.candidates,
        partners: story.partners,
        story_samples: story.samples,
        next_calls: if ids.is_empty() {
            Vec::new()
        } else {
            vec![NextCall::new("ik_search", json!({"speaker":name}))]
        },
    };
    let text = render(corpus, &result, &story.globals);
    Ok(ToolOutput::new(corpus, text, ToolData::Persona(result)))
}

fn read_call(corpus: &Corpus, doc: usize, offset: usize, limit: usize) -> NextCall {
    NextCall::new(
        "ik_read",
        json!({"docId":corpus.doc_id(doc),"offset":offset,"limit":limit.max(1)}),
    )
}

/// 档案小节与对话幕。
fn profile_material(
    corpus: &Corpus,
    d: usize,
    n_scenes: usize,
) -> (Vec<PersonaSection>, Vec<PersonaScene>) {
    let doc = &corpus.docs[d];
    let lines: Vec<&str> = doc.lines().map(|l| corpus.line_text(l)).collect();
    let sections = parse_sections(&lines);

    let mut out_sections = Vec::with_capacity(sections.len());
    for s in &sections {
        let kind = match s.kind {
            SectionKind::Profile => "profile",
            SectionKind::Trust | SectionKind::Daily | SectionKind::Bubble => "dialogue",
            SectionKind::Other => "other",
        };
        let (text, shown) = if s.kind == SectionKind::Profile {
            let mut text = String::new();
            let mut shown = 0usize;
            for line in &lines[s.body.clone()] {
                if !text.is_empty() && text.chars().count() + line.chars().count() > SECTION_CHARS {
                    break;
                }
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(line);
                shown += 1;
            }
            (Some(text), shown)
        } else {
            (None, 0)
        };
        let total = s.body.len();
        out_sections.push(PersonaSection {
            heading: if s.heading.is_empty() {
                "（开头）".into()
            } else {
                s.heading.clone()
            },
            kind: kind.into(),
            line: s.start + 1,
            line_count: s.body.end - s.start,
            scene_count: s.scenes.len(),
            truncated: text.is_some() && shown < total,
            text,
            read: if s.kind == SectionKind::Profile && shown < total {
                read_call(corpus, d, s.body.start + shown + 1, total - shown)
            } else {
                read_call(corpus, d, s.start + 1, s.body.end - s.start)
            },
        });
    }

    // 名额：信赖邀约 1 幕、随行气泡 1 组块，其余给街区日常；缺哪类就让给其余类。
    let of = |k: SectionKind| -> Vec<(&Section, &Scene)> {
        sections
            .iter()
            .filter(|s| s.kind == k)
            .flat_map(|s| s.scenes.iter().map(move |sc| (s, sc)))
            .collect()
    };
    let trust = of(SectionKind::Trust);
    let daily = of(SectionKind::Daily);
    let bubbles = of(SectionKind::Bubble);
    let mut bubble_slots = usize::from(!bubbles.is_empty() && n_scenes >= 2);
    let mut trust_slots = usize::from(!trust.is_empty() && n_scenes >= 3);
    let mut daily_slots = n_scenes - bubble_slots - trust_slots;
    if daily.is_empty() {
        trust_slots = (trust_slots + daily_slots).min(trust.len());
        daily_slots = 0;
        if trust.is_empty() && !bubbles.is_empty() {
            bubble_slots = 1;
        }
    }

    let mut scenes = Vec::new();
    for i in spread(trust.len(), trust_slots) {
        let (s, sc) = trust[i];
        scenes.push(scene(
            corpus,
            d,
            &lines,
            s,
            std::slice::from_ref(sc),
            trust.len(),
        ));
    }
    // 街区日常里有不少一两行的后续幕，优先选够长的。
    let long: Vec<usize> = (0..daily.len())
        .filter(|&i| daily[i].1.body.len() >= SCENE_MIN_LINES)
        .collect();
    let pool: Vec<usize> = if long.len() >= daily_slots {
        long
    } else {
        (0..daily.len()).collect()
    };
    for i in spread(pool.len(), daily_slots) {
        let (s, sc) = daily[pool[i]];
        scenes.push(scene(
            corpus,
            d,
            &lines,
            s,
            std::slice::from_ref(sc),
            daily.len(),
        ));
    }
    if bubble_slots > 0 {
        let picked: Vec<&Scene> = spread(bubbles.len(), BUBBLE_GROUPS)
            .into_iter()
            .map(|i| bubbles[i].1)
            .collect();
        let owned: Vec<Scene> = picked
            .iter()
            .map(|sc| Scene {
                heading: sc.heading.clone(),
                start: sc.start,
                body: sc.body.clone(),
            })
            .collect();
        scenes.push(scene(
            corpus,
            d,
            &lines,
            bubbles[0].0,
            &owned,
            bubbles.len(),
        ));
    }
    // 输出按档案行序。
    scenes.sort_by_key(|s| s.line);
    (out_sections, scenes)
}

/// 一幕（或一组随行气泡）原文：最多 SCENE_LINES 行 / SCENE_CHARS 字，按整行截断。
fn scene(
    corpus: &Corpus,
    d: usize,
    lines: &[&str],
    section: &Section,
    parts: &[Scene],
    of_total: usize,
) -> PersonaScene {
    let mut shown: Vec<PersonaLine> = Vec::new();
    let mut chars = 0usize;
    let mut total = 0usize;
    let mut next_unshown: Option<usize> = None;
    for part in parts {
        for i in part.body.clone() {
            let text = lines[i];
            if text.trim().is_empty() {
                continue;
            }
            total += 1;
            let c = display_line(text).chars().count();
            if next_unshown.is_none()
                && (shown.len() >= SCENE_LINES || (!shown.is_empty() && chars + c > SCENE_CHARS))
            {
                next_unshown = Some(i);
            }
            if next_unshown.is_none() {
                chars += c;
                shown.push(PersonaLine {
                    number: i + 1,
                    text: text.to_owned(),
                });
            }
        }
    }
    let first = parts.first().map_or(section.start, |p| p.start);
    let last = parts.last().map_or(section.body.end, |p| p.body.end);
    let bubble = section.kind == SectionKind::Bubble;
    PersonaScene {
        section: section.heading.clone(),
        heading: if bubble {
            format!("{}/{} 组", parts.len(), of_total)
        } else {
            parts.first().map(|p| p.heading.clone()).unwrap_or_default()
        },
        line: first + 1,
        line_count: total,
        lines: shown,
        truncated: next_unshown.is_some(),
        read: match (bubble, next_unshown) {
            (false, Some(i)) => read_call(corpus, d, i + 1, last - i),
            _ => read_call(corpus, d, first + 1, last - first),
        },
        section_read: read_call(
            corpus,
            d,
            section.start + 1,
            section.body.end - section.start,
        ),
        section_scenes: of_total,
    }
}

struct Story {
    spoken: usize,
    scenes: usize,
    candidates: usize,
    partners: Vec<PersonaPartner>,
    samples: Vec<PersonaSample>,
    /// 文本渲染用全局行号（样本本句, 前一句）。
    globals: Vec<(usize, Option<usize>)>,
}

/// 主线逐句署名台词：常对话对象与分散抽取的样本。
fn story_material(
    corpus: &Corpus,
    ids: &[u32],
    related: &[u32],
    cutoff: Option<usize>,
    n: usize,
) -> Story {
    let mut spoken = 0usize;
    let mut scenes = 0usize;
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut seen_text: HashSet<String> = HashSet::new();
    let mut exchanges: HashMap<&str, usize> = HashMap::new();
    let mut partner_scenes: HashMap<&str, usize> = HashMap::new();
    let target = |l: usize| ids.contains(&corpus.lines[l].speaker) && corpus.is_attributed(l);
    let partner_of = |l: usize| -> Option<&str> {
        let sp = corpus.lines[l].speaker;
        if ids.contains(&sp) || related.contains(&sp) || !corpus.is_attributed(l) {
            return None;
        }
        let identity = corpus.speaker_identity_names[sp as usize].as_str();
        (!super::character::is_placeholder(identity)).then_some(identity)
    };
    for (d, doc) in corpus.docs.iter().enumerate() {
        if doc.kind == DocKind::Lore || ids.is_empty() {
            continue;
        }
        if cutoff.is_some_and(|g| doc.group as usize > g) {
            continue;
        }
        if !corpus.doc_speakers(doc).iter().any(|s| ids.contains(s)) {
            continue;
        }
        let range = doc.lines();
        let mut here = 0usize;
        let mut partners_here: HashSet<&str> = HashSet::new();
        for l in range.clone() {
            let is_target = target(l);
            // 相邻两句署名对白，一边是该角色，一边是别人：算一次往来。
            if l > range.start {
                let other = match (target(l - 1), is_target) {
                    (true, false) => partner_of(l),
                    (false, true) => partner_of(l - 1),
                    _ => None,
                };
                if let Some(p) = other {
                    *exchanges.entry(p).or_default() += 1;
                    partners_here.insert(p);
                }
            }
            if !is_target {
                continue;
            }
            here += 1;
            let text = corpus.line_text(l).trim();
            let chars = text.chars().count();
            if !(MIN_CHARS..=MAX_CHARS).contains(&chars) || !seen_text.insert(fold(text)) {
                continue;
            }
            let partner = (l > range.start)
                .then(|| partner_of(l - 1))
                .flatten()
                .map(str::to_owned);
            candidates.push(Candidate {
                line: l,
                doc: d,
                chars,
                partner,
            });
        }
        if here > 0 {
            scenes += 1;
            spoken += here;
            for p in partners_here {
                *partner_scenes.entry(p).or_default() += 1;
            }
        }
    }

    let mut partners: Vec<(&str, usize)> = exchanges.into_iter().collect();
    partners.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    partners.truncate(MAX_PARTNERS);
    let chosen = select(&candidates, &partners, n);

    let mut samples = Vec::with_capacity(chosen.len());
    let mut globals = Vec::with_capacity(chosen.len());
    for &i in &chosen {
        let c = &candidates[i];
        let doc = &corpus.docs[c.doc];
        let first = doc.first_line as usize;
        let prev = (c.line > first)
            .then_some(c.line - 1)
            .filter(|&p| corpus.lines[p].kind != LineKind::Placeholder);
        let number = c.line - first + 1;
        let group = &corpus.groups[doc.group as usize];
        samples.push(PersonaSample {
            doc_id: corpus.doc_id(c.doc),
            title: doc.title.clone(),
            group_id: group.id.clone(),
            group_title: group.label.clone(),
            line: SourceLine::new(corpus, c.line, false),
            context: prev.map(|p| {
                let mut line = SourceLine::new(corpus, p, false);
                line.is_context = true;
                line
            }),
            read: read_call(corpus, c.doc, number.saturating_sub(3).max(1), 8),
        });
        globals.push((c.line, prev));
    }
    Story {
        spoken,
        scenes,
        candidates: candidates.len(),
        partners: partners
            .iter()
            .map(|&(p, x)| PersonaPartner {
                name: p.into(),
                exchanges: x,
                scenes: partner_scenes.get(p).copied().unwrap_or(0),
            })
            .collect(),
        samples,
        globals,
    }
}

/// 确定性抽样：先给前几位常对话对象各一条往来样本，再按篇轮流取最长的未选台词，
/// 名额少于篇数时在篇目间等距取，避免样本挤在开头几章。结果按语料行序排列。
fn select(candidates: &[Candidate], partners: &[(&str, usize)], n: usize) -> Vec<usize> {
    let mut chosen: Vec<usize> = Vec::with_capacity(n);
    let mut used = vec![false; candidates.len()];
    for &(p, _) in partners.iter().take(PARTNER_SAMPLES.min(n / 3)) {
        let best = candidates
            .iter()
            .enumerate()
            .filter(|(i, c)| !used[*i] && c.partner.as_deref() == Some(p))
            .max_by(|a, b| a.1.chars.cmp(&b.1.chars).then(b.0.cmp(&a.0)));
        if let Some((i, _)) = best {
            used[i] = true;
            chosen.push(i);
        }
    }
    // 每篇的候选按（字数降序，行序升序）排好，轮流取。
    let mut per_doc: Vec<Vec<usize>> = Vec::new();
    let mut last_doc = usize::MAX;
    for (i, c) in candidates.iter().enumerate() {
        if c.doc != last_doc {
            per_doc.push(Vec::new());
            last_doc = c.doc;
        }
        if let Some(list) = per_doc.last_mut() {
            list.push(i);
        }
    }
    for list in &mut per_doc {
        list.sort_by(|&a, &b| {
            candidates[b]
                .chars
                .cmp(&candidates[a].chars)
                .then(a.cmp(&b))
        });
        list.retain(|&i| !used[i]);
        list.reverse(); // pop() 取最优
    }
    while chosen.len() < n {
        let open: Vec<usize> = (0..per_doc.len())
            .filter(|&d| !per_doc[d].is_empty())
            .collect();
        if open.is_empty() {
            break;
        }
        let remaining = n - chosen.len();
        let picks: Vec<usize> = if remaining < open.len() {
            spread(open.len(), remaining)
                .into_iter()
                .map(|k| open[k])
                .collect()
        } else {
            open
        };
        for d in picks {
            if let Some(i) = per_doc[d].pop() {
                chosen.push(i);
            }
        }
    }
    chosen.sort_by_key(|&i| candidates[i].line);
    chosen
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut s: String = text.chars().take(max).collect();
        s.push('…');
        s
    }
}

fn render(corpus: &Corpus, r: &PersonaResult, globals: &[(usize, Option<usize>)]) -> String {
    let mut out = String::with_capacity(8192);
    let _ = write!(out, "[ik_character] {} | 扮演资料", r.name);
    match &r.profile {
        Some(p) => {
            let all: usize = p
                .sections
                .iter()
                .filter(|s| s.kind == "dialogue")
                .map(|s| s.scene_count)
                .sum();
            let _ = write!(
                out,
                " · 档案 {} · 档案对话 {} 段（共 {} 幕）",
                p.doc_id,
                r.dialogue_scenes.len(),
                all
            );
        }
        None => out.push_str(" · 无代理人档案"),
    }
    let _ = write!(
        out,
        " · 主线署名台词 {} 句 · 样本 {}/{}",
        thousands(r.story_spoken_lines),
        r.story_samples.len(),
        thousands(r.story_candidate_lines)
    );
    if let Some(u) = &r.until {
        let _ = write!(out, " · 截止 {} {}（含）", u.group_id, u.title);
    }
    out.push('\n');
    if r.resolved_names.is_empty() {
        out.push_str("不是已知说话人：没有主线署名台词。\n");
    } else {
        let _ = writeln!(out, "匹配说话人：{}", r.resolved_names.join("、"));
    }
    if !r.related_names.is_empty() {
        let _ = writeln!(out, "相关名称（未合并）：{}", r.related_names.join("、"));
    }
    if r.profile.is_some() {
        out.push_str(
            "档案对话为原文片段：【名字】是原文标注的说话人，未标注的行可能是玩家选项、旁白或角色本人，不逐句署名；行尾来源键已省略，行号可直接用于 ik_read。\n",
        );
    }
    out.push_str(
        "主线样本只取逐句署名已解析的对白（不含推断署名、选项、旁白），每篇取较长的句子，按章节分散；性格、语气等描述不是语料字段。\n",
    );
    if r.until.is_some() {
        out.push_str(
            "截止按章节目录顺序，只作用于主线台词与往来；档案不随截止过滤，可能含之后的信息。\n",
        );
    }

    if let Some(p) = &r.profile {
        let _ = writeln!(out, "\n## 档案：{} 「{}」", p.doc_id, p.title);
        let mut others = Vec::new();
        for s in &p.sections {
            match (&s.text, s.kind.as_str()) {
                (Some(text), _) => {
                    let _ = writeln!(out, "### {}", s.heading);
                    out.push_str(text);
                    out.push('\n');
                    if s.truncated {
                        let _ = writeln!(out, "（本节未完，续读：{}）", s.read.text());
                    }
                }
                (None, "other") => others.push(format!("{} → {}", s.heading, s.read.text())),
                _ => {}
            }
        }
        if !others.is_empty() {
            let _ = writeln!(out, "其他小节：{}", others.join("；"));
        }
        if !r.other_profiles.is_empty() {
            let _ = writeln!(
                out,
                "另有标题含「{}」的代理人档案：{}",
                r.name,
                r.other_profiles.join("、")
            );
        }

        out.push_str("\n## 档案对话\n");
        let mut listed: Vec<&str> = Vec::new();
        for s in &r.dialogue_scenes {
            let _ = writeln!(out, "### {} · {}", s.section, s.heading);
            for l in &s.lines {
                let _ = writeln!(out, "L{} {}", l.number, display_line(&l.text));
            }
            if s.truncated {
                let _ = writeln!(
                    out,
                    "（本段 {} 行，已列 {} 行，续读：{}）",
                    s.line_count,
                    s.lines.len(),
                    s.read.text()
                );
            }
            if !listed.contains(&s.section.as_str()) {
                listed.push(&s.section);
            }
        }
        let all: Vec<String> = p
            .sections
            .iter()
            .filter(|s| s.kind == "dialogue")
            .map(|s| {
                if s.heading.contains('共') {
                    format!("{} → {}", s.heading, s.read.text())
                } else {
                    format!("{}（{} 幕）→ {}", s.heading, s.scene_count, s.read.text())
                }
            })
            .collect();
        if r.dialogue_scenes.is_empty() {
            out.push_str("档案中没有对话小节。\n");
        } else if !all.is_empty() {
            let _ = writeln!(out, "全部档案对话：{}", all.join("；"));
        }
    } else if !r.other_profiles.is_empty() {
        let _ = writeln!(
            out,
            "\n代理人档案标题含「{}」但不唯一：{}",
            r.name,
            r.other_profiles.join("、")
        );
    }

    if !r.partners.is_empty() {
        let list: Vec<String> = r
            .partners
            .iter()
            .map(|p| format!("{} {}·{}", p.name, p.exchanges, p.scenes))
            .collect();
        let _ = writeln!(
            out,
            "\n## 主线常对话对象（相邻对白往来次数·场景数）\n{}",
            list.join("、")
        );
    }

    if !r.resolved_names.is_empty() {
        out.push_str("\n## 主线台词样本\n");
        if r.story_samples.is_empty() {
            out.push_str("范围内没有符合条件的署名台词。\n");
        }
        let mut last_doc = "";
        let mut last_line = 0usize;
        for (s, &(line, context)) in r.story_samples.iter().zip(globals) {
            if s.doc_id != last_doc {
                let _ = writeln!(
                    out,
                    "### {} {} · {}「{}」",
                    s.group_id, s.group_title, s.doc_id, s.title
                );
                last_doc = &s.doc_id;
                last_line = 0;
            }
            if let (Some(c), Some(p)) = (&s.context, context)
                && c.number != last_line
            {
                let _ = write!(out, "L{} ", c.number);
                push_line(&mut out, corpus, p, &truncate(&c.text, CONTEXT_CHARS));
                out.push('\n');
            }
            let _ = write!(out, "L{} ", s.line.number);
            push_line(&mut out, corpus, line, &s.line.text);
            out.push('\n');
            last_line = s.line.number;
        }
    }

    out.push_str("\n相关入口：");
    if !r.story_samples.is_empty() {
        out.push_str("storySamples[].read 为样本所在原文；");
    }
    for call in &r.next_calls {
        let _ = write!(out, "{} 为全部主线署名台词；", call.text());
    }
    out.push_str("ik_character(name) 为出场与章节统计。\n");
    out
}
