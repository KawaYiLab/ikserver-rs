//! 统一收件箱：私聊会话与通知分组按最后活动时间混排（KNOCK-010～015）。
//!
//! 两个数据源各自按 `(时间, 键)` 倒序游标分页，这里做双源归并。同一时刻通知排在私信前面
//! （CONV-025），所以游标记下「落在哪个数据源的哪一项上」，另一个数据源据此判断同一时刻的项
//! 是否已经出过（`ik_messaging::interop::Before`）。与某人有在场私聊时，他发来的通知分组
//! 在数据源里就被排除，合并进那个会话（未读相加，预览取更新的一条）。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ik_auth::Principal;
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::cursor::{self, Page};
use ik_kernel::{Id, Problem};
use ik_messaging::dto::{ConversationView, MessageView, PeerView};
use ik_messaging::interop::{self as dm, Before};
use serde::{Deserialize, Serialize};

use crate::NotifyState;
use crate::dto::{
    InboxEntry, InboxItem, InboxPage, InboxPreview, InboxReadResult, NotificationView, PageQuery,
    SenderView, UnreadCount,
};
use crate::interop::user_bucket;
use crate::repo::{self, BucketRow};
use crate::service::{
    not_found, page_limit, parse_bucket, sender_map, sender_view, summary, system_peer, view,
};

const DM_PREFIX: &str = "dm-";

/// 收件箱条目键。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Key {
    Conversation(i64),
    Bucket(String),
}

fn parse_key(key: &str) -> Result<Key, Problem> {
    if let Some(id) = key.strip_prefix(DM_PREFIX) {
        return id
            .parse::<Id<()>>()
            .map(|id| Key::Conversation(id.get()))
            .map_err(|_| not_found());
    }
    parse_bucket(key)?;
    Ok(Key::Bucket(key.to_owned()))
}

/// 把两个各自倒序的列表归并成一个，最多取 `take` 项。同一时刻 `notes`（通知）在前。
fn merge<D, N, T>(
    dms: Vec<D>,
    notes: Vec<N>,
    take: usize,
    dm_at: impl Fn(&D) -> DateTime<Utc>,
    note_at: impl Fn(&N) -> DateTime<Utc>,
    from_dm: impl Fn(D) -> T,
    from_note: impl Fn(N) -> T,
) -> Vec<T> {
    let mut dms = dms.into_iter().peekable();
    let mut notes = notes.into_iter().peekable();
    let mut out = Vec::with_capacity(take);
    while out.len() < take {
        let pick_note = match (dms.peek(), notes.peek()) {
            (None, None) => break,
            (Some(_), None) => false,
            (None, Some(_)) => true,
            // 同一时刻通知排在私信前面（CONV-025）。
            (Some(d), Some(n)) => note_at(n) >= dm_at(d),
        };
        let next = if pick_note {
            notes.next().map(&from_note)
        } else {
            dms.next().map(&from_dm)
        };
        match next {
            Some(item) => out.push(item),
            None => break,
        }
    }
    out
}

fn peer_as_sender(peer: PeerView) -> SenderView {
    SenderView {
        id: peer.id,
        name: peer.name,
        level: peer.level,
        avatar_url: peer.avatar_url,
    }
}

fn bucket_preview(row: &BucketRow) -> InboxPreview {
    InboxPreview {
        kind: "notification".to_owned(),
        text: summary(&row.kind, row.comment_id, &row.excerpt),
        from_me: false,
        at: row.last_at,
    }
}

/// 私聊会话条目；`bucket` 是对方发来的通知分组（合并进来）。
fn conversation_entry(me: i64, cv: ConversationView, bucket: Option<&BucketRow>) -> InboxEntry {
    let message = cv.last_message.as_ref().map(|m| InboxPreview {
        kind: "message".to_owned(),
        text: m.preview.clone(),
        from_me: m.sender_id.get() == me,
        at: m.created_at,
    });
    // 比较预览新旧用最后一条消息的时间，不用会话排序时间（后者已被通知推进过，KNOCK-011）。
    let preview = match (message, bucket.map(bucket_preview)) {
        (Some(m), Some(n)) => Some(if n.at > m.at { n } else { m }),
        (m, n) => m.or(n),
    };
    InboxEntry {
        key: format!("{DM_PREFIX}{}", cv.id),
        kind: "conversation".to_owned(),
        conversation_id: Some(cv.id),
        conversation_kind: Some(cv.kind),
        title: cv.title,
        peer: peer_as_sender(cv.peer),
        activity_at: cv.activity_at,
        preview,
        unread_count: cv.unread_count + bucket.map_or(0, |b| b.unread),
        muted: cv.muted,
        pinned: cv.pinned,
    }
}

