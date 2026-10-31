//! 论坛工具（ai-assistant.md 第 8 节）：7 个只读工具，在进程内执行。
//!
//! 约束（TOOL-001～009）：
//! - 只读、无副作用、只面向公开内容：都以游客身份调用各域的 service（隐藏、未发布的内容天然读不到）；
//! - 查看者（触发者）只来自可信的执行上下文 [`ToolContext`]，绝不从参数里读（TOOL-004）；
//! - 与查看者有拉黑关系的人的实名内容、主页不出现，匿名内容不做拉黑过滤（D-084 方案 A）；
//! - 按用户名找人只做精确匹配（D-085 方案 A），隐藏主页的人当作查无此人（TOOL-005）；
//! - 帖子一律写成 `[标题](/post/<id>)`（TOOL-003）；结果截 4000 字；执行出错只告诉模型
//!   `工具执行失败：<名>`（TOOL-002）。

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use ik_content::dto::{AuthorView, PostCard};
use ik_kernel::Problem;
use regex::Regex;
use serde_json::{Value, json};

use crate::AiState;
use crate::llm::ToolSpec;

/// 工具结果的上限（TOOL-002、LLM-012）。
pub const MAX_RESULT_CHARS: usize = 4000;
/// 评论预览的上限（TOOL-006）。
const COMMENT_PREVIEW_CHARS: usize = 160;
/// 简介的上限。
const BIO_MAX_CHARS: usize = 200;
/// 站内搜索最多返回几条（TOOL-022）。
const SEARCH_RESULTS: usize = 5;
/// 查询扩展最多几个检索词（TOOL-020）。
const MAX_SEARCH_TERMS: usize = 5;
/// 一篇帖子最多附几张图给识图模型：封面在前、正文插图在后（LLM-015）。
pub const POST_IMAGES: usize = 4;

/// Markdown 正文里的图片地址（`![…](http…)`），只认 http(s)。
static BODY_IMAGE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\(\s*<?(https?://[^\s)>]+)").ok());

pub(crate) fn body_image_urls(body: &str) -> Vec<String> {
    BODY_IMAGE.as_ref().map_or_else(Vec::new, |re| {
        re.captures_iter(body)
            .filter_map(|c| c.get(1))
            .map(|m| m.as_str().to_owned())
            .collect()
    })
}

/// 帖子发给识图模型的图片（LLM-015）：封面在前、正文插图在后，去重，最多 [`POST_IMAGES`] 张。
pub(crate) async fn post_images(
    state: &AiState,
    conn: &mut sqlx::PgConnection,
    cover_ids: &[i64],
    body: &str,
) -> Result<Vec<String>, Problem> {
    let covers: Vec<i64> = cover_ids.iter().copied().take(POST_IMAGES).collect();
    let urls = state.media.vision_urls(conn, &covers).await?;
    let mut images: Vec<String> = covers
        .iter()
        .filter_map(|id| urls.get(id).cloned())
        .collect();
    for url in body_image_urls(body) {
        if images.len() >= POST_IMAGES {
            break;
        }
        if !images.contains(&url) {
            images.push(url);
        }
    }
    Ok(images)
}

/// 执行上下文：查看者是触发者（TOOL-004）。
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub viewer_id: Option<i64>,
    /// 与查看者有拉黑关系的人（我拉黑的 ∪ 拉黑我的）。
    pub excluded: HashSet<i64>,
}

impl ToolContext {
    pub async fn new(state: &AiState, viewer_id: i64) -> Result<Self, Problem> {
        let mut conn = state.db.acquire().await?;
        let excluded = ik_social::interop::excluded_authors(&mut conn, Some(viewer_id))
            .await?
            .into_iter()
            .collect();
        Ok(Self {
            viewer_id: Some(viewer_id),
            excluded,
        })
    }

    /// 外部渠道的游客视角，不使用站内账号权限或账号的拉黑关系。
    pub fn guest() -> Self {
        Self {
            viewer_id: None,
            excluded: HashSet::new(),
        }
    }

    /// 实名作者与查看者没有拉黑关系；匿名一律可见（不做拉黑过滤，防反推）。
    fn author_visible(&self, author: &AuthorView) -> bool {
        author.anonymous
            || author
                .id
                .is_none_or(|id| !self.excluded.contains(&id.get()))
    }

    fn user_visible(&self, user_id: i64) -> bool {
        !self.excluded.contains(&user_id)
    }
}

/// 工具结果附带的结构化信息，用来生成工作流事件（WF-008/009）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolMeta {
    None,
    /// 站内搜索。
    Search {
        query: String,
        hits: usize,
        items: Vec<(i64, String)>,
    },
    /// 读了一篇帖子的正文（找到时带 id 与标题）。
    Read {
        post: Option<(i64, String)>,
    },
    /// 读了一篇帖子的评论区。
    Comments {
        post: Option<(i64, String)>,
    },
    /// 记忆工具（MEM-014/015）：记下的那条（id、种类、正文）与忘掉的那些。
    Memory {
        saved: Option<(i64, String, String)>,
        forgotten: Vec<String>,
    },
}

/// 一次工具调用的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// 喂给模型的文本（已截断）。
    pub text: String,
    pub is_error: bool,
    pub meta: ToolMeta,
    /// 结果附带的图片地址（帖子封面与插图，LLM-015）：识图模型在工具结果之后另收一条带图的消息。
    pub images: Vec<String>,
}

impl ToolOutput {
    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self {
            is_error: true,
            ..Self::text(text)
        }
    }

    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
            meta: ToolMeta::None,
            images: Vec::new(),
        }
    }

    fn with(text: impl Into<String>, meta: ToolMeta) -> Self {
        Self {
            text: text.into(),
            is_error: false,
            meta,
            images: Vec::new(),
        }
    }
}

