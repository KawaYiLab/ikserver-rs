//! `/v1/story/mcp` 的报文形状，只用于 OpenAPI 文档；处理时按原始字节解析（支持批量数组）。

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// JSON-RPC 2.0 请求或通知（MCP）。也可以是这些对象组成的数组（批量）。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct McpRequest {
    /// 固定为 `"2.0"`。
    #[schema(example = "2.0")]
    pub jsonrpc: String,
    /// 请求 id（字符串或数字）；没有 id 的是通知，服务端回 202 且无响应体。
    #[schema(value_type = Object)]
    pub id: Option<serde_json::Value>,
    /// `initialize`、`tools/list`、`tools/call`、`prompts/list`、`prompts/get`、`ping` 等。
    #[schema(example = "tools/call")]
    pub method: String,
    #[schema(value_type = Object, example = json!({"name": "zzz_search", "arguments": {"keyword": "零号空洞"}}))]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC 2.0 响应：`result` 与 `error` 二选一。
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct McpResponse {
    #[schema(example = "2.0")]
    pub jsonrpc: String,
    #[schema(value_type = Object)]
    pub id: serde_json::Value,
    #[schema(value_type = Object)]
    pub result: Option<serde_json::Value>,
    #[schema(value_type = Object)]
    pub error: Option<serde_json::Value>,
}