fn bucket_entry(row: &BucketRow, peer: SenderView) -> InboxEntry {
    InboxEntry {
        key: row.bucket_key.clone(),
        kind: "notifications".to_owned(),
        conversation_id: None,
        conversation_kind: None,
        title: None,
        peer,
        activity_at: row.last_at,
        preview: Some(bucket_preview(row)),
        unread_count: row.unread,
        muted: false,
        pinned: false,
    }
}

/// 收件箱游标：落在私聊会话上时 `c` 为会话 id，落在通知分组上时 `k` 为分组键。
#[derive(Debug, Serialize, Deserialize)]
struct EntryCursor {
    t: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    c: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    k: Option<String>,
}

enum Source {
    Conversation(ConversationView),
    Bucket(BucketRow),
}

/// 收件箱列表。第一页先列出全部置顶会话，并给出全部未读合计。
pub async fn list(
    state: &NotifyState,
    principal: &Principal,
    query: PageQuery,
) -> Result<InboxPage, Problem> {
    let me = principal.user_id.get();
    let limit = page_limit(query.limit);
    let take = usize::try_from(limit).unwrap_or(20);
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<EntryCursor>)
        .transpose()?;
    if after
        .as_ref()
        .is_some_and(|c| c.c.is_some() == c.k.is_some())
    {
        return Err(Problem::cursor_invalid());
    }
    let first_page = after.is_none();

    let mut conn = state.db.acquire().await?;
    let excluded: Vec<String> = dm::active_peers(&mut conn, me)
        .await?
        .into_iter()
        .map(user_bucket)
        .collect();
    let buckets = repo::inbox_buckets(
        &mut conn,
        me,
        &excluded,
        after.as_ref().map(|c| (c.t, c.k.clone())),
        limit + 1,
    )
    .await?;
    let total_unread = if first_page {
        Some(dm::total_unread(&mut conn, me).await? + repo::unread_count(&mut conn, me).await?)
    } else {
        None
    };
    drop(conn);
    let dm_before = after.as_ref().map(|c| match c.c {
        Some(id) => Before::Item { at: c.t, id },
        None => Before::UpTo(c.t),
    });
    let conversations = dm::page(&state.db, &state.media, me, dm_before, limit + 1).await?;
    let pinned = if first_page {
        dm::pinned(&state.db, &state.media, me).await?
    } else {
        Vec::new()
    };

    let mut merged = merge(
        conversations,
        buckets,
        take + 1,
        |c| c.activity_at,
        |b| b.last_at,
        Source::Conversation,
        Source::Bucket,
    );
    let has_more = merged.len() > take;
    merged.truncate(take);
    let next_cursor = match (has_more, merged.last()) {
        (true, Some(Source::Conversation(c))) => Some(cursor::encode(&EntryCursor {
            t: c.activity_at,
            c: Some(c.id.get()),
            k: None,
        })?),
        (true, Some(Source::Bucket(b))) => Some(cursor::encode(&EntryCursor {
            t: b.last_at,
            c: None,
            k: Some(b.bucket_key.clone()),
        })?),
        _ => None,
    };

    // 私聊会话合并对方的通知分组。
    let merged_keys: Vec<String> = pinned
        .iter()
        .chain(merged.iter().filter_map(|s| match s {
            Source::Conversation(c) => Some(c),
            Source::Bucket(_) => None,
        }))
        .map(|c| user_bucket(c.peer_user_id))
        .collect();
    let mut conn = state.db.acquire().await?;
    let stats: HashMap<String, BucketRow> = repo::bucket_stats(&mut conn, me, &merged_keys)
        .await?
        .into_iter()
        .map(|b| (b.bucket_key.clone(), b))
        .collect();
    drop(conn);
    let bucket_peers: Vec<i64> = merged
        .iter()
        .filter_map(|s| match s {
            Source::Bucket(b) => parse_bucket(&b.bucket_key).ok().flatten(),
            Source::Conversation(_) => None,
        })
        .collect();
    let authors = sender_map(state, bucket_peers.into_iter()).await?;

    let to_entry = |cv: ConversationView| {
        let bucket = stats.get(&user_bucket(cv.peer_user_id));
        conversation_entry(me, cv, bucket)
    };
    let mut items: Vec<InboxEntry> = pinned.into_iter().map(to_entry).collect();
    items.extend(merged.into_iter().map(|source| match source {
        Source::Conversation(cv) => to_entry(cv),
        Source::Bucket(b) => {
            let peer = match parse_bucket(&b.bucket_key).ok().flatten() {
                Some(id) => sender_view(id, &authors),
                None => system_peer(),
            };
            bucket_entry(&b, peer)
        }
    }));
    Ok(InboxPage {
        items,
        next_cursor,
        total_unread,
    })
}

