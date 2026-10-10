//! 评论业务逻辑。业务规则：`docs/business-rules/comments.md`。
//!
//! 加锁顺序固定为「帖子行 → 评论行」，所有写操作都先锁帖子，避免死锁，
//! 也让同一帖子下的楼层分配、置顶、采纳天然串行。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use governor::clock::{Clock, DefaultClock};
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::service::Author;
use ik_content::interop::{self, PostRef};
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::cursor::{self, Page};
use ik_kernel::{Id, Problem};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::CommentsState;
use crate::dto::{CommentView, CommentsPage, CreateCommentRequest, PageQuery, ThreadView};
use crate::repo::{self, CommentRow, NewComment};
use crate::text;

const INLINE_REPLIES: i64 = 3;
const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 50;

// ---------- 错误 ----------

fn comment_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "评论不存在或已被隐藏")
}

fn post_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "帖子不存在或已被删除")
}

fn not_top_level() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "COMMENT_NOT_TOP_LEVEL",
        "只有一级评论可以置顶或采纳",
    )
}

fn accept_disabled() -> Problem {
    Problem::new(
        StatusCode::FORBIDDEN,
        "ACCEPT_DISABLED",
        "该分区未开启评论采纳",
    )
}

/// 帖主或管理员才能置顶、采纳（CMT-038）。
fn ensure_moderator_of(principal: &Principal, post: &PostRef) -> Result<(), Problem> {
    if post.author_id == principal.user_id.get() || principal.role.is_admin() {
        Ok(())
    } else {
        Err(Problem::forbidden())
    }
}

fn bumped_event(post_id: i64) -> Result<Event, Problem> {
    Event::new(
        "post.bumped",
        Audience::Broadcast,
        &serde_json::json!({ "postId": post_id.to_string() }),
    )
}

// ---------- 发表 ----------

/// 校验正文并展开裸 @。展开后再查一次长度（D-033 方案 A）。
async fn prepare_body(state: &CommentsState, raw: &str) -> Result<String, Problem> {
    let body = raw.trim();
    if body.is_empty() {
        return Err(Problem::field("body", "REQUIRED", "评论内容不能为空"));
    }
    let too_long = || {
        Problem::field(
            "body",
            "TOO_LONG",
            format!("评论最多 {} 字", text::MAX_BODY_CHARS),
        )
    };
    if body.chars().count() > text::MAX_BODY_CHARS {
        return Err(too_long());
    }
    if text::emote_count(body) > text::MAX_EMOTES {
        return Err(Problem::field(
            "body",
            "TOO_MANY_EMOTES",
            format!("一条评论最多 {} 个表情", text::MAX_EMOTES),
        ));
    }
    let names = text::bare_mention_names(body);
    let resolved: HashMap<String, i64> =
        ik_auth::service::active_user_ids_by_usernames(&state.db, &names)
            .await?
            .into_iter()
            .map(|(name, id)| (name, id.get()))
            .collect();
    let expanded = text::expand_bare_mentions(body, &resolved);
    if expanded.chars().count() > text::MAX_BODY_CHARS {
        return Err(too_long());
    }
    Ok(expanded)
}

fn check_rate(state: &CommentsState, principal: &Principal) -> Result<(), Problem> {
    state
        .limiter
        .check_key(&principal.user_id.get())
        .map_err(|denied| {
            let wait = denied.wait_time_from(DefaultClock::default().now());
            Problem::rate_limited().with_retry_after(wait.as_secs().saturating_add(1))
        })
}

