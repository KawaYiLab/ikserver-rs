//! 站外 MCP 服务的管理（ADR 0004）：服务的增删改、拉取工具清单、工具审批、定时重拉。
//!
//! 工具不自动生效（防「工具投毒」，ADR 0004 第 3.2 节）：拉到的定义存在 `remote`，管理员确认后
//! 复制到 `approved`，运行时只用 `approved`；远端再改了定义（摘要不同）就不再提供，等管理员重新确认。
//! 拉取清单的网络请求在事务之外（规则 6）；写入时用咨询锁串行化（暴露名跨服务唯一，先查冲突再写）。

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::LazyLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ik_auth::Operator;
use ik_auth::admin::{self, Audit};
use ik_kernel::Problem;
use ik_kernel::jobs::{IntoTaskHandlerResult, TaskHandler, WorkerContext};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;

use crate::AiState;
use crate::mcp_client::{self, Endpoint, RemoteTool};
use crate::repo::{self, McpServerRow, McpToolRow};

/// 暴露给模型的名字的上限（OpenAI 兼容接口：`^[a-zA-Z0-9_-]{1,64}$`）。
const EXPOSED_MAX: usize = 64;
/// 时间线文案上限。
const TIMELINE_MAX_CHARS: usize = 32;
/// 拉取清单的超时（管理后台与定时任务）。
const LIST_TIMEOUT: Duration = Duration::from_secs(20);
/// 记进 `last_error` 的错误文本上限。
const ERROR_MAX_CHARS: usize = 200;

// ---------- 名字与摘要 ----------

/// `<服务名>__<工具名>`：工具名里的非法字符换成 `_`，整体截到 64 字符。
pub fn exposed_name(server: &str, tool: &str) -> String {
    let clean: String = tool
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{server}__{clean}")
        .chars()
        .take(EXPOSED_MAX)
        .collect()
}

/// 定义的摘要：键排序后的紧凑 JSON 的 SHA-256（与 serde_json 是否保留键顺序无关）。
pub fn digest(definition: &Value) -> Vec<u8> {
    let mut out = String::new();
    canonical(definition, &mut out);
    Sha256::digest(out.as_bytes()).to_vec()
}

fn canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                if let Some(v) = map.get(key) {
                    canonical(v, out);
                }
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------- 视图与输入 ----------

