//! 工作流时间线（ai-assistant.md 第 7 节）、多气泡分段（GEN-050/051）与帖子引用（WF-009）。
//!
//! 只记录**已脱敏**的事件（WF-003 的客户端可见形态）：思考文本、工具参数与结果从不进入事件，
//! 所以落库与实时推送用的是同一份数据。评论回复不产生时间线（D-089 方案 A），用 [`Workflow::disabled`]。

use std::collections::HashSet;
use std::sync::LazyLock;

use chrono::Utc;
use regex::Regex;
use serde_json::{Value, json};

use crate::catalog::OutputMode;
use crate::llm::Usage;
use crate::sanitize;
use crate::toolbox::ToolLabel;
use crate::tools::ToolMeta;

/// 单条消息最多保存的事件数（WF-005）。
pub const MAX_EVENTS: usize = 1000;
/// 思考累计增长这么多字符才记一次 `reasoning.delta`（WF-007）。
const REASONING_STEP_CHARS: usize = 16;
/// 每次尝试的 seq / stepId 起点间隔：同一占位重试时继续递增（WF-006）。
const ATTEMPT_SPAN: i64 = 1_000_000;
const ERROR_MAX_CHARS: usize = 80;
const TITLE_MAX_CHARS: usize = 80;
/// 时间线文案上限。
const TOOL_TITLE_MAX_CHARS: usize = 32;
const SEARCH_ITEMS: usize = 10;

struct Patterns {
    status: Regex,
    report: Regex,
    link_md: Regex,
    link_paren: Regex,
    link_bare: Regex,
}

static PATTERNS: LazyLock<Option<Patterns>> = LazyLock::new(|| {
    Some(Patterns {
        // GEN-050 的状态词表。
        status: Regex::new(
            r"^(?:检索|调用|读取|分析|确认|收到|压缩|整理|汇总|调取|查询|搜索|正在|处理|验证|完毕|完成|生成|准备|检测|连接|定位|精读|写入|提取|查证|已)",
        )
        .ok()?,
        report: Regex::new(r"^(?:---|#{1,6}\s|\*\*|> |[-*+]\s)").ok()?,
        link_md: Regex::new(r"\[[^\[\]\n]{1,80}\]\(/post/([A-Za-z0-9_-]+)\)").ok()?,
        link_paren: Regex::new(r"[^\s()\[\]]{1,40}\s?\(/post/([A-Za-z0-9_-]+)\)").ok()?,
        link_bare: Regex::new(r"/post/([A-Za-z0-9_-]+)").ok()?,
    })
});

fn is_terminal(c: char) -> bool {
    matches!(c, '。' | '！' | '？' | '.' | '!' | '?' | '…')
}

fn starts_with_status(text: &str) -> bool {
    PATTERNS
        .as_ref()
        .is_some_and(|p| p.status.is_match(text.trim_start()))
}

/// 一段是不是「状态短句」：以状态词开头、以句末标点结尾。
fn is_status_segment(segment: &str) -> bool {
    let trimmed = segment.trim();
    starts_with_status(trimmed) && trimmed.chars().next_back().is_some_and(is_terminal)
}

/// 通用工具事件的 data：`{tool, title?, source?, ms?}`（剧情库与站外工具带时间线文案与来源，ADR 0004）。
fn tool_data(name: &str, label: ToolLabel<'_>, ms: Option<u64>) -> Value {
    let mut data = json!({ "tool": name });
    if let Some(title) = label.title {
        data["title"] = json!(truncate(title, TOOL_TITLE_MAX_CHARS));
    }
    if let Some(source) = label.source {
        data["source"] = json!(source);
    }
    if let Some(ms) = ms {
        data["ms"] = json!(ms);
    }
    data
}

