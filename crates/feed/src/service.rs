//! 信息流。业务规则：`docs/business-rules/feed-search.md` 第 3 节。

use ik_auth::Principal;
use ik_content::dto::{FeedQuery, HomeFeedPage, MarkReadRequest, MarkReadResult, PostCard};
use ik_kernel::Problem;
use ik_kernel::cursor::{self, Page};
use serde::{Deserialize, Serialize};
use utoipa::IntoParams;

use crate::FeedState;

const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 50;

/// 首页：第一页带置顶与热门位，之后是按最近活跃倒序的最新流（FEED-140～149）。
pub async fn home(
    state: &FeedState,
    viewer: Option<&Principal>,
    query: FeedQuery,
) -> Result<HomeFeedPage, Problem> {
    let board = state.board();
    ik_content::service::home_feed(&state.content, viewer, query, &board.display).await
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HotQuery {
    pub cursor: Option<String>,
    /// 默认 20，最大 50。
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize)]
struct HotCursor {
    o: usize,
}

/// 热门浏览流：完整榜单（不限流、不掺置顶），按当前读者的可见性过滤**之后**再分页（FEED-111、EDGE-006）。
/// 榜单每 10 分钟刷新，刷新前后翻页可能出现少量重复或遗漏（EDGE-005，接受）。
pub async fn hot(
    state: &FeedState,
    viewer: Option<&Principal>,
    query: HotQuery,
) -> Result<Page<PostCard>, Problem> {
    let limit =
        usize::try_from(query.limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)).unwrap_or(20);
    let offset = query
        .cursor
        .as_deref()
        .map(cursor::decode::<HotCursor>)
        .transpose()?
        .map_or(0, |c| c.o);
    let board = state.board();
    let visible =
        ik_content::service::public_cards_in_order(&state.content, viewer, &board.full).await?;
    let total = visible.len();
    let items: Vec<PostCard> = visible.into_iter().skip(offset).take(limit).collect();
    let next = offset.saturating_add(items.len());
    let next_cursor = if !items.is_empty() && next < total {
        Some(cursor::encode(&HotCursor { o: next })?)
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}

/// 关注流（需要登录）。
pub async fn following(
    state: &FeedState,
    principal: &Principal,
    query: FeedQuery,
) -> Result<Page<PostCard>, Problem> {
    ik_content::service::following_feed(&state.content, principal, query).await
}

/// 未看流（需要登录）。
pub async fn unread(
    state: &FeedState,
    principal: &Principal,
    query: FeedQuery,
) -> Result<Page<PostCard>, Problem> {
    ik_content::service::unread_feed(&state.content, principal, query).await
}

/// 批量标为已读。
pub async fn mark_read(
    state: &FeedState,
    principal: &Principal,
    request: MarkReadRequest,
) -> Result<MarkReadResult, Problem> {
    ik_content::service::mark_read(&state.content, principal, request).await
}
