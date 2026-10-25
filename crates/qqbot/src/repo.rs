//! QQ owns these tables. Every mutating function receives the caller's transaction.
use crate::{
    QqState,
    admin::{BotView, GroupView, Settings},
    text,
    webhook::{Incoming, mentioned},
};
use chrono::{DateTime, NaiveDate, Utc};
use ik_kernel::{Id, Problem, jobs::JobSpec};
use sqlx::PgConnection;

pub(crate) async fn settings(c: &mut PgConnection) -> Result<Settings, Problem> {
    Ok(sqlx::query_as!(Settings,"SELECT enabled,guest_daily_turns,guest_group_daily_turns,guest_daily_tokens,guest_model_key,group_context_turns,group_context_hours,c2c_context_turns,reply_timeout_secs,max_reply_chars,link_mode,blocked_words,guest_reads_group_memory,updated_at FROM qq_settings WHERE singleton").fetch_one(c).await?)
}
pub(crate) async fn save_settings(c: &mut PgConnection, s: &Settings) -> Result<(), Problem> {
    sqlx::query!("UPDATE qq_settings SET enabled=$1,guest_daily_turns=$2,guest_group_daily_turns=$3,guest_daily_tokens=$4,guest_model_key=$5,group_context_turns=$6,group_context_hours=$7,c2c_context_turns=$8,reply_timeout_secs=$9,max_reply_chars=$10,link_mode=$11,blocked_words=$12,guest_reads_group_memory=$13,updated_at=$14 WHERE singleton",s.enabled,s.guest_daily_turns,s.guest_group_daily_turns,s.guest_daily_tokens,s.guest_model_key,s.group_context_turns,s.group_context_hours,s.c2c_context_turns,s.reply_timeout_secs,s.max_reply_chars,s.link_mode,&s.blocked_words,s.guest_reads_group_memory,s.updated_at).execute(c).await?;
    Ok(())
}
pub(crate) async fn bots(c: &mut PgConnection) -> Result<Vec<BotView>, Problem> {
    Ok(sqlx::query_as!(
        BotView,
        "SELECT app_id,display_name,default_card_slug,enabled FROM qq_bots ORDER BY app_id"
    )
    .fetch_all(c)
    .await?)
}
pub(crate) async fn groups(
    c: &mut PgConnection,
    id: Option<i64>,
    limit: i64,
    offset: i64,
) -> Result<Vec<GroupView>, Problem> {
    let day = ik_kernel::time::biz_day(Utc::now());
    let rows=sqlx::query!(r#"SELECT g.id,g.app_id,g.display_name,g.card_slug,g.enabled,g.blocked,g.left_at,g.last_active_at,
        COALESCE(u.guest_turns,0) AS "turns!",COALESCE(u.tokens,0) AS "tokens!",g.group_openid
        FROM qq_groups g LEFT JOIN qq_group_usage u ON u.group_id=g.id AND u.day=$1
        WHERE ($2::bigint IS NULL OR g.id=$2) ORDER BY g.last_active_at DESC,g.id DESC LIMIT $3 OFFSET $4"#,day,id,limit,offset).fetch_all(c).await?;
    Ok(rows
        .into_iter()
        .map(|r| GroupView {
            id: Id::new(r.id),
            app_id: r.app_id,
            display_name: group_name(r.display_name, &r.group_openid),
            card_slug: r.card_slug,
            enabled: r.enabled,
            blocked: r.blocked,
            left_at: r.left_at,
            last_active_at: r.last_active_at,
            today_guest_turns: r.turns,
            today_tokens: r.tokens,
        })
        .collect())
}

/// 群的展示名：管理员没填时用「QQ 群 #<openid 后四位>」。
pub(crate) fn group_name(display_name: String, group_openid: &str) -> String {
    if display_name.is_empty() {
        let tail: Vec<char> = group_openid.chars().rev().take(4).collect();
        format!("QQ 群 #{}", tail.into_iter().rev().collect::<String>())
    } else {
        display_name
    }
}

pub(crate) async fn ingest(
    s: &QqState,
    app_id: &str,
    event: &str,
    m: Incoming,
) -> Result<(), Problem> {
    if m.id.is_empty() || m.id.len() > 256 {
        return Err(Problem::field("id", "INVALID", "消息 id 无效"));
    }
    let group = event != "C2C_MESSAGE_CREATE";
    let openid = if group {
        m.author.member_openid.as_deref()
    } else {
        m.author.user_openid.as_deref()
    }
    .filter(|v| !v.is_empty() && v.len() <= 256)
    .ok_or_else(|| Problem::field("author", "INVALID", "缺少身份"))?;
    let trigger = !group || mentioned(&m, app_id, event);
    let now = Utc::now();
    // A future timestamp must not extend the passive reply window.
    if m.timestamp > now + chrono::Duration::minutes(1) {
        return Err(Problem::field("timestamp", "INVALID", "消息时间无效"));
    }
    let mut tx = s.db.begin().await?;
    let group_id = if group {
        let group_openid = m
            .group_openid
            .as_deref()
            .filter(|v| !v.is_empty() && v.len() <= 256)
            .ok_or_else(|| Problem::field("groupOpenid", "INVALID", "缺少群身份"))?;
        Some(sqlx::query_scalar!("INSERT INTO qq_groups(id,app_id,group_openid,last_active_at) VALUES($1,$2,$3,$4) ON CONFLICT(app_id,group_openid) DO UPDATE SET last_active_at=EXCLUDED.last_active_at RETURNING id",s.ids.next_raw(),app_id,group_openid,now).fetch_one(&mut *tx).await?)
    } else {
        None
    };
    let scene = if group { "group" } else { "c2c" };
    let nickname = text::truncate(&m.author.username, 32);
    let identity_id=sqlx::query_scalar!("INSERT INTO qq_identities(id,app_id,scene,openid,group_id,nickname,last_active_at) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(app_id,scene,openid,group_id) DO UPDATE SET nickname=EXCLUDED.nickname,last_active_at=EXCLUDED.last_active_at RETURNING id",s.ids.next_raw(),app_id,scene,openid,group_id,nickname,now).fetch_one(&mut *tx).await?;
    let content = text::incoming(&m.content);
    let message_id = s.ids.next_raw();
    let inserted=sqlx::query!("INSERT INTO qq_messages(id,app_id,identity_id,group_id,direction,mentioned,content,qq_msg_id,created_at) VALUES($1,$2,$3,$4,'in',$5,$6,$7,$8) ON CONFLICT(app_id,qq_msg_id) WHERE direction='in' DO NOTHING",message_id,app_id,identity_id,group_id,trigger,content,m.id,m.timestamp).execute(&mut *tx).await?.rows_affected()>0;
    if inserted && trigger {
        let turn_id = s.ids.next_raw();
        let execution_key = format!("qq:{app_id}:{}", m.id);
        sqlx::query!("INSERT INTO qq_turns(id,execution_key,message_id,identity_id,group_id) VALUES($1,$2,$3,$4,$5)",turn_id,execution_key,message_id,identity_id,group_id).execute(&mut *tx).await?;
        s.jobs
            .enqueue(
                &mut tx,
                crate::jobs::Reply { turn_id },
                JobSpec {
                    max_attempts: Some(2),
                    job_key: Some(execution_key),
                    queue_name: Some(format!(
                        "qq:{app_id}:{scene}:{}",
                        group_id.unwrap_or(identity_id)
                    )),
                    ..JobSpec::default()
                },
            )
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
pub(crate) async fn lifecycle(
    s: &QqState,
    app_id: &str,
    event: &str,
    d: &serde_json::Value,
    receipt: &str,
) -> Result<(), Problem> {
    let mut tx = s.db.begin().await?;
    let inserted = sqlx::query!(
        "INSERT INTO qq_lifecycle_receipts(app_id,event_hash) VALUES($1,$2) ON CONFLICT DO NOTHING",
        app_id,
        receipt
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if inserted == 0 {
        return Ok(());
    }
    let now = Utc::now();
    if event.starts_with("GROUP_") {
        if let Some(openid) = d
            .get("group_openid")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty() && v.len() <= 256)
        {
            let left = if event == "GROUP_DEL_ROBOT" {
                Some(now)
            } else {
                None
            };
            sqlx::query!("INSERT INTO qq_groups(id,app_id,group_openid,left_at,last_active_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(app_id,group_openid) DO UPDATE SET left_at=EXCLUDED.left_at,last_active_at=EXCLUDED.last_active_at",s.ids.next_raw(),app_id,openid,left,now).execute(&mut *tx).await?;
        }
    } else if let Some(openid) = d
        .get("openid")
        .or_else(|| d.get("user_openid"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty() && v.len() <= 256)
    {
        let available = event == "FRIEND_ADD";
        sqlx::query!("INSERT INTO qq_identities(id,app_id,scene,openid,available,last_active_at) VALUES($1,$2,'c2c',$3,$4,$5) ON CONFLICT(app_id,scene,openid,group_id) DO UPDATE SET available=EXCLUDED.available,last_active_at=EXCLUDED.last_active_at",s.ids.next_raw(),app_id,openid,available,now).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Turn {
    pub id: i64,
    pub execution_key: String,
    pub message_id: i64,
    pub identity_id: i64,
    pub group_id: Option<i64>,
    pub ai_user_id: Option<i64>,
    pub status: String,
    pub reason: Option<String>,
    pub hold_id: Option<i64>,
    pub quota_day: Option<NaiveDate>,
    pub hold_amount: i64,
    pub guest_reserved: bool,
    pub guest_token_reserved: i64,
    pub output: Option<Vec<String>>,
    pub segments_sent: i32,
    pub attempts: i32,
    pub app_id: String,
    pub scene: String,
    pub openid: String,
    pub qq_msg_id: Option<String>,
    pub content: String,
    pub message_at: DateTime<Utc>,
    pub card_slug: Option<String>,
    pub allowed: bool,
    /// 发消息的 QQ 身份绑定的绳网账号。
    pub user_id: Option<i64>,
    /// 这次按谁的额度计（预占时写入）：绑定用户为本人，游客为空（只计全站与游客额度）。
    pub payer_user_id: Option<i64>,
}
pub(crate) async fn turn(c: &mut PgConnection, id: i64) -> Result<Option<Turn>, Problem> {
    Ok(sqlx::query_as!(Turn,r#"SELECT t.id,t.execution_key,t.message_id,t.identity_id,t.group_id,t.ai_user_id,t.status,t.reason,t.hold_id,t.quota_day,t.hold_amount,
        t.guest_reserved,t.guest_token_reserved,t.output,t.segments_sent,t.attempts,m.app_id,i.scene,COALESCE(g.group_openid,i.openid) AS "openid!",m.qq_msg_id,m.content,m.created_at AS message_at,
        COALESCE(g.card_slug,i.card_slug,b.default_card_slug) AS card_slug,
        (b.enabled AND NOT i.blocked AND i.available AND COALESCE(g.enabled AND NOT g.blocked AND g.left_at IS NULL,TRUE)) AS "allowed!",i.user_id,t.payer_user_id
        FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id JOIN qq_identities i ON i.id=t.identity_id
        JOIN qq_bots b ON b.app_id=m.app_id LEFT JOIN qq_groups g ON g.id=t.group_id WHERE t.id=$1 FOR UPDATE OF t"#,id).fetch_optional(c).await?)
}
pub(crate) async fn history(
    c: &mut PgConnection,
    t: &Turn,
    settings: &Settings,
    ai_user_id: i64,
) -> Result<Vec<ik_ai::external::ExternalLine>, Problem> {
    let hours = if t.group_id.is_some() {
        settings.group_context_hours
    } else {
        12
    };
    let cutoff = t.message_at - chrono::Duration::hours(i64::from(hours));
    let limit = if t.group_id.is_some() {
        settings.group_context_turns
    } else {
        settings.c2c_context_turns
    };
    let mut rows=sqlx::query!(r#"SELECT m.identity_id,m.direction,m.content,m.mentioned,i.nickname,m.id
        FROM qq_messages m JOIN qq_identities i ON i.id=m.identity_id LEFT JOIN qq_groups g ON g.id=m.group_id
        WHERE (($1::bigint IS NOT NULL AND m.group_id=$1) OR ($1 IS NULL AND m.identity_id=$2 AND (m.ai_user_id=$3 OR m.id=$4)))
        AND m.created_at>=$5 AND (m.created_at,m.id)<=($6,$4)
        AND m.created_at>=COALESCE(CASE WHEN $1 IS NOT NULL THEN g.context_reset_at ELSE i.context_reset_at END,'-infinity'::timestamptz)
        AND (m.id=$4 OR (left(m.content,1)<>'/' AND (m.direction='in' OR m.ai_user_id IS NOT NULL)))
        ORDER BY m.created_at DESC,m.id DESC LIMIT $7"#,t.group_id,t.identity_id,ai_user_id,t.message_id,cutoff,t.message_at,i64::from(limit)).fetch_all(c).await?;
    rows.reverse();
    let mut names = std::collections::BTreeMap::<i64, usize>::new();
    Ok(rows
        .into_iter()
        .map(|r| {
            let assistant = r.direction == "out";
            let content = if t.group_id.is_some() && !assistant {
                let next = names.len();
                let idx = *names.entry(r.identity_id).or_insert(next);
                let nick = text::nickname(&r.nickname);
                let nick = if nick.is_empty() {
                    format!("群友 {}", label(idx))
                } else {
                    nick
                };
                let body = if r.mentioned {
                    r.content
                } else {
                    text::truncate(&r.content, 200)
                };
                format!(
                    "{}{}：{}",
                    if r.id == t.message_id {
                        "当前提问者 "
                    } else {
                        ""
                    },
                    nick,
                    body
                )
            } else {
                r.content
            };
            ik_ai::external::ExternalLine { assistant, content }
        })
        .collect())
}
fn label(mut index: usize) -> String {
    let mut chars = Vec::new();
    loop {
        chars.push((b'A' + (index % 26) as u8) as char);
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    chars.into_iter().rev().collect()
}

pub(crate) async fn terminal(
    c: &mut PgConnection,
    id: i64,
    status: &str,
    reason: &str,
) -> Result<(), Problem> {
    sqlx::query!(
        "UPDATE qq_turns SET status=$2,reason=$3,finished_at=$4 WHERE id=$1",
        id,
        status,
        reason,
        Utc::now()
    )
    .execute(c)
    .await?;
    Ok(())
}
pub(crate) async fn notify_once(
    c: &mut PgConnection,
    t: &Turn,
    day: NaiveDate,
) -> Result<bool, Problem> {
    let row=sqlx::query!("INSERT INTO qq_guest_usage(day,identity_id,notified) VALUES($1,$2,TRUE) ON CONFLICT(day,identity_id) DO UPDATE SET notified=TRUE WHERE NOT qq_guest_usage.notified RETURNING identity_id",day,t.identity_id).fetch_optional(c).await?;
    Ok(row.is_some())
}

/// 群在记忆里的场合键（ADR 0008）：群里说的个人记忆只在这个群用，群共享记忆也按它存。
pub fn group_scope(group_id: i64) -> String {
    format!("qq:group:{group_id}")
}
