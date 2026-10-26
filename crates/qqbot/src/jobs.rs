//! Durable reply task. Preparing and finalizing use short transactions; HTTP never does.
use crate::{
    QqState,
    admin::Settings,
    repo::{self, Turn},
    text,
};
use chrono::{Duration, Utc};
use ik_ai::{
    external::{self, ExternalMemory, ExternalTurn, RunOutcome},
    quota::{self, Hold},
};
use ik_kernel::{
    Problem,
    jobs::{IntoTaskHandlerResult, TaskHandler, WorkerContext},
};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

mod delivery;
mod dispatch;
pub(crate) use dispatch::enqueue_turn;
pub use dispatch::{reconcile, run_dispatch};

static GENERATIONS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dispatch {
    pub conversation_id: i64,
}
impl TaskHandler for Dispatch {
    const IDENTIFIER: &'static str = "qq.dispatch";
    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(s) = ctx.get_ext::<QqState>() else {
            return Err("执行器没有注册 QqState".to_owned());
        };
        run_dispatch(s, self.conversation_id)
            .await
            .map_err(|p| format!("{p:?}"))
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Reconcile {}
impl TaskHandler for Reconcile {
    const IDENTIFIER: &'static str = "qq.reconcile";
    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(s) = ctx.get_ext::<QqState>() else {
            return Err("执行器没有注册 QqState".to_owned());
        };
        reconcile(s).await.map_err(|p| format!("{p:?}"))
    }
}

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
fn hold(t: &Turn) -> Option<Hold> {
    Some(Hold {
        id: t.hold_id,
        user_id: t.payer_user_id.unwrap_or(quota::SITE),
        day: t.quota_day?,
        amount: t.hold_amount,
    })
}

/// 绑定账号的状态（ADR 0008 §4.2）。
enum Account {
    /// 没绑定：游客。
    Guest,
    /// 绑定了状态正常的账号：按个人额度，可以用记忆。
    Bound(i64),
    /// 绑定的账号被封禁：不回复，也不降级成游客（否则封号后还能用游客次数）。
    Blocked,
}

/// 读绑定账号的状态。已注销或合并的账号顺手解绑，按游客处理（注销即解绑）。
async fn account(c: &mut PgConnection, t: &Turn) -> Result<Account, Problem> {
    let Some(user_id) = t.user_id else {
        return Ok(Account::Guest);
    };
    let status = ik_auth::interop::user_statuses(c, &[user_id])
        .await?
        .remove(&user_id);
    match status {
        Some(user) if user.status == "active" => Ok(Account::Bound(user_id)),
        Some(user) if !user.gone() => Ok(Account::Blocked),
        _ => {
            crate::me::release(c, t.identity_id, Some(user_id)).await?;
            Ok(Account::Guest)
        }
    }
}