/// 发表评论或回复。需要通过入站考试；帖子必须公开（作者本人也不能评论自己被隐藏的帖子，CMT-030）。
pub async fn create(
    state: &CommentsState,
    principal: &Principal,
    post_id: i64,
    request: CreateCommentRequest,
) -> Result<CommentView, Problem> {
    principal.require_write()?;
    check_rate(state, principal)?;
    let body = prepare_body(state, &request.body).await?;
    let seed = if request.is_anonymous {
        Some(ik_content::anonymous::new_seed()?)
    } else {
        None
    };
    let id = state.ids.next_raw();
    let author_id = principal.user_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();

    let mut tx = state.db.begin().await?;
    let post = interop::lock_post(&mut tx, post_id)
        .await?
        .filter(|p| p.is_public)
        .ok_or_else(post_not_found)?;
    let parent_id = request.parent_id.map(Id::get);
    if let Some(parent_id) = parent_id {
        let valid = repo::get(&mut tx, parent_id).await?.is_some_and(|p| {
            p.post_id == post.id && p.parent_id.is_none() && p.hidden_at.is_none()
        });
        if !valid {
            return Err(Problem::field(
                "parentId",
                "INVALID",
                "只能回复本帖下未隐藏的一级评论",
            ));
        }
    }
    let floor = match parent_id {
        None => Some(repo::next_floor(&mut tx, post.id).await?),
        Some(_) => None,
    };
    repo::insert(
        &mut tx,
        &NewComment {
            id,
            post_id: post.id,
            parent_id,
            author_id,
            floor,
            body: &body,
            is_anonymous: request.is_anonymous,
            anonymous_seed: seed.as_deref(),
            now,
        },
    )
    .await?;
    interop::comment_added(&mut tx, post.id, author_id, now).await?;
    effects.publish(bumped_event(post.id)?);
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());

    let authors = if request.is_anonymous {
        HashMap::new()
    } else {
        ik_auth::service::authors(&state.db, &[author_id]).await?
    };
    let row = CommentRow {
        id,
        post_id: post.id,
        parent_id,
        author_id,
        floor,
        body,
        is_anonymous: request.is_anonymous,
        anonymous_seed: seed,
        like_count: 0,
        pinned_at: None,
        accepted_at: None,
        hidden_at: None,
        created_at: now,
    };
    Ok(view(&row, &authors, Some(author_id)))
}

// ---------- 删除 ----------

/// 只有作者本人能删（D-035 方案 A）；不需要通过考试（CMT-036）。一级评论连同回复一起删除。
/// 删除被采纳的评论时同时清除帖子的「已解决」（D-027 方案 A）。
pub async fn delete(
    state: &CommentsState,
    principal: &Principal,
    comment_id: i64,
) -> Result<(), Problem> {
    let mut tx = state.db.begin().await?;
    let post_id = repo::get(&mut tx, comment_id)
        .await?
        .ok_or_else(comment_not_found)?
        .post_id;
    interop::lock_post(&mut tx, post_id).await?;
    let comment = repo::lock(&mut tx, comment_id)
        .await?
        .ok_or_else(comment_not_found)?;
    if comment.author_id != principal.user_id.get() {
        return Err(Problem::forbidden());
    }
    let replies = repo::count_all_replies(&mut tx, comment.id).await?;
    // 必须在删除之前：外键 ON DELETE SET NULL 会先把帖子的 accepted_comment_id 置空，
    // 之后就匹配不到这条评论了。
    if comment.accepted_at.is_some() {
        interop::clear_solved(&mut tx, post_id, comment.id).await?;
    }
    repo::delete(&mut tx, comment.id).await?;
    let removed = i32::try_from(replies.saturating_add(1)).unwrap_or(i32::MAX);
    interop::comments_removed(&mut tx, post_id, removed).await?;
    tx.commit().await?;
    Ok(())
}

// ---------- 置顶与采纳 ----------

/// 锁住帖子与评论，返回 (帖子, 评论)。
async fn lock_for_moderation(
    conn: &mut PgConnection,
    comment_id: i64,
) -> Result<(PostRef, CommentRow), Problem> {
    let post_id = repo::get(conn, comment_id)
        .await?
        .ok_or_else(comment_not_found)?
        .post_id;
    let post = interop::lock_post(conn, post_id)
        .await?
        .ok_or_else(comment_not_found)?;
    let comment = repo::lock(conn, comment_id)
        .await?
        .ok_or_else(comment_not_found)?;
    Ok((post, comment))
}

