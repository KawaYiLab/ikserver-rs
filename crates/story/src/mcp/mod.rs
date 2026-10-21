//! MCP（JSON-RPC 2.0）协议层：与传输无关，输入一段请求字节，输出一段响应字节（通知则无输出）。
//!
//! 服务是无状态、只读的：不发服务端请求、不维持会话，任何实例都能处理任何请求；
//! 工具清单与提示清单在启动时序列化一次，之后每次 `tools/list` 只是拼接字节。

pub mod prompts;
pub mod tools;

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Value, json};

use crate::corpus::{Corpus, DocKind};
use crate::text::thousands;

/// 支持的协议版本，新的在前。客户端请求的版本在列表内就照单回应，否则回应最新版本。
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub const SERVER_NAME: &str = "绳网档案";

const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

pub struct Server {
    tools_list: Box<RawValue>,
    prompts_list: Box<RawValue>,
}

#[derive(Deserialize)]
struct Incoming<'a> {
    #[serde(default)]
    jsonrpc: Option<&'a str>,
    #[serde(default, borrow)]
    id: Option<&'a RawValue>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default, borrow)]
    params: Option<&'a RawValue>,
}

#[derive(Serialize)]
struct Outgoing<'a> {
    jsonrpc: &'static str,
    id: &'a RawValue,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a RawValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    code: i32,
    message: String,
}

impl RpcError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Serialize)]
struct TextContent<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    text: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CallToolResult<'a> {
    content: [TextContent<'a>; 1],
    is_error: bool,
}

fn null_id() -> &'static RawValue {
    static NULL: std::sync::OnceLock<Box<RawValue>> = std::sync::OnceLock::new();
    // `Box<RawValue>` 的默认值就是 `null`。
    NULL.get_or_init(Box::default)
}

fn to_raw<T: Serialize>(value: &T) -> Result<Box<RawValue>, RpcError> {
    serde_json::value::to_raw_value(value)
        .map_err(|e| RpcError::new(-32603, format!("序列化失败: {e}")))
}

impl Server {
    pub fn new() -> Result<Self, serde_json::Error> {
        Ok(Self {
            tools_list: serde_json::value::to_raw_value(&tools::definitions())?,
            prompts_list: serde_json::value::to_raw_value(&prompts::list())?,
        })
    }

    /// 处理一条消息（单个请求、通知或批量数组）。返回 `None` 表示无需响应。
    pub fn handle(&self, corpus: &Corpus, body: &[u8]) -> Option<Vec<u8>> {
        let trimmed = body.trim_ascii_start();
        if trimmed.first() == Some(&b'[') {
            let items: Vec<&RawValue> = match serde_json::from_slice(trimmed) {
                Ok(v) => v,
                Err(e) => {
                    return Some(error_bytes(
                        null_id(),
                        RpcError::new(PARSE_ERROR, e.to_string()),
                    ));
                }
            };
            if items.is_empty() {
                return Some(error_bytes(
                    null_id(),
                    RpcError::new(INVALID_REQUEST, "空的批量请求"),
                ));
            }
            let parts: Vec<Vec<u8>> = items
                .iter()
                .filter_map(|raw| self.handle_one(corpus, raw.get().as_bytes()))
                .collect();
            if parts.is_empty() {
                return None;
            }
            let mut out =
                Vec::with_capacity(parts.iter().map(Vec::len).sum::<usize>() + parts.len() + 2);
            out.push(b'[');
            for (i, p) in parts.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(p);
            }
            out.push(b']');
            return Some(out);
        }
        self.handle_one(corpus, trimmed)
    }

    fn handle_one(&self, corpus: &Corpus, body: &[u8]) -> Option<Vec<u8>> {
        let msg: Incoming<'_> = match serde_json::from_slice(body) {
            Ok(m) => m,
            Err(e) => {
                let code = if serde_json::from_slice::<serde::de::IgnoredAny>(body).is_ok() {
                    INVALID_REQUEST
                } else {
                    PARSE_ERROR
                };
                return Some(error_bytes(null_id(), RpcError::new(code, e.to_string())));
            }
        };
        let Some(method) = msg.method.as_deref() else {
            // 客户端对服务端请求的响应：本服务从不发请求，忽略。
            return match msg.id {
                Some(_) => None,
                None => Some(error_bytes(
                    null_id(),
                    RpcError::new(INVALID_REQUEST, "缺少 method"),
                )),
            };
        };
        let Some(id) = msg.id else {
            // 通知：notifications/initialized、notifications/cancelled 等，无需响应。
            tracing::debug!(method, "notification");
            return None;
        };
        if msg.jsonrpc.is_some_and(|v| v != "2.0") {
            return Some(error_bytes(
                id,
                RpcError::new(INVALID_REQUEST, "jsonrpc 必须是 \"2.0\""),
            ));
        }
        let started = Instant::now();
        let result = self.dispatch(corpus, method, msg.params);
        tracing::debug!(
            method,
            elapsed_us = started.elapsed().as_micros() as u64,
            "request"
        );
        Some(match result {
            Ok(raw) => result_bytes(id, &raw),
            Err(e) => error_bytes(id, e),
        })
    }

    fn dispatch(
        &self,
        corpus: &Corpus,
        method: &str,
        params: Option<&RawValue>,
    ) -> Result<Box<RawValue>, RpcError> {
        match method {
            "initialize" => initialize(corpus, params),
            "ping" | "logging/setLevel" => to_raw(&json!({})),
            "tools/list" => Ok(self.tools_list.clone()),
            "tools/call" => call_tool(corpus, params),
            "prompts/list" => Ok(self.prompts_list.clone()),
            "prompts/get" => {
                let p: prompts::GetParams = parse_params(params)?;
                let res = prompts::get(&p).map_err(|m| RpcError::new(INVALID_PARAMS, m))?;
                to_raw(&res)
            }
            "resources/list" => to_raw(&json!({ "resources": [] })),
            "resources/templates/list" => to_raw(&json!({ "resourceTemplates": [] })),
            _ => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("未知方法: {method}"),
            )),
        }
    }
}

