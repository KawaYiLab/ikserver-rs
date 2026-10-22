//! 直连 OpenAI 兼容的大模型接口（LLM-004、LLM-006）。
//!
//! 请求与流式帧都用自己的类型（async-openai 的 byot），以便读出各家的扩展字段：思考增量
//! （`reasoning_content` / `reasoning`）、流末的用量帧、流中途的错误对象。上游只在这里出现；
//! 业务代码面对的是 [`ChatProvider`]，测试用本地 SSE 桩服务替换上游地址即可。

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::error::OpenAIError;
use futures_util::{Stream, StreamExt};
use ik_kernel::config::AiProvider;
use serde::{Deserialize, Serialize};

/// 一条对话消息，序列化成 OpenAI 的 `messages[]` 元素。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ChatMessage {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    Tool {
        tool_call_id: String,
        content: String,
    },
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::User {
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Assistant {
            content: content.into(),
            tool_calls: Vec::new(),
        }
    }

    /// 估算用的正文长度（字符）。
    pub fn text(&self) -> &str {
        match self {
            Self::System { content }
            | Self::User { content }
            | Self::Assistant { content, .. }
            | Self::Tool { content, .. } => content,
        }
    }
}

/// 模型发起的一次工具调用（参数是模型给的 JSON 文本，可能不合法）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

impl Serialize for ToolCall {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde_json::json!({
            "id": self.id,
            "type": "function",
            "function": { "name": self.name, "arguments": self.arguments },
        })
        .serialize(serializer)
    }
}

/// 提供给模型的一个工具。
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: Cow<'static, str>,
    pub description: Cow<'static, str>,
    /// JSON Schema（object）。
    pub parameters: serde_json::Value,
}

impl Serialize for ToolSpec {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": self.name,
                "description": self.description,
                "parameters": self.parameters,
            },
        })
        .serialize(serializer)
    }
}

/// 一次流式请求。
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// 上游的模型名（`ai_models.provider_model`）。
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSpec>,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "<[ToolSpec]>::is_empty")]
    tools: &'a [ToolSpec],
    stream: bool,
    stream_options: StreamOptions,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// 上游报告的用量。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

impl Usage {
    pub fn total(self) -> i64 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }

    pub fn is_zero(self) -> bool {
        self.prompt_tokens == 0 && self.completion_tokens == 0
    }
}

/// 流里的一帧（已经按我们关心的内容拆开）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// 思考增量（不进正文，不下发文本，LLM-004）。
    Reasoning(String),
    /// 正文增量。
    Content(String),
    /// 工具调用增量：同一个 `index` 的多帧拼成一次调用。
    ToolCallDelta {
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    Usage(Usage),
    /// 本次请求结束的原因（`stop`、`tool_calls`、`length`……）。
    Finish(String),
}

pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, LlmError>> + Send>>;

#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    /// 上游限流或额度（TRIG-033：退避基数更大）。
    #[error("upstream rate limited: {0}")]
    RateLimited(String),
    #[error("upstream timeout")]
    Timeout,
    /// 其他上游错误。文本只进日志与审计，绝不作为回复正文（GEN-024）。
    #[error("upstream error: {0}")]
    Upstream(String),
    /// 模型指向的提供方没有配置（LLM-011）。
    #[error("provider not configured: {0}")]
    UnknownProvider(String),
}

impl LlmError {
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::RateLimited(_))
    }
}

/// TRIG-033 的限流识别。
static RATE_LIMIT_RE: LazyLock<Option<regex::Regex>> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b(429|rate[- ]?limit|too many requests|quota exceeded|throttled)\b")
        .ok()
});

fn looks_rate_limited(message: &str) -> bool {
    RATE_LIMIT_RE
        .as_ref()
        .is_some_and(|re| re.is_match(message))
}

fn classify(message: String) -> LlmError {
    if looks_rate_limited(&message) {
        LlmError::RateLimited(message)
    } else {
        LlmError::Upstream(message)
    }
}