/// 一个工具在后台的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpToolStatus {
    /// 启用且远端定义与确认过的一致：会提供给模型。
    Active,
    /// 没启用。
    Disabled,
    /// 启用了，但远端定义变了：停止提供，等管理员重新确认。
    Changed,
    /// 远端清单里已经没有它了。
    Missing,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminMcpTool {
    pub remote_name: String,
    /// 暴露给模型的名字。
    pub exposed_name: String,
    pub status: McpToolStatus,
    pub enabled: bool,
    /// 远端最近一次的定义（`approved` 与它不同时，启用前请先看清楚）。
    pub title: Option<String>,
    pub description: String,
    #[schema(value_type = Object)]
    pub input_schema: Value,
    /// `annotations.readOnlyHint = true`。只有只读工具能启用。
    pub read_only: bool,
    /// 远端定义的摘要（十六进制）。启用时原样带回，确认的是这一版。
    pub digest: String,
    /// 确认过的定义与远端不同（或从未确认过）。
    pub needs_review: bool,
    pub timeline_text: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AdminMcpServer {
    pub name: String,
    pub display_name: String,
    pub url: String,
    /// 鉴权头的提示：名字加值的末尾几位（如 `Authorization: …a1b2`），没有时为 null。值本身不回传。
    pub auth_header_hint: Option<String>,
    pub timeout_ms: i32,
    pub max_result_chars: i32,
    pub enabled: bool,
    pub sort_order: i32,
    pub last_listed_at: Option<DateTime<Utc>>,
    /// 最近一次拉取清单失败的原因。
    pub last_error: Option<String>,
    pub tools: Vec<AdminMcpTool>,
    /// 启用了它的角色卡（slug）。
    pub characters: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerInput {
    /// 小写字母、数字、下划线、连字符，1–32 字符；是工具名的前缀。
    pub name: String,
    pub display_name: String,
    /// Streamable HTTP 端点。`https://`；`http://` 只允许回环与内网地址。
    pub url: String,
    /// 整行请求头，如 `Authorization: Bearer xxx`。不传（或 null）保留原有的；空串表示不带。
    #[serde(default)]
    pub auth_header: Option<String>,
    /// 单次调用超时，1000–60000 毫秒。
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: i32,
    /// 结果截断上限，500–16000 字符。
    #[serde(default = "default_max_result_chars")]
    pub max_result_chars: i32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub sort_order: i32,
}

fn default_timeout_ms() -> i32 {
    15_000
}

fn default_max_result_chars() -> i32 {
    4_000
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpToolInput {
    pub enabled: bool,
    /// 启用时必填：后台看到的远端定义摘要（[`AdminMcpTool::digest`]）。与当前的不同时拒绝（409
    /// `AI_MCP_TOOL_CHANGED`），避免确认了没看过的版本。
    #[serde(default)]
    pub digest: Option<String>,
    /// 时间线文案（最多 32 字），空时用 MCP 的 title。
    #[serde(default)]
    pub timeline_text: Option<String>,
}

/// 拉取清单的结果。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpRefreshResult {
    pub ok: bool,
    pub error: Option<String>,
    /// 新出现的工具（未启用）。
    pub added: Vec<String>,
    /// 定义变了的工具（启用的会停止提供，等重新确认）。
    pub changed: Vec<String>,
    /// 远端已经没有的工具。
    pub missing: Vec<String>,
    /// 没有写入的工具与原因（如暴露名与别的服务冲突）。
    pub skipped: Vec<String>,
    pub server: AdminMcpServer,
}

// ---------- 视图 ----------

/// `Authorization: …a1b2`：只露名字与值的末四位。
fn auth_hint(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (name, value) = raw.split_once(':').unwrap_or(("", raw));
    let chars: Vec<char> = value.trim().chars().collect();
    let tail = if chars.len() < 12 {
        "…".to_owned()
    } else {
        format!("…{}", chars[chars.len() - 4..].iter().collect::<String>())
    };
    Some(format!("{}: {tail}", name.trim()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Definition {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    input_schema: Value,
    #[serde(default)]
    read_only: bool,
}

fn tool_view(row: McpToolRow) -> AdminMcpTool {
    let def: Definition = serde_json::from_value(row.remote).unwrap_or(Definition {
        title: None,
        description: String::new(),
        input_schema: Value::Null,
        read_only: false,
    });
    let reviewed = row.approved_digest.as_deref() == Some(row.remote_digest.as_slice());
    let status = if row.missing {
        McpToolStatus::Missing
    } else if !row.enabled {
        McpToolStatus::Disabled
    } else if reviewed {
        McpToolStatus::Active
    } else {
        McpToolStatus::Changed
    };
    AdminMcpTool {
        remote_name: row.remote_name,
        exposed_name: row.exposed_name,
        status,
        enabled: row.enabled,
        title: def.title,
        description: def.description,
        input_schema: def.input_schema,
        read_only: def.read_only,
        digest: hex(&row.remote_digest),
        needs_review: !reviewed,
        timeline_text: row.timeline_text,
    }
}

fn server_view(
    row: McpServerRow,
    tools: &mut Vec<McpToolRow>,
    cards: &[(String, Vec<i64>)],
) -> AdminMcpServer {
    let id = row.id;
    AdminMcpServer {
        auth_header_hint: auth_hint(&row.auth_header),
        name: row.name,
        display_name: row.display_name,
        url: row.url,
        timeout_ms: row.timeout_ms,
        max_result_chars: row.max_result_chars,
        enabled: row.enabled,
        sort_order: row.sort_order,
        last_listed_at: row.last_listed_at,
        last_error: row.last_error,
        tools: tools
            .extract_if(.., |t| t.server_id == id)
            .map(tool_view)
            .collect(),
        characters: cards
            .iter()
            .filter(|(_, ids)| ids.contains(&id))
            .map(|(slug, _)| slug.clone())
            .collect(),
    }
}

pub async fn servers(state: &AiState) -> Result<Vec<AdminMcpServer>, Problem> {
    let mut conn = state.db.acquire().await?;
    let rows = repo::mcp_servers(&mut conn).await?;
    let mut tools = repo::mcp_tools(&mut conn).await?;
    let cards: Vec<(String, Vec<i64>)> = repo::cards(&mut conn)
        .await?
        .into_iter()
        .map(|c| (c.slug, c.mcp_server_ids))
        .collect();
    Ok(rows
        .into_iter()
        .map(|row| server_view(row, &mut tools, &cards))
        .collect())
}

async fn server(state: &AiState, name: &str) -> Result<AdminMcpServer, Problem> {
    servers(state)
        .await?
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(Problem::not_found)
}

// ---------- 校验 ----------

static SERVER_NAME: LazyLock<Option<regex::Regex>> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z0-9][a-z0-9_-]{0,31}$").ok());

/// `http://` 只允许回环与内网地址（与 Meilisearch 一样走内网的自建服务）；链路本地（云厂商元数据地址）不行。
fn private_host(url: &reqwest13::Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private(),
        Ok(IpAddr::V6(ip)) => ip.is_loopback() || (ip.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => host.eq_ignore_ascii_case("localhost"),
    }
}

/// 校验 MCP 端点地址（管理后台保存时）。
pub fn validate_url(raw: &str) -> Result<(), &'static str> {
    const HINT: &str = "https:// 地址（http:// 只允许回环与内网地址），最多 512 字符，不含账号密码";
    if raw.chars().count() > 512 || raw.chars().any(char::is_whitespace) {
        return Err(HINT);
    }
    let url = reqwest13::Url::parse(raw).map_err(|_| HINT)?;
    if !url.username().is_empty() || url.password().is_some() || url.host_str().is_none() {
        return Err(HINT);
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if private_host(&url) => Ok(()),
        _ => Err(HINT),
    }
}

fn validate_server(input: &mut McpServerInput) -> Result<(), Problem> {
    let field = |name: &str, detail: &str| Problem::field(name, "invalid", detail);
    if !SERVER_NAME
        .as_ref()
        .is_some_and(|re| re.is_match(&input.name))
    {
        return Err(field("name", "小写字母、数字、下划线、连字符，1–32 字符"));
    }
    input.display_name = input.display_name.trim().to_owned();
    if !(1..=64).contains(&input.display_name.chars().count()) {
        return Err(field("displayName", "1–64 字"));
    }
    input.url = input.url.trim().to_owned();
    validate_url(&input.url).map_err(|hint| field("url", hint))?;
    if let Some(header) = input.auth_header.as_mut() {
        *header = header.trim().to_owned();
        if header.chars().count() > 4096 || mcp_client::parse_auth_header(header).is_err() {
            return Err(field(
                "authHeader",
                "整行请求头，如「Authorization: Bearer xxx」，最多 4096 字符",
            ));
        }
    }
    if !(1000..=60_000).contains(&input.timeout_ms) {
        return Err(field("timeoutMs", "1000–60000"));
    }
    if !(500..=16_000).contains(&input.max_result_chars) {
        return Err(field("maxResultChars", "500–16000"));
    }
    Ok(())
}

// ---------- 写入 ----------

/// 新建或修改服务（按名字）。不传鉴权头时保留原有的。
pub async fn save_server(
    state: &AiState,
    op: &Operator,
    name: &str,
    mut input: McpServerInput,
) -> Result<AdminMcpServer, Problem> {
    if input.name != name {
        return Err(Problem::field(
            "name",
            "MISMATCH",
            "请求体里的标识与路径不一致",
        ));
    }
    validate_server(&mut input)?;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    repo::upsert_mcp_server(
        &mut tx,
        &repo::NewMcpServer {
            id: state.ids.next_raw(),
            name: &input.name,
            display_name: &input.display_name,
            url: &input.url,
            auth_header: input.auth_header.as_deref(),
            timeout_ms: input.timeout_ms,
            max_result_chars: input.max_result_chars,
            enabled: input.enabled,
            sort_order: input.sort_order,
            now,
        },
    )
    .await?;
    // 鉴权头不进审计，只记有没有换。
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.mcp_server.save",
            target_type: "ai_mcp_server",
            target_id: Some(name.to_owned()),
            details: serde_json::json!({
                "displayName": input.display_name,
                "url": input.url,
                "enabled": input.enabled,
                "timeoutMs": input.timeout_ms,
                "maxResultChars": input.max_result_chars,
                "authHeaderChanged": input.auth_header.is_some(),
            }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    server(state, name).await
}

/// 删除服务：它的工具一起删掉，角色卡里去掉它。
pub async fn delete_server(state: &AiState, op: &Operator, name: &str) -> Result<(), Problem> {
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    if !repo::delete_mcp_server(&mut tx, name).await? {
        return Err(Problem::not_found());
    }
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.mcp_server.delete",
            target_type: "ai_mcp_server",
            target_id: Some(name.to_owned()),
            details: serde_json::json!({}),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    Ok(())
}

/// 启用 / 停用一个工具、确认当前版本、改时间线文案。
pub async fn update_tool(
    state: &AiState,
    op: &Operator,
    server_name: &str,
    remote_name: &str,
    input: McpToolInput,
) -> Result<AdminMcpTool, Problem> {
    let timeline = input
        .timeline_text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned);
    if timeline
        .as_deref()
        .is_some_and(|t| t.chars().count() > TIMELINE_MAX_CHARS)
    {
        return Err(Problem::field("timelineText", "invalid", "最多 32 字"));
    }
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let server_row = repo::mcp_server(&mut tx, server_name)
        .await?
        .ok_or_else(Problem::not_found)?;
    let row = repo::mcp_tool_for_update(&mut tx, server_row.id, remote_name)
        .await?
        .ok_or_else(Problem::not_found)?;
    if input.enabled {
        if input.digest.as_deref() != Some(hex(&row.remote_digest).as_str()) {
            return Err(Problem::conflict(
                "AI_MCP_TOOL_CHANGED",
                "工具定义已经变了，请刷新后重新确认",
            ));
        }
        if row.remote.get("readOnly").and_then(Value::as_bool) != Some(true) {
            return Err(Problem::conflict(
                "AI_MCP_TOOL_NOT_READ_ONLY",
                "只能启用声明为只读（readOnlyHint）的工具",
            ));
        }
        if row.missing {
            return Err(Problem::conflict(
                "AI_MCP_TOOL_CHANGED",
                "工具定义已经变了，请刷新后重新确认",
            ));
        }
    }
    repo::update_tool(
        &mut tx,
        row.id,
        input.enabled,
        input.enabled,
        timeline.as_deref(),
        now,
    )
    .await?;
    admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "ai.mcp_tool.save",
            target_type: "ai_mcp_tool",
            target_id: Some(row.exposed_name.clone()),
            details: serde_json::json!({
                "enabled": input.enabled,
                "digest": hex(&row.remote_digest),
                "timelineText": timeline,
            }),
        },
        now,
    )
    .await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    server(state, server_name)
        .await?
        .tools
        .into_iter()
        .find(|t| t.remote_name == remote_name)
        .ok_or_else(Problem::internal)
}

// ---------- 拉取清单 ----------

/// 拉取一个服务的工具清单并写入（管理后台「拉取工具」）。连不上时 `ok = false`，原因在 `error`
/// （同时记进服务的 `lastError`）。
pub async fn refresh_server(state: &AiState, name: &str) -> Result<McpRefreshResult, Problem> {
    let row = {
        let mut conn = state.db.acquire().await?;
        repo::mcp_server(&mut conn, name)
            .await?
            .ok_or_else(Problem::not_found)?
    };
    let outcome = refresh(state, &row).await?;
    Ok(McpRefreshResult {
        ok: outcome.error.is_none(),
        error: outcome.error,
        added: outcome.added,
        changed: outcome.changed,
        missing: outcome.missing,
        skipped: outcome.skipped,
        server: server(state, name).await?,
    })
}

#[derive(Debug, Default)]
struct Outcome {
    error: Option<String>,
    added: Vec<String>,
    changed: Vec<String>,
    missing: Vec<String>,
    skipped: Vec<String>,
}

async fn refresh(state: &AiState, server: &McpServerRow) -> Result<Outcome, Problem> {
    let endpoint = Endpoint {
        url: &server.url,
        auth_header: &server.auth_header,
        timeout: LIST_TIMEOUT,
    };
    // 网络请求在事务之外（规则 6）。
    let listed = state.mcp.list_tools(endpoint).await;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    repo::lock_mcp_tools(&mut tx).await?;
    let tools = match listed {
        Ok(tools) => tools,
        Err(error) => {
            let message: String = error.to_string().chars().take(ERROR_MAX_CHARS).collect();
            tracing::warn!(server = %server.name, %message, "拉取 MCP 工具清单失败");
            repo::set_mcp_listed(&mut tx, server.id, Some(&message), now).await?;
            tx.commit().await?;
            return Ok(Outcome {
                error: Some(message),
                ..Outcome::default()
            });
        }
    };
    let existing = repo::mcp_tools(&mut tx).await?;
    let outcome = write_tools(state, &mut tx, server, &existing, &tools, now).await?;
    repo::set_mcp_listed(&mut tx, server.id, None, now).await?;
    tx.commit().await?;
    state.catalog.invalidate().await;
    Ok(outcome)
}

async fn write_tools(
    state: &AiState,
    tx: &mut sqlx::PgConnection,
    server: &McpServerRow,
    existing: &[McpToolRow],
    tools: &[RemoteTool],
    now: DateTime<Utc>,
) -> Result<Outcome, Problem> {
    let mut outcome = Outcome::default();
    // 暴露名 → 占用它的（服务, 工具）。
    let mut taken: HashMap<String, (i64, String)> = existing
        .iter()
        .map(|t| (t.exposed_name.clone(), (t.server_id, t.remote_name.clone())))
        .collect();
    let mine: HashMap<&str, &McpToolRow> = existing
        .iter()
        .filter(|t| t.server_id == server.id)
        .map(|t| (t.remote_name.as_str(), t))
        .collect();
    let mut present: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for tool in tools {
        if tool.name.is_empty() || !seen.insert(tool.name.as_str()) {
            continue;
        }
        let exposed = match mine.get(tool.name.as_str()) {
            Some(row) => row.exposed_name.clone(),
            None => exposed_name(&server.name, &tool.name),
        };
        let owner = (server.id, tool.name.clone());
        if taken.get(&exposed).is_some_and(|o| *o != owner) {
            outcome
                .skipped
                .push(format!("{}：暴露名 {exposed} 已被占用", tool.name));
            continue;
        }
        taken.insert(exposed.clone(), owner);
        let definition = tool.definition();
        let remote_digest = digest(&definition);
        match mine.get(tool.name.as_str()) {
            None => outcome.added.push(tool.name.clone()),
            Some(row) if row.remote_digest != remote_digest => {
                outcome.changed.push(tool.name.clone());
            }
            Some(_) => {}
        }
        repo::upsert_remote_tool(
            tx,
            &repo::RemoteToolUpsert {
                id: state.ids.next_raw(),
                server_id: server.id,
                remote_name: &tool.name,
                exposed_name: &exposed,
                remote: &definition,
                remote_digest: &remote_digest,
                now,
            },
        )
        .await?;
        present.push(tool.name.clone());
    }
    outcome.missing = mine
        .iter()
        .filter(|(name, row)| !row.missing && !present.iter().any(|p| p == *name))
        .map(|(name, _)| (*name).to_owned())
        .collect();
    outcome.missing.sort();
    repo::mark_missing_tools(tx, server.id, &present, now).await?;
    if !outcome.changed.is_empty() {
        tracing::warn!(
            server = %server.name,
            tools = ?outcome.changed,
            "MCP 工具定义变了，已停止提供，等管理员重新确认"
        );
    }
    Ok(outcome)
}

/// 定时重拉所有启用的服务（每小时一次，失败的记进 `lastError`，不重试）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RefreshMcpTools {}

impl TaskHandler for RefreshMcpTools {
    const IDENTIFIER: &'static str = "ai.refresh_mcp_tools";

    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<AiState>() else {
            return Err("执行器没有注册 AiState".to_owned());
        };
        refresh_all(state).await.map_err(|p| format!("{p:?}"))
    }
}