/// 白名单（TOOL-001）。
pub const TOOL_NAMES: [&str; 7] = [
    "search_posts",
    "get_hot_posts",
    "get_user_info",
    "get_post_content",
    "get_post_comments",
    "get_category_list",
    "get_user_activity",
];

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn username_param() -> Value {
    json!({ "type": "string", "description": "用户名（不带 @）" })
}

fn document_id_param() -> Value {
    json!({ "type": "string", "description": "帖子 id（链接 /post/<id> 里的 id）" })
}

fn limit_param(max: u32, default: u32) -> Value {
    json!({ "type": "integer", "minimum": 1, "maximum": max, "default": default })
}

/// 提供给模型的工具清单（只有白名单里的 7 个）。
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: Cow::Borrowed("search_posts"),
            description: Cow::Borrowed(
                "在绳网站内找帖子：query 用短检索词，可用 category 限定公开分区；\
                 只给 category 时浏览分区最新帖子。或只给 seed_post_id 查该帖的相关推荐，\
                 不与 query/category 同用。\
                 结果里的帖子写成 markdown 链接 [标题](/post/id)，引用时必须原样保留该链接。",
            ),
            parameters: object(
                json!({
                    "query": { "type": "string", "description": "检索词，如「攻略、零号空洞」" },
                    "category": { "type": "string", "description": "公开分区的 slug 或名称关键词" },
                    "seed_post_id": document_id_param(),
                    "limit": limit_param(10, 5)
                }),
                &[],
            ),
        },
        ToolSpec {
            name: Cow::Borrowed("get_hot_posts"),
            description: Cow::Borrowed(
                "获取绳网当前的热门帖子。结果里的帖子写成 markdown 链接 [标题](/post/id)，引用时必须原样保留该链接。",
            ),
            parameters: object(json!({ "limit": limit_param(10, 5) }), &[]),
        },
        ToolSpec {
            name: Cow::Borrowed("get_user_info"),
            description: Cow::Borrowed(
                "获取用户的公开资料与统计：简介、等级、丁尼、发帖、获赞、阅读、粉丝与关注数。",
            ),
            parameters: object(json!({ "username": username_param() }), &["username"]),
        },
        ToolSpec {
            name: Cow::Borrowed("get_post_content"),
            description: Cow::Borrowed(
                "读取帖子的正文、图片和互动统计。正文较长时按结果中的 next_offset 续读；上下文已有的正文不必重复读取。",
            ),
            parameters: object(
                json!({
                    "document_id": document_id_param(),
                    "offset": { "type": "integer", "minimum": 0, "default": 0, "description": "正文的 Unicode 字符偏移；续读使用上次返回的 next_offset" }
                }),
                &["document_id"],
            ),
        },
        ToolSpec {
            name: Cow::Borrowed("get_post_comments"),
            description: Cow::Borrowed("按热度读取一篇帖子的评论（带楼层号与回复关系）。"),
            parameters: object(
                json!({ "document_id": document_id_param(), "limit": limit_param(20, 10) }),
                &["document_id"],
            ),
        },
        ToolSpec {
            name: Cow::Borrowed("get_category_list"),
            description: Cow::Borrowed("列出可以公开浏览的分区。"),
            parameters: object(json!({}), &[]),
        },
        ToolSpec {
            name: Cow::Borrowed("get_user_activity"),
            description: Cow::Borrowed(
                "获取用户近期公开的发帖或评论，kind 可选 posts/comments/all，默认 all。结果中的 [标题](/post/id) 链接引用时原样保留。",
            ),
            parameters: object(
                json!({
                    "username": username_param(),
                    "kind": { "type": "string", "enum": ["posts", "comments", "all"], "default": "all" },
                    "limit": limit_param(10, 5)
                }),
                &["username"],
            ),
        },
    ]
}

// ---------- 参数 ----------

