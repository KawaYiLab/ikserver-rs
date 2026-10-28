//! 通知读取与已读。业务规则：`docs/business-rules/messaging-notifications.md` 第 4、5 节。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::service::Author;
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::cursor::{self, Page};
use ik_kernel::{Id, Problem};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::NotifyState;
use crate::dto::{
    BucketView, ListQuery, NotificationView, PageQuery, ReadResult, SenderView, UnreadCount,
};
use crate::interop::SYSTEM_BUCKET;
use crate::repo::{self, NotificationRow};

const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 50;
/// 系统会话对端的固定名字（KNOCK-003）。
const SYSTEM_PEER_NAME: &str = "Fairy";
const GONE_USER_NAME: &str = "已注销用户";

pub(crate) fn not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "通知不存在")
}

/// 解析路径里的会话键：`system` 或 `user-<数字>`；其他一律 404。
pub(crate) fn parse_bucket(key: &str) -> Result<Option<i64>, Problem> {
    if key == SYSTEM_BUCKET {
        return Ok(None);
    }
    key.strip_prefix("user-")
        .and_then(|id| id.parse::<Id<()>>().ok())
        .map(|id| Some(id.get()))
        .ok_or_else(not_found)
}

/// 一句话概括（KNOCK 预览文案）。
pub(crate) fn summary(kind: &str, comment_id: Option<i64>, excerpt: &str) -> String {
    match kind {
        "comment" => "评论了你的帖子",
        "reply" => "回复了你的评论",
        "mention" => "在评论中提到了你",
        "like" if comment_id.is_some() => "赞了你的评论",
        "like" => "赞了你的帖子",
        "favorite" => "收藏了你的帖子",
        "denny" => "给你的帖子投了丁尼",
        "accepted" => "采纳了你的评论为最佳答案",
        _ => return excerpt.to_owned(),
    }
    .to_owned()
}

/// 发送者资料与头像。
pub(crate) struct Senders {
    authors: HashMap<i64, Author>,
    avatars: HashMap<i64, String>,
}

pub(crate) fn sender_view(sender_id: i64, senders: &Senders) -> SenderView {
    match senders.authors.get(&sender_id) {
        Some(a) if !a.gone => SenderView {
            id: Some(a.id),
            name: a.username.clone(),
            level: Some(a.level),
            avatar_url: senders.avatars.get(&sender_id).cloned(),
        },
        _ => SenderView {
            id: None,
            name: GONE_USER_NAME.to_owned(),
            level: None,
            avatar_url: None,
        },
    }
}

pub(crate) fn system_peer() -> SenderView {
    SenderView {
        id: None,
        name: SYSTEM_PEER_NAME.to_owned(),
        level: None,
        avatar_url: None,
    }
}

pub(crate) async fn sender_map(
    state: &NotifyState,
    conn: &mut PgConnection,
    ids: impl Iterator<Item = i64>,
) -> Result<Senders, Problem> {
    let mut ids: Vec<i64> = ids.collect();
    ids.sort_unstable();
    ids.dedup();
    let authors = ik_auth::service::authors(conn, &ids).await?;
    let avatars = state.media.avatar_urls(conn, authors.values()).await?;
    Ok(Senders { authors, avatars })
}

pub(crate) fn view(row: NotificationRow, authors: &Senders) -> NotificationView {
    NotificationView {
        id: Id::new(row.id),
        summary: summary(&row.kind, row.comment_id, &row.excerpt),
        sender: row.sender_id.map(|id| sender_view(id, authors)),
        kind: row.kind,
        bucket: row.bucket_key,
        excerpt: row.excerpt,
        post_id: row.post_id.map(Id::new),
        post_title: row.post_title,
        comment_id: row.comment_id.map(Id::new),
        is_read: row.is_read,
        created_at: row.created_at,
    }
}

#[derive(Serialize, Deserialize)]
struct ItemCursor {
    c: DateTime<Utc>,
    i: i64,
}

pub(crate) fn page_limit(limit: Option<u32>) -> i64 {
    i64::from(limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE))
}

async fn items(
    state: &NotifyState,
    principal: &Principal,
    bucket_key: Option<&str>,
    unread_only: bool,
    raw_cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<Page<NotificationView>, Problem> {
    let limit = page_limit(limit);
    let after = raw_cursor
        .map(cursor::decode::<ItemCursor>)
        .transpose()?
        .map(|c| (c.c, Some(c.i)));
    let mut conn = state.db.acquire().await?;
    let rows = repo::list(
        &mut conn,
        principal.user_id.get(),
        bucket_key,
        unread_only,
        after,
        limit + 1,
    )
    .await?;
    let authors = sender_map(state, &mut conn, rows.iter().filter_map(|r| r.sender_id)).await?;
    let views = rows.into_iter().map(|r| view(r, &authors)).collect();
    Page::from_rows(
        views,
        usize::try_from(limit).unwrap_or(20),
        |v: &NotificationView| ItemCursor {
            c: v.created_at,
            i: v.id.get(),
        },
    )
}

/// 我的通知（新的在前）。需要登录（D-075 方案 A）。
pub async fn list(
    state: &NotifyState,
    principal: &Principal,
    query: ListQuery,
) -> Result<Page<NotificationView>, Problem> {
    items(
        state,
        principal,
        None,
        query.unread,
        query.cursor.as_deref(),
        query.limit,
    )
    .await
}

pub async fn unread_count(
    state: &NotifyState,
    principal: &Principal,
) -> Result<UnreadCount, Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(UnreadCount {
        count: repo::unread_count(&mut conn, principal.user_id.get()).await?,
    })
}

