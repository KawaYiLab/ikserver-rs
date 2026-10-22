//! ik_character(purpose="roleplay")：角色扮演参考资料。
//!
//! 只给原文素材：代理人设定档案摘录、常对话对象、按章节分散抽取的逐句署名台词（附前一句）。
//! 性格、语气等归纳不是语料字段，不在这里生成。抽样是确定性的，同一快照同一参数结果相同。

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use serde_json::json;

use super::response::*;
use super::search::match_speakers;
use super::{clamp, err, push_line};
use crate::corpus::{Corpus, DocKind, GroupKind, LineKind};
use crate::text::{fold, thousands};

/// 台词样本的字数范围：太短的（「嗯。」「……」）信息量低，太长的多为大段说明。
const MIN_CHARS: usize = 6;
const MAX_CHARS: usize = 120;
/// 前一句在文本里的截断长度；structuredContent 保留全文。
const CONTEXT_CHARS: usize = 60;
/// 每篇设定档案摘录的字数上限（按整行截断）。
const PROFILE_CHARS: usize = 1500;
const MAX_PROFILES: usize = 3;
const MAX_PARTNERS: usize = 8;
/// 先保证这么多位常对话对象各有一条往来样本，其余名额按章节分散。
const PARTNER_SAMPLES: usize = 5;

struct Candidate {
    line: usize,
    doc: usize,
    chars: usize,
    partner: Option<String>,
}

