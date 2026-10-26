//! PostgreSQL owns conversation order and leases; queue jobs are disposable wakeups.
use super::*;
use chrono::{DateTime, Utc};
use ik_kernel::jobs::JobSpec;
use sqlx::Row;

pub(super) const LEASE_SECONDS: i64 = 30;

#[derive(Debug, Clone, Copy)]
pub(super) struct Lease {
    pub conversation_id: i64,
    pub turn_id: i64,
    pub epoch: i64,
}

#[derive(sqlx::FromRow)]
pub(super) struct Meta {
    pub phase: String,
    pub protocol_version: i16,
    pub platform_deadline_at: DateTime<Utc>,
    pub generation_deadline_at: Option<DateTime<Utc>>,
    pub delivery_deadline_at: Option<DateTime<Utc>>,
}

pub(super) async fn now(c: &mut PgConnection) -> Result<DateTime<Utc>, Problem> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(c)
        .await?)
}

pub(super) async fn meta(c: &mut PgConnection, id: i64) -> Result<Meta, Problem> {
    Ok(sqlx::query_as("SELECT phase,protocol_version,platform_deadline_at,generation_deadline_at,delivery_deadline_at FROM qq_turns WHERE id=$1")
        .bind(id).fetch_one(c).await?)
}

/// Called in the same transaction as the incoming message and turn insertion.
pub(crate) async fn enqueue_turn(
    s: &QqState,
    c: &mut PgConnection,
    turn_id: i64,
) -> Result<(), Problem> {
    attach_turn(s, c, turn_id, true).await
}

pub(super) async fn attach_legacy_turn(
    s: &QqState,
    c: &mut PgConnection,
    id: i64,
) -> Result<(), Problem> {
    attach_turn(s, c, id, false).await
}

async fn attach_turn(
    s: &QqState,
    c: &mut PgConnection,
    turn_id: i64,
    fresh: bool,
) -> Result<(), Problem> {
    let row = sqlx::query("SELECT t.conversation_id,t.status,m.app_id,i.scene,COALESCE(t.group_id,t.identity_id) target_id,m.created_at FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id JOIN qq_identities i ON i.id=t.identity_id WHERE t.id=$1")
        .bind(turn_id).fetch_optional(&mut *c).await?;
    let Some(row) = row else {
        return Ok(());
    };
    if terminal_status(row.try_get::<&str, _>("status")?) {
        return Ok(());
    }
    if let Some(id) = row.try_get::<Option<i64>, _>("conversation_id")? {
        let time = now(c).await?;
        return wake(s, c, id, time).await;
    }
    let scene: String = row.try_get("scene")?;
    let app_id: String = row.try_get("app_id")?;
    let target_id: i64 = row.try_get("target_id")?;
    let id: i64 = sqlx::query_scalar("INSERT INTO qq_conversations(id,app_id,scene,target_id) VALUES($1,$2,$3,$4) ON CONFLICT(app_id,scene,target_id) DO UPDATE SET next_seq=qq_conversations.next_seq RETURNING id")
        .bind(s.ids.next_raw()).bind(&app_id).bind(&scene).bind(target_id).fetch_one(&mut *c).await?;
    // Intake remains live during a dispatch pause. Attach earlier legacy turns
    // before assigning the fresh message a sequence, under the conversation lock.
    let pending=sqlx::query("SELECT t.id,m.created_at FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id JOIN qq_identities i ON i.id=t.identity_id WHERE t.conversation_id IS NULL AND t.status IN ('queued','processing') AND m.app_id=$1 AND i.scene=$2 AND COALESCE(t.group_id,t.identity_id)=$3 AND (t.created_at,t.id)<=(SELECT created_at,id FROM qq_turns WHERE id=$4) ORDER BY t.created_at,t.id FOR UPDATE OF t")
        .bind(&app_id).bind(&scene).bind(target_id).bind(turn_id).fetch_all(&mut *c).await?;
    for row in pending {
        let candidate: i64 = row.try_get("id")?;
        let seq: i64 = sqlx::query_scalar(
            "UPDATE qq_conversations SET next_seq=next_seq+1 WHERE id=$1 RETURNING next_seq",
        )
        .bind(id)
        .fetch_one(&mut *c)
        .await?;
        let at: DateTime<Utc> = row.try_get("created_at")?;
        let deadline = at + Duration::minutes(if scene == "group" { 5 } else { 60 });
        sqlx::query("UPDATE qq_turns SET conversation_id=$2,conversation_seq=$3,platform_deadline_at=$4,protocol_version=$5 WHERE id=$1")
            .bind(candidate).bind(id).bind(seq).bind(deadline).bind(if fresh && candidate==turn_id {2_i16} else {0_i16}).execute(&mut *c).await?;
    }
    let at = now(c).await?;
    wake(s, c, id, at).await
}

