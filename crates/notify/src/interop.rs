//! 给其他业务域用的通知写入接口。
//!
//! 都接收调用方的连接/事务：通知与触发它的业务写入同一事务提交（AGENTS.md 规则 6），
//! 实时推送放进调用方的 `Effects`，提交后才发出（NOTIF-040）。
//!
//! 统一在这里强制的抑制规则：自己不通知自己（NOTIF-003）；双方存在拉黑时不发（NOTIF-002，
//! 旧系统是抛错，新系统静默跳过，因为触发动作本身已经被拉黑拦截过）；收件人封禁或注销时不发；
//! 发送者是 AI 账号时不发（NOTIF-014：AI 不单独成组，旧系统存库但列表、未读、推送全部剔除，
//! 用户看不到；这里直接不写）。
//! 匿名相关的抑制由调用方判断（只有调用方知道帖子、评论、父评论是否匿名），匿名时不要调用。

use chrono::{DateTime, Utc};
use ik_kernel::Problem;
use ik_kernel::Snowflake;
use ik_kernel::bus::{Audience, Effects, Event};
use sqlx::PgConnection;

use crate::repo::{self, NewRow, NewSystemBatch};

/// 评论摘要最多保存的字数。
pub const EXCERPT_MAX_CHARS: usize = 200;
/// 帖子标题、作者名在系统通知文案里最多保留的字数（FOLLOW-020）。
const NAME_MAX_CHARS: usize = 100;
/// 系统通知正文最多 500 字（NOTIF-001）。
const SYSTEM_MAX_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 评论了你的帖子。
    Comment,
    /// 回复了你的评论。
    Reply,
    /// 在评论中提到了你。
    Mention,
    /// 赞了你的帖子 / 评论。
    Like,
    /// 收藏了你的帖子。
    Favorite,
    /// 给你的帖子投了丁尼。
    Denny,
    /// 采纳了你的评论。
    Accepted,
    /// 系统通知（没有发送者）。
    System,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::Reply => "reply",
            Self::Mention => "mention",
            Self::Like => "like",
            Self::Favorite => "favorite",
            Self::Denny => "denny",
            Self::Accepted => "accepted",
            Self::System => "system",
        }
    }
}

/// 敲敲会话（分组）键。
pub(crate) const SYSTEM_BUCKET: &str = "system";

pub(crate) fn user_bucket(sender_id: i64) -> String {
    format!("user-{sender_id}")
}

pub(crate) fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// 一条互动通知（有发送者）。
#[derive(Debug, Clone)]
pub struct Notice<'a> {
    pub recipient_id: i64,
    pub sender_id: i64,
    pub kind: Kind,
    pub post_id: Option<i64>,
    pub comment_id: Option<i64>,
    pub post_title: Option<&'a str>,
    /// 评论摘要（调用方已把 token 转成可读文本）；会截到 200 字。
    pub excerpt: &'a str,
}

/// `conversation_id`：收件人与发送者之间有在场私聊时，收件箱把这条通知合并进那个会话。
fn created_event(
    recipient_id: i64,
    id: i64,
    bucket_key: &str,
    conversation_id: Option<i64>,
) -> Result<Event, Problem> {
    let mut data = serde_json::json!({ "id": id.to_string(), "bucket": bucket_key });
    if let Some(conversation_id) = conversation_id {
        data["conversationId"] = conversation_id.to_string().into();
    }
    Event::new("notification.created", Audience::User(recipient_id), &data)
}

/// 发一条互动通知。返回是否真的写入（被抑制时为 false）。
pub async fn send(
    conn: &mut PgConnection,
    ids: &Snowflake,
    notice: Notice<'_>,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<bool, Problem> {
    if notice.kind == Kind::System || notice.recipient_id == notice.sender_id {
        return Ok(false);
    }
    let users =
        ik_auth::interop::active_users(conn, &[notice.recipient_id, notice.sender_id]).await?;
    if !users.iter().any(|u| u.id == notice.recipient_id)
        || users
            .iter()
            .any(|u| u.id == notice.sender_id && u.is_ai_agent)
    {
        return Ok(false);
    }
    if ik_social::interop::any_block(conn, notice.sender_id, notice.recipient_id).await? {
        return Ok(false);
    }
    let id = ids.next_raw();
    let bucket_key = user_bucket(notice.sender_id);
    let excerpt = truncate(notice.excerpt, EXCERPT_MAX_CHARS);
    let title = notice.post_title.map(|t| truncate(t, NAME_MAX_CHARS));
    repo::insert(
        conn,
        &NewRow {
            id,
            recipient_id: notice.recipient_id,
            sender_id: notice.sender_id,
            kind: notice.kind.as_str(),
            bucket_key: &bucket_key,
            post_id: notice.post_id,
            comment_id: notice.comment_id,
            post_title: title.as_deref(),
            excerpt: &excerpt,
            now,
        },
    )
    .await?;
    // 收件箱里与发送者的私聊会话随这条通知排到前面（CONV-005）。
    let conversation_id =
        ik_messaging::interop::touch_direct(conn, notice.recipient_id, notice.sender_id, now)
            .await?;
    effects.publish(created_event(
        notice.recipient_id,
        id,
        &bucket_key,
        conversation_id,
    )?);
    Ok(true)
}

/// 被撤回通知的目标。
#[derive(Debug, Clone, Copy)]
pub enum Target {
    Post(i64),
    Comment(i64),
}