/// Explicitly drive a conversation; Graphile's legacy qq.reply handler uses this too.
pub async fn run_reply(s: &QqState, id: i64) -> Result<(), Problem> {
    let mut tx = s.db.begin().await?;
    let attached: Option<Option<i64>> =
        sqlx::query_scalar("SELECT conversation_id FROM qq_turns WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some(attached) = attached else {
        return Ok(());
    };
    if attached.is_none() {
        dispatch::attach_legacy_turn(s, &mut tx, id).await?;
    }
    let conversation: Option<i64> =
        sqlx::query_scalar("SELECT conversation_id FROM qq_turns WHERE id=$1")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    tx.commit().await?;
    if let Some(conversation) = conversation {
        run_dispatch(s, conversation).await?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) async fn test_heartbeat<F>(s: &QqState, turn_id: i64, future: F) -> Result<(), Problem>
where
    F: std::future::Future<Output = Result<(), Problem>>,
{
    let (conversation_id, epoch): (i64, i64) = sqlx::query_as("SELECT c.id,c.lease_epoch FROM qq_conversations c JOIN qq_turns t ON t.conversation_id=c.id WHERE t.id=$1")
        .bind(turn_id).fetch_one(&s.db).await?;
    dispatch::heartbeat(
        s,
        &dispatch::Lease {
            conversation_id,
            turn_id,
            epoch,
        },
        future,
    )
    .await
}

async fn generate(s: &QqState, lease: &dispatch::Lease) -> Result<(), Problem> {
    let id = lease.turn_id;
    let settings = s.settings().await?;
    let mut tx = s.db.begin().await?;
    crate::identity::lock_turn_identity(&mut tx, id).await?;
    let Some(mut t) = repo::turn(&mut tx, id).await? else {
        return Ok(());
    };
    if matches!(t.status.as_str(), "completed" | "failed" | "skipped") {
        return Ok(());
    }
    if !dispatch::valid(&mut tx, lease).await? {
        return Ok(());
    }
    if !s.config.enabled
        || !settings.enabled
        || !t.allowed
        || !s.config.apps.values().any(|app| app.app_id == t.app_id)
    {
        abandon(&mut tx, &t, "disabled_or_blocked").await?;
        dispatch::fence(&mut tx, lease).await?;
        tx.commit().await?;
        return Ok(());
    }
    let metadata = dispatch::meta(&mut tx, id).await?;
    let time = dispatch::now(&mut tx).await?;
    let budget = (metadata.platform_deadline_at - time - Duration::seconds(15))
        .num_seconds()
        .min(i64::from(settings.reply_timeout_secs));
    if budget < 10 && !t.content.starts_with('/') {
        abandon(&mut tx, &t, "window_expired").await?;
        dispatch::fence(&mut tx, lease).await?;
        tx.commit().await?;
        return Ok(());
    }
    let _permit = if t.content.starts_with('/') {
        None
    } else {
        match GENERATIONS.try_acquire() {
            Ok(p) => Some(p),
            Err(_) => {
                dispatch::reschedule(s, &mut tx, lease, time + Duration::seconds(1)).await?;
                tx.commit().await?;
                return Ok(());
            }
        }
    };
    t.attempts += 1;
    sqlx::query(
        "UPDATE qq_turns SET attempts=$2,phase='preparing',generation_deadline_at=$3 WHERE id=$1",
    )
    .bind(id)
    .bind(t.attempts)
    .bind(time + Duration::seconds(budget.max(1)))
    .execute(&mut *tx)
    .await?;
    let account = account(&mut tx, &t).await?;
    if matches!(account, Account::Blocked) {
        repo::terminal(&mut tx, id, "skipped", "account_blocked").await?;
        dispatch::fence(&mut tx, lease).await?;
        tx.commit().await?;
        return Ok(());
    }
    let bound = match account {
        Account::Bound(user_id) => Some(user_id),
        _ => None,
    };
    if t.content.starts_with('/') {
        let reply = crate::commands::run(s, &mut tx, &t, bound, &settings).await?;
        t.output = Some(vec![text::truncate(&reply.text, 4000)]);
        crate::identity::freeze(&mut tx, t.message_id).await?;
        t.reason = reply.reason.map(str::to_owned);
        save_output(&mut tx, &t, crate::message::MessageFormat::Plain).await?;
        dispatch::fence(&mut tx, lease).await?;
        tx.commit().await?;
        reply.effects.dispatch(s.ai.bus.as_ref());
        return delivery::deliver(s, lease).await;
    }
    let snapshot = crate::identity::freeze(&mut tx, t.message_id).await?;
    if !crate::authorization::validate(&mut tx, &snapshot).await? {
        repo::terminal(&mut tx, id, "skipped", "authorization_changed").await?;
        dispatch::fence(&mut tx, lease).await?;
        tx.commit().await?;
        return Ok(());
    }
    let Some(slug) = t.card_slug.clone() else {
        repo::terminal(&mut tx, id, "skipped", "no_role_card").await?;
        dispatch::fence(&mut tx, lease).await?;
        tx.commit().await?;
        return Ok(());
    };
    dispatch::fence(&mut tx, lease).await?;
    tx.commit().await?;
    let catalog = s.ai.catalog.get().await?;
    let Some(card) = catalog
        .cards
        .iter()
        .find(|c| c.slug == slug && c.enabled && c.qq_enabled)
    else {
        let mut tx = s.db.begin().await?;
        repo::terminal(&mut tx, id, "skipped", "role_disabled").await?;
        dispatch::fence(&mut tx, lease).await?;
        tx.commit().await?;
        return Ok(());
    };
    let mut conn = s.db.acquire().await?;
    let history = repo::history(&mut conn, &t, &settings, card.bound_user_id).await?;
    drop(conn);
    let scope = t.group_id.map(repo::group_scope);
    // 开场（这段上下文里还没有回复过）：私下场合可以问起到期的待跟进。
    let opening = !history.iter().any(|line| line.assistant);
    let prepared = match external::prepare_authorized(
        &s.ai,
        ExternalTurn {
            channel: "qq",
            card_slug: slug.clone(),
            // 游客专用模型只给游客；绑定用户与站内一样用角色卡的模型。
            model_key: if bound.is_some() {
                None
            } else {
                settings.guest_model_key.clone()
            },
            // 群是公开场合，论坛工具一律按游客视角（不按某个人的权限去读）；单聊里绑定用户用自己的身份（TOOL-004）。
            viewer: bound.filter(|_| t.group_id.is_none()),
            history,
            max_chars: settings.max_reply_chars as usize,
            output_mode: settings.reply_format.output_mode(),
            timeout_secs: budget.max(1) as u64,
            memory: bound.map(|user_id| ExternalMemory {
                user_id,
                public: scope.clone(),
                opening,
            }),
            shared_scope: scope
                .clone()
                .filter(|_| bound.is_some() || settings.guest_reads_group_memory),
        },
        std::sync::Arc::new(
            crate::authorization::QqAuthorization::new(snapshot.clone()).with_role(slug.clone()),
        ),
    )
    .await
    {
        Ok(p) => p,
        Err(p) => {
            let mut tx = s.db.begin().await?;
            repo::terminal(&mut tx, id, "skipped", "ai_unavailable").await?;
            dispatch::fence(&mut tx, lease).await?;
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
    let authorized = crate::authorization::validate(&mut tx, &snapshot).await?;
    let Some(mut t) = repo::turn(&mut tx, id).await? else {
        return Ok(());
    };
    if t.status != "queued" {
        return Ok(());
    }
    let paused: bool =
        sqlx::query_scalar("SELECT dispatch_paused FROM qq_settings WHERE singleton")
            .fetch_one(&mut *tx)
            .await?;
    if paused {
        dispatch::fence(&mut tx, lease).await?;
        sqlx::query("UPDATE qq_conversations SET lease_until=NULL,wake_at=NULL WHERE id=$1")
            .bind(lease.conversation_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(());
    }
    if !authorized
        || !t.allowed
        || t.user_id != bound
        || t.card_slug.as_deref() != Some(&slug)
        || dispatch::now(&mut tx).await? >= metadata.platform_deadline_at - Duration::seconds(15)
    {
        repo::terminal(&mut tx, id, "skipped", "state_changed").await?;
        dispatch::fence(&mut tx, lease).await?;
        tx.commit().await?;
        return Ok(());
    }
    let now = Utc::now();
    let ttl = Duration::seconds(900);
    let hold = if let Some(user_id) = bound {
        // 绑定用户：与站内私信同一个每日额度（QUOTA-005～010）。
        match quota::reserve(
            &mut tx,
            &s.ids,
            prepared.limits,
            user_id,
            estimate,
            now,
            ttl,
        )
        .await?
        {
            Ok(hold) => hold,
            Err(reason) => {
                // 配额日在北京时间 04:00 切换（QUOTA-004）。
                quota_notice(
                    &mut tx,
                    &mut t,
                    reason.as_str(),
                    "你今天的 AI 额度已用完，04:00 重置。",
                )
                .await?;
                dispatch::fence(&mut tx, lease).await?;
                tx.commit().await?;
                drop(_permit);
                return if t.output.is_some() {
                    delivery::deliver(s, lease).await
                } else {
                    Ok(())
                };
            }
        }
    } else {
        // Fixed lock order: site quota -> all guest tokens -> group usage -> identity usage.
        let site_hold =
            match quota::reserve_site_only(&mut tx, &s.ids, prepared.limits, estimate, now, ttl)
                .await?
            {
                Ok(h) => h,
                Err(reason) => {
                    quota_notice(
                        &mut tx,
                        &mut t,
                        reason.as_str(),
                        "今天的 AI 额度已用完，04:00 重置。",
                    )
                    .await?;
                    dispatch::fence(&mut tx, lease).await?;
                    tx.commit().await?;
                    drop(_permit);
                    return if t.output.is_some() {
                        delivery::deliver(s, lease).await
                    } else {
                        Ok(())
                    };
                }
            };
        if let Some(reason) = reserve_guest(&mut tx, &t, &settings, estimate, site_hold.day).await?
        {
            quota::settle(&mut tx, &site_hold, &t.execution_key, 0, now).await?;
            quota_notice(&mut tx,&mut t,reason,&format!("今天的免费次数或游客额度用完了（每天 {} 次，04:00 重置）。在绳网「设置 → 绑定 QQ」绑定后按个人额度使用，还能让我记住你。发送 /帮助 查看绑定方法。",settings.guest_daily_turns)).await?;
            dispatch::fence(&mut tx, lease).await?;
            tx.commit().await?;
            drop(_permit);
            return if t.output.is_some() {
                delivery::deliver(s, lease).await
            } else {
                Ok(())
            };
        }
        site_hold
    };
    let guest = bound.is_none();
    let guest_tokens = if guest { estimate } else { 0 };
    t.hold_id = hold.id;
    t.hold_amount = hold.amount;
    t.quota_day = Some(hold.day);
    t.payer_user_id = bound;
    t.guest_reserved = guest;
    t.guest_token_reserved = guest_tokens;
    t.ai_user_id = Some(prepared.ai_user_id);
    if !prepared.memory_enabled() {
        sqlx::query("UPDATE qq_turns SET personal_memory_processed_at=COALESCE(personal_memory_processed_at,clock_timestamp()),group_memory_processed_at=COALESCE(group_memory_processed_at,clock_timestamp()) WHERE id=$1").bind(id).execute(&mut *tx).await?;
    }
    sqlx::query("UPDATE qq_turns SET phase='generating',status='processing',ai_user_id=$2,hold_id=$3,quota_day=$4,hold_amount=$5,guest_reserved=$6,guest_token_reserved=$7,model_key=$8,payer_user_id=$9 WHERE id=$1")
        .bind(id).bind(prepared.ai_user_id).bind(hold.id).bind(hold.day).bind(hold.amount).bind(guest).bind(guest_tokens).bind(&prepared.model_key).bind(bound).execute(&mut *tx).await?;
    sqlx::query!(
        "UPDATE qq_messages SET ai_user_id=$2 WHERE id=$1",
        t.message_id,
        prepared.ai_user_id
    )
    .execute(&mut *tx)
    .await?;
    dispatch::fence(&mut tx, lease).await?;
    tx.commit().await?;

    let mut conn = s.db.acquire().await?;
    let generation_deadline = dispatch::meta(&mut conn, id)
        .await?
        .generation_deadline_at
        .unwrap_or(metadata.platform_deadline_at - Duration::seconds(15));
    let remaining = (generation_deadline - dispatch::now(&mut conn).await?)
        .to_std()
        .unwrap_or_default();
    drop(conn);
    let reply = tokio::time::timeout(remaining, external::reply(&s.ai, &prepared, id)).await;
    // Delivery has its own AppID capacity; a slow POST must not occupy a model slot.
    drop(_permit);
    let reply = match reply {
        Ok(reply) => reply,
        Err(_) => {
            let mut tx = s.db.begin().await?;
            crate::identity::lock_turn_identity(&mut tx, id).await?;
            if let Some(t) = repo::turn(&mut tx, id).await? {
                finalize_cost(
                    &mut tx,
                    &t,
                    if remaining.is_zero() {
                        0
                    } else {
                        hold.amount.max(estimate)
                    },
                )
                .await?;
                dispatch::finish(s, &mut tx, lease, &t, "failed", "generation_timeout").await?;
            }
            tx.commit().await?;
            return Ok(());
        }
    };
    let actual = reply.charge(hold.amount.max(estimate));
    let mut tx = s.db.begin().await?;
    let authorized = crate::authorization::validate(&mut tx, &snapshot).await?
        && delivery::role_enabled(&mut tx, Some(prepared.ai_user_id)).await?;
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
        settings.reply_format,
        &settings.blocked_words,
        settings.max_reply_chars as usize,
        t.group_id.is_some(),
    );
    let window_expired = dispatch::now(&mut tx).await? >= metadata.platform_deadline_at;
    if reply.outcome != RunOutcome::Completed
        || segments.is_empty()
        || window_expired
        || !t.allowed
        || t.user_id != bound
        || !authorized
    {
        refund_turn(&mut tx, &t).await?;
        let reason = if window_expired {
            "window_expired"
        } else if !t.allowed || t.user_id != bound {
            "disabled_or_blocked"
        } else {
            "generation_failed"
        };
        repo::terminal(&mut tx, id, "failed", reason).await?;
        dispatch::fence(&mut tx, lease).await?;
        tx.commit().await?;
        return Ok(());
    }
    t.output = Some(segments);
    if blocked {
        t.reason = Some("blocked_word".to_owned());
    }
    save_output(
        &mut tx,
        &t,
        if blocked {
            crate::message::MessageFormat::Plain
        } else {
            settings.reply_format
        },
    )
    .await?;
    sqlx::query("UPDATE qq_turns SET follow_up_ids=$2 WHERE id=$1")
        .bind(id)
        .bind(prepared.follow_up_ids())
        .execute(&mut *tx)
        .await?;
    dispatch::fence(&mut tx, lease).await?;
    tx.commit().await?;
    delivery::deliver(s, lease).await
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
        save_output(c, t, crate::message::MessageFormat::Plain).await
    } else {
        repo::terminal(c, t.id, "skipped", reason).await
    }
}
async fn save_output(
    c: &mut PgConnection,
    t: &Turn,
    format: crate::message::MessageFormat,
) -> Result<(), Problem> {
    sqlx::query("UPDATE qq_turns SET status='processing',phase='delivering',output=$2,reason=$3,delivery_deadline_at=LEAST(platform_deadline_at-interval '2 seconds',clock_timestamp()+interval '60 seconds') WHERE id=$1")
        .bind(t.id).bind(t.output.as_deref()).bind(&t.reason).execute(&mut *c).await?;
    for (i, content) in t.output.iter().flatten().enumerate() {
        let hash = format.hash(content);
        sqlx::query("INSERT INTO qq_reply_segments(turn_id,msg_seq,content,payload_hash,format) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(t.id).bind(i as i32+1).bind(content).bind(hash).bind(format).execute(&mut *c).await?;
    }
    Ok(())
}

async fn finalize_cost(c: &mut PgConnection, t: &Turn, actual: i64) -> Result<(), Problem> {
    let finalized: bool =
        sqlx::query_scalar("SELECT cost_finalized_at IS NOT NULL FROM qq_turns WHERE id=$1")
            .bind(t.id)
            .fetch_one(&mut *c)
            .await?;
    if finalized {
        return Ok(());
    }
    let Some(h) = hold(t) else {
        return Ok(());
    };
    if matches!(
        quota::settle(c, &h, &t.execution_key, actual, Utc::now()).await?,
        quota::Settled::Duplicate
    ) {
        sqlx::query("UPDATE qq_turns SET cost_finalized_at=clock_timestamp() WHERE id=$1")
            .bind(t.id)
            .execute(c)
            .await?;
        return Ok(());
    }
    // 游客合计只记游客的用量；绑定用户的用量已经记在个人与全站计数上。
    if t.payer_user_id.is_none() {
        let delta = actual - t.guest_token_reserved;
        sqlx::query!(
            "UPDATE qq_guest_tokens SET used=GREATEST(0,used+$2) WHERE day=$1",
            h.day,
            delta
        )
        .execute(&mut *c)
        .await?;
    }
    // 群的当日用量统计（后台展示）：绑定用户没有经过游客预占，这一行可能还不存在。
    if let Some(g) = t.group_id {
        sqlx::query!(
            "INSERT INTO qq_group_usage(day,group_id,tokens) VALUES($1,$2,$3) ON CONFLICT(day,group_id) DO UPDATE SET tokens=qq_group_usage.tokens+EXCLUDED.tokens",
            h.day,
            g,
            actual
        )
        .execute(&mut *c)
        .await?;
    }
    sqlx::query("UPDATE qq_turns SET guest_token_reserved=0,cost_finalized_at=clock_timestamp() WHERE id=$1").bind(t.id)
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

pub async fn prune(s: &QqState) -> Result<(), Problem> {
    reconcile(s).await?;
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
    // 过期一天以上的绑定码（留一天给排障）。
    sqlx::query!(
        "DELETE FROM qq_bind_codes WHERE expires_at<$1",
        Utc::now() - Duration::days(1)
    )
    .execute(&mut *tx)
    .await?;
    let left = sqlx::query_scalar!(
        "SELECT id FROM qq_groups WHERE left_at<$1",
        Utc::now() - Duration::days(30)
    )
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    // 机器人退群 30 天后清掉那个群的共享记忆（ADR 0008 §8.3）；重新加群从头开始。
    for group_id in left {
        ik_ai::shared_memory::clear(&s.ai, &repo::group_scope(group_id)).await?;
    }
    Ok(())
}
