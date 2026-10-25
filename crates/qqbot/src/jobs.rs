//! Durable reply task. Preparing and finalizing use short transactions; HTTP never does.
use crate::{
    QqState,
    admin::Settings,
    repo::{self, Turn},
    text,
};
use chrono::{Duration, Utc};
use ik_ai::{
    external::{self, ExternalTurn, RunOutcome},
    quota::{self, Hold},
};
use ik_kernel::{
    Problem,
    jobs::{IntoTaskHandlerResult, TaskHandler, WorkerContext},
};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reply {
    pub turn_id: i64,
}
impl TaskHandler for Reply {
    const IDENTIFIER: &'static str = "qq.reply";
    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<QqState>() else {
            return Err("执行器没有注册 QqState".to_owned());
        };
        run_reply(state, self.turn_id)
            .await
            .map_err(|p| format!("{p:?}"))
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prune {}
impl TaskHandler for Prune {
    const IDENTIFIER: &'static str = "qq.prune";
    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<QqState>() else {
            return Err("执行器没有注册 QqState".to_owned());
        };
        prune(state).await.map_err(|p| format!("{p:?}"))
    }
}
fn window(t: &Turn) -> Duration {
    if t.group_id.is_some() {
        Duration::minutes(5)
    } else {
        Duration::minutes(60)
    }
}
fn expired(t: &Turn, reserve_secs: i64) -> bool {
    Utc::now() + Duration::seconds(reserve_secs) >= t.message_at + window(t)
}
fn hold(t: &Turn) -> Option<Hold> {
    Some(Hold {
        id: t.hold_id,
        user_id: quota::SITE,
        day: t.quota_day?,
        amount: t.hold_amount,
    })
}