/// 撤回 `sender_id` 对某个目标发出的某类通知（取消点赞、取消收藏，D-011 方案 A：已读的也删）。
pub async fn retract(
    conn: &mut PgConnection,
    sender_id: i64,
    kind: Kind,
    target: Target,
    effects: &mut Effects,
) -> Result<(), Problem> {
    let (post_id, comment_id) = match target {
        Target::Post(id) => (Some(id), None),
        Target::Comment(id) => (None, Some(id)),
    };
    let mut recipients =
        repo::delete_by_target(conn, sender_id, kind.as_str(), post_id, comment_id).await?;
    recipients.sort_unstable();
    recipients.dedup();
    for recipient in recipients {
        effects.publish(Event::new(
            "notification.removed",
            Audience::User(recipient),
            &serde_json::json!({ "bucket": user_bucket(sender_id) }),
        )?);
    }
    Ok(())
}

/// 新帖扇出的一批：从粉丝 id 大于 `after` 处起最多 `limit` 个关注记录。
#[derive(Debug, Clone, Copy)]
pub struct FanoutPage {
    pub after: i64,
    pub limit: i64,
}

/// 一批扇出的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanoutOutcome {
    /// 这一批实际通知到的人数。
    pub sent: usize,
    /// 还有下一批时，下一批的 `after`。
    pub next_after: Option<i64>,
}

/// 新帖扇出：通知作者的粉丝「你关注的 X 发布了新委托《T》」（FOLLOW-009～020）。
///
/// 只在首次发布时调用；匿名帖不要调用（FOLLOW-016）。粉丝可能很多，所以按批进行，由调用方
/// （content 的后台任务）逐批推进，每批一个事务。收件人排除：与作者有拉黑关系、封禁或注销、
/// AI 账号（FOLLOW-017）。每个帖子对每个收件人只通知一次（幂等键 `follow:<帖子 id>`），
/// 所以同一批重做是安全的。
#[allow(clippy::too_many_arguments)]
pub async fn fanout_new_post(
    conn: &mut PgConnection,
    ids: &Snowflake,
    author_id: i64,
    post_id: i64,
    title: &str,
    page: FanoutPage,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<FanoutOutcome, Problem> {
    let (followers, next_after) =
        ik_social::interop::followers_for_fanout(conn, author_id, page.after, page.limit).await?;
    let done = |sent| Ok(FanoutOutcome { sent, next_after });
    if followers.is_empty() {
        return done(0);
    }
    let Some(author) = ik_auth::interop::active_users(conn, &[author_id])
        .await?
        .into_iter()
        .next()
    else {
        // 作者已经封禁或注销：后面的批次也不用再发。
        return Ok(FanoutOutcome {
            sent: 0,
            next_after: None,
        });
    };
    let mut recipients: Vec<i64> = ik_auth::interop::active_users(conn, &followers)
        .await?
        .into_iter()
        .filter(|u| !u.is_ai_agent && u.id != author_id)
        .map(|u| u.id)
        .collect();
    recipients.sort_unstable();
    if recipients.is_empty() {
        return done(0);
    }
    let title = truncate(title, NAME_MAX_CHARS);
    let message = truncate(
        &format!(
            "你关注的 {} 发布了新委托《{title}》",
            truncate(&author.username, NAME_MAX_CHARS)
        ),
        SYSTEM_MAX_CHARS,
    );
    let row_ids: Vec<i64> = recipients.iter().map(|_| ids.next_raw()).collect();
    let dedup_key = format!("follow:{post_id}");
    let sent = repo::insert_system_batch(
        conn,
        &NewSystemBatch {
            ids: &row_ids,
            recipients: &recipients,
            post_id: Some(post_id),
            post_title: Some(&title),
            excerpt: &message,
            dedup_key: Some(&dedup_key),
            now,
        },
    )
    .await?;
    if !sent.is_empty() {
        effects.publish(Event::new(
            "notification.created",
            Audience::Users(sent.clone().into()),
            &serde_json::json!({ "bucket": SYSTEM_BUCKET }),
        )?);
    }
    done(sent.len())
}

/// 发系统通知（举报处理结果等，RPTADM-090）：每个收件人一条，正文截到 500 字。
/// 已注销、已合并的收件人跳过；被封禁的照样收到（这样才看得到封禁原因）。返回写入的条数。
pub async fn system(
    conn: &mut PgConnection,
    ids: &Snowflake,
    recipients: &[i64],
    message: &str,
    post_id: Option<i64>,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<usize, Problem> {
    let mut recipients: Vec<i64> = ik_auth::interop::user_statuses(conn, recipients)
        .await?
        .into_values()
        .filter(|u| !u.gone())
        .map(|u| u.id)
        .collect();
    recipients.sort_unstable();
    if recipients.is_empty() {
        return Ok(0);
    }
    let message = truncate(message, SYSTEM_MAX_CHARS);
    let row_ids: Vec<i64> = recipients.iter().map(|_| ids.next_raw()).collect();
    let sent = repo::insert_system_batch(
        conn,
        &NewSystemBatch {
            ids: &row_ids,
            recipients: &recipients,
            post_id,
            post_title: None,
            excerpt: &message,
            dedup_key: None,
            now,
        },
    )
    .await?;
    if !sent.is_empty() {
        effects.publish(Event::new(
            "notification.created",
            Audience::Users(sent.clone().into()),
            &serde_json::json!({ "bucket": SYSTEM_BUCKET }),
        )?);
    }
    Ok(sent.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_counts_chars_not_bytes() {
        assert_eq!(truncate("绳网委托", 2), "绳网");
        assert_eq!(truncate("ab", 10), "ab");
    }

    #[test]
    fn bucket_keys() {
        assert_eq!(user_bucket(42), "user-42");
        assert_eq!(Kind::Accepted.as_str(), "accepted");
    }
}
