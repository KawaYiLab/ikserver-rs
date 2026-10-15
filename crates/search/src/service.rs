//! 搜索查询。业务规则：`docs/business-rules/feed-search.md` 3.9～3.11 节。
//!
//! 搜索引擎只负责「找候选、排序」，返回内容一律回查数据库（SRCH-013、PRIV-006）：
//! - 引擎报错或超时 → 数据库兜底；
//! - 引擎有命中、但这一页回查后一条都不剩（全是索引残留，EDGE-007）→ 数据库兜底；
//! - 引擎本来就没有命中 → 返回空，不查数据库。
//!
//! 没有配置搜索引擎时直接走数据库兜底。

use std::net::IpAddr;

use governor::clock::{Clock, DefaultClock};
use ik_auth::Principal;
use ik_comments::dto::CommentSearchView;
use ik_content::dto::PostCard;
use ik_kernel::cursor;
use ik_kernel::{Id, Problem};
use serde::{Deserialize, Serialize};

use crate::SearchState;
use crate::dto::{CommentSearchQuery, PostSearchQuery, SearchPage, SuggestItem, SuggestQuery};
use crate::engine::{Hits, PostSort};
use crate::text::{escape_html, highlight_html, normalize_query, strip_marks};

/// 帖子搜索与联想的查询词上限（D-042 方案 A）。
pub const POST_QUERY_MAX_CHARS: usize = 64;
/// 评论搜索的查询词上限（SRCH-030）。
pub const COMMENT_QUERY_MAX_CHARS: usize = 32;
/// 数据库兜底的最短查询词（SRCH-014）。单字查询只走搜索引擎。
const FALLBACK_MIN_CHARS: usize = 2;
const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 50;
const DEFAULT_SUGGEST: u32 = 8;
const MAX_SUGGEST: u32 = 10;
/// 翻页深度上限：搜索引擎默认最多给前 1000 条，数据库兜底窗口也是 1000（RATE-005）。
const MAX_OFFSET: usize = 1000;

#[derive(Serialize, Deserialize)]
struct OffsetCursor {
    o: usize,
}

fn decode_offset(raw: Option<&str>) -> Result<usize, Problem> {
    let offset = raw
        .map(cursor::decode::<OffsetCursor>)
        .transpose()?
        .map_or(0, |c| c.o);
    if offset > MAX_OFFSET {
        return Err(Problem::cursor_invalid());
    }
    Ok(offset)
}

fn page_limit(limit: Option<u32>) -> usize {
    usize::try_from(limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)).unwrap_or(20)
}

fn next_cursor(offset: usize, consumed: usize, total: usize) -> Result<Option<String>, Problem> {
    let next = offset + consumed;
    if consumed == 0 || next >= total || next >= MAX_OFFSET {
        return Ok(None);
    }
    Ok(Some(cursor::encode(&OffsetCursor { o: next })?))
}

