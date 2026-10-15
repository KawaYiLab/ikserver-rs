//! 论坛工具（ai-assistant.md 第 8 节）：12 个只读工具，在进程内执行。
//!
//! 约束（TOOL-001～009）：
//! - 只读、无副作用、只面向公开内容：都以游客身份调用各域的 service（隐藏、未发布的内容天然读不到）；
//! - 查看者（触发者）只来自可信的执行上下文 [`ToolContext`]，绝不从参数里读（TOOL-004）；
//! - 与查看者有拉黑关系的人的实名内容、主页不出现，匿名内容不做拉黑过滤（D-084 方案 A）；
//! - 按用户名找人只做精确匹配（D-085 方案 A），隐藏主页的人当作查无此人（TOOL-005）；
//! - 帖子一律写成 `[标题](/post/<id>)`（TOOL-003）；结果截 4000 字；执行出错只告诉模型
//!   `工具执行失败：<名>`（TOOL-002）。

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

/// 执行上下文：查看者是触发者（TOOL-004）。
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub viewer_id: i64,
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
            viewer_id,
            excluded,
        })
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
}

/// 一次工具调用的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// 喂给模型的文本（已截断）。
    pub text: String,
    pub is_error: bool,
    pub meta: ToolMeta,
}

impl ToolOutput {
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
            meta: ToolMeta::None,
        }
    }

    fn with(text: impl Into<String>, meta: ToolMeta) -> Self {
        Self {
            text: text.into(),
            is_error: false,
            meta,
        }
    }
}