impl From<OpenAIError> for LlmError {
    fn from(error: OpenAIError) -> Self {
        match error {
            OpenAIError::ApiError(response) => {
                let message = format!("{} {}", response.status_code, response.api_error.message);
                if response.status_code.as_u16() == 429 {
                    Self::RateLimited(message)
                } else {
                    classify(message)
                }
            }
            OpenAIError::Reqwest(error) if error.is_timeout() => Self::Timeout,
            other => classify(other.to_string()),
        }
    }
}

/// 一个能发流式对话请求的上游。
#[async_trait::async_trait]
pub trait ChatProvider: Send + Sync {
    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, LlmError>;
}

/// OpenAI 兼容协议的上游（DeepSeek、Qwen、GLM、Kimi……）。
pub struct OpenAiCompatible {
    client: Client<OpenAIConfig>,
}

/// rustls + ring + webpki 根证书（不引入 aws-lc）。进程内只建一次，各提供方共用连接池。
static HTTP_CLIENT: LazyLock<Result<reqwest13::Client, LlmError>> =
    LazyLock::new(build_http_client);

/// rustls + ring + webpki 根证书的客户端构造器（MCP 客户端也用它）。
pub(crate) fn client_builder() -> Result<reqwest13::ClientBuilder, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls config: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(reqwest13::Client::builder()
        .tls_backend_preconfigured(tls)
        .connect_timeout(Duration::from_secs(10)))
}

fn build_http_client() -> Result<reqwest13::Client, LlmError> {
    client_builder()
        .map_err(LlmError::Upstream)?
        // 两帧之间的最长间隔；总时长由生成方的墙钟预算控制（GEN-020）。
        .read_timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| LlmError::Upstream(format!("http client: {e}")))
}

fn http_client() -> Result<reqwest13::Client, LlmError> {
    HTTP_CLIENT.clone()
}

impl OpenAiCompatible {
    pub fn new(provider: &AiProvider) -> Result<Self, LlmError> {
        Self::connect(&provider.base_url, provider.api_key.expose())
    }

    /// `base_url` 例如 `https://api.deepseek.com/v1`。
    pub fn connect(base_url: &str, api_key: &str) -> Result<Self, LlmError> {
        let config = OpenAIConfig::new()
            .with_api_base(base_url.trim_end_matches('/'))
            .with_api_key(api_key);
        Ok(Self {
            client: Client::build(http_client()?, config),
        })
    }
}

/// 上游列出的模型名（`GET {base_url}/models`），管理后台测试连接与填写上游模型名时用。
pub async fn list_models(base_url: &str, api_key: &str) -> Result<Vec<String>, LlmError> {
    #[derive(Deserialize)]
    struct Listed {
        #[serde(default)]
        data: Vec<Entry>,
    }
    #[derive(Deserialize)]
    struct Entry {
        id: String,
    }

    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let mut request = http_client()?.get(url).timeout(Duration::from_secs(15));
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }
    let response = request.send().await.map_err(|e| {
        if e.is_timeout() {
            LlmError::Timeout
        } else {
            LlmError::Upstream(e.to_string())
        }
    })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let body: String = body.chars().take(300).collect();
        return Err(classify(format!("{} {body}", status.as_u16())));
    }
    let listed: Listed = response
        .json()
        .await
        .map_err(|e| LlmError::Upstream(format!("unexpected response: {e}")))?;
    let mut ids: Vec<String> = listed.data.into_iter().map(|m| m.id).collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

#[derive(Debug, Deserialize)]
struct WireChunk {
    #[serde(default)]
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
    /// 有的上游在 200 的流里用错误对象结束。
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct WireChoice {
    #[serde(default)]
    delta: Option<WireDelta>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct WireDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<WireToolCall>>,
}

#[derive(Debug, Deserialize)]
struct WireToolCall {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<WireFunction>,
}

#[derive(Debug, Deserialize)]
struct WireFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: Option<i64>,
    #[serde(default)]
    completion_tokens: Option<i64>,
}

fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|t| !t.is_empty())
}

/// 一帧拆成若干事件。
fn split_chunk(chunk: WireChunk) -> Result<Vec<StreamEvent>, LlmError> {
    if let Some(error) = chunk.error {
        let message = error
            .get("message")
            .and_then(|m| m.as_str())
            .map_or_else(|| error.to_string(), str::to_owned);
        return Err(classify(message));
    }
    let mut events = Vec::new();
    for choice in chunk.choices {
        if let Some(delta) = choice.delta {
            if let Some(text) = non_empty(delta.reasoning_content).or(non_empty(delta.reasoning)) {
                events.push(StreamEvent::Reasoning(text));
            }
            if let Some(text) = non_empty(delta.content) {
                events.push(StreamEvent::Content(text));
            }
            for call in delta.tool_calls.unwrap_or_default() {
                let (name, arguments) = call
                    .function
                    .map_or((None, None), |f| (f.name, f.arguments));
                events.push(StreamEvent::ToolCallDelta {
                    index: call.index,
                    id: call.id,
                    name,
                    arguments,
                });
            }
        }
        if let Some(reason) = choice.finish_reason {
            events.push(StreamEvent::Finish(reason));
        }
    }
    if let Some(usage) = chunk.usage {
        let usage = Usage {
            prompt_tokens: usage.prompt_tokens.unwrap_or(0).max(0),
            completion_tokens: usage.completion_tokens.unwrap_or(0).max(0),
        };
        // 全 0 的统计帧不算用量（GEN-080）。
        if !usage.is_zero() {
            events.push(StreamEvent::Usage(usage));
        }
    }
    Ok(events)
}

#[async_trait::async_trait]
impl ChatProvider for OpenAiCompatible {
    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, LlmError> {
        let body = WireRequest {
            model: &request.model,
            messages: &request.messages,
            tools: &request.tools,
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
        };
        let raw = self
            .client
            .chat()
            .create_stream_byot::<_, WireChunk>(body)
            .await?;
        let events = raw.flat_map(|item| {
            let events: Vec<Result<StreamEvent, LlmError>> =
                match item.map_err(LlmError::from).and_then(split_chunk) {
                    Ok(events) => events.into_iter().map(Ok).collect(),
                    Err(error) => vec![Err(error)],
                };
            futures_util::stream::iter(events)
        });
        Ok(Box::pin(events))
    }
}

/// 按名字找上游（`ai_models.provider`）。
#[derive(Clone, Default)]
pub struct Providers {
    by_name: BTreeMap<String, Arc<dyn ChatProvider>>,
}

impl std::fmt::Debug for Providers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.by_name.keys()).finish()
    }
}

impl Providers {
    pub fn from_config(config: &BTreeMap<String, AiProvider>) -> Result<Self, LlmError> {
        let mut by_name = BTreeMap::new();
        for (name, provider) in config {
            let client: Arc<dyn ChatProvider> = Arc::new(OpenAiCompatible::new(provider)?);
            by_name.insert(name.clone(), client);
        }
        Ok(Self { by_name })
    }

    /// 测试或别的实现直接注册。
    pub fn with(mut self, name: impl Into<String>, provider: Arc<dyn ChatProvider>) -> Self {
        self.by_name.insert(name.into(), provider);
        self
    }

    /// 补上没有的名字；已有的（部署配置里的）不被覆盖。
    pub fn or_with(mut self, name: impl Into<String>, provider: Arc<dyn ChatProvider>) -> Self {
        self.by_name.entry(name.into()).or_insert(provider);
        self
    }

    pub fn get(&self, name: &str) -> Result<Arc<dyn ChatProvider>, LlmError> {
        self.by_name
            .get(name)
            .cloned()
            .ok_or_else(|| LlmError::UnknownProvider(name.to_owned()))
    }

    /// 已配置的提供方名字（管理后台展示）。
    pub fn names(&self) -> Vec<String> {
        self.by_name.keys().cloned().collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }
}