pub(super) async fn wake(
    s: &QqState,
    c: &mut PgConnection,
    id: i64,
    at: DateTime<Utc>,
) -> Result<(), Problem> {
    let version: i64 = sqlx::query_scalar("UPDATE qq_conversations SET wake_version=wake_version+1,wake_at=LEAST(COALESCE(wake_at,$2),$2) WHERE id=$1 RETURNING wake_version")
        .bind(id).bind(at).fetch_one(&mut *c).await?;
    s.jobs
        .enqueue(
            c,
            Dispatch {
                conversation_id: id,
            },
            JobSpec {
                queue_name: None,
                job_key: Some(format!("qq-dispatch:{id}:{version}")),
                run_at: Some(at),
                max_attempts: Some(3),
                ..Default::default()
            },
        )
        .await
}

pub(super) fn terminal_status(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "skipped")
}

async fn claim(s: &QqState, id: i64) -> Result<Option<Lease>, Problem> {
    let mut tx = s.db.begin().await?;
    let paused: bool =
        sqlx::query_scalar("SELECT dispatch_paused FROM qq_settings WHERE singleton")
            .fetch_one(&mut *tx)
            .await?;
    if paused {
        return Ok(None);
    }
    let row=sqlx::query("SELECT active_turn_id,lease_epoch,lease_until,wake_at FROM qq_conversations WHERE id=$1 FOR UPDATE")
        .bind(id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else { return Ok(None) };
    let time = now(&mut tx).await?;
    if row
        .try_get::<Option<DateTime<Utc>>, _>("lease_until")?
        .is_some_and(|until| until > time)
    {
        return Ok(None);
    }
    let active: Option<i64> = row.try_get("active_turn_id")?;
    let turn=sqlx::query_scalar::<_,i64>("SELECT id FROM qq_turns WHERE conversation_id=$1 AND status IN ('queued','processing') ORDER BY conversation_seq LIMIT 1")
        .bind(id).fetch_optional(&mut *tx).await?;
    let Some(turn_id) = turn else {
        sqlx::query("UPDATE qq_conversations SET active_turn_id=NULL,lease_until=NULL,wake_at=NULL WHERE id=$1").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(None);
    };
    if active == Some(turn_id)
        && row
            .try_get::<Option<DateTime<Utc>>, _>("wake_at")?
            .is_some_and(|at| at > time)
    {
        return Ok(None);
    }
    let epoch:i64=sqlx::query_scalar("UPDATE qq_conversations SET active_turn_id=$2,lease_epoch=lease_epoch+1,lease_until=$3,wake_at=NULL WHERE id=$1 RETURNING lease_epoch")
        .bind(id).bind(turn_id).bind(time+Duration::seconds(LEASE_SECONDS)).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(Lease {
        conversation_id: id,
        turn_id,
        epoch,
    }))
}

pub(super) async fn valid(c: &mut PgConnection, l: &Lease) -> Result<bool, Problem> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM qq_conversations WHERE id=$1 AND active_turn_id=$2 AND lease_epoch=$3 AND lease_until>clock_timestamp())")
        .bind(l.conversation_id).bind(l.turn_id).bind(l.epoch).fetch_one(c).await?)
}