/// Public for integration testing with fake AI and QQ HTTP providers.
pub async fn run_reply(s: &QqState, id: i64) -> Result<(), Problem> {
    let settings = s.settings().await?;
    let mut tx = s.db.begin().await?;
    let Some(mut t) = repo::turn(&mut tx, id).await? else {
        return Ok(());
    };
    if matches!(t.status.as_str(), "completed" | "failed" | "skipped") {
        return Ok(());
    }
    if !s.config.enabled
        || !settings.enabled
        || !t.allowed
        || t.user_id.is_some()
        || !s.config.apps.values().any(|app| app.app_id == t.app_id)
    {
        abandon(&mut tx, &t, "disabled_or_blocked").await?;
        tx.commit().await?;
        return Ok(());
    }
    if expired(
        &t,
        if t.output.is_some() {
            0
        } else {
            i64::from(settings.reply_timeout_secs) + 15
        },
    ) {
        abandon(&mut tx, &t, "window_expired").await?;
        tx.commit().await?;
        return Ok(());
    }
    if t.status == "processing" && t.output.is_none() {
        // A worker died after claiming the turn. Never issue a second billable generation.
        // Its first request may already have incurred cost (QUOTA-019).
        finalize_cost(&mut tx, &t, t.hold_amount.max(t.guest_token_reserved)).await?;
        refund_turn(&mut tx, &t).await?;
        repo::terminal(&mut tx, t.id, "failed", "generation_interrupted").await?;
        tx.commit().await?;
        return Ok(());
    }
    t.attempts += 1;
    sqlx::query!(
        "UPDATE qq_turns SET attempts=$2 WHERE id=$1",
        id,
        t.attempts
    )
    .execute(&mut *tx)
    .await?;
    if t.output.is_some() {
        tx.commit().await?;
        return deliver(s, t).await;
    }
    let command = t.content.split_whitespace().next().unwrap_or("");
    if command.starts_with('/') {
        let content=match command {
            "/帮助"=>"在群里 @我或直接给我发消息就能聊天。游客每天有免费次数，北京时间 04:00 重置。发送 /额度 查看剩余次数。账号绑定功能将在下一期开放。".to_owned(),
            "/额度"=>{
                let day=ik_kernel::time::biz_day(Utc::now());
                let used=sqlx::query_scalar!("SELECT turns FROM qq_guest_usage WHERE day=$1 AND identity_id=$2",day,t.identity_id).fetch_optional(&mut *tx).await?.unwrap_or(0);
                format!("今天还剩 {} 次免费对话，04:00 重置。",(settings.guest_daily_turns-used).max(0))
            }
            _=>"暂不支持这个指令，发送 /帮助 查看可用指令。".to_owned(),
        };
        t.output = Some(vec![content]);
        save_output(&mut tx, &t).await?;
        tx.commit().await?;
        return deliver(s, t).await;
    }
    let Some(slug) = t.card_slug.clone() else {
        repo::terminal(&mut tx, id, "skipped", "no_role_card").await?;
        tx.commit().await?;
        return Ok(());
    };
    tx.commit().await?;
    let catalog = s.ai.catalog.get().await?;
    let Some(card) = catalog
        .cards
        .iter()
        .find(|c| c.slug == slug && c.enabled && c.qq_enabled)
    else {
        let mut tx = s.db.begin().await?;
        repo::terminal(&mut tx, id, "skipped", "role_disabled").await?;
        tx.commit().await?;
        return Ok(());
    };
    let mut conn = s.db.acquire().await?;
    let history = repo::history(&mut conn, &t, &settings, card.bound_user_id).await?;
    drop(conn);
    let note = if t.group_id.is_some() {
        "\n你在 QQ 群里和多位群友聊天；回答当前提问者，一般不超过 200 字。不要输出链接或 markdown。不要谈论对方绑定的绳网身份、私信、其他群的事。"
    } else {
        "\n你在 QQ 单聊中回复对方，用纯文本简短回答，不要输出链接或 markdown。"
    };
    let prepared = match external::prepare(
        &s.ai,
        ExternalTurn {
            channel: "qq",
            card_slug: slug.clone(),
            model_key: settings.guest_model_key.clone(),
            viewer: None,
            history,
            system_note: note.to_owned(),
            max_chars: settings.max_reply_chars as usize,
            timeout_secs: settings.reply_timeout_secs as u64,
        },
    )
    .await
    {
        Ok(p) => p,
        Err(p) => {
            let mut tx = s.db.begin().await?;
            repo::terminal(&mut tx, id, "skipped", "ai_unavailable").await?;
            tx.commit().await?;
            tracing::warn!(
                turn_id = id,
                code = p.code(),
                "QQ AI preparation unavailable"
            );
            return Ok(());
        }
    };
    let estimate = external::estimate(&prepared);
    let mut tx = s.db.begin().await?;
    let Some(mut t) = repo::turn(&mut tx, id).await? else {
        return Ok(());
    };
    if t.status != "queued" {
        return Ok(());
    }
    if !t.allowed
        || t.user_id.is_some()
        || t.card_slug.as_deref() != Some(&slug)
        || expired(&t, i64::from(settings.reply_timeout_secs) + 15)
    {
        repo::terminal(&mut tx, id, "skipped", "state_changed").await?;
        tx.commit().await?;
        return Ok(());
    }
    // Fixed lock order: site quota -> all guest tokens -> group usage -> identity usage.
    let now = Utc::now();
    let reserved = quota::reserve_site_only(
        &mut tx,
        &s.ids,
        prepared.limits,
        estimate,
        now,
        Duration::seconds(900),
    )
    .await?;
    let site_hold = match reserved {
        Ok(h) => h,
        Err(reason) => {
            quota_notice(
                &mut tx,
                &mut t,
                reason.as_str(),
                "今天的 AI 额度已用完，04:00 重置。",
            )
            .await?;
            tx.commit().await?;
            return if t.output.is_some() {
                deliver(s, t).await
            } else {
                Ok(())
            };
        }
    };
    if let Some(reason) = reserve_guest(&mut tx, &t, &settings, estimate, site_hold.day).await? {
        quota::settle(&mut tx, &site_hold, &t.execution_key, 0, now).await?;
        quota_notice(&mut tx,&mut t,reason,&format!("今天的免费次数或游客额度用完了（每天 {} 次，04:00 重置）。账号绑定功能将在下一期开放。",settings.guest_daily_turns)).await?;
        tx.commit().await?;
        return if t.output.is_some() {
            deliver(s, t).await
        } else {
            Ok(())
        };
    }
    t.hold_id = site_hold.id;
    t.hold_amount = site_hold.amount;
    t.quota_day = Some(site_hold.day);
    t.guest_reserved = true;
    t.guest_token_reserved = estimate;
    t.ai_user_id = Some(prepared.ai_user_id);
    sqlx::query!("UPDATE qq_turns SET status='processing',ai_user_id=$2,hold_id=$3,quota_day=$4,hold_amount=$5,guest_reserved=TRUE,guest_token_reserved=$6,model_key=$7 WHERE id=$1",id,prepared.ai_user_id,site_hold.id,site_hold.day,site_hold.amount,estimate,prepared.model_key).execute(&mut *tx).await?;
    sqlx::query!(
        "UPDATE qq_messages SET ai_user_id=$2 WHERE id=$1",
        t.message_id,
        prepared.ai_user_id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let reply = external::reply(&s.ai, &prepared, id).await;
    let actual = reply.charge(site_hold.amount.max(estimate));
    let mut tx = s.db.begin().await?;
    let Some(mut t) = repo::turn(&mut tx, id).await? else {
        return Ok(());
    };
    if t.status != "processing" || t.output.is_some() {
        return Ok(());
    }
    finalize_cost(&mut tx, &t, actual).await?;
    t.guest_token_reserved = 0;
    sqlx::query!(
        "UPDATE qq_turns SET model_key=$2,prompt_tokens=$3,completion_tokens=$4 WHERE id=$1",
        id,
        reply.model_key,
        reply.usage.prompt_tokens,
        reply.usage.completion_tokens
    )
    .execute(&mut *tx)
    .await?;
    let (segments, blocked) = text::segments(
        &reply.segments,
        &settings.link_mode,
        &settings.blocked_words,
        settings.max_reply_chars as usize,
        t.group_id.is_some(),
    );
    if reply.outcome != RunOutcome::Completed
        || segments.is_empty()
        || expired(&t, 0)
        || !t.allowed
        || t.user_id.is_some()
    {
        refund_turn(&mut tx, &t).await?;
        let reason = if expired(&t, 0) {
            "window_expired"
        } else if !t.allowed || t.user_id.is_some() {
            "disabled_or_blocked"
        } else {
            "generation_failed"
        };
        repo::terminal(&mut tx, id, "failed", reason).await?;
        tx.commit().await?;
        return Ok(());
    }
    t.output = Some(segments);
    if blocked {
        t.reason = Some("blocked_word".to_owned());
    }
    save_output(&mut tx, &t).await?;
    tx.commit().await?;
    deliver(s, t).await
}

async fn reserve_guest(
    c: &mut PgConnection,
    t: &Turn,
    s: &Settings,
    amount: i64,
    day: chrono::NaiveDate,
) -> Result<Option<&'static str>, Problem> {
    let tokens=sqlx::query_scalar!("INSERT INTO qq_guest_tokens(day,used) VALUES($1,0) ON CONFLICT(day) DO UPDATE SET used=qq_guest_tokens.used RETURNING used",day).fetch_one(&mut *c).await?;
    let group_turns = if let Some(g) = t.group_id {
        Some(sqlx::query_scalar!("INSERT INTO qq_group_usage(day,group_id) VALUES($1,$2) ON CONFLICT(day,group_id) DO UPDATE SET guest_turns=qq_group_usage.guest_turns RETURNING guest_turns",day,g).fetch_one(&mut *c).await?)
    } else {
        None
    };
    let turns=sqlx::query_scalar!("INSERT INTO qq_guest_usage(day,identity_id) VALUES($1,$2) ON CONFLICT(day,identity_id) DO UPDATE SET turns=qq_guest_usage.turns RETURNING turns",day,t.identity_id).fetch_one(&mut *c).await?;
    if turns >= s.guest_daily_turns {
        return Ok(Some("guest_daily_turns"));
    }
    if group_turns.is_some_and(|n| n >= s.guest_group_daily_turns) {
        return Ok(Some("guest_group_daily_turns"));
    }
    if s.guest_daily_tokens > 0 && tokens >= s.guest_daily_tokens {
        return Ok(Some("guest_daily_tokens"));
    }
    sqlx::query!(
        "UPDATE qq_guest_tokens SET used=used+$2 WHERE day=$1",
        day,
        amount
    )
    .execute(&mut *c)
    .await?;
    if let Some(g) = t.group_id {
        sqlx::query!(
            "UPDATE qq_group_usage SET guest_turns=guest_turns+1 WHERE day=$1 AND group_id=$2",
            day,
            g
        )
        .execute(&mut *c)
        .await?;
    }
    sqlx::query!(
        "UPDATE qq_guest_usage SET turns=turns+1 WHERE day=$1 AND identity_id=$2",
        day,
        t.identity_id
    )
    .execute(c)
    .await?;
    Ok(None)
}
async fn quota_notice(
    c: &mut PgConnection,
    t: &mut Turn,
    reason: &str,
    content: &str,
) -> Result<(), Problem> {
    t.reason = Some(reason.to_owned());
    if repo::notify_once(c, t, ik_kernel::time::biz_day(Utc::now())).await? {
        t.output = Some(vec![content.to_owned()]);
        save_output(c, t).await
    } else {
        repo::terminal(c, t.id, "skipped", reason).await
    }
}
async fn save_output(c: &mut PgConnection, t: &Turn) -> Result<(), Problem> {
    sqlx::query!(
        "UPDATE qq_turns SET status='processing',output=$2,reason=$3 WHERE id=$1",
        t.id,
        t.output.as_deref(),
        t.reason
    )
    .execute(c)
    .await?;
    Ok(())
}
async fn finalize_cost(c: &mut PgConnection, t: &Turn, actual: i64) -> Result<(), Problem> {
    let Some(h) = hold(t) else {
        return Ok(());
    };
    if matches!(
        quota::settle(c, &h, &t.execution_key, actual, Utc::now()).await?,
        quota::Settled::Duplicate
    ) {
        return Ok(());
    }
    let delta = actual - t.guest_token_reserved;
    sqlx::query!(
        "UPDATE qq_guest_tokens SET used=GREATEST(0,used+$2) WHERE day=$1",
        h.day,
        delta
    )
    .execute(&mut *c)
    .await?;
    if let Some(g) = t.group_id {
        sqlx::query!(
            "UPDATE qq_group_usage SET tokens=tokens+$3 WHERE day=$1 AND group_id=$2",
            h.day,
            g,
            actual
        )
        .execute(&mut *c)
        .await?;
    }
    sqlx::query!(
        "UPDATE qq_turns SET guest_token_reserved=0 WHERE id=$1",
        t.id
    )
    .execute(c)
    .await?;
    Ok(())
}
async fn refund_turn(c: &mut PgConnection, t: &Turn) -> Result<(), Problem> {
    if !t.guest_reserved || t.segments_sent > 0 {
        return Ok(());
    }
    let Some(day) = t.quota_day else {
        return Ok(());
    };
    // Global and token locks were acquired by finalize_cost before these count rows.
    if let Some(g) = t.group_id {
        sqlx::query!("UPDATE qq_group_usage SET guest_turns=GREATEST(0,guest_turns-1) WHERE day=$1 AND group_id=$2",day,g).execute(&mut *c).await?;
    }
    sqlx::query!(
        "UPDATE qq_guest_usage SET turns=GREATEST(0,turns-1) WHERE day=$1 AND identity_id=$2",
        day,
        t.identity_id
    )
    .execute(&mut *c)
    .await?;
    sqlx::query!("UPDATE qq_turns SET guest_reserved=FALSE WHERE id=$1", t.id)
        .execute(c)
        .await?;
    Ok(())
}
async fn abandon(c: &mut PgConnection, t: &Turn, reason: &str) -> Result<(), Problem> {
    if t.output.is_none() {
        finalize_cost(
            c,
            t,
            if t.status == "processing" {
                t.hold_amount.max(t.guest_token_reserved)
            } else {
                0
            },
        )
        .await?;
    }
    refund_turn(c, t).await?;
    repo::terminal(c, t.id, "skipped", reason).await
}