pub(crate) fn execute(
    corpus: &Corpus,
    name: &str,
    until: Option<&str>,
    max_samples: Option<usize>,
    include_evidence: Option<bool>,
) -> OutputResult {
    if include_evidence == Some(true) {
        return err(format!(
            "purpose=roleplay 不展开逐行证据；逐句署名证据见 ik_search(speaker=\"{name}\", includeEvidence=true)"
        ));
    }
    let n = clamp(max_samples, 24, 60);
    let folded = fold(name);
    let ids = match_speakers(corpus, name);
    let exact = ids.iter().any(|&i| {
        corpus.speakers_folded[i as usize] == folded
            || corpus.speaker_identities[i as usize] == folded
    });
    // 包含匹配可能把几个不同角色混在一起；扮演资料必须是同一个人。
    let identities: Vec<&str> = {
        let mut seen = Vec::new();
        for &i in &ids {
            let identity = corpus.speaker_identity_names[i as usize].as_str();
            if !seen.contains(&identity) {
                seen.push(identity);
            }
        }
        seen
    };
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

    let cutoff = match until.map(str::trim).filter(|s| !s.is_empty()) {
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

    // 设定档案：代理人分类里标题含名字的条目。设定不随 until 过滤。
    let titled: Vec<usize> = corpus
        .doc_keys
        .matching(&folded, true)
        .into_iter()
        .map(|d| d as usize)
        .filter(|&d| corpus.docs[d].kind == DocKind::Lore)
        .collect();
    let is_agent = |d: usize| corpus.groups[corpus.docs[d].group as usize].id == "lore:agents";
    let profile_docs: Vec<usize> = titled.iter().copied().filter(|&d| is_agent(d)).collect();
    let other_titled = titled.len() - profile_docs.len();

    if ids.is_empty() && titled.is_empty() {
        return err(format!(
            "语料中没有找到说话人或设定档案「{name}」。试试全名、简称或别的写法（如「零号·安比」「11号」）"
        ));
    }

    // 台词与往来。
    let mut spoken = 0usize;
    let mut scenes = 0usize;
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut seen_text: HashSet<String> = HashSet::new();
    let mut exchanges: HashMap<&str, usize> = HashMap::new();
    let mut partner_scenes: HashMap<&str, usize> = HashMap::new();
    let partner_of = |l: usize| -> Option<&str> {
        let sp = corpus.lines[l].speaker;
        if ids.contains(&sp) || related.contains(&sp) || !corpus.is_attributed(l) {
            return None;
        }
        let identity = corpus.speaker_identity_names[sp as usize].as_str();
        (!super::character::is_placeholder(identity)).then_some(identity)
    };
    for (d, doc) in corpus.docs.iter().enumerate() {
        if doc.kind == DocKind::Lore {
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
            let target = ids.contains(&corpus.lines[l].speaker) && corpus.is_attributed(l);
            // 相邻两句署名对白，一边是该角色，一边是别人：算一次往来。
            if l > range.start {
                let prev_target =
                    ids.contains(&corpus.lines[l - 1].speaker) && corpus.is_attributed(l - 1);
                let other = match (prev_target, target) {
                    (true, false) => partner_of(l),
                    (false, true) => partner_of(l - 1),
                    _ => None,
                };
                if let Some(p) = other {
                    *exchanges.entry(p).or_default() += 1;
                    partners_here.insert(p);
                }
            }
            if !target {
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
        spoken_lines: spoken,
        scenes,
        candidate_lines: candidates.len(),
        profiles: profile_docs
            .iter()
            .take(MAX_PROFILES)
            .map(|&d| profile(corpus, d))
            .collect(),
        other_titled_lore: other_titled,
        partners: partners
            .iter()
            .map(|&(p, x)| PersonaPartner {
                name: p.into(),
                exchanges: x,
                scenes: partner_scenes.get(p).copied().unwrap_or(0),
            })
            .collect(),
        samples: chosen
            .iter()
            .map(|&i| {
                let c = &candidates[i];
                let doc = &corpus.docs[c.doc];
                let first = doc.first_line as usize;
                let context = (c.line > first)
                    .then_some(c.line - 1)
                    .filter(|&p| corpus.lines[p].kind != LineKind::Placeholder)
                    .map(|p| {
                        let mut line = SourceLine::new(corpus, p, false);
                        line.is_context = true;
                        line
                    });
                let number = c.line - first + 1;
                let group = &corpus.groups[doc.group as usize];
                PersonaSample {
                    doc_id: corpus.doc_id(c.doc),
                    title: doc.title.clone(),
                    group_id: group.id.clone(),
                    group_title: group.label.clone(),
                    line: SourceLine::new(corpus, c.line, false),
                    context,
                    read: NextCall::new(
                        "ik_read",
                        json!({"docId":corpus.doc_id(c.doc),"offset":number.saturating_sub(3).max(1),"limit":8}),
                    ),
                }
            })
            .collect(),
        next_calls: if ids.is_empty() {
            Vec::new()
        } else {
            vec![NextCall::new("ik_search", json!({"speaker":name}))]
        },
    };
    // 文本渲染用全局行号（样本本句, 前一句）。
    let globals: Vec<(usize, Option<usize>)> = chosen
        .iter()
        .zip(&result.samples)
        .map(|(&i, s)| {
            let l = candidates[i].line;
            (l, s.context.as_ref().map(|_| l - 1))
        })
        .collect();
    let text = render(corpus, &result, &globals);
    Ok(ToolOutput::new(corpus, text, ToolData::Persona(result)))
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
            (0..remaining)
                .map(|k| open[k * open.len() / remaining])
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

fn profile(corpus: &Corpus, d: usize) -> PersonaProfile {
    let doc = &corpus.docs[d];
    let mut excerpt = String::new();
    let mut taken = 0usize;
    for l in doc.lines() {
        let text = corpus.line_text(l);
        if !excerpt.is_empty() && excerpt.chars().count() + text.chars().count() > PROFILE_CHARS {
            break;
        }
        if !excerpt.is_empty() {
            excerpt.push('\n');
        }
        excerpt.push_str(text);
        taken += 1;
    }
    let total = doc.line_count as usize;
    PersonaProfile {
        doc_id: corpus.doc_id(d),
        title: doc.title.clone(),
        excerpt,
        excerpt_lines: taken,
        line_count: total,
        truncated: taken < total,
        next_read: (taken < total).then(|| {
            NextCall::new(
                "ik_read",
                json!({"docId":corpus.doc_id(d),"offset":taken + 1}),
            )
        }),
    }
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
    let mut out = String::with_capacity(4096);
    let _ = write!(
        out,
        "[ik_character] {} | 扮演资料 · 署名台词 {} 句 · {} 场景 · 样本 {}/{}",
        r.name,
        thousands(r.spoken_lines),
        thousands(r.scenes),
        r.samples.len(),
        thousands(r.candidate_lines)
    );
    if let Some(u) = &r.until {
        let _ = write!(out, " · 截止 {} {}（含）", u.group_id, u.title);
    }
    out.push('\n');
    if r.resolved_names.is_empty() {
        out.push_str("不是已知说话人：没有署名台词，只有设定档案。\n");
    } else {
        let _ = writeln!(out, "匹配说话人：{}", r.resolved_names.join("、"));
    }
    if !r.related_names.is_empty() {
        let _ = writeln!(out, "相关名称（未合并）：{}", r.related_names.join("、"));
    }
    out.push_str(
        "台词样本只取逐句署名已解析的对白（不含推断署名、选项、旁白），每篇取较长的句子，按章节分散；性格、语气等描述不是语料字段。\n",
    );
    if r.until.is_some() {
        out.push_str("截止按章节目录顺序，含该章的主线与补充内容；设定档案不随截止过滤，可能含之后的信息。\n");
    }

    out.push_str("\n## 设定档案\n");
    if r.profiles.is_empty() {
        out.push_str("代理人分类中没有标题含此名字的档案。\n");
    }
    for p in &r.profiles {
        let _ = writeln!(out, "{} 「{}」", p.doc_id, p.title);
        out.push_str(&p.excerpt);
        out.push('\n');
        if let Some(next) = &p.next_read {
            let _ = writeln!(
                out,
                "（摘录 {}/{} 行，续读：{}）",
                p.excerpt_lines,
                p.line_count,
                next.text()
            );
        }
    }
    if r.other_titled_lore > 0 {
        let _ = writeln!(
            out,
            "另有 {} 篇设定标题含「{}」（多为委托/物品）：ik_catalog(keyword=\"{}\")",
            r.other_titled_lore, r.name, r.name
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
            "\n## 常对话对象（相邻对白往来次数·场景数）\n{}",
            list.join("、")
        );
    }

    if !r.resolved_names.is_empty() {
        out.push_str("\n## 台词样本\n");
        if r.samples.is_empty() {
            out.push_str("范围内没有符合条件的署名台词。\n");
        }
        let mut last_doc = "";
        let mut last_line = 0usize;
        for (s, &(line, context)) in r.samples.iter().zip(globals) {
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
    if !r.samples.is_empty() {
        out.push_str("samples[].read 为样本所在原文；");
    }
    for call in &r.next_calls {
        let _ = write!(out, "{} 为全部署名台词；", call.text());
    }
    out.push_str("ik_character(name) 为出场与章节统计。\n");
    out
}
