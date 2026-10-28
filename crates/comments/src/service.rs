//! 评论业务逻辑。业务规则：`docs/business-rules/comments.md`。
//!
//! 加锁顺序固定为「帖子行 → 评论行」，所有写操作都先锁帖子，避免死锁，
//! 也让同一帖子下的楼层分配、置顶、采纳天然串行。

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use governor::clock::{Clock, DefaultClock};
use http::StatusCode;
use ik_auth::Principal;
use ik_content::interop::{self, PostRef};
use ik_economy::service::{Dedup, ExpAction, grant_exp};
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::cursor::{self, Page};
use ik_kernel::{Id, Problem, Snowflake};
use ik_notify::interop::{Kind, Notice, Target};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::CommentsState;
use crate::dto::{
    AuthorCommentView, CommentLikeResult, CommentSearchView, CommentSort, CommentView,
    CommentsPage, CommentsQuery, CreateCommentRequest, PageQuery, QuotedComment, ReplyToView,
};
use crate::repo::{self, CommentRow, NewComment};
use crate::text;

/// 被回复评论的摘要最多多少字。
const QUOTE_CHARS: usize = 80;
const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 50;

// ---------- 错误 ----------

fn comment_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "评论不存在或已被隐藏")
}

fn post_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "帖子不存在或已被删除")
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

/// 顶帖广播不推给顶帖人自己：自己的评论不算信息流里的「新内容」。
pub(crate) fn bumped_event(post_id: i64, by: i64) -> Result<Event, Problem> {
    Event::new(
        "post.bumped",
        Audience::BroadcastExcept(by),
        &serde_json::json!({ "postId": post_id.to_string() }),
    )
}

// ---------- 发表 ----------