async fn deliver(s: &QqState, mut t: Turn) -> Result<(), Problem> {
    let output = t.output.clone().unwrap_or_default();
    let Some(msg_id) = t.qq_msg_id.as_deref() else {
        return Err(Problem::internal());
    };
    for (index, content) in output.iter().enumerate().skip(t.segments_sent as usize) {
        let deadline = t.message_at + window(&t);
        let remaining = (deadline - Utc::now()).to_std().unwrap_or_default();
        let mut accepted = content.clone();
        let error = if remaining.is_zero() {
            Some(("window_expired", false))
        } else {
            match tokio::time::timeout(
                remaining,
                s.client.send_rendered(
                    &t.app_id,
                    &t.scene,
                    &t.openid,
                    msg_id,
                    index as i32 + 1,
                    content,
                ),
            )
            .await
            {
                Ok(Ok(rendered)) => {
                    accepted = rendered;
                    None
                }
                Ok(Err(e)) => Some((e.reason(), e.retryable())),
                Err(_) => Some(("window_expired", false)),
            }
        };
        if let Some((reason, retryable)) = error {
            if retryable && t.attempts < 2 && !expired(&t, 10) {
                tracing::warn!(
                    turn_id = t.id,
                    reason,
                    "QQ send will retry using the same msg_id and msg_seq"
                );
                return Err(Problem::service_unavailable());
            }
            let mut tx = s.db.begin().await?;
            if let Some(current) = repo::turn(&mut tx, t.id).await? {
                refund_turn(&mut tx, &current).await?;
                repo::terminal(&mut tx, t.id, "failed", reason).await?;
            }
            tx.commit().await?;
            return Ok(());
        }
        let mut tx = s.db.begin().await?;
        let Some(current) = repo::turn(&mut tx, t.id).await? else {
            return Ok(());
        };
        if current.segments_sent <= index as i32 {
            sqlx::query!("INSERT INTO qq_messages(id,app_id,identity_id,group_id,ai_user_id,direction,content,reply_to_id) VALUES($1,$2,$3,$4,$5,'out',$6,$7)",s.ids.next_raw(),t.app_id,t.identity_id,t.group_id,t.ai_user_id,accepted,t.message_id).execute(&mut *tx).await?;
            sqlx::query!(
                "UPDATE qq_turns SET segments_sent=$2 WHERE id=$1",
                t.id,
                index as i32 + 1
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        t.segments_sent = index as i32 + 1;
    }
    let mut tx = s.db.begin().await?;
    sqlx::query!(
        "UPDATE qq_turns SET status='completed',finished_at=$2 WHERE id=$1",
        t.id,
        Utc::now()
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn prune(s: &QqState) -> Result<(), Problem> {
    // Drain expired turns before deleting their messages, so guest reservations cannot leak.
    let ids=sqlx::query_scalar!("SELECT t.id FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id WHERE t.status IN ('queued','processing') AND m.created_at < $1 ORDER BY t.id LIMIT 500",Utc::now()-Duration::hours(1)).fetch_all(&s.db).await?;
    for id in ids {
        let mut tx = s.db.begin().await?;
        if let Some(t) = repo::turn(&mut tx, id).await?
            && matches!(t.status.as_str(), "queued" | "processing")
        {
            abandon(&mut tx, &t, "window_expired").await?;
        }
        tx.commit().await?;
    }
    let mut tx = s.db.begin().await?;
    sqlx::query!("DELETE FROM qq_messages m USING qq_identities i WHERE i.id=m.identity_id AND ((m.direction='in' AND m.group_id IS NOT NULL AND NOT m.mentioned AND m.created_at < $1) OR (i.user_id IS NULL AND m.created_at < $2) OR m.created_at < $3) AND NOT EXISTS (SELECT 1 FROM qq_turns t WHERE t.message_id=m.id AND t.status IN ('queued','processing'))",Utc::now()-Duration::days(3),Utc::now()-Duration::days(30),Utc::now()-Duration::days(180)).execute(&mut *tx).await?;
    let before = ik_kernel::time::biz_day(Utc::now()) - Duration::days(90);
    sqlx::query!("DELETE FROM qq_guest_usage WHERE day<$1", before)
        .execute(&mut *tx)
        .await?;
    sqlx::query!("DELETE FROM qq_group_usage WHERE day<$1", before)
        .execute(&mut *tx)
        .await?;
    sqlx::query!("DELETE FROM qq_guest_tokens WHERE day<$1", before)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