fn initialize(corpus: &Corpus, params: Option<&RawValue>) -> Result<Box<RawValue>, RpcError> {
    #[derive(Deserialize, Default)]
    #[serde(rename_all = "camelCase")]
    struct InitParams {
        #[serde(default)]
        protocol_version: String,
    }
    let p: InitParams = parse_params(params).unwrap_or_default();
    let version = PROTOCOL_VERSIONS
        .iter()
        .find(|v| **v == p.protocol_version)
        .unwrap_or(&PROTOCOL_VERSIONS[0]);
    to_raw(&json!({
        "protocolVersion": version,
        "capabilities": {
            "tools": { "listChanged": false },
            "prompts": { "listChanged": false },
            "resources": { "subscribe": false, "listChanged": false },
            "logging": {}
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "title": "绳网档案",
            "version": env!("CARGO_PKG_VERSION")
        },
        "instructions": instructions(corpus)
    }))
}

pub fn instructions(c: &Corpus) -> String {
    let story_docs = c.docs.iter().filter(|d| d.kind == DocKind::Story).count();
    let lore_docs = c.docs.iter().filter(|d| d.kind == DocKind::Lore).count();
    let version = if c.meta.source_version.is_empty() {
        "未知"
    } else {
        &c.meta.source_version
    };
    format!(
        "绳网档案（绝区零 Zenless Zone Zero 剧情与世界观知识库），数据版本 {version}（{}），主线 {} 场景、设定 {} 篇。\
             涉及绝区零的剧情、人物、阵营、地点、事件、台词出处时，先用 ik_search 检索、再用 ik_read 读原文确认，不要凭记忆作答。\
             总结整章时先 ik_catalog(path=\"ch:…\") 规划，再 ik_read(docId=\"ch:…\", follow=true) 顺读并分层压缩；\
             角色相关先用 ik_character。较大的总结/考据任务开始前可读一次 ik_guide(mode=summary|character|timeline|verify)。\
             docId 必须原样使用工具返回的值。提取模式：{}；没有说话人的行不要臆测说话人。\
             多词检索为同篇 AND，不会静默降级；allowPartial=true 才允许部分匹配。\
             目录排列未证实为剧情先后，follow 只保证目录覆盖；未关联的 Comic 不可按相邻位置拼接。",
        c.meta.language,
        thousands(story_docs),
        thousands(lore_docs),
        match c.meta.accuracy_first {
            Some(true) => "完整性校对（双方分支候选保留）",
            Some(false) => "男主分支筛选（默认哲视角）",
            None => "旧语料未声明",
        }
    )
}

fn call_tool(corpus: &Corpus, params: Option<&RawValue>) -> Result<Box<RawValue>, RpcError> {
    #[derive(Deserialize)]
    struct CallParams {
        name: String,
        #[serde(default)]
        arguments: Option<Value>,
    }
    let p: CallParams = parse_params(params)?;
    let started = Instant::now();
    let outcome = tools::call(corpus, &p.name, p.arguments)
        .map_err(|_| RpcError::new(INVALID_PARAMS, format!("未知工具: {}", p.name)))?;
    let (text, is_error) = match &outcome {
        Ok(text) => (text.as_str(), false),
        Err(e) => (e.0.as_str(), true),
    };
    tracing::info!(
        tool = %p.name,
        elapsed_us = started.elapsed().as_micros() as u64,
        chars = text.len(),
        is_error,
        "tools/call"
    );
    to_raw(&CallToolResult {
        content: [TextContent { kind: "text", text }],
        is_error,
    })
}

fn parse_params<T: for<'de> Deserialize<'de>>(params: Option<&RawValue>) -> Result<T, RpcError> {
    let raw = params.map_or("{}", RawValue::get);
    serde_json::from_str(raw).map_err(|e| RpcError::new(INVALID_PARAMS, format!("参数错误: {e}")))
}

fn result_bytes(id: &RawValue, result: &RawValue) -> Vec<u8> {
    let msg = Outgoing {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    };
    serde_json::to_vec(&msg).unwrap_or_default()
}

fn error_bytes(id: &RawValue, error: RpcError) -> Vec<u8> {
    let msg = Outgoing {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(error),
    };
    serde_json::to_vec(&msg).unwrap_or_default()
}