/// 把正文切成气泡（GEN-050）：前一段是状态短句，且后一句也以状态词开头，或后面是报告边界
/// （空行后接 `---`、标题、加粗、引用、列表）时切开；`---` 分割线本身被吞掉。返回未清洗的各段。
pub fn split_segments(text: &str) -> Vec<String> {
    let Some(p) = PATTERNS.as_ref() else {
        return vec![text.trim().to_owned()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();
    };
    let mut segments = Vec::new();
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((index, c)) = chars.next() {
        if index < start {
            continue;
        }
        // 句末：一串句末标点之后。
        if is_terminal(c) {
            let mut end = index + c.len_utf8();
            while let Some(&(next, n)) = chars.peek() {
                if is_terminal(n) {
                    end = next + n.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let segment = &text[start..end];
            let rest = &text[end..];
            if is_status_segment(segment) && starts_with_status(rest) && !rest.trim().is_empty() {
                segments.push(segment.trim().to_owned());
                start = end;
            }
            continue;
        }
        // 报告边界：空行之后接报告标记。
        if c == '\n' && text[index..].starts_with("\n\n") {
            let after = &text[index + 2..];
            let segment = &text[start..index];
            if is_status_segment(segment) && p.report.is_match(after) {
                segments.push(segment.trim().to_owned());
                start = index + 2;
                if after.starts_with("---") {
                    start += 3;
                    while text[start..].starts_with('\n') {
                        start += 1;
                    }
                }
            }
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        segments.push(tail.to_owned());
    }
    segments
}

/// 正文里引用的帖子 id（WF-009）：`[标题](/post/id)`、`标题 (/post/id)`、裸 `/post/id`，按出现顺序去重。
/// 不是数字的 id 忽略（本系统的帖子 id 是雪花数字）。
pub fn cited_post_ids(text: &str) -> Vec<i64> {
    let Some(p) = PATTERNS.as_ref() else {
        return Vec::new();
    };
    let mut found: Vec<(usize, i64)> = Vec::new();
    for re in [&p.link_md, &p.link_paren, &p.link_bare] {
        for caps in re.captures_iter(text) {
            if let Some(m) = caps.get(1)
                && let Ok(id) = m.as_str().parse::<i64>()
            {
                found.push((m.start(), id));
            }
        }
    }
    found.sort_by_key(|(at, _)| *at);
    let mut ids = Vec::new();
    for (_, id) in found {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn usage_json(usage: Usage) -> Value {
    json!({ "promptTokens": usage.prompt_tokens, "completionTokens": usage.completion_tokens })
}

/// 一次生成的时间线记录器。
pub struct Workflow {
    enabled: bool,
    mode: OutputMode,
    seq: i64,
    step: i64,
    events: Vec<Value>,
    /// 新产生、还没推送的事件。
    pending: Vec<Value>,
    cited: HashSet<i64>,
    reasoning: Option<(String, usize, usize)>,
    answer_step: Option<String>,
    /// 当前这一轮的正文（分段在一轮之内进行）。
    round_text: String,
    /// 本轮已经发出的段数。
    round_parts: usize,
    part_index: i64,
}

impl Workflow {
    /// `attempt` 从 1 开始；每次尝试的 seq 与 stepId 从不同的起点开始，重试后也单调递增（WF-006）。
    pub fn new(attempt: i16, mode: OutputMode) -> Self {
        let base = i64::from(attempt.max(1) - 1) * ATTEMPT_SPAN;
        Self {
            enabled: true,
            mode,
            seq: base,
            step: base,
            events: Vec::new(),
            pending: Vec::new(),
            cited: HashSet::new(),
            reasoning: None,
            answer_step: None,
            round_text: String::new(),
            round_parts: 0,
            part_index: 0,
        }
    }

    /// 不记录任何事件（评论回复）。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::new(1, OutputMode::Plain)
        }
    }

    fn next_step(&mut self) -> String {
        self.step += 1;
        format!("s{}", self.step)
    }

    fn emit(&mut self, kind: &str, step: &str, data: Option<Value>, usage: Option<Usage>) {
        if !self.enabled {
            return;
        }
        self.seq += 1;
        let mut event = json!({
            "type": kind,
            "stepId": step,
            "seq": self.seq,
            "at": Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        });
        if let Some(data) = data.filter(|d| d.as_object().is_some_and(|o| !o.is_empty())) {
            event["data"] = data;
        }
        if let Some(usage) = usage {
            event["usage"] = usage_json(usage);
        }
        if self.events.len() >= MAX_EVENTS {
            // 超限：先丢最早的思考增量，其次丢最早的事件（WF-005）。
            let victim = self
                .events
                .iter()
                .position(|e| e["type"] == "reasoning.delta")
                .unwrap_or(0);
            self.events.remove(victim);
        }
        self.events.push(event.clone());
        self.pending.push(event);
    }

    /// 取走新事件（实时推送）。
    pub fn take_pending(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.pending)
    }

    /// 落库用的整条序列（按 seq 有序）。
    pub fn events(&self) -> Value {
        Value::Array(self.events.clone())
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 思考帧（WF-007）：第一次到来发 `reasoning.start`，累计多 16 字以上发一次 `reasoning.delta`。
    pub fn reasoning(&mut self, chars: usize) {
        if self.reasoning.is_none() {
            let step = self.next_step();
            self.emit("reasoning.start", &step, None, None);
            self.reasoning = Some((step, 0, 0));
        }
        let mut delta_step = None;
        if let Some((step, total, pushed)) = self.reasoning.as_mut() {
            *total += chars;
            if *total >= *pushed + REASONING_STEP_CHARS {
                *pushed = *total;
                delta_step = Some(step.clone());
            }
        }
        if let Some(step) = delta_step {
            self.emit("reasoning.delta", &step, None, None);
        }
    }

    fn finish_reasoning(&mut self) {
        if let Some((step, _, _)) = self.reasoning.take() {
            self.emit("reasoning.finish", &step, None, None);
        }
    }

    fn ensure_answer(&mut self) -> String {
        if let Some(step) = &self.answer_step {
            return step.clone();
        }
        self.finish_reasoning();
        let step = self.next_step();
        self.emit("answer.start", &step, None, None);
        self.answer_step = Some(step.clone());
        step
    }

    fn emit_part(&mut self, step: &str, raw: &str) {
        let text = sanitize::sanitize(raw, self.mode, sanitize::DM_MAX_CHARS);
        if text.is_empty() {
            return;
        }
        let part = self.part_index;
        self.part_index += 1;
        self.emit(
            "answer.delta",
            step,
            Some(json!({ "partIndex": part, "text": text })),
            None,
        );
    }

    /// 本轮正文增长（WF-007、GEN-050）：第一次先结束思考再发 `answer.start`；切出的完整段发 `answer.delta`。
    pub fn content(&mut self, text: &str) {
        if !self.enabled {
            return;
        }
        self.round_text.push_str(text);
        let step = self.ensure_answer();
        let segments = split_segments(&self.round_text);
        // 最后一段可能还在增长，只发已经封口的段。
        let closed = segments.len().saturating_sub(1);
        while self.round_parts < closed {
            let raw = segments[self.round_parts].clone();
            self.emit_part(&step, &raw);
            self.round_parts += 1;
        }
    }

    /// 冲刷本轮的最后一段（工具调用、本轮结束时，GEN-050）。
    pub fn flush(&mut self) {
        if !self.enabled || self.round_text.trim().is_empty() {
            self.round_text.clear();
            self.round_parts = 0;
            return;
        }
        let step = self.ensure_answer();
        let segments = split_segments(&self.round_text);
        while self.round_parts < segments.len() {
            let raw = segments[self.round_parts].clone();
            self.emit_part(&step, &raw);
            self.round_parts += 1;
        }
        self.round_text.clear();
        self.round_parts = 0;
    }

    /// 工具开始（WF-008）。返回步骤 id，结束时传回。
    pub fn tool_start(&mut self, name: &str, query: Option<&str>, label: ToolLabel<'_>) -> String {
        self.flush();
        self.finish_reasoning();
        let step = self.next_step();
        match name {
            "search_posts" => self.emit(
                "forum.search.start",
                &step,
                Some(json!({ "query": query.unwrap_or_default() })),
                None,
            ),
            "get_post_content" => self.emit("forum.read.start", &step, None, None),
            _ => self.emit(
                "tool.start",
                &step,
                Some(tool_data(name, label, None)),
                None,
            ),
        }
        step
    }

    /// 工具结束（WF-008/009）。
    pub fn tool_finish(
        &mut self,
        step: &str,
        name: &str,
        meta: &ToolMeta,
        ms: u128,
        label: ToolLabel<'_>,
    ) {
        let ms = u64::try_from(ms).unwrap_or(u64::MAX);
        match (name, meta) {
            ("search_posts", ToolMeta::Search { query, hits, items }) => {
                let items: Vec<Value> = items
                    .iter()
                    .take(SEARCH_ITEMS)
                    .map(|(id, title)| {
                        json!({ "documentId": id.to_string(), "title": truncate(title, TITLE_MAX_CHARS) })
                    })
                    .collect();
                self.emit(
                    "forum.search.finish",
                    step,
                    Some(json!({ "query": query, "hits": hits, "ms": ms, "items": items })),
                    None,
                );
            }
            ("get_post_content", ToolMeta::Read { post }) => {
                if let Some((id, title)) = post {
                    let data = json!({ "documentId": id.to_string(), "title": truncate(title, TITLE_MAX_CHARS) });
                    self.emit("forum.read.item", step, Some(data.clone()), None);
                    self.emit("forum.read.finish", step, Some(data), None);
                    self.cite(*id, title);
                } else {
                    self.emit("forum.read.finish", step, None, None);
                }
            }
            (_, ToolMeta::Comments { post }) => {
                self.emit(
                    "tool.finish",
                    step,
                    Some(json!({ "tool": name, "ms": ms })),
                    None,
                );
                if let Some((id, title)) = post {
                    self.cite(*id, title);
                }
            }
            _ => self.emit(
                "tool.finish",
                step,
                Some(tool_data(name, label, Some(ms))),
                None,
            ),
        }
    }

    /// 引用一篇帖子（按 id 去重，WF-009）。
    pub fn cite(&mut self, id: i64, title: &str) {
        if !self.enabled || !self.cited.insert(id) {
            return;
        }
        let step = self.next_step();
        self.emit(
            "citation.add",
            &step,
            Some(
                json!({ "documentId": id.to_string(), "title": truncate(title, TITLE_MAX_CHARS) }),
            ),
            None,
        );
    }

    pub fn is_cited(&self, id: i64) -> bool {
        self.cited.contains(&id)
    }

    /// 收尾（WF-007）：冲刷正文，补上 `answer.start`（空回复也要收尾），发 `answer.finish`（附用量）。
    pub fn finish(&mut self, usage: Usage) {
        self.flush();
        let step = self.ensure_answer();
        self.emit("answer.finish", &step, None, Some(usage));
    }

    /// 生成错误（只给固定的简短说明，不外泄上游错误文本）。
    pub fn error(&mut self, message: &str) {
        self.flush();
        self.finish_reasoning();
        let step = self.next_step();
        self.emit(
            "error",
            &step,
            Some(json!({ "message": truncate(message, ERROR_MAX_CHARS) })),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn patterns_compile() {
        assert!(PATTERNS.is_some());
    }

    #[test]
    fn status_sentences_become_their_own_bubbles() {
        assert_eq!(
            split_segments("正在检索相关帖子。已找到 3 篇。整理如下：一切正常。"),
            vec![
                "正在检索相关帖子。",
                "已找到 3 篇。",
                "整理如下：一切正常。"
            ]
        );
        // 前一句不是状态短句：不切。
        assert_eq!(split_segments("好的。正在检索。"), vec!["好的。正在检索。"]);
        // 后一句不以状态词开头：不切。
        assert_eq!(
            split_segments("检索完毕。下面是结果。"),
            vec!["检索完毕。下面是结果。"]
        );
    }

    #[test]
    fn report_boundaries_split_and_rules_are_swallowed() {
        assert_eq!(
            split_segments("检索完毕。\n\n## 报告\n内容"),
            vec!["检索完毕。", "## 报告\n内容"]
        );
        assert_eq!(
            split_segments("已完成分析。\n\n---\n结论：可信。"),
            vec!["已完成分析。", "结论：可信。"]
        );
        assert_eq!(
            split_segments("普通回答。\n\n- 列表"),
            vec!["普通回答。\n\n- 列表"]
        );
    }

    #[test]
    fn citations_cover_three_link_forms() {
        let text = "见 [攻略](/post/12)，以及 合集 (/post/34) 和 /post/12 还有 /post/abc /post/56";
        assert_eq!(cited_post_ids(text), vec![12, 34, 56]);
    }

    #[test]
    fn generic_tool_events_carry_title_and_source() {
        let mut wf = Workflow::new(1, OutputMode::Plain);
        let label = ToolLabel {
            title: Some("检索虚空终端"),
            source: Some("akasha"),
        };
        let step = wf.tool_start("akasha__search", None, label);
        wf.tool_finish(&step, "akasha__search", &ToolMeta::None, 7, label);
        let plain = wf.tool_start("get_hot_posts", None, ToolLabel::default());
        wf.tool_finish(
            &plain,
            "get_hot_posts",
            &ToolMeta::None,
            1,
            ToolLabel::default(),
        );
        let events = wf.take_pending();
        assert_eq!(
            events[0]["data"],
            json!({ "tool": "akasha__search", "title": "检索虚空终端", "source": "akasha" })
        );
        assert_eq!(
            events[1]["data"],
            json!({ "tool": "akasha__search", "title": "检索虚空终端", "source": "akasha", "ms": 7 })
        );
        // 论坛工具不带新字段，与以前一致。
        assert_eq!(events[2]["data"], json!({ "tool": "get_hot_posts" }));
        assert_eq!(
            events[3]["data"],
            json!({ "tool": "get_hot_posts", "ms": 1 })
        );
    }

    #[test]
    fn events_are_ordered_redacted_and_capped() {
        let mut wf = Workflow::new(1, OutputMode::Plain);
        wf.reasoning(10);
        wf.reasoning(10);
        wf.content("正在检索。");
        let search = wf.tool_start("search_posts", Some("攻略"), ToolLabel::default());
        wf.tool_finish(
            &search,
            "search_posts",
            &ToolMeta::Search {
                query: "攻略".into(),
                hits: 1,
                items: vec![(9, "标题".into())],
            },
            12,
            ToolLabel::default(),
        );
        let read = wf.tool_start("get_post_content", None, ToolLabel::default());
        wf.tool_finish(
            &read,
            "get_post_content",
            &ToolMeta::Read {
                post: Some((9, "标题".into())),
            },
            3,
            ToolLabel::default(),
        );
        wf.content("**结论**：可信。");
        wf.cite(9, "标题");
        wf.finish(Usage {
            prompt_tokens: 5,
            completion_tokens: 6,
        });
        let events = wf.take_pending();
        let types: Vec<&str> = events.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(
            types,
            vec![
                "reasoning.start",
                "reasoning.delta",
                "reasoning.finish",
                "answer.start",
                "answer.delta",
                "forum.search.start",
                "forum.search.finish",
                "forum.read.start",
                "forum.read.item",
                "forum.read.finish",
                "citation.add",
                "answer.delta",
                "answer.finish",
            ]
        );
        let seqs: Vec<i64> = events.iter().map(|e| e["seq"].as_i64().unwrap()).collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));
        assert!(events[1].get("data").is_none());
        assert_eq!(
            events[4]["data"],
            json!({ "partIndex": 0, "text": "正在检索。" })
        );
        assert_eq!(
            events[11]["data"],
            json!({ "partIndex": 1, "text": "结论：可信。" })
        );
        assert_eq!(
            events[12]["usage"],
            json!({ "promptTokens": 5, "completionTokens": 6 })
        );
        // 重试从新的起点继续递增（WF-006）。
        let retry = Workflow::new(2, OutputMode::Plain);
        assert!(retry.seq > seqs[seqs.len() - 1]);

        let mut big = Workflow::new(1, OutputMode::Plain);
        big.reasoning(1);
        for _ in 0..(MAX_EVENTS * 2) {
            big.reasoning(REASONING_STEP_CHARS);
        }
        big.finish(Usage::default());
        let all = big.events();
        let all = all.as_array().unwrap();
        assert_eq!(all.len(), MAX_EVENTS);
        assert_eq!(all[0]["type"], "reasoning.start");
        assert_eq!(all[MAX_EVENTS - 1]["type"], "answer.finish");
    }
}
