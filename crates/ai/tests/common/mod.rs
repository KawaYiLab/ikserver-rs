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
    embedded: Mutex<Vec<String>>,
}

impl Scripted {
    pub fn push(&self, steps: Vec<Step>) {
        self.scripts.lock().unwrap().push_back(steps);
    }

    pub fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }

    /// 算过向量的文本（按调用顺序）。
    pub fn embedded(&self) -> Vec<String> {
        self.embedded.lock().unwrap().clone()
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

    /// 假向量：字的二元组散列到 64 维再归一化，字面相近的文本余弦相似度高。
    async fn embed(&self, _model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        self.embedded.lock().unwrap().extend(inputs.iter().cloned());
        Ok(inputs.iter().map(|text| fake_embedding(text)).collect())
    }
}

pub fn fake_embedding(text: &str) -> Vec<f32> {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut vector = vec![0f32; 64];
    for pair in chars.windows(2) {
        let slot = (u32::from(pair[0]).wrapping_mul(31) ^ u32::from(pair[1])) as usize % 64;
        vector[slot] += 1.0;
    }
    let norm = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut vector {
            *x /= norm;
        }
    }
    vector
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