/// 参数（TOOL-002）：JSON 对象；是字符串时再按 JSON 解析一次；解析失败按空参数。
pub fn parse_args(raw: &str) -> serde_json::Map<String, Value> {
    let value: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
    let value = match value {
        Value::String(inner) => serde_json::from_str(&inner).unwrap_or(Value::Null),
        other => other,
    };
    match value {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

fn arg_str(args: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match args.get(*k)? {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn arg_limit(args: &serde_json::Map<String, Value>, default: u32, max: u32) -> u32 {
    let raw = match args.get("limit") {
        Some(Value::Number(n)) => n.as_i64(),
        Some(Value::String(s)) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    raw.map_or(default, |n| {
        u32::try_from(n.clamp(1, i64::from(max))).unwrap_or(default)
    })
}

/// 帖子 id：数字或数字字符串；也接受 `/post/<id>` 这样的链接。
fn post_id(args: &serde_json::Map<String, Value>) -> Option<Result<i64, String>> {
    id_arg(args, &["document_id", "documentId", "id", "post_id"])
}

fn id_arg(args: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<Result<i64, String>> {
    let raw = arg_str(args, keys)?;
    let digits = raw.rsplit('/').next().unwrap_or(&raw).trim().to_owned();
    Some(digits.parse::<i64>().map_err(|_| raw))
}

// ---------- 文本 ----------

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// 截断工具结果，并告诉模型后面还有多少（TOOL-015）：免得它把半截结果当成全部。
pub(crate) fn truncate_marked(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_owned();
    }
    let mut out = truncate(text, max);
    out.push_str(&format!(
        "\n…（结果过长，已截断，后面还有约 {} 字）",
        total - max
    ));
    out
}

/// 严格解析工具参数（TOOL-014）：空串当作没有参数；被转义成字符串的对象也接受；其余不是 JSON 对象的
/// 返回给模型的错误说明。
pub(crate) fn strict_args(raw: &str) -> Result<serde_json::Map<String, Value>, String> {
    if raw.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    let value = serde_json::from_str::<Value>(raw)
        .ok()
        .map(|value| match value {
            Value::String(inner) => serde_json::from_str(&inner).unwrap_or(Value::Null),
            other => other,
        });
    match value {
        Some(Value::Object(map)) => Ok(map),
        _ => Err(format!(
            "参数不是合法的 JSON 对象，没有执行。收到的是：{}",
            truncate(raw, 200)
        )),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `[标题](/post/<id>)`：标题里的方括号去掉，空标题为「（无标题）」。
pub fn post_link(id: i64, title: &str) -> String {
    let clean: String = title
        .trim()
        .chars()
        .filter(|c| !matches!(c, '[' | ']' | '\n' | '\r'))
        .collect();
    let title = if clean.is_empty() {
        "（无标题）"
    } else {
        clean.as_str()
    };
    format!("[{title}](/post/{id})")
}

fn title_or_placeholder(title: &str) -> &str {
    if title.trim().is_empty() {
        "（无标题）"
    } else {
        title.trim()
    }
}

fn card_lines_budget(cards: &[&PostCard], budget: usize) -> (String, usize) {
    let mut text = String::new();
    let mut count = 0;
    let mut used = 0;
    for card in cards {
        let category = card.category.as_ref().map_or("未分区", |c| c.name.as_str());
        let mut line = format!(
            "{}. {}\n   分区：{} · 发布：{} UTC",
            count + 1,
            post_link(card.id.get(), &card.title),
            category,
            card.published_at.format("%Y-%m-%d")
        );
        let excerpt = collapse(&card.excerpt);
        if !excerpt.is_empty() {
            line.push_str(&format!("\n   {}", truncate(&excerpt, 120)));
        }
        let cost = line.chars().count() + usize::from(count > 0);
        if used + cost > budget {
            break;
        }
        if count > 0 {
            text.push('\n');
        }
        text.push_str(&line);
        used += cost;
        count += 1;
    }
    (text, count)
}

/// 评论预览（TOOL-006）：token 转可读文本，压缩空白，截 160；纯图片为「[图片]」。
fn comment_preview(body: &str) -> String {
    let text = collapse(&ik_comments::text::plain_text(body));
    if text.is_empty() {
        "[图片]".to_owned()
    } else {
        truncate(&text, COMMENT_PREVIEW_CHARS)
    }
}

// ---------- 执行 ----------

/// 执行一次工具调用。白名单外的名字拒绝；出错时只告诉模型「工具执行失败」（TOOL-002）。
pub async fn execute(
    state: &AiState,
    ctx: &ToolContext,
    name: &str,
    arguments: &str,
) -> ToolOutput {
    if !TOOL_NAMES.contains(&name) {
        return ToolOutput {
            text: format!("未知工具：{name}"),
            is_error: true,
            meta: ToolMeta::None,
            images: Vec::new(),
        };
    }
    let args = parse_args(arguments);
    let result = match name {
        "search_posts" => search_posts(state, ctx, &args).await,
        "get_hot_posts" => hot_posts(state, ctx, &args).await,
        "get_user_info" => user_info(state, ctx, &args).await,
        "get_post_content" => post_content(state, ctx, &args).await,
        "get_post_comments" => post_comments(state, ctx, &args).await,
        "get_category_list" => category_list(state).await,
        _ => user_activity(state, ctx, &args).await,
    };
    match result {
        Ok(mut output) => {
            output.text = truncate_marked(&output.text, MAX_RESULT_CHARS);
            output
        }
        Err(problem) => {
            tracing::warn!(tool = name, ?problem, "AI 工具执行失败");
            ToolOutput {
                text: format!("工具执行失败：{name}"),
                is_error: true,
                meta: ToolMeta::None,
                images: Vec::new(),
            }
        }
    }
}

type Args = serde_json::Map<String, Value>;

const NO_USERNAME: &str = "（未提供用户名）";
const NO_POST_ID: &str = "（未提供帖子 document_id）";

fn post_not_found(raw: impl std::fmt::Display) -> String {
    format!("没有找到帖子（document_id: {raw}）。")
}

/// 按用户名找人（TOOL-005、D-085 方案 A）：去掉开头的 @，只做精确匹配；隐藏主页、与查看者有拉黑关系的
/// 当作查无此人。
async fn find_user(
    state: &AiState,
    ctx: &ToolContext,
    raw: &str,
) -> Result<Result<ik_profile::dto::ProfileView, String>, Problem> {
    let name = raw.trim().trim_start_matches('@').trim().to_owned();
    let not_found = || format!("没有找到用户「{name}」。");
    if name.is_empty() {
        return Ok(Err(not_found()));
    }
    let ids =
        ik_auth::service::active_user_ids_by_usernames(&state.db, std::slice::from_ref(&name))
            .await?;
    let Some(id) = ids.get(&name).map(|id| id.get()) else {
        return Ok(Err(not_found()));
    };
    if !ctx.user_visible(id) {
        return Ok(Err(not_found()));
    }
    match ik_profile::service::profile(&state.forum.profile, None, id).await {
        Ok(view) if !view.is_hidden => Ok(Ok(view)),
        Ok(_) => Ok(Err(not_found())),
        Err(problem) if problem.status() == http::StatusCode::NOT_FOUND => Ok(Err(not_found())),
        Err(problem) => Err(problem),
    }
}

/// 公开帖子详情；作者与查看者有拉黑关系时当作不存在（匿名帖不过滤）。
async fn visible_post(
    state: &AiState,
    ctx: &ToolContext,
    id: i64,
) -> Result<Option<ik_content::dto::PostDetail>, Problem> {
    match ik_content::service::detail(&state.forum.content, None, id).await {
        Ok(detail) if ctx.author_visible(&detail.author) => Ok(Some(detail)),
        Ok(_) => Ok(None),
        Err(problem) if problem.status() == http::StatusCode::NOT_FOUND => Ok(None),
        Err(problem) => Err(problem),
    }
}

// ----- search_posts -----

struct SearchPatterns {
    punctuation: Regex,
    filler: Regex,
}

static SEARCH: LazyLock<Option<SearchPatterns>> = LazyLock::new(|| {
    Some(SearchPatterns {
        punctuation: Regex::new(r"[\p{P}\p{S}]+").ok()?,
        filler: Regex::new(
            r"(?i)有没有|有无|帮我|帮忙|请问|麻烦|推荐一下|推荐|搜索一下|搜索|搜一下|找一下|查一下|找找|看看|一下|相关的|相关|关于|帖子|帖文|什么|哪些|最近|一些|的|吗|呢|吧|啊|search|find|posts?|please|about",
        )
        .ok()?,
    })
});

/// 同义词（TOOL-020）。查不到精确键时按包含关系取最长的键。
const SYNONYMS: &[(&str, &[&str])] = &[
    ("同人图", &["图楼", "插画", "画作", "同人"]),
    ("攻略", &["指南", "教程", "心得"]),
    ("代理人", &["角色", "人物"]),
];

fn synonyms_of(word: &str) -> &'static [&'static str] {
    if let Some((_, list)) = SYNONYMS.iter().find(|(key, _)| *key == word) {
        return list;
    }
    SYNONYMS
        .iter()
        .filter(|(key, _)| word.contains(key))
        .max_by_key(|(key, _)| key.chars().count())
        .map_or(&[], |(_, list)| list)
}

/// 查询扩展（TOOL-020）：去标点与停用词，按空白拆词，每个词带上同义词，最多 5 个检索词；
/// 多个词时再补一个拼接词。返回空表示没有有效检索词。
pub fn expand_query(raw: &str) -> Vec<String> {
    let Some(p) = SEARCH.as_ref() else {
        return vec![raw.trim().to_owned()];
    };
    let cleaned = p.punctuation.replace_all(raw, " ");
    let cleaned = p.filler.replace_all(&cleaned, " ");
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    if words.concat().chars().count() < 2 {
        return Vec::new();
    }
    let mut terms: Vec<String> = Vec::new();
    let push = |term: &str, terms: &mut Vec<String>| {
        if terms.len() < MAX_SEARCH_TERMS && !terms.iter().any(|t| t == term) {
            terms.push(term.to_owned());
        }
    };
    for word in &words {
        push(word, &mut terms);
    }
    for word in &words {
        for synonym in synonyms_of(word) {
            push(synonym, &mut terms);
        }
    }
    if words.len() > 1 {
        let joined = words.concat();
        if !terms.contains(&joined) {
            terms.push(joined);
        }
    }
    terms
}

/// 多词合并（TOOL-022）：第 i 个词的第 j 名得 `1/(j+1+i×0.1)`，跨词累加后降序。
pub fn merge_ranked(lists: &[Vec<i64>]) -> Vec<i64> {
    let mut scores: HashMap<i64, (f64, usize)> = HashMap::new();
    let mut order = 0usize;
    for (i, list) in lists.iter().enumerate() {
        for (j, id) in list.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let score = 1.0 / (j as f64 + 1.0 + i as f64 * 0.1);
            let entry = scores.entry(*id).or_insert_with(|| {
                order += 1;
                (0.0, order)
            });
            entry.0 += score;
        }
    }
    let mut ranked: Vec<(i64, f64, usize)> =
        scores.into_iter().map(|(id, (s, o))| (id, s, o)).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.2.cmp(&b.2)));
    ranked.into_iter().map(|(id, _, _)| id).collect()
}

/// 一个检索词的公开、实名、作者未与查看者拉黑的结果（搜索引擎 → 数据库兜底由 search 域处理）。
async fn search_term(
    state: &AiState,
    ctx: &ToolContext,
    term: &str,
    category: Option<&str>,
    limit: u32,
) -> Result<Vec<PostCard>, Problem> {
    let page = ik_search::service::search_posts(
        &state.forum.search,
        None,
        ik_search::dto::PostSearchQuery {
            q: Some(term.to_owned()),
            category: category.map(str::to_owned),
            sort: None,
            cursor: None,
            limit: Some(limit),
        },
    )
    .await?;
    Ok(page
        .items
        .into_iter()
        .filter(|card| !card.is_anonymous && ctx.author_visible(&card.author))
        .collect())
}

async fn search_posts(
    state: &AiState,
    ctx: &ToolContext,
    args: &Args,
) -> Result<ToolOutput, Problem> {
    let query = match args.get("queries") {
        Some(Value::Array(items)) if !items.is_empty() => items
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("、"),
        _ => arg_str(args, &["query", "q"]).unwrap_or_default(),
    };
    let category_key = arg_str(args, &["category"]);
    if let Some(seed) = id_arg(args, &["seed_post_id"]) {
        if !query.trim().is_empty() || category_key.is_some() {
            return Ok(ToolOutput::error(
                "seed_post_id 不能与 query 或 category 同时使用。",
            ));
        }
        return match seed {
            Ok(id) => related_posts(state, ctx, id, arg_limit(args, 5, 10)).await,
            Err(raw) => Ok(ToolOutput::error(post_not_found(raw))),
        };
    }
    let category = if let Some(raw) = category_key {
        match find_category(state, &raw).await? {
            Ok(category) => Some(category),
            Err(text) => return Ok(ToolOutput::text(text)),
        }
    } else {
        None
    };
    let limit = arg_limit(args, 5, 10);
    if query.trim().is_empty()
        && let Some(category) = category.as_ref()
    {
        return posts_by_category(state, ctx, category, limit).await;
    }
    let terms = expand_query(&query);
    if terms.is_empty() {
        return Ok(ToolOutput::text("（未提供有效检索词）"));
    }
    let mut lists = Vec::with_capacity(terms.len());
    let mut cards: HashMap<i64, PostCard> = HashMap::new();
    for term in &terms {
        let found = search_term(
            state,
            ctx,
            term,
            category.as_ref().map(|c| c.slug.as_str()),
            limit * 3,
        )
        .await?;
        lists.push(found.iter().map(|c| c.id.get()).collect::<Vec<_>>());
        for card in found {
            cards.entry(card.id.get()).or_insert(card);
        }
    }
    let ranked = merge_ranked(&lists);
    let hits = ranked.len();
    let top: Vec<&PostCard> = ranked
        .iter()
        .filter_map(|id| cards.get(id))
        .take(usize::try_from(limit).unwrap_or(SEARCH_RESULTS))
        .collect();
    let (lines, count) = card_lines_budget(&top, MAX_RESULT_CHARS);
    let text = if top.is_empty() {
        format!("没有找到与「{}」相关的帖子。", query.trim())
    } else {
        lines
    };
    Ok(ToolOutput::with(
        text,
        search_meta(
            category.as_ref().map_or_else(
                || query.trim().to_owned(),
                |c| format!("{} · {}", c.name, query.trim()),
            ),
            hits,
            &top[..count],
        ),
    ))
}

// ----- get_hot_posts -----

async fn hot_posts(state: &AiState, ctx: &ToolContext, args: &Args) -> Result<ToolOutput, Problem> {
    let limit = arg_limit(args, 5, 10);
    let page = ik_feed::service::hot(
        &state.forum.feed,
        None,
        ik_feed::service::HotQuery {
            cursor: None,
            limit: Some(limit.saturating_mul(2)),
        },
    )
    .await?;
    let lines: Vec<String> = page
        .items
        .iter()
        .filter(|card| ctx.author_visible(&card.author))
        .take(usize::try_from(limit).unwrap_or(5))
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}. {}（赞{}·评{}·阅{}）",
                i + 1,
                post_link(c.id.get(), &c.title),
                c.like_count,
                c.comment_count,
                c.view_count
            )
        })
        .collect();
    Ok(ToolOutput::text(if lines.is_empty() {
        "当前没有可推荐的热帖。".to_owned()
    } else {
        lines.join("\n")
    }))
}