/// 白名单（TOOL-001）。
pub const TOOL_NAMES: [&str; 12] = [
    "search_posts",
    "get_hot_posts",
    "get_user_info",
    "get_post_content",
    "get_post_comments",
    "get_posts_by_author",
    "search_posts_by_category",
    "get_user_stats",
    "get_related_posts",
    "get_category_list",
    "get_post_engagement",
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

/// 提供给模型的工具清单（只有白名单里的 12 个）。
pub fn specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "search_posts",
            description: "在绳网站内搜索帖子。query 请拆成 2-4 个用顿号或逗号分隔的短词。\
                 结果里的帖子写成 markdown 链接 [标题](/post/id)，引用时必须原样保留该链接。",
            parameters: object(
                json!({
                    "query": { "type": "string", "description": "检索词，如「攻略、零号空洞」" },
                    "queries": { "type": "array", "items": { "type": "string" } }
                }),
                &["query"],
            ),
        },
        ToolSpec {
            name: "get_hot_posts",
            description: "获取绳网当前的热门帖子。结果里的帖子写成 markdown 链接 [标题](/post/id)，引用时必须原样保留该链接。",
            parameters: object(json!({ "limit": limit_param(10, 5) }), &[]),
        },
        ToolSpec {
            name: "get_user_info",
            description: "获取用户的公开资料：等级、丁尼、简介。",
            parameters: object(json!({ "username": username_param() }), &["username"]),
        },
        ToolSpec {
            name: "get_post_content",
            description: "读取一篇帖子的标题与正文。",
            parameters: object(
                json!({ "document_id": document_id_param() }),
                &["document_id"],
            ),
        },
        ToolSpec {
            name: "get_post_comments",
            description: "按热度读取一篇帖子的评论（含部分楼中楼回复）。",
            parameters: object(
                json!({ "document_id": document_id_param(), "limit": limit_param(20, 10) }),
                &["document_id"],
            ),
        },
        ToolSpec {
            name: "get_posts_by_author",
            description: "获取某位用户近期公开发布的帖子。结果里的帖子写成 markdown 链接 [标题](/post/id)，引用时必须原样保留该链接。",
            parameters: object(
                json!({ "username": username_param(), "limit": limit_param(10, 5) }),
                &["username"],
            ),
        },
        ToolSpec {
            name: "search_posts_by_category",
            description: "获取某个分区下的最新帖子。category 可以是分区 slug 或名称关键词。结果里的帖子写成 markdown 链接 [标题](/post/id)，引用时必须原样保留该链接。",
            parameters: object(
                json!({
                    "category": { "type": "string", "description": "分区 slug 或名称关键词" },
                    "limit": limit_param(10, 5)
                }),
                &["category"],
            ),
        },
        ToolSpec {
            name: "get_user_stats",
            description: "获取用户的统计：等级、丁尼、发帖数、获赞总数、总阅读量、粉丝数、关注数。",
            parameters: object(json!({ "username": username_param() }), &["username"]),
        },
        ToolSpec {
            name: "get_related_posts",
            description: "获取与某篇帖子相关的其他帖子。结果里的帖子写成 markdown 链接 [标题](/post/id)，引用时必须原样保留该链接。",
            parameters: object(
                json!({ "document_id": document_id_param(), "limit": limit_param(10, 5) }),
                &["document_id"],
            ),
        },
        ToolSpec {
            name: "get_category_list",
            description: "列出可以公开浏览的分区。",
            parameters: object(json!({}), &[]),
        },
        ToolSpec {
            name: "get_post_engagement",
            description: "获取一篇帖子的互动数据：阅读、点赞、收藏、评论、丁尼。",
            parameters: object(
                json!({ "document_id": document_id_param() }),
                &["document_id"],
            ),
        },
        ToolSpec {
            name: "get_user_activity",
            description: "获取用户近期公开的发帖与评论。结果里的帖子写成 markdown 链接 [标题](/post/id)，引用时必须原样保留该链接。",
            parameters: object(
                json!({ "username": username_param(), "limit": limit_param(10, 5) }),
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
    let raw = arg_str(args, &["document_id", "documentId", "id", "post_id"])?;
    let digits = raw.rsplit('/').next().unwrap_or(&raw).trim().to_owned();
    Some(digits.parse::<i64>().map_err(|_| raw))
}

// ---------- 文本 ----------

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
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

fn card_lines(cards: &[&PostCard]) -> String {
    cards
        .iter()
        .enumerate()
        .map(|(i, card)| {
            let mut line = format!("{}. {}", i + 1, post_link(card.id.get(), &card.title));
            let excerpt = collapse(&card.excerpt);
            if !excerpt.is_empty() {
                line.push_str(&format!("\n   {}", truncate(&excerpt, 120)));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        };
    }
    let args = parse_args(arguments);
    let result = match name {
        "search_posts" => search_posts(state, ctx, &args).await,
        "get_hot_posts" => hot_posts(state, ctx, &args).await,
        "get_user_info" => user_info(state, ctx, &args).await,
        "get_post_content" => post_content(state, ctx, &args).await,
        "get_post_comments" => post_comments(state, ctx, &args).await,
        "get_posts_by_author" => posts_by_author(state, ctx, &args).await,
        "search_posts_by_category" => posts_by_category(state, ctx, &args).await,
        "get_user_stats" => user_stats(state, ctx, &args).await,
        "get_related_posts" => related_posts(state, ctx, &args).await,
        "get_category_list" => category_list(state).await,
        "get_post_engagement" => post_engagement(state, ctx, &args).await,
        _ => user_activity(state, ctx, &args).await,
    };
    match result {
        Ok(mut output) => {
            output.text = truncate(&output.text, MAX_RESULT_CHARS);
            output
        }
        Err(problem) => {
            tracing::warn!(tool = name, ?problem, "AI 工具执行失败");
            ToolOutput {
                text: format!("工具执行失败：{name}"),
                is_error: true,
                meta: ToolMeta::None,
            }
        }
    }
}

type Args = serde_json::Map<String, Value>;

const NO_USERNAME: &str = "（未提供用户名）";
const NO_POST_ID: &str = "（未提供帖子 document_id）";
const NO_CATEGORY: &str = "（未提供分类标识）";

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
    limit: u32,
) -> Result<Vec<PostCard>, Problem> {
    let page = ik_search::service::search_posts(
        &state.forum.search,
        None,
        ik_search::dto::PostSearchQuery {
            q: Some(term.to_owned()),
            category: None,
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
    let terms = expand_query(&query);
    if terms.is_empty() {
        return Ok(ToolOutput::text("（未提供有效检索词）"));
    }
    let mut lists = Vec::with_capacity(terms.len());
    let mut cards: HashMap<i64, PostCard> = HashMap::new();
    for term in &terms {
        let found = search_term(state, ctx, term, 15).await?;
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
        .take(SEARCH_RESULTS)
        .collect();
    let items = top
        .iter()
        .map(|c| (c.id.get(), c.title.clone()))
        .collect::<Vec<_>>();
    let text = if top.is_empty() {
        format!("没有找到与「{}」相关的帖子。", query.trim())
    } else {
        card_lines(&top)
    };
    Ok(ToolOutput::with(
        text,
        ToolMeta::Search {
            query: query.trim().to_owned(),
            hits,
            items,
        },
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
    Ok(ToolOutput::text(format!(
        "用户：{name}（@{name}）\n等级：Lv.{}\n丁尼：{denny}\n简介：{bio}",
        user.level,
        name = user.username,
    )))
}

async fn user_stats(
    state: &AiState,
    ctx: &ToolContext,
    args: &Args,
) -> Result<ToolOutput, Problem> {
    let Some(raw) = arg_str(args, &["username", "user"]) else {
        return Ok(ToolOutput::text(NO_USERNAME));
    };
    let user = match find_user(state, ctx, &raw).await? {
        Ok(user) => user,
        Err(text) => return Ok(ToolOutput::text(text)),
    };
    let denny = denny_of(state, user.id.get()).await?;
    let (posts, likes, views) = user
        .stats
        .as_ref()
        .map_or((0, 0, 0), |s| (s.post_count, s.total_likes, s.total_views));
    Ok(ToolOutput::text(format!(
        "用户：@{}\n等级：Lv.{}\n丁尼：{denny}\n发帖数：{posts}\n获赞总数：{likes}\n总阅读量：{views}\n粉丝数：{}\n关注数：{}",
        user.username, user.level, user.follower_count, user.following_count,
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

async fn posts_by_author(
    state: &AiState,
    ctx: &ToolContext,
    args: &Args,
) -> Result<ToolOutput, Problem> {
    let Some(raw) = arg_str(args, &["username", "user"]) else {
        return Ok(ToolOutput::text(NO_USERNAME));
    };
    let user = match find_user(state, ctx, &raw).await? {
        Ok(user) => user,
        Err(text) => return Ok(ToolOutput::text(text)),
    };
    let cards = author_cards(state, user.id.get(), arg_limit(args, 5, 10)).await?;
    if cards.is_empty() {
        return Ok(ToolOutput::text(format!(
            "@{} 还没有公开发布的帖子。",
            user.username
        )));
    }
    let refs: Vec<&PostCard> = cards.iter().collect();
    Ok(ToolOutput::text(format!(
        "@{} 近期发布的帖子：\n{}",
        user.username,
        card_lines(&refs)
    )))
}

async fn user_activity(
    state: &AiState,
    ctx: &ToolContext,
    args: &Args,
) -> Result<ToolOutput, Problem> {
    let Some(raw) = arg_str(args, &["username", "user"]) else {
        return Ok(ToolOutput::text(NO_USERNAME));
    };
    let user = match find_user(state, ctx, &raw).await? {
        Ok(user) => user,
        Err(text) => return Ok(ToolOutput::text(text)),
    };
    let limit = arg_limit(args, 5, 10);
    let cards = author_cards(state, user.id.get(), limit).await?;
    // 只列发帖与评论，不列点赞、收藏（TOOL-009）。
    let comments = ik_profile::service::comments(
        &state.forum.profile,
        None,
        user.id.get(),
        ik_profile::dto::PageQuery {
            cursor: None,
            limit: Some(limit),
        },
    )
    .await?
    .items;
    if cards.is_empty() && comments.is_empty() {
        return Ok(ToolOutput::text(format!(
            "@{} 暂无公开动态。",
            user.username
        )));
    }
    let mut text = format!("@{} 的近期动态：", user.username);
    if !cards.is_empty() {
        let refs: Vec<&PostCard> = cards.iter().collect();
        text.push_str(&format!("\n最新发帖：\n{}", card_lines(&refs)));
    }
    if !comments.is_empty() {
        text.push_str("\n最新评论：");
        for (i, c) in comments.iter().enumerate() {
            text.push_str(&format!(
                "\n{}. 在{}中：{}（赞{}）",
                i + 1,
                post_link(c.post_id.get(), &c.post_title),
                comment_preview(&c.body),
                c.like_count
            ));
        }
    }
    Ok(ToolOutput::text(text))
}

// ----- 帖子 -----

async fn post_content(
    state: &AiState,
    ctx: &ToolContext,
    args: &Args,
) -> Result<ToolOutput, Problem> {
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
    Ok(ToolOutput::with(
        format!(
            "帖子标题：{}\n帖子正文：{}",
            title_or_placeholder(&post.title),
            truncate(post.body.trim(), MAX_RESULT_CHARS)
        ),
        ToolMeta::Read {
            post: Some((id, post.title.clone())),
        },
    ))
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
        ik_comments::service::hot_threads(&state.forum.comments, Some(ctx.viewer_id), id, limit, 3)
            .await?
    else {
        return Ok(ToolOutput::text(post_not_found(id)));
    };
    let title = title_or_placeholder(&post.title);
    let meta = ToolMeta::Comments {
        post: Some((id, post.title.clone())),
    };
    if hot.threads.is_empty() {
        return Ok(ToolOutput::with(
            format!("帖子《{title}》目前还没有评论。"),
            meta,
        ));
    }
    let mut text = format!(
        "帖子《{title}》共有 {} 条顶层评论，按热度展示前 {} 条（含部分楼中楼回复）：",
        hot.total,
        hot.threads.len()
    );
    for (i, (top, replies)) in hot.threads.iter().enumerate() {
        text.push_str(&format!(
            "\n{}. {}：{}（赞{}）",
            i + 1,
            top.author.name,
            comment_preview(&top.body),
            top.like_count
        ));
        for reply in replies {
            text.push_str(&format!(
                "\n   └ {}：{}",
                reply.author.name,
                comment_preview(&reply.body)
            ));
        }
    }
    Ok(ToolOutput::with(text, meta))
}

async fn post_engagement(
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
    Ok(ToolOutput::text(format!(
        "帖子《{}》互动数据：\n阅读：{}\n点赞：{}\n收藏：{}\n评论：{}\n丁尼：{}",
        title_or_placeholder(&post.title),
        post.view_count,
        post.like_count,
        post.favorite_count,
        post.comment_count,
        post.denny_count
    )))
}

/// 相关帖子：用种子帖的标题站内搜索，排除种子本身、查看者自己的帖与匿名帖（本系统没有详情页的
/// 「相关委托」，按 TOOL 明细表的口径用搜索实现）。
async fn related_posts(
    state: &AiState,
    ctx: &ToolContext,
    args: &Args,
) -> Result<ToolOutput, Problem> {
    let id = match post_id(args) {
        None => return Ok(ToolOutput::text(NO_POST_ID)),
        Some(Err(raw)) => return Ok(ToolOutput::text(post_not_found(raw))),
        Some(Ok(id)) => id,
    };
    let Some(seed) = visible_post(state, ctx, id).await? else {
        return Ok(ToolOutput::text(post_not_found(id)));
    };
    let limit = usize::try_from(arg_limit(args, 5, 10)).unwrap_or(5);
    let terms = expand_query(&seed.title);
    let mut lists = Vec::new();
    let mut cards: HashMap<i64, PostCard> = HashMap::new();
    for term in &terms {
        let found: Vec<PostCard> = search_term(state, ctx, term, 15)
            .await?
            .into_iter()
            .filter(|c| c.id.get() != id && c.author.id.is_none_or(|a| a.get() != ctx.viewer_id))
            .collect();
        lists.push(found.iter().map(|c| c.id.get()).collect::<Vec<_>>());
        for card in found {
            cards.entry(card.id.get()).or_insert(card);
        }
    }
    let top: Vec<&PostCard> = merge_ranked(&lists)
        .iter()
        .filter_map(|id| cards.get(id))
        .take(limit)
        .collect();
    if top.is_empty() {
        return Ok(ToolOutput::text("没有找到相关帖子。"));
    }
    Ok(ToolOutput::text(format!(
        "与《{}》相关的帖子：\n{}",
        title_or_placeholder(&seed.title),
        card_lines(&top)
    )))
}

// ----- 分区 -----

async fn public_categories(state: &AiState) -> Result<Vec<ik_content::dto::CategoryView>, Problem> {
    Ok(ik_content::service::categories(&state.forum.content)
        .await?
        .into_iter()
        .filter(|c| !c.admin_only)
        .collect())
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
    args: &Args,
) -> Result<ToolOutput, Problem> {
    let Some(raw) = arg_str(args, &["category", "slug", "name"]) else {
        return Ok(ToolOutput::text(NO_CATEGORY));
    };
    let key = raw.to_lowercase();
    let categories = public_categories(state).await?;
    let Some(category) = categories
        .iter()
        .find(|c| c.slug.to_lowercase() == key)
        .or_else(|| {
            categories
                .iter()
                .find(|c| c.name.to_lowercase().contains(&key))
        })
    else {
        return Ok(ToolOutput::text(format!("没有找到分类「{raw}」。")));
    };
    let limit = arg_limit(args, 5, 10);
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
    if cards.is_empty() {
        return Ok(ToolOutput::text(format!(
            "分类「{}」下暂时没有帖子。",
            category.name
        )));
    }
    Ok(ToolOutput::text(format!(
        "分类「{}」下的最新帖子：\n{}",
        category.name,
        card_lines(&cards)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_match_the_whitelist() {
        let names: Vec<&str> = specs().iter().map(|s| s.name).collect();
        assert_eq!(names, TOOL_NAMES.to_vec());
        for spec in specs() {
            assert_eq!(spec.parameters["type"], "object");
        }
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