/// 把工具调用增量拼成完整调用（按 `index`）。
#[derive(Debug, Default)]
pub struct ToolCallAssembler {
    calls: BTreeMap<u32, ToolCall>,
}

impl ToolCallAssembler {
    pub fn push(
        &mut self,
        index: u32,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    ) {
        let call = self.calls.entry(index).or_insert_with(|| ToolCall {
            id: String::new(),
            name: String::new(),
            arguments: String::new(),
        });
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            call.id = id;
        }
        if let Some(name) = name {
            call.name.push_str(&name);
        }
        if let Some(arguments) = arguments {
            call.arguments.push_str(&arguments);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// 完整调用；上游没给 id 的补一个稳定的。
    pub fn finish(self) -> Vec<ToolCall> {
        self.calls
            .into_iter()
            .map(|(index, mut call)| {
                if call.id.is_empty() {
                    call.id = format!("call_{index}");
                }
                call
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn rate_limit_detection_matches_trig_033() {
        for text in [
            "429 Too Many Requests",
            "Rate limit reached",
            "rate-limit",
            "quota exceeded for model",
            "request throttled",
        ] {
            assert!(looks_rate_limited(text), "{text}");
        }
        assert!(!looks_rate_limited("invalid api key"));
        assert!(!looks_rate_limited("id 14290"));
    }

    #[test]
    fn messages_serialize_as_openai_json() {
        let messages = vec![
            ChatMessage::system("s"),
            ChatMessage::Assistant {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "search_posts".into(),
                    arguments: "{\"query\":\"x\"}".into(),
                }],
            },
            ChatMessage::Tool {
                tool_call_id: "call_1".into(),
                content: "r".into(),
            },
        ];
        let json = serde_json::to_value(&messages).unwrap();
        assert_eq!(
            json[0],
            serde_json::json!({"role": "system", "content": "s"})
        );
        assert_eq!(json[1]["tool_calls"][0]["function"]["name"], "search_posts");
        assert_eq!(json[1]["tool_calls"][0]["type"], "function");
        assert_eq!(json[2]["role"], "tool");
        assert_eq!(json[2]["tool_call_id"], "call_1");
    }

    #[test]
    fn chunks_split_into_events() {
        let chunk: WireChunk = serde_json::from_str(
            r#"{"choices":[{"delta":{"reasoning_content":"想","content":"答",
                "tool_calls":[{"index":0,"id":"call_a","function":{"name":"x","arguments":"{"}}]},
                "finish_reason":null}],"usage":{"prompt_tokens":0,"completion_tokens":0}}"#,
        )
        .unwrap();
        let events = split_chunk(chunk).unwrap();
        assert_eq!(
            events,
            vec![
                StreamEvent::Reasoning("想".into()),
                StreamEvent::Content("答".into()),
                StreamEvent::ToolCallDelta {
                    index: 0,
                    id: Some("call_a".into()),
                    name: Some("x".into()),
                    arguments: Some("{".into()),
                },
            ]
        );
        let error: WireChunk =
            serde_json::from_str(r#"{"error":{"message":"rate limit exceeded"}}"#).unwrap();
        assert!(split_chunk(error).unwrap_err().is_rate_limited());
    }

    #[test]
    fn tool_calls_assemble_by_index() {
        let mut assembler = ToolCallAssembler::default();
        assembler.push(1, None, Some("b".into()), Some("{}".into()));
        assembler.push(0, Some("call_a".into()), Some("search_".into()), None);
        assembler.push(0, None, Some("posts".into()), Some("{\"q\"".into()));
        assembler.push(0, None, None, Some(":1}".into()));
        let calls = assembler.finish();
        assert_eq!(calls[0].id, "call_a");
        assert_eq!(calls[0].name, "search_posts");
        assert_eq!(calls[0].arguments, "{\"q\":1}");
        assert_eq!(calls[1].id, "call_1");
    }
}