// ----- 用户 -----

async fn denny_of(state: &AiState, user_id: i64) -> Result<i32, Problem> {
    let mut conn = state.db.acquire().await?;
    ik_economy::service::balance_of(&mut conn, user_id).await
}

async fn user_info(state: &AiState, ctx: &ToolContext, args: &Args) -> Result<ToolOutput, Problem> {
    let Some(raw) = arg_str(args, &["username", "user"]) else {
        return Ok(ToolOutput::text(NO_USERNAME));
    };
    let user = match find_user(state, ctx, &raw).await? {
        Ok(user) => user,
        Err(text) => return Ok(ToolOutput::text(text)),
    };
    let denny = denny_of(state, user.id.get()).await?;
    let bio = collapse(&user.bio);
    let bio = if bio.is_empty() {
        "（无）".to_owned()
    } else {
        truncate(&bio, BIO_MAX_CHARS)
    };
    let (posts, likes, views) = user
        .stats
        .as_ref()
        .map_or((0, 0, 0), |s| (s.post_count, s.total_likes, s.total_views));
    Ok(ToolOutput::text(format!(
        "用户：{name}（@{name}）\n等级：Lv.{}\n丁尼：{denny}\n简介：{bio}\n发帖数：{posts}\n获赞总数：{likes}\n总阅读量：{views}\n粉丝数：{}\n关注数：{}",
        user.level,
        user.follower_count,
        user.following_count,
        name = user.username,
    )))
}

