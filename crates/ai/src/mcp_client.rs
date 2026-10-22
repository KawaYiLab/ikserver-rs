//! 站外 MCP 服务的最小客户端（ADR 0004 第 3.1 节）：只支持 Streamable HTTP，只用
//! `initialize` → `notifications/initialized` → `tools/list` → `tools/call`。
//!
//! - 不声明任何客户端能力（sampling、elicitation、roots），服务端发来的请求一律不响应；
//! - 响应接受 `application/json` 与 `text/event-stream`（读到对应 id 的响应即断开）；
//! - 服务端回 `Mcp-Session-Id` 时按（地址, 鉴权头）在进程内缓存，收到 404 重新握手一次；
//!   多实例下各实例各自握手；
//! - 不跟随重定向；响应体超过 [`MAX_BODY_BYTES`] 按失败处理；
//! - 同一服务连续失败 [`BREAKER_FAILURES`] 次后熔断 [`BREAKER_COOLDOWN`]，期间不再提供它的工具。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use http::{HeaderName, HeaderValue, StatusCode, header};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};

/// 客户端请求的协议版本（最新的在前）。服务端回应的版本不在列表里时按失败处理。
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26"];
/// 响应体上限。
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// 工具清单最多翻几页、最多多少个工具。
const MAX_LIST_PAGES: usize = 10;
pub const MAX_LISTED_TOOLS: usize = 200;
/// 连续失败这么多次后熔断。
pub const BREAKER_FAILURES: u32 = 5;
pub const BREAKER_COOLDOWN: Duration = Duration::from_secs(60);
/// 非文本内容块在结果里的占位。
const NON_TEXT: &str = "[非文本内容]";

const CLIENT_NAME: &str = "ik-server";

/// 一次调用要连的服务。
#[derive(Debug, Clone, Copy)]
pub struct Endpoint<'a> {
    pub url: &'a str,
    /// 整行请求头（`Name: value`），空串表示不带。
    pub auth_header: &'a str,
    /// 单次 HTTP 请求的超时。
    pub timeout: Duration,
}

impl Endpoint<'_> {
    fn key(&self) -> String {
        format!("{}\n{}", self.url, self.auth_header)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    #[error("连接失败：{0}")]
    Transport(String),
    #[error("超时")]
    Timeout,
    #[error("HTTP {0}")]
    Status(u16),
    #[error("响应超过 1 MiB")]
    TooLarge,
    #[error("协议错误：{0}")]
    Protocol(String),
    #[error("服务端错误 {code}：{message}")]
    Rpc { code: i64, message: String },
    #[error("鉴权头格式不对，应为「名字: 值」")]
    BadAuthHeader,
    #[error("暂时不可用（连续失败，熔断中）")]
    CircuitOpen,
}

/// 远端的一个工具（`tools/list` 的一项，已规整）。
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteTool {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    /// `annotations.readOnlyHint == true`。
    pub read_only: bool,
}

impl RemoteTool {
    /// 入库与比较摘要用的定义（键名固定）。
    pub fn definition(&self) -> Value {
        json!({
            "title": self.title,
            "description": self.description,
            "inputSchema": self.input_schema,
            "readOnly": self.read_only,
        })
    }
}

/// `tools/call` 的结果（只取文本）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallOutput {
    pub text: String,
    pub is_error: bool,
}

#[derive(Debug, Clone)]
struct Session {
    id: Option<String>,
    version: String,
}

#[derive(Debug, Default, Clone, Copy)]
struct Health {
    failures: u32,
    open_until: Option<Instant>,
}

/// 进程内共享：HTTP 连接池、会话缓存、熔断计数。
#[derive(Clone)]
pub struct McpClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: Result<reqwest13::Client, String>,
    next_id: AtomicU64,
    sessions: Mutex<HashMap<String, Session>>,
    health: Mutex<HashMap<String, Health>>,
}

static SHARED: LazyLock<McpClient> = LazyLock::new(McpClient::build);

impl Default for McpClient {
    fn default() -> Self {
        SHARED.clone()
    }
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient").finish_non_exhaustive()
    }
}

