//! 上下文的 token 预算（LLM-018）：按模型的上下文窗口裁剪历史与工具结果，免得长帖、剧情库原文把请求撑爆。
//!
//! - 预算 = 上下文窗口 − 给回复留的量（模型的输出上限，没填按 8192）− 工具清单本身；
//! - 准备阶段之后、第一次请求之前：历史超出「预算 − 工具回灌预留」时从最早的消息删起，保留 system 与触发消息；
//! - 工具结果回灌之前（TOOL-002）：工具结果不按固定字数截断，放得下就原样回灌；放不下先占用剩余空间，
//!   不够时最多占到预算的一半（省略较早轮次的工具结果、删最早的历史腾地方），仍放不下才把本轮最长的几条截短，
//!   并注明后面还有多少（TOOL-015）；
//! - 工具循环里每轮请求之前：还超就把较早轮次里的大段工具结果换成一句省略说明，最新一轮的结果不动；
//!   再超才继续删最早的历史。
//!
//! 只有超了才动，平常请求原样发出，不破坏上游的前缀缓存（LLM-017）。token 按 [`quota::estimate_text_tokens`]
//! 估算（宁可估高）。

use crate::catalog::Model;
use crate::llm::{ChatMessage, ToolSpec};
use crate::quota;
use crate::tools;

/// 换掉的工具结果。
pub(crate) const PRUNED_NOTE: &str =
    "（这条工具结果较早，为节省上下文已省略；需要细节时请重新调用工具）";
/// 比这短的工具结果不值得换。
const PRUNE_MIN_CHARS: usize = 200;
/// 模型没填输出上限时给回复（含思考）留的 token。
const DEFAULT_OUTPUT_RESERVE: i64 = 8192;
/// 有工具时，给工具回灌预留的比例上限与绝对上限。
const TOOL_RESERVE_MAX: i64 = 16_384;
/// 每条消息的格式开销（角色、分隔符）。
const PER_MESSAGE: i64 = 8;
/// 预算的下限：窗口填得离谱时也留一点余地。
const MIN_BUDGET: i64 = 2048;
/// 截短时每条工具结果至少留这么多字：上下文再紧也让模型看到开头。
const RESULT_FLOOR_CHARS: usize = 500;

/// 一次请求里消息部分能用的 token。
pub(crate) fn budget(model: &Model, tools: &[ToolSpec]) -> i64 {
    let output = model
        .params
        .max_output_tokens
        .map_or(DEFAULT_OUTPUT_RESERVE, i64::from);
    let specs: i64 = tools
        .iter()
        .map(|t| {
            quota::estimate_text_tokens(&t.description)
                + i64::try_from(t.parameters.to_string().len() / 3).unwrap_or(i64::MAX)
                + PER_MESSAGE
        })
        .sum();
    (model.context_tokens() - output - specs).max(MIN_BUDGET)
}

/// 准备阶段给历史的目标：有工具时为工具回灌留出一部分。
pub(crate) fn history_target(budget: i64, with_tools: bool) -> i64 {
    if with_tools {
        budget - (budget / 4).min(TOOL_RESERVE_MAX)
    } else {
        budget
    }
}

fn message_tokens(message: &ChatMessage) -> i64 {
    let mut tokens = PER_MESSAGE
        + quota::estimate_text_tokens(message.text())
        + i64::try_from(message.image_count()).unwrap_or(0) * quota::IMAGE_TOKENS;
    if let ChatMessage::Assistant {
        reasoning_content,
        tool_calls,
        ..
    } = message
    {
        tokens += reasoning_content
            .as_deref()
            .map_or(0, quota::estimate_text_tokens);
        for call in tool_calls {
            tokens += quota::estimate_text_tokens(&call.arguments) + PER_MESSAGE;
        }
    }
    tokens
}

pub(crate) fn estimate(messages: &[ChatMessage]) -> i64 {
    messages.iter().map(message_tokens).sum()
}

/// 从最早的历史删起，直到不超过 `budget`。`keep_from` 起（含）的消息与 system 不删，并且至少留下
/// `keep_from` 前的最后一条（触发消息）。删掉带工具调用的回复时连同它的工具结果一起删。返回删了几条。
pub(crate) fn trim_history(
    messages: &mut Vec<ChatMessage>,
    budget: i64,
    keep_from: usize,
) -> usize {
    let start = usize::from(matches!(messages.first(), Some(ChatMessage::System { .. })));
    let mut total = estimate(messages);
    let mut keep_from = keep_from.min(messages.len());
    let mut removed = 0;
    while total > budget && keep_from > start + 1 {
        total -= message_tokens(&messages[start]);
        messages.remove(start);
        keep_from -= 1;
        removed += 1;
        while keep_from > start + 1 && matches!(messages.get(start), Some(ChatMessage::Tool { .. }))
        {
            total -= message_tokens(&messages[start]);
            messages.remove(start);
            keep_from -= 1;
            removed += 1;
        }
    }
    removed
}