async fn author_cards(state: &AiState, user_id: i64, limit: u32) -> Result<Vec<PostCard>, Problem> {
    let page = ik_profile::service::posts(
        &state.forum.profile,
        None,
        user_id,
        ik_profile::dto::ProfilePostsQuery {
            scope: ik_profile::dto::PostsScope::All,
            cursor: None,
            limit: Some(limit),
        },
    )
    .await?;
    Ok(page.items.into_iter().filter(|c| !c.is_anonymous).collect())
}

async fn user_activity(
    state: &AiState,
    ctx: &ToolContext,
    args: &Args,
) -> Result<ToolOutput, Problem> {
    let kind = arg_str(args, &["kind"]).unwrap_or_else(|| "all".to_owned());
    if !matches!(kind.as_str(), "posts" | "comments" | "all") {
        return Ok(ToolOutput::error("kind 只能是 posts、comments 或 all。"));
    }
    let Some(raw) = arg_str(args, &["username", "user"]) else {
        return Ok(ToolOutput::text(NO_USERNAME));
    };
    let user = match find_user(state, ctx, &raw).await? {
        Ok(user) => user,
        Err(text) => return Ok(ToolOutput::text(text)),
    };
    let limit = arg_limit(args, 5, 10);
    let cards = if kind == "comments" {
        Vec::new()
    } else {
        author_cards(state, user.id.get(), limit).await?
    };
    // 只列发帖与评论，不列点赞、收藏（TOOL-009）。
    let comments = if kind == "posts" {
        Vec::new()
    } else {
        ik_profile::service::comments(
            &state.forum.profile,
            None,
            user.id.get(),
            ik_profile::dto::PageQuery {
                cursor: None,
                limit: Some(limit),
            },
        )
        .await?
        .items
    };
    // 主页评论按游客读取后，批量复核所在帖子对触发者的拉黑可见性。
    let post_ids: Vec<i64> = comments.iter().map(|c| c.post_id.get()).collect();
    let visible_ids: HashSet<i64> =
        ik_content::service::public_cards_in_order(&state.forum.content, None, &post_ids)
            .await?
            .into_iter()
            .filter(|c| ctx.author_visible(&c.author))
            .map(|c| c.id.get())
            .collect();
    let comments: Vec<_> = comments
        .into_iter()
        .filter(|c| visible_ids.contains(&c.post_id.get()))
        .collect();
    if cards.is_empty() && comments.is_empty() {
        return Ok(ToolOutput::text(format!(
            "@{} 暂无公开动态。",
            user.username
        )));
    }
    let mut text = format!("@{} 的近期动态：", user.username);
    // 两种动态都有时各留一半预算，长帖子列表不挤掉评论。
    let section_budget = if !cards.is_empty() && !comments.is_empty() {
        (MAX_RESULT_CHARS - 200) / 2
    } else {
        MAX_RESULT_CHARS - 200
    };
    if !cards.is_empty() {
        let refs: Vec<&PostCard> = cards.iter().collect();
        text.push_str(&format!(
            "\n最新发帖：\n{}",
            card_lines_budget(&refs, section_budget - 10).0
        ));
    }
    if !comments.is_empty() {
        let mut section = String::from("\n最新评论：");
        for (i, c) in comments.iter().enumerate() {
            let entry = format!(
                "\n{}. 在{}中：{}（赞{}）",
                i + 1,
                post_link(c.post_id.get(), &c.post_title),
                comment_preview(&c.body),
                c.like_count
            );
            if section.chars().count() + entry.chars().count() > section_budget {
                break;
            }
            section.push_str(&entry);
        }
        text.push_str(&section);
    }
    Ok(ToolOutput::text(text))
}

