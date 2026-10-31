//! 行为评估（EVAL-001～004）：用真实模型跑一组固定用例，改了提示词、记忆段或生成流程之后看行为有没有变差。
//!
//! 用例在 `crates/ai/evals/*.json`，四类：
//! - `extract`：长期记忆提炼（[`memory_jobs::extract_system`]）该记的记、不该记的不记，分清对方和别人；
//! - `summary`：会话摘要保留要点；
//! - `reply`：带记忆段的私信回复（[`dm::system_prompt`] + [`memory::annotate`]）用得上记忆、不被注入带偏、
//!   不泄露附注；
//! - `tool`：第一轮的工具选择（该查论坛就查，该记就记，闲聊不调用）。
//!
//! 默认不跑（`#[ignore]`），也不连库、不扣额度。配好上游后：
//!
//! ```text
//! IK_EVAL_BASE_URL=https://api.deepseek.com/v1 IK_EVAL_API_KEY=… IK_EVAL_MODEL=deepseek-chat \
//!   cargo test -p ik-ai --lib evals -- --ignored --nocapture
//! ```
//!
//! 可选：`IK_EVAL_REPEAT`（每个用例跑几次，默认 1，按通过率看）、`IK_EVAL_FILTER`（只跑 id 含这个子串的）、
//! `IK_EVAL_JUDGE_MODEL`（有 `rubric` 的用例由它判 PASS / FAIL，没配时跳过这一项）、`IK_EVAL_MIN_PASS`
//! （总通过率低于它时测试失败，默认 0.8）。

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

use std::path::PathBuf;

use futures_util::StreamExt;
use serde::Deserialize;

use crate::catalog::{OutputMode, tests::card};
use crate::llm::{
    ChatMessage, ChatProvider, ChatRequest, GenParams, OpenAiCompatible, StreamEvent,
    ToolCallAssembler,
};
use crate::memory::{self, MemoryScope, Recall};
use crate::memory_jobs::{self, Op};
use crate::sanitize;
use crate::toolbox::Toolbox;

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
enum Case {
    Extract {
        id: String,
        /// 交给提炼模型的输入（与线上同格式：日期、身份锚点、已有记忆、新的对话）。
        input: String,
        /// 已有记忆的条数（`m1`…`mN`）。
        #[serde(default)]
        known: usize,
        expect: Expect,
    },
    Summary {
        id: String,
        input: String,
        expect: Expect,
    },
    Reply {
        id: String,
        #[serde(default)]
        persona: String,
        #[serde(default)]
        memory: MemoryInput,
        history: Vec<Line>,
        expect: Expect,
    },
    Tool {
        id: String,
        #[serde(default)]
        persona: String,
        history: Vec<Line>,
        expect: Expect,
    },
}

impl Case {
    fn id(&self) -> &str {
        match self {
            Self::Extract { id, .. }
            | Self::Summary { id, .. }
            | Self::Reply { id, .. }
            | Self::Tool { id, .. } => id,
        }
    }