pub async fn refresh_all(state: &AiState) -> Result<(), Problem> {
    let rows = {
        let mut conn = state.db.acquire().await?;
        repo::mcp_servers(&mut conn).await?
    };
    for row in rows.iter().filter(|r| r.enabled) {
        refresh(state, row).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn exposed_names_are_prefixed_sanitized_and_capped() {
        assert_eq!(exposed_name("akasha", "search"), "akasha__search");
        assert_eq!(exposed_name("kb", "docs.search/v2"), "kb__docs_search_v2");
        assert_eq!(exposed_name("kb", "检索"), "kb____");
        let long = exposed_name("a", &"x".repeat(100));
        assert_eq!(long.chars().count(), 64);
    }

    #[test]
    fn digest_ignores_key_order() {
        let a = json!({ "b": 1, "a": { "y": [1, { "q": 1, "p": 2 }], "x": null } });
        let b = json!({ "a": { "x": null, "y": [1, { "p": 2, "q": 1 }] }, "b": 1 });
        assert_eq!(digest(&a), digest(&b));
        assert_ne!(digest(&a), digest(&json!({ "b": 2 })));
        assert_eq!(hex(&[0, 15, 255]), "000fff");
    }

    #[test]
    fn urls_require_https_or_a_private_http_host() {
        assert!(validate_url("https://kb.example.com/mcp").is_ok());
        assert!(validate_url("http://127.0.0.1:8080/mcp").is_ok());
        assert!(validate_url("http://localhost/mcp").is_ok());
        assert!(validate_url("http://10.0.0.5/mcp").is_ok());
        assert!(validate_url("http://[::1]/mcp").is_ok());
        assert!(validate_url("http://kb.example.com/mcp").is_err());
        assert!(validate_url("http://169.254.169.254/").is_err());
        assert!(validate_url("https://user:pw@kb.example.com/").is_err());
        assert!(validate_url("ftp://kb.example.com/").is_err());
        assert!(validate_url("https://kb.example.com/ mcp").is_err());
    }

    #[test]
    fn auth_hints_hide_the_value() {
        assert_eq!(auth_hint(""), None);
        assert_eq!(
            auth_hint("Authorization: Bearer abcdefgh1234"),
            Some("Authorization: …1234".into())
        );
        assert_eq!(auth_hint("X-Key: short"), Some("X-Key: …".into()));
    }
}