// ----- 帖子 -----

/// 为元信息和续读指示单独留预算，字符偏移不会切开 UTF-8 字符。
fn body_page(header: &str, body: &str, offset: usize) -> Result<String, String> {
    let total = body.chars().count();
    if offset > total {
        return Err(format!("offset 超出正文范围；正文共 {total} 个字符。"));
    }
    let prefix = format!("{header}\n帖子正文：");
    // usize 的十进制位数与固定文案都包含在这个上界里。
    let footer_budget = 160;
    let budget = MAX_RESULT_CHARS.saturating_sub(prefix.chars().count() + footer_budget);
    let chunk: String = body.chars().skip(offset).take(budget).collect();
    let end = offset + chunk.chars().count();
    let continuation = if end < total {
        format!("truncated: true\nnext_offset: {end}")
    } else {
        "truncated: false\nnext_offset: null".to_owned()
    };
    Ok(format!(
        "{prefix}{chunk}\n读取范围：{offset}..{end} / {total} 字符\n{continuation}"
    ))
}

async fn post_content(
    state: &AiState,
    ctx: &ToolContext,
    args: &Args,
) -> Result<ToolOutput, Problem> {
    let offset = match args.get("offset") {
        None => Some(0),
        Some(Value::Number(n)) => n.as_u64().and_then(|n| usize::try_from(n).ok()),
        Some(Value::String(s)) => s.trim().parse::<usize>().ok(),
        _ => None,
    };
    let Some(offset) = offset else {
        return Ok(ToolOutput::error("offset 必须是非负整数。"));
    };
    let id = match post_id(args) {
        None => return Ok(ToolOutput::text(NO_POST_ID)),
        Some(Err(raw)) => {
            return Ok(ToolOutput::with(
                post_not_found(raw),
                ToolMeta::Read { post: None },
            ));
        }
        Some(Ok(id)) => id,
    };
    let Some(post) = visible_post(state, ctx, id).await? else {
        return Ok(ToolOutput::with(
            post_not_found(id),
            ToolMeta::Read { post: None },
        ));
    };
    // 匿名帖照样可读，但不输出作者（TOOL 明细表）。
    let cover_ids: Vec<i64> = post.covers.iter().map(|c| c.id.get()).collect();
    let header = format!(
        "帖子标题：{}\n帖子链接：{}\n互动：阅读 {} · 点赞 {} · 收藏 {} · 评论 {} · 丁尼 {}",
        title_or_placeholder(&post.title),
        post_link(id, &post.title),
        post.view_count,
        post.like_count,
        post.favorite_count,
        post.comment_count,
        post.denny_count
    );
    let text = match body_page(&header, &post.body, offset) {
        Ok(text) => text,
        Err(text) => return Ok(ToolOutput::error(text)),
    };
    let mut output = ToolOutput::with(
        text,
        ToolMeta::Read {
            post: Some((id, post.title.clone())),
        },
    );
    if offset == 0 {
        let mut conn = state.db.acquire().await?;
        output.images = post_images(state, &mut conn, &cover_ids, &post.body).await?;
    }
    Ok(output)
}

async fn post_comments(
    state: &AiState,
    ctx: &ToolContext,
    args: &Args,
) -> Result<ToolOutput, Problem> {
    let id = match post_id(args) {
        None => return Ok(ToolOutput::text(NO_POST_ID)),
        Some(Err(raw)) => return Ok(ToolOutput::text(post_not_found(raw))),
        Some(Ok(id)) => id,
    };
    let Some(post) = visible_post(state, ctx, id).await? else {
        return Ok(ToolOutput::text(post_not_found(id)));
    };
    let limit = i64::from(arg_limit(args, 10, 20));
    let Some(hot) =
        ik_comments::service::hot_comments(&state.forum.comments, ctx.viewer_id, id, limit).await?
    else {
        return Ok(ToolOutput::text(post_not_found(id)));
    };
    let title = title_or_placeholder(&post.title);
    let meta = ToolMeta::Comments {
        post: Some((id, post.title.clone())),
    };
    if hot.comments.is_empty() {
        return Ok(ToolOutput::with(
            format!("帖子《{title}》目前还没有评论。"),
            meta,
        ));
    }
    let mut text = format!(
        "帖子《{title}》共有 {} 条评论，按热度展示前 {} 条：",
        hot.total,
        hot.comments.len()
    );
    for comment in &hot.comments {
        let reply_to = match &comment.reply_to {
            None => String::new(),
            Some(r) => match &r.comment {
                Some(quoted) => format!("回复 #{} {}，", r.floor, quoted.author.name),
                None if r.deleted => format!("回复 #{}（原评论已删除），", r.floor),
                None => format!("回复 #{}，", r.floor),
            },
        };
        text.push_str(&format!(
            "\n#{} {}：{}{}（赞{}）",
            comment.floor,
            comment.author.name,
            reply_to,
            comment_preview(&comment.body),
            comment.like_count
        ));
    }
    Ok(ToolOutput::with(text, meta))
}