impl McpClient {
    /// 新建一个独立的客户端（测试用；生产用 [`Default`] 共享的那个）。
    pub fn build() -> Self {
        let http = crate::llm::client_builder().and_then(|b| {
            b.redirect(reqwest13::redirect::Policy::none())
                .build()
                .map_err(|e| format!("http client: {e}"))
        });
        Self {
            inner: Arc::new(Inner {
                http,
                next_id: AtomicU64::new(1),
                sessions: Mutex::new(HashMap::new()),
                health: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// 熔断中返回 false。
    pub fn available(&self, url: &str) -> bool {
        let mut health = self.inner.health.lock();
        let Some(h) = health.get_mut(url) else {
            return true;
        };
        match h.open_until {
            Some(until) if Instant::now() < until => false,
            Some(_) => {
                // 冷却结束：放行，再失败一次就重新熔断。
                h.open_until = None;
                h.failures = BREAKER_FAILURES.saturating_sub(1);
                true
            }
            None => true,
        }
    }

    fn record(&self, url: &str, ok: bool) {
        let mut health = self.inner.health.lock();
        if ok {
            health.remove(url);
            return;
        }
        let h = health.entry(url.to_owned()).or_default();
        h.failures = h.failures.saturating_add(1);
        if h.failures >= BREAKER_FAILURES {
            h.open_until = Some(Instant::now() + BREAKER_COOLDOWN);
            tracing::warn!(url, "MCP 服务连续失败，熔断 60 秒");
        }
    }

    /// 拉取工具清单（含翻页）。管理后台与定时任务用，不受熔断限制。
    pub async fn list_tools(&self, endpoint: Endpoint<'_>) -> Result<Vec<RemoteTool>, McpError> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_LIST_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let result = self.request(endpoint, "tools/list", params).await?;
            let page: ListPage = serde_json::from_value(result)
                .map_err(|e| McpError::Protocol(format!("tools/list 结果：{e}")))?;
            out.extend(page.tools.into_iter().map(RemoteTool::from));
            if out.len() >= MAX_LISTED_TOOLS {
                out.truncate(MAX_LISTED_TOOLS);
                break;
            }
            match page.next_cursor.filter(|c| !c.is_empty()) {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        Ok(out)
    }

    /// 调用一个工具。网络、协议错误计入熔断；`isError = true` 是工具自己的结果，不计。
    pub async fn call_tool(
        &self,
        endpoint: Endpoint<'_>,
        name: &str,
        arguments: Value,
    ) -> Result<CallOutput, McpError> {
        if !self.available(endpoint.url) {
            return Err(McpError::CircuitOpen);
        }
        let result = self
            .request(
                endpoint,
                "tools/call",
                json!({ "name": name, "arguments": arguments }),
            )
            .await;
        self.record(endpoint.url, result.is_ok());
        let result: CallResult = serde_json::from_value(result?)
            .map_err(|e| McpError::Protocol(format!("tools/call 结果：{e}")))?;
        Ok(result.into_output())
    }

    /// 发一个请求；会话过期（404）时重新握手一次。
    async fn request(
        &self,
        endpoint: Endpoint<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, McpError> {
        let session = self.session(endpoint).await?;
        match self.post(endpoint, Some(&session), method, &params).await {
            Err(McpError::Status(404)) if session.id.is_some() => {
                self.inner.sessions.lock().remove(&endpoint.key());
                let session = self.session(endpoint).await?;
                self.post(endpoint, Some(&session), method, &params)
                    .await
                    .map(|(v, _)| v)
            }
            other => other.map(|(v, _)| v),
        }
    }

    async fn session(&self, endpoint: Endpoint<'_>) -> Result<Session, McpError> {
        let key = endpoint.key();
        if let Some(session) = self.inner.sessions.lock().get(&key).cloned() {
            return Ok(session);
        }
        let params = json!({
            "protocolVersion": PROTOCOL_VERSIONS[0],
            "capabilities": {},
            "clientInfo": { "name": CLIENT_NAME, "version": env!("CARGO_PKG_VERSION") },
        });
        let (result, session_id) = self.post(endpoint, None, "initialize", &params).await?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if !PROTOCOL_VERSIONS.contains(&version.as_str()) {
            return Err(McpError::Protocol(format!("不支持的协议版本「{version}」")));
        }
        let session = Session {
            id: session_id,
            version,
        };
        self.notify(endpoint, &session, "notifications/initialized")
            .await?;
        self.inner.sessions.lock().insert(key, session.clone());
        Ok(session)
    }

    fn http(&self) -> Result<&reqwest13::Client, McpError> {
        self.inner
            .http
            .as_ref()
            .map_err(|e| McpError::Transport(e.clone()))
    }

    fn headers(
        endpoint: Endpoint<'_>,
        session: Option<&Session>,
    ) -> Result<http::HeaderMap, McpError> {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(
            header::ACCEPT,
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        if let Some(session) = session {
            if let Ok(v) = HeaderValue::from_str(&session.version) {
                headers.insert("mcp-protocol-version", v);
            }
            if let Some(id) = session
                .id
                .as_deref()
                .and_then(|id| HeaderValue::from_str(id).ok())
            {
                headers.insert("mcp-session-id", id);
            }
        }
        if let Some((name, value)) = parse_auth_header(endpoint.auth_header)? {
            headers.insert(name, value);
        }
        Ok(headers)
    }

    async fn notify(
        &self,
        endpoint: Endpoint<'_>,
        session: &Session,
        method: &str,
    ) -> Result<(), McpError> {
        let body = json!({ "jsonrpc": "2.0", "method": method });
        let response = self
            .http()?
            .post(endpoint.url)
            .headers(Self::headers(endpoint, Some(session))?)
            .timeout(endpoint.timeout)
            .json(&body)
            .send()
            .await
            .map_err(transport)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(McpError::Status(status.as_u16()))
        }
    }

    /// 发一个 JSON-RPC 请求，返回 `result` 与响应头里的会话 id。
    async fn post(
        &self,
        endpoint: Endpoint<'_>,
        session: Option<&Session>,
        method: &str,
        params: &Value,
    ) -> Result<(Value, Option<String>), McpError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let send = async {
            let mut response = self
                .http()?
                .post(endpoint.url)
                .headers(Self::headers(endpoint, session)?)
                .json(&body)
                .send()
                .await
                .map_err(transport)?;
            let status = response.status();
            if status != StatusCode::OK {
                return Err(McpError::Status(status.as_u16()));
            }
            let session_id = response
                .headers()
                .get("mcp-session-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            let is_sse = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.starts_with("text/event-stream"));
            let message = if is_sse {
                read_sse(&mut response, id).await?
            } else {
                let bytes = read_capped(&mut response).await?;
                serde_json::from_slice(&bytes)
                    .map_err(|e| McpError::Protocol(format!("响应不是 JSON：{e}")))?
            };
            Ok((into_result(message, id)?, session_id))
        };
        tokio::time::timeout(endpoint.timeout, send)
            .await
            .map_err(|_| McpError::Timeout)?
    }
}

fn transport(error: reqwest13::Error) -> McpError {
    if error.is_timeout() {
        McpError::Timeout
    } else {
        // 不带地址：错误文本会进管理后台。
        McpError::Transport(error.without_url().to_string())
    }
}

/// `Name: value` → 请求头。空串为 None。
pub fn parse_auth_header(raw: &str) -> Result<Option<(HeaderName, HeaderValue)>, McpError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let (name, value) = raw.split_once(':').ok_or(McpError::BadAuthHeader)?;
    let name =
        HeaderName::from_bytes(name.trim().as_bytes()).map_err(|_| McpError::BadAuthHeader)?;
    let mut value = HeaderValue::from_str(value.trim()).map_err(|_| McpError::BadAuthHeader)?;
    if value.is_empty() {
        return Err(McpError::BadAuthHeader);
    }
    value.set_sensitive(true);
    Ok(Some((name, value)))
}

async fn read_capped(response: &mut reqwest13::Response) -> Result<Vec<u8>, McpError> {
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if out.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(McpError::TooLarge);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// 读 SSE 流，直到出现 id 相同的响应；其他消息（服务端请求、通知、别的 id）跳过。
async fn read_sse(response: &mut reqwest13::Response, id: u64) -> Result<Value, McpError> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut total = 0usize;
    let mut parser = SseParser::default();
    loop {
        let Some(chunk) = response.chunk().await.map_err(transport)? else {
            // 流结束：最后一个事件可能没有空行收尾。
            for data in parser.finish(&buffer) {
                if let Some(message) = matching(&data, id) {
                    return Ok(message);
                }
            }
            return Err(McpError::Protocol("事件流结束时没有收到响应".to_owned()));
        };
        total += chunk.len();
        if total > MAX_BODY_BYTES {
            return Err(McpError::TooLarge);
        }
        buffer.extend_from_slice(&chunk);
        let consumed = parser.feed(&buffer);
        let events = parser.take();
        buffer.drain(..consumed);
        for data in events {
            if let Some(message) = matching(&data, id) {
                return Ok(message);
            }
        }
    }
}

fn matching(data: &str, id: u64) -> Option<Value> {
    let value: Value = serde_json::from_str(data).ok()?;
    let candidates = match value {
        Value::Array(items) => items,
        other => vec![other],
    };
    candidates
        .into_iter()
        .find(|m| m.get("method").is_none() && m.get("id").and_then(Value::as_u64) == Some(id))
}

/// 按行切 SSE（兼容 `\r\n`），空行结束一个事件，多行 `data:` 用 `\n` 拼接。
#[derive(Default)]
struct SseParser {
    data: Vec<String>,
    done: Vec<String>,
}

impl SseParser {
    /// 处理完整的行，返回消耗的字节数（不完整的末行留在缓冲里）。
    fn feed(&mut self, buffer: &[u8]) -> usize {
        let mut consumed = 0;
        while let Some(pos) = buffer[consumed..].iter().position(|&b| b == b'\n') {
            let line = &buffer[consumed..consumed + pos];
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            self.line(&String::from_utf8_lossy(line));
            consumed += pos + 1;
        }
        consumed
    }

    fn line(&mut self, line: &str) {
        if line.is_empty() {
            if !self.data.is_empty() {
                self.done.push(self.data.join("\n"));
                self.data.clear();
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            self.data
                .push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
        }
        // event:、id:、retry:、注释行都不需要。
    }

    fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.done)
    }

    fn finish(&mut self, rest: &[u8]) -> Vec<String> {
        if !rest.is_empty() {
            self.line(&String::from_utf8_lossy(rest));
        }
        self.line("");
        self.take()
    }
}

fn into_result(message: Value, id: u64) -> Result<Value, McpError> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(default)]
        id: Option<Value>,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<RpcError>,
    }
    #[derive(Deserialize)]
    struct RpcError {
        code: i64,
        #[serde(default)]
        message: String,
    }
    let envelope: Envelope = serde_json::from_value(message)
        .map_err(|e| McpError::Protocol(format!("不是 JSON-RPC 响应：{e}")))?;
    if let Some(error) = envelope.error {
        return Err(McpError::Rpc {
            code: error.code,
            message: error.message.chars().take(200).collect(),
        });
    }
    if envelope.id.as_ref().and_then(Value::as_u64) != Some(id) {
        return Err(McpError::Protocol("响应 id 不匹配".to_owned()));
    }
    envelope
        .result
        .ok_or_else(|| McpError::Protocol("响应没有 result".to_owned()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListPage {
    #[serde(default)]
    tools: Vec<ListedTool>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedTool {
    name: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    input_schema: Option<Value>,
    #[serde(default)]
    annotations: Option<Annotations>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Annotations {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    read_only_hint: Option<bool>,
}

impl From<ListedTool> for RemoteTool {
    fn from(tool: ListedTool) -> Self {
        let annotations = tool.annotations;
        let title = tool
            .title
            .or_else(|| annotations.as_ref().and_then(|a| a.title.clone()))
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty());
        let input_schema = match tool.input_schema {
            Some(Value::Object(map)) => Value::Object(map),
            _ => json!({ "type": "object", "properties": {} }),
        };
        Self {
            name: tool.name,
            title,
            description: tool.description.unwrap_or_default(),
            input_schema,
            read_only: annotations.and_then(|a| a.read_only_hint) == Some(true),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallResult {
    #[serde(default)]
    content: Vec<Value>,
    #[serde(default)]
    structured_content: Option<Value>,
    #[serde(default)]
    is_error: bool,
}

impl CallResult {
    /// 文本块按顺序用换行拼起来；图片、音频、资源等记一个占位。没有内容块时用结构化结果的 JSON。
    fn into_output(self) -> CallOutput {
        let mut parts: Vec<String> = Vec::new();
        for block in &self.content {
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        parts.push(text.to_owned());
                    }
                }
                _ => {
                    if parts.last().map(String::as_str) != Some(NON_TEXT) {
                        parts.push(NON_TEXT.to_owned());
                    }
                }
            }
        }
        if parts.is_empty()
            && let Some(structured) = &self.structured_content
        {
            parts.push(structured.to_string());
        }
        CallOutput {
            text: parts.join("\n"),
            is_error: self.is_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_parser_handles_crlf_multiline_and_split_chunks() {
        let mut p = SseParser::default();
        let mut buf = b"event: message\r\ndata: {\"a\":\r\n".to_vec();
        let used = p.feed(&buf);
        buf.drain(..used);
        assert!(p.take().is_empty());
        buf.extend_from_slice(b"data: 1}\r\n\r\n: comment\ndata: x");
        let used = p.feed(&buf);
        buf.drain(..used);
        assert_eq!(p.take(), vec!["{\"a\":\n1}".to_owned()]);
        assert_eq!(p.finish(&buf), vec!["x".to_owned()]);
    }

    #[test]
    fn matching_skips_requests_and_other_ids() {
        assert!(matching(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#, 3).is_none());
        assert!(matching(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#, 3).is_none());
        assert!(matching(r#"[{"jsonrpc":"2.0","id":3,"result":{}}]"#, 3).is_some());
    }

    #[test]
    fn auth_header_parses_name_and_value() {
        assert!(parse_auth_header("  ").unwrap().is_none());
        let (name, value) = parse_auth_header("Authorization: Bearer abc")
            .unwrap()
            .unwrap();
        assert_eq!(name, header::AUTHORIZATION);
        assert_eq!(value, "Bearer abc");
        assert!(value.is_sensitive());
        assert_eq!(parse_auth_header("token"), Err(McpError::BadAuthHeader));
        assert_eq!(parse_auth_header("X-Key:"), Err(McpError::BadAuthHeader));
        assert_eq!(
            parse_auth_header("bad name: v"),
            Err(McpError::BadAuthHeader)
        );
    }

    #[test]
    fn listed_tools_normalize_title_schema_and_read_only() {
        let listed: ListedTool = serde_json::from_value(json!({
            "name": "search",
            "annotations": { "title": " 检索 ", "readOnlyHint": true }
        }))
        .unwrap();
        let tool = RemoteTool::from(listed);
        assert_eq!(tool.title.as_deref(), Some("检索"));
        assert_eq!(tool.input_schema["type"], "object");
        assert!(tool.read_only);
        let listed: ListedTool =
            serde_json::from_value(json!({ "name": "w", "annotations": {} })).unwrap();
        assert!(!RemoteTool::from(listed).read_only);
    }

    #[test]
    fn call_results_keep_text_and_mark_other_blocks() {
        let result: CallResult = serde_json::from_value(json!({
            "content": [
                { "type": "text", "text": "a" },
                { "type": "image", "data": "..." },
                { "type": "audio", "data": "..." },
                { "type": "text", "text": "b" }
            ],
            "isError": true
        }))
        .unwrap();
        assert_eq!(
            result.into_output(),
            CallOutput {
                text: "a\n[非文本内容]\nb".into(),
                is_error: true
            }
        );
        let result: CallResult =
            serde_json::from_value(json!({ "content": [], "structuredContent": { "n": 1 } }))
                .unwrap();
        assert_eq!(result.into_output().text, r#"{"n":1}"#);
    }

    #[test]
    fn breaker_opens_after_repeated_failures() {
        let client = McpClient::build();
        for _ in 0..BREAKER_FAILURES - 1 {
            client.record("u", false);
        }
        assert!(client.available("u"));
        client.record("u", false);
        assert!(!client.available("u"));
        assert!(client.available("other"));
        client.record("u", true);
        assert!(client.available("u"));
    }
}