fn parse_sort(sort: Option<&str>) -> PostSort {
    match sort.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("latest") => PostSort::Latest,
        Some("hot") => PostSort::Hot,
        _ => PostSort::Relevance,
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

fn total_of(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// 登录用户按用户、游客按 IP 限流。
pub fn check_rate(
    state: &SearchState,
    viewer: Option<&Principal>,
    ip: IpAddr,
) -> Result<(), Problem> {
    let result = match viewer {
        Some(p) => state.limits.per_user.check_key(&p.user_id.get()),
        None => state.limits.per_ip.check_key(&ip),
    };
    result.map_err(|denied| {
        let wait = denied.wait_time_from(DefaultClock::default().now());
        Problem::rate_limited().with_retry_after(wait.as_secs().saturating_add(1))
    })
}

async fn excluded_authors(
    state: &SearchState,
    viewer: Option<&Principal>,
) -> Result<Vec<i64>, Problem> {
    let mut conn = state.content.db.acquire().await?;
    ik_social::interop::excluded_authors(&mut conn, viewer.map(|p| p.user_id.get())).await
}

/// 回查数据库取卡片，保持命中顺序；分区以数据库为准（索引可能滞后）。
async fn post_cards(
    state: &SearchState,
    viewer: Option<&Principal>,
    ids: &[i64],
    category: Option<&str>,
) -> Result<Vec<PostCard>, Problem> {
    let mut cards = ik_content::service::public_cards_in_order(&state.content, viewer, ids).await?;
    if let Some(slug) = category {
        cards.retain(|c| c.category.as_ref().is_some_and(|cat| cat.slug == slug));
    }
    Ok(cards)
}

/// 帖子搜索（SRCH-010～016）。
pub async fn search_posts(
    state: &SearchState,
    viewer: Option<&Principal>,
    query: PostSearchQuery,
) -> Result<SearchPage<PostCard>, Problem> {
    let offset = decode_offset(query.cursor.as_deref())?;
    let q = normalize_query(query.q.as_deref(), POST_QUERY_MAX_CHARS);
    if q.is_empty() {
        return Ok(SearchPage::empty());
    }
    let limit = page_limit(query.limit);
    let sort = parse_sort(query.sort.as_deref());
    let category = non_empty(query.category.as_deref());

    if let Some(engine) = &state.engine {
        let excluded = excluded_authors(state, viewer).await?;
        match engine
            .search_posts(&q, category, &excluded, sort, offset, limit)
            .await
        {
            Ok(Hits { ids, total }) if ids.is_empty() => {
                return Ok(SearchPage {
                    items: Vec::new(),
                    next_cursor: None,
                    total: total_of(total),
                });
            }
            Ok(Hits { ids, total }) => {
                let items = post_cards(state, viewer, &ids, category).await?;
                if !items.is_empty() {
                    return Ok(SearchPage {
                        items,
                        next_cursor: next_cursor(offset, ids.len(), total)?,
                        total: total_of(total),
                    });
                }
                tracing::warn!(hits = ids.len(), "帖子搜索命中全是索引残留，改走数据库");
            }
            Err(error) => tracing::warn!(?error, "帖子搜索引擎不可用，改走数据库"),
        }
    }

    if q.chars().count() < FALLBACK_MIN_CHARS {
        return Ok(SearchPage::empty());
    }
    let ids = ik_content::service::search_post_ids(
        &state.content,
        viewer,
        &q,
        category,
        false,
        sort == PostSort::Hot,
    )
    .await?;
    let window: Vec<i64> = ids.iter().skip(offset).take(limit).copied().collect();
    let items = post_cards(state, viewer, &window, category).await?;
    Ok(SearchPage {
        items,
        next_cursor: next_cursor(offset, window.len(), ids.len())?,
        total: total_of(ids.len()),
    })
}

/// 标题联想（SRCH-020～022）。
pub async fn suggest(
    state: &SearchState,
    viewer: Option<&Principal>,
    query: SuggestQuery,
) -> Result<Vec<SuggestItem>, Problem> {
    let q = normalize_query(query.q.as_deref(), POST_QUERY_MAX_CHARS);
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let limit =
        usize::try_from(query.limit.unwrap_or(DEFAULT_SUGGEST).clamp(1, MAX_SUGGEST)).unwrap_or(8);
    let category = non_empty(query.category.as_deref());

    if let Some(engine) = &state.engine {
        let excluded = excluded_authors(state, viewer).await?;
        match engine.suggest_posts(&q, category, &excluded, limit).await {
            Ok(hits) if hits.is_empty() => return Ok(Vec::new()),
            Ok(hits) => {
                let ids: Vec<i64> = hits.iter().map(|h| h.id).collect();
                let cards = post_cards(state, viewer, &ids, category).await?;
                if !cards.is_empty() {
                    let mut marks: std::collections::HashMap<i64, _> =
                        hits.into_iter().map(|h| (h.id, h)).collect();
                    return Ok(cards
                        .into_iter()
                        .map(|card| {
                            let hit = marks.remove(&card.id.get());
                            // 标题以数据库为准；索引里的标题过期时不用它的高亮。
                            let highlighted = hit
                                .as_ref()
                                .and_then(|h| h.title_marked.as_deref())
                                .filter(|marked| strip_marks(marked) == card.title)
                                .map_or_else(|| escape_html(&card.title), highlight_html);
                            let excerpt = hit
                                .and_then(|h| h.excerpt)
                                .map(|e| strip_marks(&e))
                                .unwrap_or_default();
                            suggest_item(card, highlighted, excerpt)
                        })
                        .collect());
                }
                tracing::warn!("联想命中全是索引残留，改走数据库");
            }
            Err(error) => tracing::warn!(?error, "联想时搜索引擎不可用，改走数据库"),
        }
    }

    if q.chars().count() < FALLBACK_MIN_CHARS {
        return Ok(Vec::new());
    }
    let ids =
        ik_content::service::search_post_ids(&state.content, viewer, &q, category, true, false)
            .await?;
    let window: Vec<i64> = ids.into_iter().take(limit).collect();
    Ok(post_cards(state, viewer, &window, category)
        .await?
        .into_iter()
        .map(|card| {
            let highlighted = escape_html(&card.title);
            suggest_item(card, highlighted, String::new())
        })
        .collect())
}

fn suggest_item(card: PostCard, title_highlighted: String, excerpt: String) -> SuggestItem {
    SuggestItem {
        id: card.id,
        title: card.title,
        title_highlighted,
        excerpt,
        author: card.author,
        category: card.category,
        is_anonymous: card.is_anonymous,
    }
}

/// 评论搜索（SRCH-030～034）。
pub async fn search_comments(
    state: &SearchState,
    viewer: Option<&Principal>,
    query: CommentSearchQuery,
) -> Result<SearchPage<CommentSearchView>, Problem> {
    let offset = decode_offset(query.cursor.as_deref())?;
    let q = normalize_query(query.q.as_deref(), COMMENT_QUERY_MAX_CHARS);
    if q.is_empty() {
        return Ok(SearchPage::empty());
    }
    let limit = page_limit(query.limit);
    let post_id = query.post_id.map(Id::get);

    if let Some(engine) = &state.engine {
        let excluded = excluded_authors(state, viewer).await?;
        match engine
            .search_comments(&q, post_id, &excluded, offset, limit)
            .await
        {
            Ok(Hits { ids, total }) if ids.is_empty() => {
                return Ok(SearchPage {
                    items: Vec::new(),
                    next_cursor: None,
                    total: total_of(total),
                });
            }
            Ok(Hits { ids, total }) => {
                let items =
                    ik_comments::service::search_views(&state.comments, viewer, &ids).await?;
                if !items.is_empty() {
                    return Ok(SearchPage {
                        items,
                        next_cursor: next_cursor(offset, ids.len(), total)?,
                        total: total_of(total),
                    });
                }
                tracing::warn!(hits = ids.len(), "评论搜索命中全是索引残留，改走数据库");
            }
            Err(error) => tracing::warn!(?error, "评论搜索引擎不可用，改走数据库"),
        }
    }

    if q.chars().count() < FALLBACK_MIN_CHARS {
        return Ok(SearchPage::empty());
    }
    let ids =
        ik_comments::service::search_comment_ids(&state.comments, viewer, &q, post_id).await?;
    let window: Vec<i64> = ids.iter().skip(offset).take(limit).copied().collect();
    let items = ik_comments::service::search_views(&state.comments, viewer, &window).await?;
    Ok(SearchPage {
        items,
        next_cursor: next_cursor(offset, window.len(), ids.len())?,
        total: total_of(ids.len()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_parsing_is_lenient() {
        assert_eq!(parse_sort(Some(" LATEST ")), PostSort::Latest);
        assert_eq!(parse_sort(Some("hot")), PostSort::Hot);
        assert_eq!(parse_sort(Some("whatever")), PostSort::Relevance);
        assert_eq!(parse_sort(None), PostSort::Relevance);
    }

    #[test]
    fn offsets_are_bounded() {
        assert_eq!(decode_offset(None).unwrap(), 0);
        let deep = cursor::encode(&OffsetCursor { o: 1001 }).unwrap();
        assert!(decode_offset(Some(&deep)).is_err());
        assert!(decode_offset(Some("garbage")).is_err());
        assert_eq!(next_cursor(0, 20, 20).unwrap(), None);
        assert_eq!(next_cursor(0, 0, 100).unwrap(), None);
        assert_eq!(next_cursor(980, 20, 5000).unwrap(), None, "到 1000 为止");
        let next = next_cursor(20, 20, 100).unwrap().unwrap();
        assert_eq!(decode_offset(Some(&next)).unwrap(), 40);
    }
}