/// 复用论坛的相关推荐，在候选截断前应用工具的公开实名口径。
async fn related_posts(
    state: &AiState,
    ctx: &ToolContext,
    id: i64,
    limit: u32,
) -> Result<ToolOutput, Problem> {
    let Some(seed) = visible_post(state, ctx, id).await? else {
        return Ok(ToolOutput::text(post_not_found(id)));
    };
    let cards =
        ik_content::service::related_named(&state.forum.content, ctx.viewer_id, id, Some(limit))
            .await?;
    let top: Vec<&PostCard> = cards
        .iter()
        .filter(|c| !c.is_anonymous && ctx.author_visible(&c.author))
        .filter(|c| c.id.get() != id && c.author.id.is_none_or(|a| Some(a.get()) != ctx.viewer_id))
        .take(usize::try_from(limit).unwrap_or(SEARCH_RESULTS))
        .collect();
    let header = format!("与《{}》相关的帖子：\n", title_or_placeholder(&seed.title));
    let (lines, count) = card_lines_budget(
        &top,
        MAX_RESULT_CHARS.saturating_sub(header.chars().count()),
    );
    let text = if top.is_empty() {
        "没有找到相关帖子。".to_owned()
    } else {
        format!("{header}{lines}")
    };
    Ok(ToolOutput::with(
        text,
        search_meta(
            format!("与《{}》相关", seed.title),
            top.len(),
            &top[..count],
        ),
    ))
}

// ----- 分区 -----

async fn public_categories(state: &AiState) -> Result<Vec<ik_content::dto::CategoryView>, Problem> {
    Ok(ik_content::service::categories(&state.forum.content)
        .await?
        .into_iter()
        .filter(|c| !c.admin_only)
        .collect())
}

async fn find_category(
    state: &AiState,
    raw: &str,
) -> Result<Result<ik_content::dto::CategoryView, String>, Problem> {
    let key = raw.to_lowercase();
    let categories = public_categories(state).await?;
    Ok(categories
        .iter()
        .find(|c| c.slug.to_lowercase() == key)
        .or_else(|| {
            categories
                .iter()
                .find(|c| c.name.to_lowercase().contains(&key))
        })
        .cloned()
        .ok_or_else(|| format!("没有找到分类「{raw}」。")))
}

fn search_meta(query: String, hits: usize, cards: &[&PostCard]) -> ToolMeta {
    ToolMeta::Search {
        query,
        hits,
        items: cards
            .iter()
            .map(|c| (c.id.get(), c.title.clone()))
            .collect(),
    }
}

async fn category_list(state: &AiState) -> Result<ToolOutput, Problem> {
    let categories = public_categories(state).await?;
    if categories.is_empty() {
        return Ok(ToolOutput::text("当前没有可公开浏览的分类。"));
    }
    let lines: Vec<String> = categories
        .iter()
        .take(50)
        .enumerate()
        .map(|(i, c)| {
            let description = collapse(&c.description);
            if description.is_empty() {
                format!("{}. {}（/c/{}）", i + 1, c.name, c.slug)
            } else {
                format!("{}. {}（/c/{}） - {description}", i + 1, c.name, c.slug)
            }
        })
        .collect();
    Ok(ToolOutput::text(lines.join("\n")))
}