/// 校验正文并展开裸 @。展开后再查一次长度（D-033 方案 A）。
async fn prepare_body(
    state: &CommentsState,
    raw: &str,
    has_images: bool,
) -> Result<String, Problem> {
    let body = raw.trim();
    if body.is_empty() {
        // 纯图片评论可以没有正文（CMT-006）。
        if has_images {
            return Ok(String::new());
        }
        return Err(Problem::field(
            "body",
            "REQUIRED",
            "评论内容和图片不能都为空",
        ));
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

pub(crate) struct NewCommentNotice<'a> {
    pub post: &'a PostRef,
    pub reply_to: Option<&'a CommentRow>,
    pub comment_id: i64,
    pub author_id: i64,
    pub body: &'a str,
    pub is_anonymous: bool,
}

/// 新评论的通知：直接评论帖子通知帖主（`comment`），回复某条评论通知它的作者（`reply`），
/// 正文里 @ 到的人各一条（`mention`，最多 10 人，已经收到前两种通知的人不重复）。
/// 评论、帖子、被回复的评论任一匿名就都不发（CMT-129）。自己、拉黑、收件人不可用由 notify 抑制。
pub(crate) async fn notify_new_comment(
    ids: &Snowflake,
    conn: &mut PgConnection,
    c: &NewCommentNotice<'_>,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<(), Problem> {
    if c.is_anonymous || c.post.is_anonymous || c.reply_to.is_some_and(|r| r.is_anonymous) {
        return Ok(());
    }
    // 纯图片评论的预览统一为「[图片]」（Q-08）。
    let excerpt = if c.body.is_empty() {
        "[图片]".to_owned()
    } else {
        text::plain_text(c.body)
    };
    let (primary, kind) = match c.reply_to {
        None => (c.post.author_id, Kind::Comment),
        Some(target) => (target.author_id, Kind::Reply),
    };
    let notice = |recipient_id, kind| Notice {
        recipient_id,
        sender_id: c.author_id,
        kind,
        post_id: Some(c.post.id),
        comment_id: Some(c.comment_id),
        post_title: Some(&c.post.title),
        excerpt: &excerpt,
    };
    ik_notify::interop::send(conn, ids, notice(primary, kind), now, effects).await?;
    for target in text::mentioned_user_ids(c.body) {
        if target != primary {
            ik_notify::interop::send(conn, ids, notice(target, Kind::Mention), now, effects)
                .await?;
        }
    }
    Ok(())
}

/// 发表评论或回复。需要通过入站考试；帖子必须公开（作者本人也不能评论自己被隐藏的帖子，CMT-030）。
/// 后台「内容与安全」的规则：屏蔽词（拦截或替换），新账号不能贴链接、24 小时内评论条数有上限。
async fn apply_site_rules(
    state: &CommentsState,
    conn: &mut PgConnection,
    principal: &Principal,
    body: String,
    now: DateTime<Utc>,
) -> Result<String, Problem> {
    let Some(site) = &state.site else {
        return Ok(body);
    };
    if ik_auth::guard::exempt(principal) {
        return Ok(body);
    }
    let values = site.get().await;
    let body = ik_auth::guard::screen(&values, "body", &body)?;
    if let Some(since) = ik_auth::guard::new_user_window(conn, &values, principal, now).await? {
        if !values.new_user.allow_links && ik_kernel::site::has_link(&body) {
            return Err(ik_auth::guard::links_not_allowed("body"));
        }
        let limit = values.new_user.comments_per_day;
        if limit > 0
            && repo::count_since(conn, principal.user_id.get(), since).await? >= i64::from(limit)
        {
            return Err(ik_auth::guard::new_user_limit("发评论", limit));
        }
    }
    Ok(body)
}

pub async fn create(
    state: &CommentsState,
    principal: &Principal,
    post_id: i64,
    request: CreateCommentRequest,
) -> Result<CommentView, Problem> {
    principal.require_write()?;
    check_rate(state, principal)?;
    // 按原始条数拦截（不查库，CMT-021），通过后去重保序。
    let limit = usize::from(
        ik_economy::benefits::for_level(principal.benefit_level().max(1)).comment_max_images,
    );
    if request.image_ids.len() > limit {
        return Err(Problem::field(
            "imageIds",
            "TOO_MANY",
            format!("当前等级评论图片最多 {limit} 张"),
        ));
    }
    let mut image_ids: Vec<i64> = Vec::with_capacity(request.image_ids.len());
    for id in &request.image_ids {
        if !image_ids.contains(&id.get()) {
            image_ids.push(id.get());
        }
    }
    let body = prepare_body(state, &request.body, !image_ids.is_empty()).await?;
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
    let body = apply_site_rules(state, &mut tx, principal, body, now).await?;
    let post = interop::lock_post(&mut tx, post_id)
        .await?
        .filter(|p| p.is_public)
        .ok_or_else(post_not_found)?;
    // 图片必须是自己图库里的（D-012 方案 A）。
    if !image_ids.is_empty() {
        let owned = ik_media::interop::owned_media(&mut tx, author_id, &image_ids).await?;
        if !image_ids.iter().all(|id| owned.contains(id)) {
            return Err(Problem::field(
                "imageIds",
                "NOT_FOUND",
                "图片不存在或不在你的图库里",
            ));
        }
    }
    // 拉黑拦截只看被评论 / 被回复的内容是否匿名（BLOCK-015），匿名内容不拦，防止反推作者。
    if !post.is_anonymous {
        ik_social::interop::ensure_no_block(&mut tx, author_id, post.author_id).await?;
    }
    let reply_to = match request.reply_to_id.map(Id::get) {
        None => None,
        Some(target_id) => {
            let target = repo::get(&mut tx, target_id)
                .await?
                .filter(|t| t.post_id == post.id && t.hidden_at.is_none());
            let Some(target) = target else {
                return Err(Problem::field(
                    "replyToId",
                    "INVALID",
                    "只能回复本帖下未隐藏的评论",
                ));
            };
            if !target.is_anonymous {
                ik_social::interop::ensure_no_block(&mut tx, author_id, target.author_id).await?;
            }
            Some(target)
        }
    };
    let floor = interop::comment_added(&mut tx, post.id, author_id, now).await?;
    repo::insert(
        &mut tx,
        &NewComment {
            id,
            post_id: post.id,
            reply_to: reply_to.as_ref().map(|t| (t.id, t.floor)),
            author_id,
            floor,
            body: &body,
            is_anonymous: request.is_anonymous,
            anonymous_seed: seed.as_deref(),
            image_ids: &image_ids,
            now,
        },
    )
    .await?;
    // 在别人的帖子下评论：每个业务日第一条 +3 经验；匿名评论同样给（CMT-101）。
    if post.author_id != author_id {
        grant_exp(
            &mut tx,
            &state.ids,
            author_id,
            ExpAction::CreateComment,
            3,
            Dedup::Daily,
            now,
        )
        .await?;
    }
    notify_new_comment(
        &state.ids,
        &mut tx,
        &NewCommentNotice {
            post: &post,
            reply_to: reply_to.as_ref(),
            comment_id: id,
            author_id,
            body: &body,
            is_anonymous: request.is_anonymous,
        },
        now,
        &mut effects,
    )
    .await?;
    effects.publish(bumped_event(post.id, author_id)?);
    if let Some(hooks) = state.hooks() {
        let created = crate::CreatedComment {
            id,
            post_id: post.id,
            post_author_id: post.author_id,
            post_is_anonymous: post.is_anonymous,
            reply_to: reply_to.as_ref().map(|t| crate::ReplyToRef {
                id: t.id,
                author_id: t.author_id,
                is_anonymous: t.is_anonymous,
            }),
            author_id,
            author_is_ai: principal.is_ai_agent,
            body: body.clone(),
            is_anonymous: request.is_anonymous,
            created_at: now,
        };
        hooks.created(&mut tx, &created, &mut effects).await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());

    let row = CommentRow {
        id,
        post_id: post.id,
        reply_to_id: reply_to.as_ref().map(|t| t.id),
        reply_to_floor: reply_to.as_ref().map(|t| t.floor),
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
        image_ids,
    };
    let quoted: Vec<CommentRow> = reply_to.into_iter().collect();
    let mut conn = state.db.acquire().await?;
    let lookup = lookup_with(state, &mut conn, &[&row], &quoted, HashSet::new()).await?;
    Ok(view(&row, &lookup, Some(author_id)))
}

// ---------- 删除 ----------

/// 只有作者本人能删（D-035 方案 A）；不需要通过考试（CMT-036）。回复它的评论保留，
/// 显示「原评论已删除」。删除被采纳的评论时同时清除帖子的「已解决」（D-027 方案 A）。
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
    remove_locked(&mut tx, post_id, &comment).await?;
    tx.commit().await?;
    // 内部事件：搜索索引删除这条评论。
    let mut effects = Effects::new();
    effects.publish(removed_event(comment.id, post_id)?);
    effects.dispatch(state.bus.as_ref());
    Ok(())
}