/// 时间线游标：落在私信上时 `m` 为消息 id，落在通知上时 `n` 为通知 id。
#[derive(Debug, Serialize, Deserialize)]
struct ItemCursor {
    t: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    m: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n: Option<i64>,
}

/// 收件箱条目的时间线（新的在前）。私聊会话混排对方发来的通知（MSG-011），
/// 通知分组只有通知。
pub async fn items(
    state: &NotifyState,
    principal: &Principal,
    key: &str,
    query: PageQuery,
) -> Result<Page<InboxItem>, Problem> {
    let me = principal.user_id.get();
    let key = parse_key(key)?;
    let limit = page_limit(query.limit);
    let take = usize::try_from(limit).unwrap_or(20);
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<ItemCursor>)
        .transpose()?;
    if after
        .as_ref()
        .is_some_and(|c| c.m.is_some() == c.n.is_some())
    {
        return Err(Problem::cursor_invalid());
    }

    let mut conn = state.db.acquire().await?;
    let (messages, bucket_key) = match key {
        Key::Conversation(id) => {
            let peer = dm::active_peer(&mut conn, id, me)
                .await?
                .ok_or_else(not_found)?;
            let before = after.as_ref().map(|c| match c.m {
                Some(id) => Before::Item { at: c.t, id },
                None => Before::UpTo(c.t),
            });
            (
                dm::messages(&mut conn, &state.media, id, before, limit + 1).await?,
                user_bucket(peer),
            )
        }
        Key::Bucket(key) => (Vec::new(), key),
    };
    let rows = repo::list(
        &mut conn,
        me,
        Some(&bucket_key),
        false,
        after.as_ref().map(|c| (c.t, c.n)),
        limit + 1,
    )
    .await?;
    drop(conn);
    let authors = sender_map(state, rows.iter().filter_map(|r| r.sender_id)).await?;
    let notifications: Vec<NotificationView> =
        rows.into_iter().map(|r| view(r, &authors)).collect();

    let mut merged = merge(
        messages,
        notifications,
        take + 1,
        |m: &MessageView| m.created_at,
        |n: &NotificationView| n.created_at,
        |message| InboxItem::Message { message },
        |notification| InboxItem::Notification { notification },
    );
    let has_more = merged.len() > take;
    merged.truncate(take);
    let next_cursor = match (has_more, merged.last()) {
        (true, Some(InboxItem::Message { message })) => Some(cursor::encode(&ItemCursor {
            t: message.created_at,
            m: Some(message.id.get()),
            n: None,
        })?),
        (true, Some(InboxItem::Notification { notification })) => {
            Some(cursor::encode(&ItemCursor {
                t: notification.created_at,
                m: None,
                n: Some(notification.id.get()),
            })?)
        }
        _ => None,
    };
    Ok(Page {
        items: merged,
        next_cursor,
    })
}

