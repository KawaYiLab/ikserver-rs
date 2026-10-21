//! Bounded, deterministic recovery hints for group selectors; no automatic scope changes.
use std::fmt::Write as _;

use serde_json::Value;

use super::{ToolError, releases, response::NextCall};
use crate::corpus::Corpus;
use crate::text::fold;

const MAX_SUGGESTIONS: usize = 5;
const MAX_QUERY_CHARS: usize = 128;
const MAX_DISTANCE: usize = 3;
const MAX_RETRY_BYTES: usize = 16_384;

/// Successful results and cursor errors never reach this function. The original
/// error remains the first line; suggestions are calls, not substituted results.
pub(crate) fn append(c: &Corpus, tool: &str, args: &Value, mut error: ToolError) -> ToolError {
    if !matches!(tool, "ik_catalog" | "ik_search" | "ik_read") {
        return error;
    }
    if error.0.starts_with("版本「") && error.0.contains("没有已确认的主线章节映射")
    {
        reviewed_versions(c, &mut error.0);
        return error;
    }
    if !error.0.starts_with("找不到分组「") {
        return error;
    }
    let key = if tool == "ik_read" { "docId" } else { "path" };
    let Some(query) = args.get(key).and_then(Value::as_str).map(str::trim) else {
        return error;
    };
    if query.is_empty() || c.find_group(query).is_some() || releases::lookup(query).is_err() {
        return error;
    }
    let q = fold(query);
    let bare = matches!(q.as_str(), "ch:" | "lore:");
    if bare {
        let _ = writeln!(error.0, "\n缺少章节或分类名称；请使用下面的完整分组ID。");
    }
    let mut candidates = candidates(c, &q, bare);
    // Existing explicit version/doc filters remain hard constraints even in
    // recovery hints, so a suggested retry cannot silently widen the range.
    if tool == "ik_search" {
        if let Some(keyword) = args.get("keyword").and_then(Value::as_str)
            && let Ok((_, Some(release))) = releases::query(keyword)
        {
            candidates.retain(|&(_, d)| c.groups[d].id == release.group);
        }
        if let Some(doc) = args
            .get("docId")
            .and_then(Value::as_str)
            .and_then(|id| c.find_doc(id))
        {
            candidates.retain(|&(_, d)| d == c.docs[doc].group as usize);
        }
    }
    candidates.sort_by(|(sa, a), (sb, b)| {
        sa.cmp(sb)
            .then_with(|| c.groups[*a].id.cmp(&c.groups[*b].id))
    });
    let count = candidates.len();
    let _ = writeln!(
        error.0,
        "\n候选 {count} 个，显示 {} 个；请明确选择，不会自动改换范围。",
        count.min(MAX_SUGGESTIONS)
    );
    let mut omitted_retries = false;
    for (_, index) in candidates.into_iter().take(MAX_SUGGESTIONS) {
        let g = &c.groups[index];
        let _ = writeln!(
            error.0,
            "{} 「{}」 · {} 篇",
            g.id,
            preview(&g.label),
            g.docs.len()
        );
        if omitted_retries {
            continue;
        }
        let mut retry = args.clone();
        if let Some(map) = retry.as_object_mut() {
            map.remove("cursor");
            map.insert(key.into(), Value::String(g.id.clone()));
        }
        let call = NextCall::new(tool, retry).text();
        if call.len() > MAX_RETRY_BYTES {
            omitted_retries = true;
            continue;
        }
        let _ = writeln!(error.0, "建议重试：{call}");
    }
    if omitted_retries {
        error.0.push_str(
            "参数过长，完整重试调用超过16KiB；请缩短查询并保留筛选条件后重新选择分组。\n",
        );
    }
    error.0.push_str("查看全部分组：ik_catalog()");
    error
}

fn candidates(c: &Corpus, query: &str, bare: bool) -> Vec<(usize, usize)> {
    let prefix = if query.starts_with("ch:") {
        Some("ch:")
    } else if query.starts_with("lore:") {
        Some("lore:")
    } else {
        None
    };
    let groups = || {
        c.groups
            .iter()
            .enumerate()
            .filter(|(_, g)| prefix.is_none_or(|p| g.id.starts_with(p)))
    };
    if bare {
        return groups().map(|(i, _)| (0, i)).collect();
    }
    // Preserve the resolver's preference for exact labels over partial labels.
    let exact = groups()
        .filter(|(_, g)| fold(&g.label) == query)
        .map(|(i, _)| (0, i))
        .collect::<Vec<_>>();
    if !exact.is_empty() {
        return exact;
    }
    let partial = groups()
        .filter(|(_, g)| fold(&g.label).contains(query))
        .map(|(i, _)| (1, i))
        .collect::<Vec<_>>();
    if !partial.is_empty() {
        return partial;
    }
    if query.chars().count() > MAX_QUERY_CHARS {
        return Vec::new();
    }
    groups()
        .filter_map(|(i, g)| {
            let id = fold(&g.id);
            let label = fold(&g.label);
            let tail = id.split_once(':').map(|(_, t)| t).unwrap_or(&id);
            [id.as_str(), tail, label.as_str()]
                .into_iter()
                .filter_map(|value| distance(query, value, MAX_DISTANCE))
                .min()
                .map(|score| (score + 2, i))
        })
        .collect()
}

/// Input and candidate lengths are bounded; discard rows whose best possible
/// distance already exceeds the threshold rather than scan arbitrarily long IDs.
fn distance(a: &str, b: &str, max: usize) -> Option<usize> {
    let a = a.chars().collect::<Vec<_>>();
    let b = b.chars().take(MAX_QUERY_CHARS + 1).collect::<Vec<_>>();
    if b.len() > MAX_QUERY_CHARS || a.len().abs_diff(b.len()) > max {
        return None;
    }
    let mut previous = (0..=b.len()).collect::<Vec<_>>();
    let mut row = vec![0; b.len() + 1];
    for (i, ac) in a.into_iter().enumerate() {
        row[0] = i + 1;
        for (j, bc) in b.iter().enumerate() {
            row[j + 1] = (previous[j + 1] + 1)
                .min(row[j] + 1)
                .min(previous[j] + usize::from(ac != *bc));
        }
        if row.iter().all(|&n| n > max) {
            return None;
        }
        std::mem::swap(&mut row, &mut previous);
    }
    let result = previous[b.len()];
    (result <= max).then_some(result)
}

fn reviewed_versions(c: &Corpus, out: &mut String) {
    out.push_str("\n已核验版本（仅供重新选择，不猜测原版本对应章节）：\n");
    for (version, group) in releases::revision_data() {
        let available = if c.find_group(group).is_some() {
            "已收录"
        } else {
            "当前未收录"
        };
        let _ = writeln!(
            out,
            "{version} → {group}（{available}）；目录入口：{}",
            NextCall::new("ik_catalog", serde_json::json!({"path":group})).text()
        );
    }
}

fn preview(value: &str) -> String {
    let mut result = value.chars().take(64).collect::<String>();
    if value.chars().count() > 64 {
        result.push('…');
    }
    result
}