/// Call after turn/identity mutations, before commit. Lock order is turn -> conversation.
pub(super) async fn fence(c: &mut PgConnection, l: &Lease) -> Result<(), Problem> {
    let changed=sqlx::query("UPDATE qq_conversations SET lease_until=clock_timestamp()+interval '30 seconds' WHERE id=$1 AND active_turn_id=$2 AND lease_epoch=$3 AND lease_until>clock_timestamp()")
        .bind(l.conversation_id).bind(l.turn_id).bind(l.epoch).execute(c).await?.rows_affected();
    if changed == 0 {
        return Err(Problem::service_unavailable());
    }
    Ok(())
}

pub(super) async fn reschedule(
    s: &QqState,
    c: &mut PgConnection,
    l: &Lease,
    at: DateTime<Utc>,
) -> Result<(), Problem> {
    fence(c, l).await?;
    sqlx::query("UPDATE qq_conversations SET lease_until=NULL,wake_at=$2 WHERE id=$1")
        .bind(l.conversation_id)
        .bind(at)
        .execute(&mut *c)
        .await?;
    wake(s, c, l.conversation_id, at).await
}

pub(super) async fn finish(
    s: &QqState,
    c: &mut PgConnection,
    l: &Lease,
    t: &Turn,
    status: &str,
    reason: &str,
) -> Result<(), Problem> {
    // Quota finalization is permanent; a later stale task cannot refund twice.
    let finalized: Option<DateTime<Utc>> =
        sqlx::query_scalar("SELECT quota_finalized_at FROM qq_turns WHERE id=$1")
            .bind(t.id)
            .fetch_one(&mut *c)
            .await?;
    if finalized.is_none() {
        super::refund_turn(c, t).await?;
        sqlx::query("UPDATE qq_turns SET quota_finalized_at=clock_timestamp() WHERE id=$1")
            .bind(t.id)
            .execute(&mut *c)
            .await?;
    }
    repo::terminal(c, t.id, status, reason).await?;
    sqlx::query("UPDATE qq_turns SET phase='terminal' WHERE id=$1")
        .bind(t.id)
        .execute(&mut *c)
        .await?;
    sqlx::query("UPDATE qq_reply_segments SET status='cancelled',updated_at=clock_timestamp() WHERE turn_id=$1 AND status='pending'").bind(t.id).execute(&mut *c).await?;
    let snapshot:Option<crate::identity::Snapshot>=sqlx::query_as("SELECT m.identity_id,m.owner_user_id,m.binding_version FROM qq_messages m WHERE m.id=$1 AND m.binding_version IS NOT NULL")
        .bind(t.message_id).fetch_optional(&mut *c).await?;
    let can_remember = if let Some(snapshot) = snapshot {
        t.allowed
            && super::delivery::role_enabled(c, t.ai_user_id).await?
            && crate::authorization::validate(c, &snapshot).await?
    } else {
        false
    };
    if t.segments_sent > 0 && can_remember {
        let ids: Vec<i64> = sqlx::query_scalar("SELECT follow_up_ids FROM qq_turns WHERE id=$1")
            .bind(t.id)
            .fetch_one(&mut *c)
            .await?;
        let time = now(c).await?;
        ik_ai::external::mark_delivered(c, &ids, time).await?;
        crate::memory_jobs::finish_turn(s, c, t.id, time).await?;
    }
    fence(c, l).await?;
    sqlx::query(
        "UPDATE qq_conversations SET active_turn_id=NULL,lease_until=NULL,wake_at=NULL WHERE id=$1",
    )
    .bind(l.conversation_id)
    .execute(&mut *c)
    .await?;
    let next:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM qq_turns WHERE conversation_id=$1 AND status IN ('queued','processing'))").bind(l.conversation_id).fetch_one(&mut *c).await?;
    if next {
        let at = now(c).await?;
        wake(s, c, l.conversation_id, at).await?;
    }
    Ok(())
}