fn read_event(user_id: i64, key: &str, result: &InboxReadResult) -> Result<Event, Problem> {
    Event::new(
        "inbox.read",
        Audience::User(user_id),
        &serde_json::json!({
            "key": key,
            "lastReadAt": result.last_read_at,
            "notificationsUpdated": result.notifications_updated,
        }),
    )
}

/// 把一个收件箱条目标为已读。私聊会话：推进已读水位，并把对方发来的通知一并标为已读
/// （点开会话就等于看过了两者，CONV-034）。只标调用时刻及之前的通知。
pub async fn mark_read(
    state: &NotifyState,
    principal: &Principal,
    key: &str,
) -> Result<InboxReadResult, Problem> {
    let me = principal.user_id.get();
    let parsed = parse_key(key)?;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let result = match parsed {
        Key::Conversation(id) => {
            let peer = dm::mark_read(&mut tx, me, id, now)
                .await?
                .ok_or_else(not_found)?;
            InboxReadResult {
                last_read_at: Some(now),
                notifications_updated: repo::mark_all_read(
                    &mut tx,
                    me,
                    Some(&user_bucket(peer)),
                    Some(now),
                )
                .await?,
            }
        }
        Key::Bucket(bucket) => InboxReadResult {
            last_read_at: None,
            notifications_updated: repo::mark_all_read(&mut tx, me, Some(&bucket), Some(now))
                .await?,
        },
    };
    tx.commit().await?;
    let mut effects = Effects::new();
    effects.publish(read_event(me, key, &result)?);
    effects.dispatch(state.bus.as_ref());
    Ok(result)
}

/// 全部标为已读：所有私聊会话与所有通知（CONV-036）。
pub async fn mark_all_read(
    state: &NotifyState,
    principal: &Principal,
) -> Result<InboxReadResult, Problem> {
    let me = principal.user_id.get();
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    dm::mark_all_read(&mut tx, me, now).await?;
    let updated = repo::mark_all_read(&mut tx, me, None, Some(now)).await?;
    tx.commit().await?;
    let mut effects = Effects::new();
    effects.publish(Event::new(
        "inbox.read.all",
        Audience::User(me),
        &serde_json::json!({ "at": now }),
    )?);
    effects.dispatch(state.bus.as_ref());
    Ok(InboxReadResult {
        last_read_at: Some(now),
        notifications_updated: updated,
    })
}

/// 未读合计：未读私信 + 未读通知。
pub async fn unread_count(
    state: &NotifyState,
    principal: &Principal,
) -> Result<UnreadCount, Problem> {
    let me = principal.user_id.get();
    let mut conn = state.db.acquire().await?;
    Ok(UnreadCount {
        count: dm::total_unread(&mut conn, me).await? + repo::unread_count(&mut conn, me).await?,
    })
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(s: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(s, 0).single().unwrap()
    }

    #[test]
    fn keys_parse_strictly() {
        assert_eq!(parse_key("dm-42").unwrap(), Key::Conversation(42));
        assert_eq!(parse_key("system").unwrap(), Key::Bucket("system".into()));
        assert_eq!(parse_key("user-7").unwrap(), Key::Bucket("user-7".into()));
        for bad in ["dm-", "dm-x", "dm-0", "pseudo:system", "user-x"] {
            assert!(parse_key(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn merge_puts_notifications_first_on_ties() {
        let dms = vec![(at(10), "d10"), (at(5), "d5"), (at(1), "d1")];
        let notes = vec![(at(10), "n10"), (at(7), "n7")];
        let merged = merge(dms, notes, 10, |d| d.0, |n| n.0, |d| d.1, |n| n.1);
        assert_eq!(merged, ["n10", "d10", "n7", "d5", "d1"]);
    }

    #[test]
    fn merge_stops_at_take() {
        let dms = vec![(at(3), "d3"), (at(1), "d1")];
        let notes = vec![(at(2), "n2")];
        let merged = merge(dms, notes, 2, |d| d.0, |n| n.0, |d| d.1, |n| n.1);
        assert_eq!(merged, ["d3", "n2"]);
    }
}