/// 把较早轮次里的大段工具结果换成 [`PRUNED_NOTE`]，直到不超过 `budget`。最后一条带工具调用的回复之后的
/// 结果（模型马上要用的）不动。返回换了几条。
pub(crate) fn prune_tool_results(messages: &mut [ChatMessage], budget: i64) -> usize {
    let mut total = estimate(messages);
    if total <= budget {
        return 0;
    }
    let latest = latest_tool_call(messages);
    let mut pruned = 0;
    for message in &mut messages[..latest] {
        if total <= budget {
            break;
        }
        if let Some(saved) = prune_saving(message)
            && let ChatMessage::Tool { content, .. } = message
        {
            total -= saved;
            PRUNED_NOTE.clone_into(content);
            pruned += 1;
        }
    }
    pruned
}

/// 最后一条带工具调用的回复的位置（它之后的工具结果是模型马上要用的）。
fn latest_tool_call(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .rposition(
            |m| matches!(m, ChatMessage::Assistant { tool_calls, .. } if !tool_calls.is_empty()),
        )
        .unwrap_or(0)
}

/// 这条消息若是值得省略的工具结果，省略后能省下的 token。
fn prune_saving(message: &ChatMessage) -> Option<i64> {
    match message {
        ChatMessage::Tool { content, .. } if content.chars().count() > PRUNE_MIN_CHARS => {
            Some(quota::estimate_text_tokens(content) - quota::estimate_text_tokens(PRUNED_NOTE))
        }
        _ => None,
    }
}

/// [`fit_results`] 做了什么。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fitted {
    /// 换成省略说明的较早工具结果。
    pub pruned: usize,
    /// 删掉的最早历史。
    pub removed: usize,
    /// 截短的本轮结果。
    pub cut: usize,
}

/// 把本轮的工具结果放进上下文（TOOL-002、TOOL-015）。本轮结果最多占「剩余空间（较早轮次的工具结果可以省略）」
/// 与「预算的一半」中较大的那个：超出时先把最长的几条截短（短的原样保留，长的平分剩下的）；再为它们腾地方，
/// 先省略较早轮次的工具结果，再删最早的历史（`keep_from` 起的消息不删）。平常放得下时什么都不动。
pub(crate) fn fit_results(
    messages: &mut Vec<ChatMessage>,
    results: &mut [String],
    budget: i64,
    keep_from: usize,
) -> Fitted {
    let mut fitted = Fitted::default();
    let overhead = PER_MESSAGE * i64::try_from(results.len()).unwrap_or(i64::MAX);
    let prunable: i64 = messages[..latest_tool_call(messages)]
        .iter()
        .filter_map(prune_saving)
        .sum();
    let room = (budget - estimate(messages) + prunable).max(budget / 2) - overhead;
    let sizes: Vec<i64> = results
        .iter()
        .map(|r| quota::estimate_text_tokens(r))
        .collect();
    if sizes.iter().sum::<i64>() > room {
        for ((result, size), cap) in results.iter_mut().zip(&sizes).zip(share(&sizes, room)) {
            if *size > cap {
                let total = result.chars().count();
                // token 与字数大致成正比：按比例算出能留的字数。
                let keep = usize::try_from(
                    i128::from(cap.max(0)) * i128::try_from(total).unwrap_or(i128::MAX)
                        / i128::from(*size),
                )
                .unwrap_or(total)
                .max(RESULT_FLOOR_CHARS);
                if keep < total {
                    *result = tools::truncate_marked(result, keep);
                    fitted.cut += 1;
                }
            }
        }
    }
    let need: i64 = results
        .iter()
        .map(|r| quota::estimate_text_tokens(r))
        .sum::<i64>()
        + overhead;
    if estimate(messages) + need > budget {
        fitted.pruned = prune_tool_results(messages, budget - need);
        fitted.removed = trim_history(messages, budget - need, keep_from);
    }
    fitted
}