pub(crate) fn removed_event(comment_id: i64, post_id: i64) -> Result<Event, Problem> {
    Event::new(
        ik_kernel::bus::internal::COMMENT_REMOVED,
        Audience::Internal,
        &serde_json::json!({ "commentId": comment_id, "postId": post_id }),
    )
}

/// 删除已锁住的评论，并维护帖子的评论数与采纳。调用方已锁住帖子与评论。
pub(crate) async fn remove_locked(
    conn: &mut PgConnection,
    post_id: i64,
    comment: &CommentRow,
) -> Result<(), Problem> {
    // 必须在删除之前：外键 ON DELETE SET NULL 会先把帖子的 accepted_comment_id 置空，
    // 之后就匹配不到这条评论了。
    if comment.accepted_at.is_some() {
        interop::clear_solved(conn, post_id, comment.id).await?;
    }
    repo::delete(conn, comment.id).await?;
    interop::comments_removed(conn, post_id, 1).await?;
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
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let (post, comment) = lock_for_moderation(&mut tx, comment_id).await?;
    if !post.allow_comment_accept {
        return Err(accept_disabled());
    }
    ensure_moderator_of(principal, &post)?;
    if comment.hidden_at.is_some() {
        return Err(comment_not_found());
    }
    if comment.accepted_at.is_none() {
        let now = Utc::now();
        repo::clear_accepts(&mut tx, post.id).await?;
        repo::set_accept(&mut tx, comment.id, principal.user_id.get(), now).await?;
        interop::mark_solved(&mut tx, post.id, comment.id, now).await?;
        // 答主被别人采纳：+10 经验，按评论终身一次；匿名评论照常给（CMT-102/151）。
        if comment.author_id != principal.user_id.get() {
            grant_exp(
                &mut tx,
                &state.ids,
                comment.author_id,
                ExpAction::CommentAccepted,
                10,
                Dedup::Ref(format!("comment:{}", comment.id)),
                now,
            )
            .await?;
        }
        // 采纳通知：评论或帖子匿名时不发；采纳自己的评论由 notify 抑制。
        if !comment.is_anonymous && !post.is_anonymous {
            let excerpt = text::plain_text(&comment.body);
            ik_notify::interop::send(
                &mut tx,
                &state.ids,
                Notice {
                    recipient_id: comment.author_id,
                    sender_id: principal.user_id.get(),
                    kind: Kind::Accepted,
                    post_id: Some(post.id),
                    comment_id: Some(comment.id),
                    post_title: Some(&post.title),
                    excerpt: &excerpt,
                },
                now,
                &mut effects,
            )
            .await?;
        }
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
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

// ---------- 点赞 ----------

/// 点赞（幂等）。需要通过入站考试。评论必须可见：未隐藏、帖子公开（COMMON-007）；
/// 非匿名评论还要求双方没有拉黑（LIKE-007）。
/// 每个业务日第一次点赞（帖子与评论合计）+1 经验，赞自己的评论不给（LIKE-015）。
/// 通知评论作者；匿名评论不发（LIKE-013）。
pub async fn like(
    state: &CommentsState,
    principal: &Principal,
    comment_id: i64,
) -> Result<CommentLikeResult, Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let comment = repo::get(&mut tx, comment_id)
        .await?
        .filter(|c| c.hidden_at.is_none())
        .ok_or_else(comment_not_found)?;
    let post = interop::post(&mut tx, comment.post_id)
        .await?
        .filter(|p| p.is_public)
        .ok_or_else(comment_not_found)?;
    if !comment.is_anonymous {
        ik_social::interop::ensure_no_block(&mut tx, me, comment.author_id).await?;
    }
    let added = repo::insert_like(&mut tx, comment.id, me, now).await?;
    let like_count = repo::add_like_count(&mut tx, comment.id, i32::from(added))
        .await?
        .ok_or_else(comment_not_found)?;
    if added && comment.author_id != me {
        grant_exp(
            &mut tx,
            &state.ids,
            me,
            ExpAction::LikeGive,
            1,
            Dedup::Daily,
            now,
        )
        .await?;
    }
    if added && !comment.is_anonymous {
        let excerpt = text::plain_text(&comment.body);
        ik_notify::interop::send(
            &mut tx,
            &state.ids,
            Notice {
                recipient_id: comment.author_id,
                sender_id: me,
                kind: Kind::Like,
                post_id: Some(post.id),
                comment_id: Some(comment.id),
                post_title: Some(&post.title),
                excerpt: &excerpt,
            },
            now,
            &mut effects,
        )
        .await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(CommentLikeResult {
        liked: true,
        like_count,
    })
}

/// 取消点赞（幂等）。不要求通过考试；评论已经不可见时也能取消（LIKE-018）。点赞通知撤回（D-011）。
pub async fn unlike(
    state: &CommentsState,
    principal: &Principal,
    comment_id: i64,
) -> Result<CommentLikeResult, Problem> {
    let me = principal.user_id.get();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let removed = repo::delete_like(&mut tx, comment_id, me).await?;
    let like_count = repo::add_like_count(&mut tx, comment_id, -i32::from(removed))
        .await?
        .ok_or_else(comment_not_found)?;
    if removed {
        ik_notify::interop::retract(
            &mut tx,
            me,
            Kind::Like,
            Target::Comment(comment_id),
            &mut effects,
        )
        .await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(CommentLikeResult {
        liked: false,
        like_count,
    })
}

// ---------- 读取 ----------
/// 渲染评论需要的批量数据：作者（含头像）、图片、读者点过的赞、被回复的评论、收到的回复数。
struct Lookup {
    authors: ik_content::service::Authors,
    media: HashMap<i64, ik_media::dto::MediaView>,
    liked: HashSet<i64>,
    /// 被回复的评论中对读者可见的。
    quoted: HashMap<i64, CommentRow>,
    reply_counts: HashMap<i64, i64>,
}

impl Lookup {
    fn images(&self, row: &CommentRow) -> Vec<ik_media::dto::MediaView> {
        row.image_ids
            .iter()
            .filter_map(|id| self.media.get(id).cloned())
            .collect()
    }

    fn author(&self, row: &CommentRow) -> ik_content::dto::AuthorView {
        ik_content::service::author_view(
            row.author_id,
            row.is_anonymous,
            row.anonymous_seed.as_deref(),
            &self.authors,
        )
    }

    fn reply_to(&self, row: &CommentRow) -> Option<ReplyToView> {
        let floor = row.reply_to_floor?;
        Some(ReplyToView {
            floor,
            deleted: row.reply_to_id.is_none(),
            comment: row
                .reply_to_id
                .and_then(|id| self.quoted.get(&id))
                .map(|target| QuotedComment {
                    id: Id::new(target.id),
                    author: self.author(target),
                    excerpt: excerpt(&target.body),
                }),
        })
    }
}

/// 被回复评论的摘要：纯文本，最多 [`QUOTE_CHARS`] 字；纯图片评论为「[图片]」。
fn excerpt(body: &str) -> String {
    let text = text::plain_text(body);
    let text = text.trim();
    if text.is_empty() {
        return "[图片]".to_owned();
    }
    let mut chars = text.chars();
    let mut out: String = chars.by_ref().take(QUOTE_CHARS).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// 读者能看到的被回复评论：未隐藏，作者与读者没有拉黑关系（匿名评论不按拉黑过滤）。
async fn quoted_rows(
    conn: &mut PgConnection,
    rows: &[&CommentRow],
    excluded: &[i64],
) -> Result<Vec<CommentRow>, Problem> {
    let mut ids: Vec<i64> = rows.iter().filter_map(|r| r.reply_to_id).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut quoted = repo::by_ids(conn, &ids).await?;
    quoted
        .retain(|q| q.hidden_at.is_none() && (q.is_anonymous || !excluded.contains(&q.author_id)));
    Ok(quoted)
}

/// 一页评论的完整渲染数据。
async fn lookup(
    state: &CommentsState,
    conn: &mut PgConnection,
    viewer_id: Option<i64>,
    excluded: &[i64],
    rows: &[&CommentRow],
) -> Result<Lookup, Problem> {
    let liked = liked_set(conn, viewer_id, rows).await?;
    let quoted = quoted_rows(conn, rows, excluded).await?;
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let reply_counts = repo::visible_reply_counts(conn, &ids, excluded)
        .await?
        .into_iter()
        .map(|c| (c.reply_to_id, c.n))
        .collect();
    let mut lookup = lookup_with(state, conn, rows, &quoted, liked).await?;
    lookup.reply_counts = reply_counts;
    Ok(lookup)
}

/// 不需要查点赞与回复数时（刚发表的评论、搜索结果）用这个。
async fn lookup_with(
    state: &CommentsState,
    conn: &mut PgConnection,
    rows: &[&CommentRow],
    quoted: &[CommentRow],
    liked: HashSet<i64>,
) -> Result<Lookup, Problem> {
    let everyone: Vec<&CommentRow> = rows.iter().copied().chain(quoted.iter()).collect();
    let authors = author_map(state, conn, &everyone).await?;
    let mut media_ids: Vec<i64> = rows
        .iter()
        .flat_map(|r| r.image_ids.iter().copied())
        .collect();
    media_ids.sort_unstable();
    media_ids.dedup();
    let media = state.media.views(conn, &media_ids).await?;
    Ok(Lookup {
        authors,
        media,
        liked,
        quoted: quoted.iter().map(|q| (q.id, q.clone())).collect(),
        reply_counts: HashMap::new(),
    })
}

fn view(row: &CommentRow, lookup: &Lookup, viewer_id: Option<i64>) -> CommentView {
    CommentView {
        id: Id::new(row.id),
        post_id: Id::new(row.post_id),
        floor: row.floor,
        reply_to: lookup.reply_to(row),
        reply_count: lookup.reply_counts.get(&row.id).copied().unwrap_or(0),
        body: row.body.clone(),
        images: lookup.images(row),
        author: lookup.author(row),
        is_anonymous: row.is_anonymous,
        like_count: row.like_count,
        is_pinned: row.pinned_at.is_some(),
        is_accepted: row.accepted_at.is_some(),
        is_mine: viewer_id == Some(row.author_id),
        liked: lookup.liked.contains(&row.id),
        created_at: row.created_at,
    }
}

/// 读者点过赞的评论。未登录时为空。
async fn liked_set(
    conn: &mut PgConnection,
    viewer_id: Option<i64>,
    rows: &[&CommentRow],
) -> Result<HashSet<i64>, Problem> {
    let Some(viewer_id) = viewer_id else {
        return Ok(HashSet::new());
    };
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    Ok(repo::liked_among(conn, viewer_id, &ids)
        .await?
        .into_iter()
        .collect())
}

/// 只查非匿名评论的作者：匿名评论的真实作者不进入任何后续流程。
async fn author_map(
    state: &CommentsState,
    conn: &mut PgConnection,
    rows: &[&CommentRow],
) -> Result<ik_content::service::Authors, Problem> {
    let mut ids: Vec<i64> = rows
        .iter()
        .filter(|r| !r.is_anonymous)
        .map(|r| r.author_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ik_content::service::load_authors(conn, &state.media, &ids).await
}

/// 作者主页的游标（按时间）。
#[derive(Serialize, Deserialize)]
struct CommentCursor {
    c: DateTime<Utc>,
    i: i64,
}

/// 评论列表与回复列表的游标（按楼层）。`b` 为 true 时往前翻（楼层更小的）。
#[derive(Serialize, Deserialize)]
struct FloorCursor {
    f: i32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    b: bool,
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

fn floor_cursor(f: i32, b: bool) -> Result<String, Problem> {
    cursor::encode(&FloorCursor { f, b })
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

/// 按热度的评论区摘要（AI 工具 `get_post_comments`，TOOL-008/009）。
#[derive(Debug)]
pub struct HotComments {
    /// 可见的评论总数。
    pub total: i64,
    /// 赞数降序、楼层升序。
    pub comments: Vec<CommentView>,
}

/// 公开帖子的热门评论：只看公开内容（帖子公开；评论未隐藏），与 `viewer_id` 有拉黑关系的人的
/// 实名评论不出现（匿名评论不做拉黑过滤，防止反推）。匿名评论的作者是假名。帖子不公开时返回 None。
pub async fn hot_comments(
    state: &CommentsState,
    viewer_id: Option<i64>,
    post_id: i64,
    limit: i64,
) -> Result<Option<HotComments>, Problem> {
    let mut conn = state.db.acquire().await?;
    if !interop::post(&mut conn, post_id)
        .await?
        .is_some_and(|p| p.is_public)
    {
        return Ok(None);
    }
    let excluded = ik_social::interop::excluded_authors(&mut conn, viewer_id).await?;
    let total = repo::visible_count(&mut conn, post_id, &excluded).await?;
    let rows = repo::hot(&mut conn, post_id, &excluded, limit).await?;
    let refs: Vec<&CommentRow> = rows.iter().collect();
    let quoted = quoted_rows(&mut conn, &refs, &excluded).await?;
    let lookup = lookup_with(state, &mut conn, &refs, &quoted, HashSet::new()).await?;
    let comments = rows.iter().map(|r| view(r, &lookup, None)).collect();
    Ok(Some(HotComments { total, comments }))
}

/// 从 `from`（不含）往一个方向取一页可见评论：`older` 时往楼层小的方向（楼层降序），否则楼层升序。
/// 多取一条，第二个值表示这个方向上还有没有。
async fn fetch(
    conn: &mut PgConnection,
    post_id: i64,
    excluded: &[i64],
    from: i32,
    older: bool,
    limit: i64,
) -> Result<(Vec<CommentRow>, bool), Problem> {
    let mut rows = if older {
        repo::page_before(conn, post_id, excluded, from, limit + 1).await?
    } else {
        repo::page_after(conn, post_id, excluded, from, limit + 1).await?
    };
    let more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    Ok((rows, more))
}

/// 帖子的评论，默认按楼层从旧到新，`sort = newest` 时从新到旧。第一页额外返回采纳与置顶
/// （各最多一条，它们在 `items` 里照常出现）。
///
/// 游标只记楼层号和往哪边取（`b` 为 true 往楼层小的方向），与排序无关；排序只决定这一页是
/// 「往下翻」（`nextCursor`）还是「往回翻」（`prevCursor`），以及 `items` 的顺序。
/// 三种起点：游标、`anchor`（从那条评论所在楼层开始，往回还有时带 `prevCursor`）、都没有时从排序的开头开始。
pub async fn list(
    state: &CommentsState,
    viewer: Option<&Principal>,
    post_id: i64,
    query: CommentsQuery,
) -> Result<CommentsPage, Problem> {
    let viewer_id = viewer.map(|p| p.user_id.get());
    let limit = page_limit(query.limit);
    let cursor = query
        .cursor
        .as_deref()
        .map(cursor::decode::<FloorCursor>)
        .transpose()?;
    let mut conn = state.db.acquire().await?;
    readable_post(&mut conn, post_id, viewer_id).await?;
    let excluded = ik_social::interop::excluded_authors(&mut conn, viewer_id).await?;

    let start_floor = match (&cursor, query.anchor) {
        (None, Some(anchor)) => repo::get(&mut conn, anchor.get())
            .await?
            .filter(|c| c.post_id == post_id && c.hidden_at.is_none())
            .filter(|c| c.is_anonymous || !excluded.contains(&c.author_id))
            .map(|c| c.floor),
        _ => None,
    };

    // 往下翻时取的方向（与游标的 `b` 同义）：从旧到新往楼层大的方向取，从新到旧往楼层小的方向取。
    let down = query.sort == CommentSort::Newest;
    let (fetched, older, more) = match (cursor, start_floor) {
        (Some(FloorCursor { f, b }), _) => {
            let (rows, more) = fetch(&mut conn, post_id, &excluded, f, b, limit).await?;
            (rows, b, more)
        }
        (None, Some(floor)) => {
            // 包含起点那一层。
            let from = if down {
                floor.saturating_add(1)
            } else {
                floor - 1
            };
            let (rows, more) = fetch(&mut conn, post_id, &excluded, from, down, limit).await?;
            (rows, down, more)
        }
        (None, None) => {
            let from = if down { i32::MAX } else { 0 };
            let (rows, more) = fetch(&mut conn, post_id, &excluded, from, down, limit).await?;
            (rows, down, more)
        }
    };
    // 同方向接着取的游标：取的顺序里最后一条。
    let onward = match (more, fetched.last()) {
        (true, Some(last)) => Some(floor_cursor(last.floor, older)?),
        _ => None,
    };
    let mut rows = fetched;
    let (next_cursor, mut prev_cursor) = if older == down {
        (onward, None)
    } else {
        rows.reverse();
        (None, onward)
    };
    // 从中间开始时，往回还有没有可见的评论。
    if start_floor.is_some()
        && older == down
        && let Some(first) = rows.first()
        && fetch(&mut conn, post_id, &excluded, first.floor, !down, 0)
            .await?
            .1
    {
        prev_cursor = Some(floor_cursor(first.floor, !down)?);
    }

    let first_page = query.cursor.is_none() && start_floor.is_none();
    let (accepted, pinned) = if first_page {
        (
            repo::accepted(&mut conn, post_id, &excluded).await?,
            repo::pinned(&mut conn, post_id, &excluded).await?,
        )
    } else {
        (None, None)
    };
    let everyone: Vec<&CommentRow> = accepted
        .iter()
        .chain(pinned.iter())
        .chain(rows.iter())
        .collect();
    let lookup = lookup(state, &mut conn, viewer_id, &excluded, &everyone).await?;
    let render = |row: &CommentRow| view(row, &lookup, viewer_id);
    Ok(CommentsPage {
        accepted: accepted.as_ref().map(render),
        pinned: pinned.as_ref().map(render),
        items: rows.iter().map(render).collect(),
        next_cursor,
        prev_cursor,
    })
}

/// 回复某条评论的评论，旧的在前。目标不存在、被隐藏、或帖子不可见时 404（CMT-074）；
/// 目标是与读者有拉黑关系的人的非匿名评论时同样 404。
pub async fn replies(
    state: &CommentsState,
    viewer: Option<&Principal>,
    comment_id: i64,
    query: PageQuery,
) -> Result<Page<CommentView>, Problem> {
    let viewer_id = viewer.map(|p| p.user_id.get());
    let limit = page_limit(query.limit);
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<FloorCursor>)
        .transpose()?
        .map_or(0, |c| c.f);
    let mut conn = state.db.acquire().await?;
    let excluded = ik_social::interop::excluded_authors(&mut conn, viewer_id).await?;
    let target = repo::get(&mut conn, comment_id)
        .await?
        .filter(|c| c.hidden_at.is_none())
        .filter(|c| c.is_anonymous || !excluded.contains(&c.author_id))
        .ok_or_else(comment_not_found)?;
    readable_post(&mut conn, target.post_id, viewer_id).await?;

    let rows = repo::replies_page(&mut conn, target.id, &excluded, after, limit + 1).await?;
    let refs: Vec<&CommentRow> = rows.iter().collect();
    let lookup = lookup(state, &mut conn, viewer_id, &excluded, &refs).await?;
    let items: Vec<CommentView> = rows.iter().map(|r| view(r, &lookup, viewer_id)).collect();
    Page::from_rows(
        items,
        usize::try_from(limit).unwrap_or(20),
        |item: &CommentView| FloorCursor {
            f: item.floor,
            b: false,
        },
    )
}

// ---------- 作者主页 ----------

/// 作者的非匿名、未隐藏评论数（作者主页统计；不按所属帖子是否公开过滤）。
pub async fn author_comment_count(state: &CommentsState, author_id: i64) -> Result<i64, Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(repo::author_comment_count(&mut conn, author_id).await?)
}

/// 作者主页的评论（新的在前，PROFILE-020）：非匿名、未隐藏、所属帖子公开。
/// 所属帖子的可见性在取出之后过滤，所以一页可能不满；游标照常按原始行推进，`nextCursor` 为空才是到底。
pub async fn author_comments(
    state: &CommentsState,
    viewer: Option<&Principal>,
    author_id: i64,
    query: PageQuery,
) -> Result<Page<AuthorCommentView>, Problem> {
    let viewer_id = viewer.map(|p| p.user_id.get());
    let limit = page_limit(query.limit);
    let after = decode_cursor(query.cursor.as_deref())?;
    let mut conn = state.db.acquire().await?;
    let mut rows = repo::author_comments(&mut conn, author_id, after, limit + 1).await?;
    let has_more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let next_cursor = match (has_more, rows.last()) {
        (true, Some(last)) => Some(cursor::encode(&CommentCursor {
            c: last.created_at,
            i: last.id,
        })?),
        _ => None,
    };
    let mut post_ids: Vec<i64> = rows.iter().map(|r| r.post_id).collect();
    post_ids.sort_unstable();
    post_ids.dedup();
    let titles = interop::public_post_titles(&mut conn, &post_ids).await?;
    let visible: Vec<&CommentRow> = rows
        .iter()
        .filter(|r| titles.contains_key(&r.post_id))
        .collect();
    let liked = liked_set(&mut conn, viewer_id, &visible).await?;
    let lookup = lookup_with(state, &mut conn, &visible, &[], liked).await?;
    let items = visible
        .into_iter()
        .filter_map(|row| {
            titles.get(&row.post_id).map(|title| AuthorCommentView {
                id: Id::new(row.id),
                post_id: Id::new(row.post_id),
                post_title: title.clone(),
                body: row.body.clone(),
                images: lookup.images(row),
                like_count: row.like_count,
                liked: lookup.liked.contains(&row.id),
                created_at: row.created_at,
            })
        })
        .collect();
    Ok(Page { items, next_cursor })
}

// ---------- 搜索（search 域用）----------

/// 数据库兜底搜索的窗口上限（SRCH-033、EDGE-008）。
pub const SEARCH_FALLBACK_CAP: i64 = 1000;

/// 对读者可见的评论（保持输入顺序，SRCH-032）：未隐藏、所属帖子公开；
/// 评论作者、帖子作者与读者有拉黑关系时过滤（匿名评论 / 匿名帖不按拉黑过滤）。
async fn visible_rows(
    conn: &mut PgConnection,
    viewer_id: Option<i64>,
    ids: &[i64],
) -> Result<(Vec<CommentRow>, HashMap<i64, ik_content::interop::PostMeta>), Problem> {
    if ids.is_empty() {
        return Ok((Vec::new(), HashMap::new()));
    }
    let mut rows = repo::listed_by_ids(conn, ids).await?;
    let mut post_ids: Vec<i64> = rows.iter().map(|r| r.post_id).collect();
    post_ids.sort_unstable();
    post_ids.dedup();
    let posts = interop::public_post_meta(conn, &post_ids).await?;
    let excluded: HashSet<i64> = ik_social::interop::excluded_authors(conn, viewer_id)
        .await?
        .into_iter()
        .collect();
    rows.retain(|row| {
        posts.get(&row.post_id).is_some_and(|post| {
            (row.is_anonymous || !excluded.contains(&row.author_id))
                && (post.is_anonymous || !excluded.contains(&post.author_id))
        })
    });
    rows.sort_by_key(|row| ids.iter().position(|id| *id == row.id));
    Ok((rows, posts))
}

/// 对读者可见的评论 id（保持输入顺序）。
pub async fn search_visible_ids(
    state: &CommentsState,
    viewer: Option<&Principal>,
    ids: &[i64],
) -> Result<Vec<i64>, Problem> {
    let mut conn = state.db.acquire().await?;
    let (rows, _) = visible_rows(&mut conn, viewer.map(|p| p.user_id.get()), ids).await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// 评论搜索结果（保持输入顺序，不可见的跳过）。
pub async fn search_views(
    state: &CommentsState,
    viewer: Option<&Principal>,
    ids: &[i64],
) -> Result<Vec<CommentSearchView>, Problem> {
    let viewer_id = viewer.map(|p| p.user_id.get());
    let mut conn = state.db.acquire().await?;
    let (rows, posts) = visible_rows(&mut conn, viewer_id, ids).await?;
    let refs: Vec<&CommentRow> = rows.iter().collect();
    let liked = liked_set(&mut conn, viewer_id, &refs).await?;
    let lookup = lookup_with(state, &mut conn, &refs, &[], liked).await?;
    let cover_ids: Vec<i64> = posts.values().filter_map(|p| p.cover_id).collect();
    let covers = state.media.views(&mut conn, &cover_ids).await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            posts.get(&row.post_id).map(|post| CommentSearchView {
                id: Id::new(row.id),
                post_id: Id::new(row.post_id),
                post_title: post.title.clone(),
                post_cover: post.cover_id.and_then(|id| covers.get(&id).cloned()),
                body: row.body.clone(),
                images: lookup.images(row),
                author: lookup.author(row),
                is_anonymous: row.is_anonymous,
                like_count: row.like_count,
                liked: lookup.liked.contains(&row.id),
                created_at: row.created_at,
            })
        })
        .collect())
}

/// 数据库兜底搜索（SRCH-033）：正文包含 `q` 的评论 id（新的在前），最多 1000 个候选，
/// 再按读者可见性过滤。
pub async fn search_comment_ids(
    state: &CommentsState,
    viewer: Option<&Principal>,
    q: &str,
    post_id: Option<i64>,
) -> Result<Vec<i64>, Problem> {
    let mut conn = state.db.acquire().await?;
    let candidates = repo::search_candidates(&mut conn, q, post_id, SEARCH_FALLBACK_CAP).await?;
    if i64::try_from(candidates.len()).unwrap_or(i64::MAX) >= SEARCH_FALLBACK_CAP {
        tracing::warn!(cap = SEARCH_FALLBACK_CAP, "评论搜索兜底触顶，结果被截断");
    }
    let (rows, _) = visible_rows(&mut conn, viewer.map(|p| p.user_id.get()), &candidates).await?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}
