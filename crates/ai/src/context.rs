//! 上下文的 token 预算（LLM-018）：按模型的上下文窗口裁剪历史与工具结果，免得长帖、剧情库原文把请求撑爆。
//!
//! - 预算 = 上下文窗口 − 给回复留的量（模型的输出上限，没填按 8192）− 工具清单本身；
//! - 准备阶段之后、第一次请求之前：历史超出「预算 − 工具回灌预留」时从最早的消息删起，保留 system 与触发消息；
//! - 工具循环里每轮请求之前：还超就把较早轮次里的大段工具结果换成一句省略说明，最新一轮的结果不动；
//!   再超才继续删最早的历史。
//!
//! 只有超了才动，平常请求原样发出，不破坏上游的前缀缓存（LLM-017）。token 按 [`quota::estimate_text_tokens`]
//! 估算（宁可估高）。

use crate::catalog::Model;
use crate::llm::{ChatMessage, ToolSpec};
use crate::quota;

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
    let latest = messages
        .iter()
        .rposition(
            |m| matches!(m, ChatMessage::Assistant { tool_calls, .. } if !tool_calls.is_empty()),
        )
        .unwrap_or(0);
    let mut pruned = 0;
    for message in &mut messages[..latest] {
        if total <= budget {
            break;
        }
        if let ChatMessage::Tool { content, .. } = message
            && content.chars().count() > PRUNE_MIN_CHARS
        {
            total -=
                quota::estimate_text_tokens(content) - quota::estimate_text_tokens(PRUNED_NOTE);
            PRUNED_NOTE.clone_into(content);
            pruned += 1;
        }
    }
    pruned
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
}