async fn posts_by_category(
    state: &AiState,
    ctx: &ToolContext,
    category: &ik_content::dto::CategoryView,
    limit: u32,
) -> Result<ToolOutput, Problem> {
    let page = ik_content::service::feed(
        &state.forum.content,
        None,
        ik_content::dto::FeedQuery {
            category: Some(category.slug.clone()),
            tag: None,
            cursor: None,
            limit: Some(limit.saturating_mul(2)),
        },
    )
    .await?;
    let mut cards: Vec<&PostCard> = page
        .pinned
        .iter()
        .chain(page.items.iter())
        .filter(|c| !c.is_anonymous && ctx.author_visible(&c.author))
        .collect();
    cards.sort_by_key(|c| std::cmp::Reverse(c.bumped_at));
    cards.truncate(usize::try_from(limit).unwrap_or(5));
    let header = format!("分类「{}」下的最新帖子：\n", category.name);
    let (lines, count) = card_lines_budget(
        &cards,
        MAX_RESULT_CHARS.saturating_sub(header.chars().count()),
    );
    let text = if cards.is_empty() {
        format!("分类「{}」下暂时没有帖子。", category.name)
    } else {
        format!("{header}{lines}")
    };
    Ok(ToolOutput::with(
        text,
        search_meta(
            format!("分区：{}", category.name),
            cards.len(),
            &cards[..count],
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_args_reject_broken_json_but_accept_empty_and_escaped() {
        assert!(strict_args("").unwrap().is_empty());
        assert_eq!(strict_args(r#"{"q":"a"}"#).unwrap()["q"], "a");
        assert_eq!(strict_args(r#""{\"q\":\"a\"}""#).unwrap()["q"], "a");
        let error = strict_args(r#"{"q":"a"#).unwrap_err();
        assert!(error.contains("没有执行"));
        assert!(strict_args("[1]").is_err());
    }

    #[test]
    fn truncation_tells_how_much_is_left() {
        assert_eq!(truncate_marked("短", 10), "短");
        let text = truncate_marked(&"字".repeat(15), 10);
        assert!(text.starts_with(&"字".repeat(10)));
        assert!(text.ends_with("后面还有约 5 字）"));
    }

    #[test]
    fn specs_match_the_whitelist() {
        let names: Vec<String> = specs().iter().map(|s| s.name.to_string()).collect();
        assert_eq!(names, TOOL_NAMES.to_vec());
        assert_eq!(names.len(), 7);
        for spec in specs() {
            assert_eq!(spec.parameters["type"], "object");
        }
    }

    #[test]
    fn body_pages_preserve_unicode_and_whitespace_and_report_end() {
        let body = format!("  {}\n", "汉🙂é".repeat(3000));
        let mut offset = 0;
        let mut reconstructed = String::new();
        loop {
            let page = body_page("帖子标题：测试\n互动：点赞 9", &body, offset).unwrap();
            assert!(page.chars().count() <= MAX_RESULT_CHARS);
            assert!(page.starts_with("帖子标题：测试\n互动：点赞 9\n"));
            let chunk = page
                .split_once("\n帖子正文：")
                .unwrap()
                .1
                .rsplit_once("\n读取范围：")
                .unwrap()
                .0;
            reconstructed.push_str(chunk);
            if page.ends_with("next_offset: null") {
                assert!(page.contains("truncated: false"));
                break;
            }
            let next: usize = page
                .lines()
                .last()
                .unwrap()
                .strip_prefix("next_offset: ")
                .unwrap()
                .parse()
                .unwrap();
            assert_eq!(next, offset + chunk.chars().count());
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(reconstructed, body);
        let at_end = body_page("标题", &body, body.chars().count()).unwrap();
        assert!(at_end.ends_with("truncated: false\nnext_offset: null"));
        assert!(body_page("标题", &body, body.chars().count() + 1).is_err());
    }

    #[test]
    fn card_budget_keeps_complete_entries_and_matching_citations() {
        let first = PostCard {
            id: ik_kernel::Id::new(7),
            title: "甲".repeat(50),
            excerpt: "乙".repeat(120),
            cover: None,
            video: None,
            author: AuthorView {
                id: Some(ik_kernel::Id::new(1)),
                name: "作者".to_owned(),
                level: Some(1),
                anonymous: false,
                avatar_url: None,
            },
            category: None,
            tags: Vec::new(),
            is_anonymous: false,
            is_pinned: false,
            is_solved: false,
            view_count: 0,
            like_count: 0,
            comment_count: 0,
            published_at: chrono::Utc::now(),
            bumped_at: chrono::Utc::now(),
            edited_at: None,
            bumped_by_self: false,
            is_read: false,
        };
        let second = PostCard {
            id: ik_kernel::Id::new(8),
            ..first.clone()
        };
        let (one, _) = card_lines_budget(&[&first], MAX_RESULT_CHARS);
        let cards = [&first, &second];
        let (text, count) = card_lines_budget(&cards, one.chars().count() + 5);
        assert_eq!(text, one);
        assert_eq!(count, 1);
        let ToolMeta::Search { hits, items, .. } =
            search_meta("测试".to_owned(), cards.len(), &cards[..count])
        else {
            panic!("expected search metadata");
        };
        assert_eq!(hits, 2);
        assert_eq!(items, vec![(7, first.title)]);
        assert!(!text.contains("/post/8"));
    }

    #[test]
    fn args_accept_objects_strings_and_garbage() {
        assert_eq!(parse_args(r#"{"query":"x"}"#)["query"], "x");
        assert_eq!(parse_args(r#""{\"query\":\"y\"}""#)["query"], "y");
        assert!(parse_args("not json").is_empty());
        assert!(parse_args("[1,2]").is_empty());
        let args = parse_args(r#"{"limit":"99","document_id":"/post/42"}"#);
        assert_eq!(arg_limit(&args, 5, 10), 10);
        assert_eq!(post_id(&args), Some(Ok(42)));
        assert_eq!(arg_limit(&parse_args(r#"{"limit":0}"#), 5, 10), 1);
    }

    #[test]
    fn query_expansion_drops_fillers_and_adds_synonyms() {
        assert_eq!(
            expand_query("有没有 零号空洞 攻略 的帖子？"),
            vec!["零号空洞", "攻略", "指南", "教程", "心得", "零号空洞攻略"]
        );
        assert!(expand_query("帖子？").is_empty());
        assert_eq!(
            expand_query("代理人图鉴"),
            vec!["代理人图鉴", "角色", "人物"]
        );
    }

    #[test]
    fn ranking_accumulates_across_terms() {
        let merged = merge_ranked(&[vec![1, 2, 3], vec![3, 4], vec![3]]);
        assert_eq!(merged[0], 3);
        assert_eq!(merged[1..], [1, 2, 4]);
    }

    #[test]
    fn body_images_are_http_markdown_images() {
        let body = "开头![封面](https://cdn.test/a.png)\n![本地](/x.png)\n\
                    [链接](https://cdn.test/c.png) ![x]( http://cdn.test/d.jpg \"标题\")";
        assert_eq!(
            body_image_urls(body),
            vec![
                "https://cdn.test/a.png".to_owned(),
                "http://cdn.test/d.jpg".to_owned(),
            ]
        );
    }

    #[test]
    fn links_are_safe_markdown() {
        assert_eq!(post_link(7, " [置顶] 公告 "), "[置顶 公告](/post/7)");
        assert_eq!(post_link(8, ""), "[（无标题）](/post/8)");
        assert_eq!(
            comment_preview("@[小明](3)  你好\n\n:ik-smile:"),
            "@小明 你好 [表情:smile]"
        );
        assert_eq!(comment_preview(""), "[图片]");
    }
}