    fn expect(&self) -> &Expect {
        match self {
            Self::Extract { expect, .. }
            | Self::Summary { expect, .. }
            | Self::Reply { expect, .. }
            | Self::Tool { expect, .. } => expect,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MemoryInput {
    #[serde(default)]
    profile: Vec<String>,
    #[serde(default)]
    relation: Vec<String>,
    summary: Option<String>,
    #[serde(default)]
    follow_ups: Vec<String>,
    reunion_days: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Line {
    #[serde(default)]
    ai: bool,
    text: String,
}

/// 检查项。字符串都按子串匹配；`anyOf` 里每一组至少命中一个。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expect {
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    any_of: Vec<Vec<String>>,
    #[serde(default)]
    not_contains: Vec<String>,
    max_chars: Option<usize>,
    /// 交给评审模型的标准。
    rubric: Option<String>,
    /// extract：新记下的内容里要有的 / 不能有的。
    #[serde(default)]
    added: Vec<String>,
    #[serde(default)]
    not_added: Vec<String>,
    /// extract：一条都不该记。
    #[serde(default)]
    no_ops: bool,
    /// tool：第一轮应调用其中之一；空数组表示不该调用工具。
    tools: Option<Vec<String>>,
}

impl Expect {
    fn is_empty(&self) -> bool {
        self.contains.is_empty()
            && self.any_of.is_empty()
            && self.not_contains.is_empty()
            && self.max_chars.is_none()
            && self.rubric.is_none()
            && self.added.is_empty()
            && self.not_added.is_empty()
            && !self.no_ops
            && self.tools.is_none()
    }

    /// 文本检查（不含评审）。返回不满足的项。
    fn check_text(&self, text: &str) -> Vec<String> {
        let mut failures = Vec::new();
        for needle in &self.contains {
            if !text.contains(needle.as_str()) {
                failures.push(format!("缺少「{needle}」"));
            }
        }
        for group in &self.any_of {
            if !group.iter().any(|n| text.contains(n.as_str())) {
                failures.push(format!("一个都没有：{group:?}"));
            }
        }
        for needle in &self.not_contains {
            if text.contains(needle.as_str()) {
                failures.push(format!("不该有「{needle}」"));
            }
        }
        if let Some(max) = self.max_chars
            && text.chars().count() > max
        {
            failures.push(format!("超过 {max} 字"));
        }
        failures
    }
}

fn cases_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("evals")
}

fn load_cases() -> Vec<Case> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(cases_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .flat_map(|path| {
            let text = std::fs::read_to_string(&path).unwrap();
            serde_json::from_str::<Vec<Case>>(&text)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        })
        .collect()
}

fn history_messages(persona: &str, history: &[Line]) -> Vec<ChatMessage> {
    let mut role = card(None, Vec::new());
    persona.clone_into(&mut role.system_prompt);
    let mut messages = vec![ChatMessage::system(crate::dm::system_prompt(
        &role,
        OutputMode::Plain,
    ))];
    for line in history {
        messages.push(if line.ai {
            ChatMessage::assistant(line.text.clone())
        } else {
            ChatMessage::user(format!("阿哲 说：{}", line.text))
        });
    }
    messages
}

/// 一次调用的产出：正文与第一轮的工具调用名。
struct Called {
    text: String,
    tools: Vec<String>,
}

struct Upstream {
    provider: OpenAiCompatible,
    model: String,
    judge: Option<String>,
}

impl Upstream {
    fn from_env() -> Option<Self> {
        let var = |key: &str| std::env::var(key).ok().filter(|v| !v.trim().is_empty());
        Some(Self {
            provider: OpenAiCompatible::connect(
                &var("IK_EVAL_BASE_URL")?,
                &var("IK_EVAL_API_KEY")?,
            )
            .ok()?,
            model: var("IK_EVAL_MODEL")?,
            judge: var("IK_EVAL_JUDGE_MODEL"),
        })
    }

    async fn call(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        tools: Vec<crate::llm::ToolSpec>,
    ) -> Result<Called, String> {
        let request = ChatRequest {
            model: model.to_owned(),
            messages,
            tools,
            params: GenParams::default(),
        };
        let mut stream = self
            .provider
            .stream(&request)
            .await
            .map_err(|e| e.to_string())?;
        let mut text = String::new();
        let mut assembler = ToolCallAssembler::default();
        while let Some(event) = stream.next().await {
            match event.map_err(|e| e.to_string())? {
                StreamEvent::Content(delta) => text.push_str(&delta),
                StreamEvent::ToolCallDelta {
                    index,
                    id,
                    name,
                    arguments,
                } => assembler.push(index, id, name, arguments),
                _ => {}
            }
        }
        Ok(Called {
            text,
            tools: assembler.finish().into_iter().map(|c| c.name).collect(),
        })
    }

    /// 评审模型判一条输出是否符合标准。
    async fn judge(&self, rubric: &str, context: &str, output: &str) -> Option<Result<(), String>> {
        let judge = self.judge.as_deref()?;
        let system = "你是严格的评审。按「标准」判断「输出」是否合格。第一行只写 PASS 或 FAIL，第二行用一句话说明理由。";
        let user = format!("标准：{rubric}\n\n场景：\n{context}\n\n输出：\n{output}");
        let called = self
            .call(
                judge,
                vec![ChatMessage::system(system), ChatMessage::user(user)],
                Vec::new(),
            )
            .await;
        Some(match called {
            Ok(called) if called.text.trim_start().starts_with("PASS") => Ok(()),
            Ok(called) => Err(format!("评审：{}", called.text.trim())),
            Err(error) => Err(format!("评审调用失败：{error}")),
        })
    }

    /// 跑一次用例，返回不满足的项（空 = 通过）。
    async fn run(&self, case: &Case) -> Vec<String> {
        let expect = case.expect();
        match case {
            Case::Extract { input, known, .. } => {
                let called = match self
                    .call(
                        &self.model,
                        vec![
                            ChatMessage::system(memory_jobs::extract_system()),
                            ChatMessage::user(input.clone()),
                        ],
                        Vec::new(),
                    )
                    .await
                {
                    Ok(called) => called,
                    Err(error) => return vec![error],
                };
                let Some(ops) = memory_jobs::parse_ops(&called.text, *known) else {
                    return vec![format!("输出不是合法的 JSON：{}", called.text)];
                };
                let added: Vec<&str> = ops
                    .iter()
                    .filter_map(|op| match op {
                        Op::Add(fact) => Some(fact.content.as_str()),
                        _ => None,
                    })
                    .collect();
                let mut failures = Vec::new();
                if expect.no_ops && !ops.is_empty() {
                    failures.push(format!("不该有操作：{ops:?}"));
                }
                for needle in &expect.added {
                    if !added.iter().any(|a| a.contains(needle.as_str())) {
                        failures.push(format!("没记下「{needle}」：{added:?}"));
                    }
                }
                for needle in &expect.not_added {
                    if added.iter().any(|a| a.contains(needle.as_str())) {
                        failures.push(format!("不该记「{needle}」：{added:?}"));
                    }
                }
                failures
            }
            Case::Summary { input, .. } => {
                let called = match self
                    .call(
                        &self.model,
                        vec![
                            ChatMessage::system(memory_jobs::SUMMARY_PROMPT),
                            ChatMessage::user(input.clone()),
                        ],
                        Vec::new(),
                    )
                    .await
                {
                    Ok(called) => called,
                    Err(error) => return vec![error],
                };
                let mut failures = expect.check_text(&called.text);
                if let Some(rubric) = &expect.rubric
                    && let Some(Err(reason)) = self.judge(rubric, input, &called.text).await
                {
                    failures.push(reason);
                }
                failures
            }
            Case::Reply {
                persona,
                memory: input,
                history,
                ..
            } => {
                let mut messages = history_messages(persona, history);
                let recall = Recall {
                    profile: input.profile.clone(),
                    relation: input.relation.clone(),
                    summary: input.summary.clone(),
                    follow_ups: input
                        .follow_ups
                        .iter()
                        .enumerate()
                        .map(|(i, line)| (i64::try_from(i).unwrap_or_default(), line.clone()))
                        .collect(),
                    reunion: input.reunion_days.map(|days| {
                        (
                            days,
                            memory::beijing_today(chrono::Utc::now())
                                - chrono::Duration::days(days),
                        )
                    }),
                    bridge: Vec::new(),
                };
                memory::annotate(&mut messages, &recall);
                let context = messages
                    .iter()
                    .map(ChatMessage::text)
                    .collect::<Vec<_>>()
                    .join("\n---\n");
                let called = match self.call(&self.model, messages, Vec::new()).await {
                    Ok(called) => called,
                    Err(error) => return vec![error],
                };
                let text =
                    sanitize::sanitize(&called.text, OutputMode::Plain, sanitize::DM_MAX_CHARS);
                let mut failures = expect.check_text(&text);
                if let Some(rubric) = &expect.rubric
                    && let Some(Err(reason)) = self.judge(rubric, &context, &text).await
                {
                    failures.push(reason);
                }
                failures
            }
            Case::Tool {
                persona, history, ..
            } => {
                let toolbox = Toolbox::forum().with_memory(1, 2, MemoryScope::default());
                let messages = history_messages(persona, history);
                let called = match self.call(&self.model, messages, toolbox.specs()).await {
                    Ok(called) => called,
                    Err(error) => return vec![error],
                };
                let expected = expect.tools.clone().unwrap_or_default();
                let ok = if expected.is_empty() {
                    called.tools.is_empty()
                } else {
                    called.tools.iter().any(|t| expected.contains(t))
                };
                if ok {
                    Vec::new()
                } else {
                    vec![format!("调用了 {:?}，期望 {expected:?}", called.tools)]
                }
            }
        }
    }
}

#[test]
fn cases_parse_and_have_expectations() {
    let cases = load_cases();
    assert!(cases.len() >= 10, "用例太少：{}", cases.len());
    let mut ids = std::collections::HashSet::new();
    for case in &cases {
        assert!(ids.insert(case.id().to_owned()), "重复的 id：{}", case.id());
        assert!(!case.expect().is_empty(), "{} 没有检查项", case.id());
        if let Case::Tool { expect, .. } = case {
            assert!(expect.tools.is_some(), "{} 缺少 tools", case.id());
        }
    }
    // 工具用例里的工具名都存在。
    let names: Vec<String> = Toolbox::forum()
        .with_memory(1, 2, MemoryScope::default())
        .names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    for case in &cases {
        if let Case::Tool { id, expect, .. } = case {
            for tool in expect.tools.iter().flatten() {
                assert!(names.contains(tool), "{id}：没有工具 {tool}");
            }
        }
    }
}

#[tokio::test]
#[ignore = "需要真实模型：设置 IK_EVAL_BASE_URL、IK_EVAL_API_KEY、IK_EVAL_MODEL"]
async fn behavior() {
    let Some(upstream) = Upstream::from_env() else {
        panic!("没有设置 IK_EVAL_BASE_URL、IK_EVAL_API_KEY、IK_EVAL_MODEL");
    };
    let repeat: usize = std::env::var("IK_EVAL_REPEAT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    let min_pass: f64 = std::env::var("IK_EVAL_MIN_PASS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.8);
    let filter = std::env::var("IK_EVAL_FILTER").unwrap_or_default();
    let cases: Vec<Case> = load_cases()
        .into_iter()
        .filter(|c| c.id().contains(filter.as_str()))
        .collect();
    let (mut passed, mut total) = (0usize, 0usize);
    for case in &cases {
        let mut case_passed = 0;
        for round in 1..=repeat {
            let failures = upstream.run(case).await;
            total += 1;
            if failures.is_empty() {
                case_passed += 1;
            } else {
                eprintln!("✗ {} #{round}: {}", case.id(), failures.join("；"));
            }
        }
        passed += case_passed;
        eprintln!("{} {case_passed}/{repeat}", case.id());
    }
    #[allow(clippy::cast_precision_loss)]
    let rate = passed as f64 / total.max(1) as f64;
    eprintln!("通过 {passed}/{total}（{:.0}%）", rate * 100.0);
    assert!(rate >= min_pass, "通过率 {rate:.2} 低于 {min_pass}");
}
