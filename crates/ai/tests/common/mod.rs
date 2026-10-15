//! 测试共用：脚本化的假上游（实现 `ChatProvider`，不连网络）。

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::sync::Mutex;

use futures_util::StreamExt;
use ik_ai::llm::{ChatProvider, ChatRequest, EventStream, LlmError, StreamEvent, Usage};

/// 一次调用的脚本。
#[derive(Clone)]
pub enum Step {
    Emit(StreamEvent),
    Fail(LlmError),
    /// 永远不再出帧（模拟卡住的上游）。
    Hang,
}

#[derive(Default)]
pub struct Scripted {
    scripts: Mutex<VecDeque<Vec<Step>>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl Scripted {
    pub fn push(&self, steps: Vec<Step>) {
        self.scripts.lock().unwrap().push_back(steps);
    }

    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl ChatProvider for Scripted {
    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, LlmError> {
        self.requests.lock().unwrap().push(request.clone());
        let steps = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .expect("没有剩余脚本");
        let stream = futures_util::stream::iter(steps).then(|step| async move {
            match step {
                Step::Emit(event) => Some(Ok(event)),
                Step::Fail(error) => Some(Err(error)),
                Step::Hang => {
                    std::future::pending::<()>().await;
                    None
                }
            }
        });
        Ok(Box::pin(stream.filter_map(|item| async move { item })))
    }
}

pub fn content(text: &str) -> Step {
    Step::Emit(StreamEvent::Content(text.to_owned()))
}

pub fn usage(prompt: i64, completion: i64) -> Step {
    Step::Emit(StreamEvent::Usage(Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
    }))
}