/// 把 `total` 分给各条：比平均份额小的拿走自己要的，剩下的由长的平分。
fn share(sizes: &[i64], total: i64) -> Vec<i64> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| sizes[i]);
    let mut caps = vec![0; sizes.len()];
    let mut left = total.max(0);
    for (taken, &i) in order.iter().enumerate() {
        let fair = left / i64::try_from(sizes.len() - taken).unwrap_or(1);
        caps[i] = sizes[i].min(fair);
        left -= caps[i];
    }
    caps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCall;

    fn round(id: &str, result: &str) -> [ChatMessage; 2] {
        [
            ChatMessage::Assistant {
                content: String::new(),
                reasoning_content: None,
                tool_calls: vec![ToolCall {
                    id: id.into(),
                    name: "get_post_content".into(),
                    arguments: "{}".into(),
                }],
            },
            ChatMessage::Tool {
                tool_call_id: id.into(),
                content: result.into(),
            },
        ]
    }

    #[test]
    fn nothing_changes_within_budget() {
        let mut messages = vec![ChatMessage::system("s"), ChatMessage::user("你好")];
        assert_eq!(trim_history(&mut messages, 10_000, 2), 0);
        assert_eq!(prune_tool_results(&mut messages, 10_000), 0);
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn history_is_trimmed_from_the_oldest_and_keeps_the_trigger() {
        let long = "长".repeat(1000);
        let mut messages = vec![
            ChatMessage::system("s"),
            ChatMessage::user(long.clone()),
            ChatMessage::assistant(long.clone()),
            ChatMessage::user("最新的问题"),
        ];
        let removed = trim_history(&mut messages, 1200, 4);
        assert_eq!(removed, 1);
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[2].text(), "最新的问题");
        // 再小也不删触发消息。
        trim_history(&mut messages, 1, 3);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].text(), "最新的问题");
    }

    #[test]
    fn trimming_drops_tool_results_with_their_call() {
        let [call, result] = round("a", "结果");
        let mut messages = vec![
            ChatMessage::system("s"),
            call,
            result,
            ChatMessage::user("问"),
        ];
        trim_history(&mut messages, 1, 4);
        assert!(
            !messages
                .iter()
                .any(|m| matches!(m, ChatMessage::Tool { .. }))
        );
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn older_tool_results_are_pruned_first() {
        let big = "帖".repeat(3000);
        let [c1, r1] = round("a", &big);
        let [c2, r2] = round("b", &big);
        let mut messages = vec![
            ChatMessage::system("s"),
            ChatMessage::user("问"),
            c1,
            r1,
            c2,
            r2,
        ];
        let pruned = prune_tool_results(&mut messages, 4000);
        assert_eq!(pruned, 1);
        assert_eq!(messages[3].text(), PRUNED_NOTE);
        assert_eq!(messages[5].text(), big);
    }

    #[test]
    fn results_that_fit_are_left_alone() {
        let mut messages = vec![ChatMessage::system("s"), ChatMessage::user("问")];
        let mut results = vec!["结".repeat(3000)];
        let fitted = fit_results(&mut messages, &mut results, 10_000, 2);
        assert_eq!(fitted, Fitted::default());
        assert_eq!(results[0].chars().count(), 3000);
    }

    #[test]
    fn older_results_make_room_before_the_new_one_is_cut() {
        let big = "帖".repeat(3000);
        let [c1, r1] = round("a", &big);
        let [c2, _] = round("b", "");
        let mut messages = vec![
            ChatMessage::system("s"),
            ChatMessage::user("问"),
            c1,
            r1,
            c2,
        ];
        let mut results = vec!["新".repeat(4000)];
        let fitted = fit_results(&mut messages, &mut results, 6000, 2);
        assert_eq!(fitted.pruned, 1);
        assert_eq!(fitted.cut, 0);
        assert_eq!(messages[3].text(), PRUNED_NOTE);
        assert_eq!(results[0].chars().count(), 4000);
    }

    #[test]
    fn oversized_results_are_cut_longest_first_and_say_how_much_is_left() {
        let mut messages = vec![ChatMessage::system("s"), ChatMessage::user("问")];
        let short = "短".repeat(300);
        let mut results = vec!["长".repeat(20_000), short.clone()];
        let fitted = fit_results(&mut messages, &mut results, 6000, 2);
        assert_eq!(fitted.cut, 1);
        assert_eq!(results[1], short);
        assert!(results[0].contains("已截断"));
        let used = estimate(&messages)
            + results
                .iter()
                .map(|r| quota::estimate_text_tokens(r) + PER_MESSAGE)
                .sum::<i64>();
        assert!(used <= 6000 + 64, "{used}");
    }

    #[test]
    fn a_huge_result_does_not_wipe_out_the_history() {
        let line = "聊".repeat(500);
        let mut messages = vec![ChatMessage::system("s")];
        for _ in 0..8 {
            messages.push(ChatMessage::user(line.clone()));
            messages.push(ChatMessage::assistant(line.clone()));
        }
        messages.push(ChatMessage::user("最新的问题"));
        let keep = messages.len();
        let mut results = vec!["页".repeat(100_000)];
        let fitted = fit_results(&mut messages, &mut results, 8000, keep);
        assert_eq!(fitted.cut, 1);
        // 结果最多占一半预算，历史至少留下一半左右。
        assert!(estimate(&messages) >= 3000, "{}", estimate(&messages));
    }

    #[test]
    fn share_gives_short_ones_what_they_need() {
        assert_eq!(share(&[100, 5000, 50], 1000), vec![100, 850, 50]);
        assert_eq!(share(&[900, 900], 1000), vec![500, 500]);
        assert_eq!(share(&[10], -5), vec![0]);
    }
}