/// 置顶（幂等）。每帖最多一条，置顶新的会替换旧的（CMT-053/054）。
pub async fn pin(
    state: &CommentsState,
    principal: &Principal,
    comment_id: i64,
) -> Result<(), Problem> {
    principal.require_write()?;
    let mut tx = state.db.begin().await?;
    let (post, comment) = lock_for_moderation(&mut tx, comment_id).await?;
    ensure_moderator_of(principal, &post)?;
    if comment.hidden_at.is_some() {
        return Err(comment_not_found());
    }
    if comment.parent_id.is_some() {
        return Err(not_top_level());
    }
    if comment.pinned_at.is_none() {
        repo::clear_pins(&mut tx, post.id).await?;
        repo::set_pin(&mut tx, comment.id, principal.user_id.get(), Utc::now()).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 取消置顶（幂等）。
pub async fn unpin(
    state: &CommentsState,
    principal: &Principal,
    comment_id: i64,
) -> Result<(), Problem> {
    principal.require_write()?;
    let mut tx = state.db.begin().await?;
    let (post, comment) = lock_for_moderation(&mut tx, comment_id).await?;
    ensure_moderator_of(principal, &post)?;
    if comment.pinned_at.is_some() {
        repo::unpin(&mut tx, comment.id).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 采纳为最佳答案。分区必须开启采纳（管理员也不例外，且先于身份检查，CMT-039）。
/// 再次采纳同一条是空操作（D-030 方案 A）；采纳另一条会替换旧的。
pub async fn accept(
    state: &CommentsState,
    principal: &Principal,
    comment_id: i64,
) -> Result<(), Problem> {
    principal.require_write()?;
    let mut tx = state.db.begin().await?;
    let (post, comment) = lock_for_moderation(&mut tx, comment_id).await?;
    if !post.allow_comment_accept {
        return Err(accept_disabled());
    }
    ensure_moderator_of(principal, &post)?;
    if comment.hidden_at.is_some() {
        return Err(comment_not_found());
    }
    if comment.parent_id.is_some() {
        return Err(not_top_level());
    }
    if comment.accepted_at.is_none() {
        let now = Utc::now();
        repo::clear_accepts(&mut tx, post.id).await?;
        repo::set_accept(&mut tx, comment.id, principal.user_id.get(), now).await?;
        interop::mark_solved(&mut tx, post.id, comment.id, now).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// 取消采纳（幂等）。不检查分区开关：分区关闭后存量答案仍要能撤销（CMT-040）。
pub async fn unaccept(
    state: &CommentsState,
    principal: &Principal,
    comment_id: i64,
) -> Result<(), Problem> {
    principal.require_write()?;
    let mut tx = state.db.begin().await?;
    let (post, comment) = lock_for_moderation(&mut tx, comment_id).await?;
    ensure_moderator_of(principal, &post)?;
    if comment.accepted_at.is_some() {
        repo::unaccept(&mut tx, comment.id).await?;
        interop::clear_solved(&mut tx, post.id, comment.id).await?;
    }
    tx.commit().await?;
    Ok(())
}

// ---------- 读取 ----------

fn view(row: &CommentRow, authors: &HashMap<i64, Author>, viewer_id: Option<i64>) -> CommentView {
    CommentView {
        id: Id::new(row.id),
        post_id: Id::new(row.post_id),
        parent_id: row.parent_id.map(Id::new),
        floor: row.floor,
        body: row.body.clone(),
        author: ik_content::service::author_view(
            row.author_id,
            row.is_anonymous,
            row.anonymous_seed.as_deref(),
            authors,
        ),
        is_anonymous: row.is_anonymous,
        like_count: row.like_count,
        is_pinned: row.pinned_at.is_some(),
        is_accepted: row.accepted_at.is_some(),
        is_mine: viewer_id == Some(row.author_id),
        created_at: row.created_at,
    }
}

/// 只查非匿名评论的作者：匿名评论的真实作者不进入任何后续流程。
async fn author_map(
    state: &CommentsState,
    rows: &[&CommentRow],
) -> Result<HashMap<i64, Author>, Problem> {
    let mut ids: Vec<i64> = rows
        .iter()
        .filter(|r| !r.is_anonymous)
        .map(|r| r.author_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ik_auth::service::authors(&state.db, &ids).await
}

#[derive(Serialize, Deserialize)]
struct CommentCursor {
    c: DateTime<Utc>,
    i: i64,
}

fn page_limit(limit: Option<u32>) -> i64 {
    i64::from(limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE))
}

fn decode_cursor(raw: Option<&str>) -> Result<Option<(DateTime<Utc>, i64)>, Problem> {
    Ok(raw
        .map(cursor::decode::<CommentCursor>)
        .transpose()?
        .map(|c| (c.c, c.i)))
}

async fn readable_post(
    conn: &mut PgConnection,
    post_id: i64,
    viewer_id: Option<i64>,
) -> Result<PostRef, Problem> {
    interop::post(conn, post_id)
        .await?
        .filter(|p| p.readable_by(viewer_id))
        .ok_or_else(post_not_found)
}

/// 帖子的一级评论。第一页额外返回采纳与置顶（各最多一条），它们不再出现在 `items` 里。
pub async fn list(
    state: &CommentsState,
    viewer: Option<&Principal>,
    post_id: i64,
    query: PageQuery,
) -> Result<CommentsPage, Problem> {
    let viewer_id = viewer.map(|p| p.user_id.get());
    let limit = page_limit(query.limit);
    let after = decode_cursor(query.cursor.as_deref())?;
    let mut conn = state.db.acquire().await?;
    readable_post(&mut conn, post_id, viewer_id).await?;

    let (accepted, pinned) = if after.is_none() {
        (
            repo::accepted(&mut conn, post_id).await?,
            repo::pinned(&mut conn, post_id).await?,
        )
    } else {
        (None, None)
    };
    let mut rows = repo::top_level_page(&mut conn, post_id, after, limit + 1).await?;
    let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));

    let tops: Vec<&CommentRow> = accepted
        .iter()
        .chain(pinned.iter())
        .chain(rows.iter())
        .collect();
    let top_ids: Vec<i64> = tops.iter().map(|r| r.id).collect();
    let replies = repo::inline_replies(&mut conn, &top_ids, INLINE_REPLIES).await?;
    let counts: HashMap<i64, i64> = repo::visible_reply_counts(&mut conn, &top_ids)
        .await?
        .into_iter()
        .map(|c| (c.parent_id, c.n))
        .collect();
    let everyone: Vec<&CommentRow> = tops.iter().copied().chain(replies.iter()).collect();
    let authors = author_map(state, &everyone).await?;

    let thread = |row: &CommentRow| {
        let replies: Vec<CommentView> = replies
            .iter()
            .filter(|r| r.parent_id == Some(row.id))
            .map(|r| view(r, &authors, viewer_id))
            .collect();
        let reply_count = counts.get(&row.id).copied().unwrap_or(0);
        ThreadView {
            comment: view(row, &authors, viewer_id),
            replies_has_more: reply_count > i64::try_from(replies.len()).unwrap_or(i64::MAX),
            replies,
            reply_count,
        }
    };

    let next_cursor = match (has_more, rows.last()) {
        (true, Some(last)) => Some(cursor::encode(&CommentCursor {
            c: last.created_at,
            i: last.id,
        })?),
        _ => None,
    };
    Ok(CommentsPage {
        accepted: accepted.as_ref().map(thread),
        pinned: pinned.as_ref().map(thread),
        items: rows.iter().map(thread).collect(),
        next_cursor,
    })
}

/// 一级评论的回复，旧的在前。目标不存在、被隐藏、本身是回复、或帖子不可见时 404（CMT-074）。
pub async fn replies(
    state: &CommentsState,
    viewer: Option<&Principal>,
    comment_id: i64,
    query: PageQuery,
) -> Result<Page<CommentView>, Problem> {
    let viewer_id = viewer.map(|p| p.user_id.get());
    let limit = page_limit(query.limit);
    let after = decode_cursor(query.cursor.as_deref())?;
    let mut conn = state.db.acquire().await?;
    let parent = repo::get(&mut conn, comment_id)
        .await?
        .filter(|c| c.parent_id.is_none() && c.hidden_at.is_none())
        .ok_or_else(comment_not_found)?;
    readable_post(&mut conn, parent.post_id, viewer_id).await?;

    let rows = repo::replies_page(&mut conn, parent.id, after, limit + 1).await?;
    let authors = author_map(state, &rows.iter().collect::<Vec<_>>()).await?;
    let items: Vec<CommentView> = rows.iter().map(|r| view(r, &authors, viewer_id)).collect();
    Page::from_rows(
        items,
        usize::try_from(limit).unwrap_or(20),
        |item: &CommentView| CommentCursor {
            c: item.created_at,
            i: item.id.get(),
        },
    )
}