pub async fn run_dispatch(s: &QqState, id: i64) -> Result<(), Problem> {
    let Some(lease) = claim(s, id).await? else {
        return Ok(());
    };
    let mut tx = s.db.begin().await?;
    crate::identity::lock_turn_identity(&mut tx, lease.turn_id).await?;
    let Some(t) = repo::turn(&mut tx, lease.turn_id).await? else {
        return Ok(());
    };
    if !valid(&mut tx, &lease).await? {
        return Ok(());
    }
    let m = meta(&mut tx, t.id).await?;
    if m.protocol_version == 0 {
        if t.status == "processing" {
            if t.output.is_none() {
                super::finalize_cost(&mut tx, &t, t.hold_amount.max(t.guest_token_reserved))
                    .await?;
            }
            let done = t
                .output
                .as_ref()
                .is_some_and(|parts| t.segments_sent as usize == parts.len());
            finish(
                s,
                &mut tx,
                &lease,
                &t,
                if done { "completed" } else { "failed" },
                if done {
                    "legacy_completed"
                } else {
                    "legacy_delivery_unknown"
                },
            )
            .await?;
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query("UPDATE qq_turns SET protocol_version=2 WHERE id=$1")
            .bind(t.id)
            .execute(&mut *tx)
            .await?;
    }
    if m.phase == "generating" {
        super::finalize_cost(&mut tx, &t, t.hold_amount.max(t.guest_token_reserved)).await?;
        finish(s, &mut tx, &lease, &t, "failed", "generation_interrupted").await?;
        tx.commit().await?;
        return Ok(());
    }
    if m.phase == "delivering" {
        sqlx::query("UPDATE qq_reply_segments SET status='unknown',updated_at=clock_timestamp() WHERE turn_id=$1 AND status='sending'").bind(t.id).execute(&mut *tx).await?;
        fence(&mut tx, &lease).await?;
        tx.commit().await?;
        return heartbeat(s, &lease, super::delivery::deliver(s, &lease)).await;
    }
    fence(&mut tx, &lease).await?;
    tx.commit().await?;
    heartbeat(s, &lease, super::generate(s, &lease)).await?;
    // Generation can terminate without ever creating an outbox (quota, disabled,
    // failed model); move the conversation on in the same fenced transaction.
    let mut tx = s.db.begin().await?;
    crate::identity::lock_turn_identity(&mut tx, lease.turn_id).await?;
    if !valid(&mut tx, &lease).await? {
        return Ok(());
    }
    if let Some(t) = repo::turn(&mut tx, lease.turn_id).await? {
        if terminal_status(&t.status) {
            finish(
                s,
                &mut tx,
                &lease,
                &t,
                &t.status,
                t.reason.as_deref().unwrap_or("finished"),
            )
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

pub(super) async fn heartbeat<F>(s: &QqState, l: &Lease, future: F) -> Result<(), Problem>
where
    F: std::future::Future<Output = Result<(), Problem>>,
{
    async fn renew(s: &QqState, l: &Lease) -> Result<(), Problem> {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        loop {
            interval.tick().await;
            let mut conn = s.db.acquire().await?;
            fence(&mut conn, l).await?;
        }
    }
    // A renewal may wait for the conversation lock held by the work transaction.
    // Keep polling that transaction so it can commit and release the lock.
    tokio::select! {
        biased;
        result=future=>result,
        result=renew(s,l)=>result,
    }
}

pub async fn reconcile(s: &QqState) -> Result<(), Problem> {
    let legacy:Vec<i64>=sqlx::query_scalar("SELECT id FROM qq_turns WHERE conversation_id IS NULL AND status IN ('queued','processing') ORDER BY created_at,id LIMIT 500").fetch_all(&s.db).await?;
    for id in legacy {
        let mut tx = s.db.begin().await?;
        attach_turn(s, &mut tx, id, false).await?;
        tx.commit().await?;
    }
    let mut tx = s.db.begin().await?;
    let ids:Vec<i64>=sqlx::query_scalar("SELECT c.id FROM qq_conversations c WHERE (c.lease_until IS NULL OR c.lease_until<=clock_timestamp()) AND EXISTS(SELECT 1 FROM qq_turns t WHERE t.conversation_id=c.id AND t.status IN ('queued','processing')) AND (c.wake_at IS NULL OR c.wake_at<=clock_timestamp()) ORDER BY c.id LIMIT 500 FOR UPDATE SKIP LOCKED")
        .fetch_all(&mut *tx).await?;
    for id in ids {
        let at = now(&mut tx).await?;
        wake(s, &mut tx, id, at).await?;
    }
    tx.commit().await?;
    Ok(())
}