fn read_event(user_id: i64, bucket: Option<&str>, count: u64) -> Result<Event, Problem> {
    Event::new(
        "notification.read",
        Audience::User(user_id),
        &serde_json::json!({ "bucket": bucket, "count": count }),
    )
}

/// 标记一条为已读（幂等）。不是自己的通知一律 404。
pub async fn mark_read(
    state: &NotifyState,
    principal: &Principal,
    id: i64,
) -> Result<ReadResult, Problem> {
    let me = principal.user_id.get();
    let mut conn = state.db.acquire().await?;
    let changed = repo::mark_read(&mut conn, me, id)
        .await?
        .ok_or_else(not_found)?;
    let updated = u64::from(changed);
    if changed {
        let mut effects = Effects::new();
        effects.publish(read_event(me, None, updated)?);
        effects.dispatch(state.bus.as_ref());
    }
    Ok(ReadResult { updated })
}

/// 全部标为已读。
pub async fn mark_all_read(
    state: &NotifyState,
    principal: &Principal,
) -> Result<ReadResult, Problem> {
    mark_many_read(state, principal, None).await
}

async fn mark_many_read(
    state: &NotifyState,
    principal: &Principal,
    bucket: Option<&str>,
) -> Result<ReadResult, Problem> {
    let me = principal.user_id.get();
    let mut conn = state.db.acquire().await?;
    let updated = repo::mark_all_read(&mut conn, me, bucket, None).await?;
    if updated > 0 {
        let mut effects = Effects::new();
        effects.publish(read_event(me, bucket, updated)?);
        effects.dispatch(state.bus.as_ref());
    }
    Ok(ReadResult { updated })
}

// ---------- 敲敲会话 ----------

#[derive(Serialize, Deserialize)]
struct BucketCursor {
    t: DateTime<Utc>,
    k: String,
}

/// 敲敲会话列表：同一发送者的通知归为一组，系统通知一组，按最后一条的时间倒序。
pub async fn buckets(
    state: &NotifyState,
    principal: &Principal,
    query: PageQuery,
) -> Result<Page<BucketView>, Problem> {
    let limit = page_limit(query.limit);
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<BucketCursor>)
        .transpose()?
        .map(|c| (c.t, c.k));
    let mut conn = state.db.acquire().await?;
    let rows = repo::buckets(&mut conn, principal.user_id.get(), after, limit + 1).await?;
    let peers: Vec<(String, Option<i64>)> = rows
        .iter()
        .map(|r| {
            (
                r.bucket_key.clone(),
                parse_bucket(&r.bucket_key).ok().flatten(),
            )
        })
        .collect();
    let authors = sender_map(state, &mut conn, peers.iter().filter_map(|(_, id)| *id)).await?;
    let views = rows
        .into_iter()
        .zip(peers)
        .map(|(row, (_, peer_id))| BucketView {
            peer: peer_id.map_or_else(system_peer, |id| sender_view(id, &authors)),
            last_summary: summary(&row.kind, row.comment_id, &row.excerpt),
            key: row.bucket_key,
            unread_count: row.unread,
            last_at: row.last_at,
        })
        .collect();
    Page::from_rows(
        views,
        usize::try_from(limit).unwrap_or(20),
        |v: &BucketView| BucketCursor {
            t: v.last_at,
            k: v.key.clone(),
        },
    )
}

/// 某个敲敲会话里的通知（新的在前）。
pub async fn bucket_items(
    state: &NotifyState,
    principal: &Principal,
    key: &str,
    query: PageQuery,
) -> Result<Page<NotificationView>, Problem> {
    parse_bucket(key)?;
    items(
        state,
        principal,
        Some(key),
        false,
        query.cursor.as_deref(),
        query.limit,
    )
    .await
}

/// 把某个敲敲会话全部标为已读。
pub async fn mark_bucket_read(
    state: &NotifyState,
    principal: &Principal,
    key: &str,
) -> Result<ReadResult, Problem> {
    parse_bucket(key)?;
    mark_many_read(state, principal, Some(key)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_keys_parse_strictly() {
        assert_eq!(parse_bucket("system").unwrap(), None);
        assert_eq!(parse_bucket("user-42").unwrap(), Some(42));
        for bad in [
            "user-",
            "user-0",
            "user-abc",
            "user-+1",
            "other:system",
            "anonymous-x",
        ] {
            assert!(parse_bucket(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn summaries() {
        assert_eq!(summary("like", Some(1), ""), "赞了你的评论");
        assert_eq!(summary("like", None, ""), "赞了你的帖子");
        assert_eq!(summary("system", None, "正文"), "正文");
    }
}
