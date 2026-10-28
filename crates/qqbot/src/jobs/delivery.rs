//! Durable, single-POST delivery. Unknown outcomes never become fabricated receipts.
use super::{
    dispatch::{self, Lease},
    *,
};
use crate::client::{ApiEndpoint, ApiMetadata, ErrorClass, SendAttempt, SendOutcome};
use crate::message::{Message, MessageFormat};
use chrono::{DateTime, Utc};
use sqlx::Row;
use std::time::Duration as StdDuration;

const MAX_ATTEMPTS: i32 = 3;
const APP_INTERVAL_MS: i64 = 200;
const APP_IN_FLIGHT: i64 = 4;

#[derive(sqlx::FromRow)]
struct Segment {
    msg_seq: i32,
    content: String,
    format: MessageFormat,
    payload_hash: Vec<u8>,
    payload_version: i32,
    status: String,
    attempt_count: i32,
    retry_at: Option<DateTime<Utc>>,
    auth_corrected: bool,
    url_corrected: bool,
}

fn timeout_attempt() -> SendAttempt {
    SendAttempt {
        outcome: SendOutcome::Unknown,
        class: ErrorClass::Transient,
        meta: ApiMetadata {
            endpoint: ApiEndpoint::Message,
            http_status: None,
            code: None,
            err_code: None,
            trace_id: None,
            retry_after: None,
            malformed: false,
        },
        message_id: None,
        audit_id: None,
        token_generation: None,
        message_attempted: true,
    }
}

fn retry_delay(attempt: i32) -> Duration {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 2];
    let jitter = if ring::rand::SystemRandom::new().fill(&mut bytes).is_ok() {
        i64::from(u16::from_be_bytes(bytes) % 501)
    } else {
        0
    };
    Duration::milliseconds(if attempt <= 1 { 2000 } else { 5000 } + jitter)
}

pub(super) async fn snapshot(
    c: &mut PgConnection,
    id: i64,
) -> Result<crate::identity::Snapshot, Problem> {
    Ok(sqlx::query_as("SELECT m.identity_id,m.owner_user_id,m.binding_version FROM qq_messages m JOIN qq_turns t ON t.message_id=m.id WHERE t.id=$1 AND m.binding_version IS NOT NULL")
        .bind(id).fetch_one(c).await?)
}

pub(super) async fn role_enabled(
    c: &mut PgConnection,
    ai_user_id: Option<i64>,
) -> Result<bool, Problem> {
    let Some(id) = ai_user_id else {
        return Ok(true);
    };
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ai_role_cards WHERE bound_user_id=$1 AND enabled AND qq_enabled)").bind(id).fetch_one(c).await?)
}

async fn uncertain(c: &mut PgConnection, id: i64) -> Result<bool, Problem> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM qq_reply_segments WHERE turn_id=$1 AND status IN ('unknown','sending','pending_review')) OR EXISTS(SELECT 1 FROM qq_send_attempts a JOIN qq_reply_segments p ON p.turn_id=a.turn_id AND p.msg_seq=a.msg_seq WHERE a.turn_id=$1 AND p.status<>'accepted' AND a.result IN ('unknown','pending_review'))")
        .bind(id).fetch_one(c).await?)
}

async fn stop(
    s: &QqState,
    c: &mut PgConnection,
    l: &Lease,
    t: &Turn,
    reason: &str,
) -> Result<(), Problem> {
    let reason = if uncertain(c, t.id).await? {
        "delivery_unknown"
    } else {
        reason
    };
    dispatch::finish(s, c, l, t, "failed", reason).await
}

pub(super) async fn deliver(s: &QqState, l: &Lease) -> Result<(), Problem> {
    // At most four successful bubbles; waiting/retries always release the worker.
    for _ in 0..5 {
        let mut tx = s.db.begin().await?;
        let frozen = snapshot(&mut tx, l.turn_id).await?;
        let authorized = crate::authorization::validate(&mut tx, &frozen).await?;
        let Some(t) = repo::turn(&mut tx, l.turn_id).await? else {
            return Ok(());
        };
        if !dispatch::valid(&mut tx, l).await? || dispatch::terminal_status(&t.status) {
            return Ok(());
        }
        if !authorized
            || !s.config.enabled
            || !t.allowed
            || !role_enabled(&mut tx, t.ai_user_id).await?
        {
            stop(s, &mut tx, l, &t, "authorization_changed").await?;
            tx.commit().await?;
            return Ok(());
        }
        let paused: bool =
            sqlx::query_scalar("SELECT dispatch_paused FROM qq_settings WHERE singleton")
                .fetch_one(&mut *tx)
                .await?;
        if paused {
            dispatch::fence(&mut tx, l).await?;
            sqlx::query("UPDATE qq_conversations SET lease_until=NULL,wake_at=NULL WHERE id=$1")
                .bind(l.conversation_id)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            return Ok(());
        }
        // The send deadline limits further requests, not completion of receipts
        // already committed before a worker stopped.
        let segment:Option<Segment>=sqlx::query_as("SELECT msg_seq,content,format,payload_hash,payload_version,status,attempt_count,retry_at,auth_corrected,url_corrected FROM qq_reply_segments WHERE turn_id=$1 AND status<>'accepted' ORDER BY msg_seq LIMIT 1 FOR UPDATE")
            .bind(t.id).fetch_optional(&mut *tx).await?;
        let Some(segment) = segment else {
            dispatch::finish(
                s,
                &mut tx,
                l,
                &t,
                "completed",
                t.reason.as_deref().unwrap_or("delivered"),
            )
            .await?;
            tx.commit().await?;
            return Ok(());
        };
        let metadata = dispatch::meta(&mut tx, t.id).await?;
        let time = dispatch::now(&mut tx).await?;
        let deadline = metadata
            .delivery_deadline_at
            .unwrap_or(metadata.platform_deadline_at);
        if time >= deadline {
            stop(s, &mut tx, l, &t, "delivery_expired").await?;
            tx.commit().await?;
            return Ok(());
        }
        if matches!(segment.status.as_str(), "rejected" | "cancelled")
            || segment.attempt_count >= MAX_ATTEMPTS
        {
            stop(s, &mut tx, l, &t, "send_attempts_exhausted").await?;
            tx.commit().await?;
            return Ok(());
        }
        if let Some(at) = segment.retry_at.filter(|at| *at > time) {
            dispatch::reschedule(s, &mut tx, l, at.min(deadline)).await?;
            tx.commit().await?;
            return Ok(());
        }
        // One row serializes admission for all instances of this AppID.
        let limits=sqlx::query("INSERT INTO qq_app_delivery_limits(app_id) VALUES($1) ON CONFLICT(app_id) DO UPDATE SET next_send_at=qq_app_delivery_limits.next_send_at RETURNING GREATEST(next_send_at,clock_timestamp()-interval '1 second') AS next_send_at,GREATEST(blocked_until,clock_timestamp()-interval '1 second') AS blocked_until")
            .bind(&t.app_id).fetch_one(&mut *tx).await?;
        // Admission may have waited on another process. Recheck the database
        // clock after the lock, then use elapsed monotonic time during HTTP.
        let budget_checked_at = std::time::Instant::now();
        let time = dispatch::now(&mut tx).await?;
        if time >= deadline {
            stop(s, &mut tx, l, &t, "delivery_expired").await?;
            tx.commit().await?;
            return Ok(());
        }
        let gate = limits
            .try_get::<DateTime<Utc>, _>("next_send_at")?
            .max(limits.try_get("blocked_until")?);
        let in_flight:i64=sqlx::query_scalar("SELECT count(*) FROM qq_reply_segments p JOIN qq_turns t ON t.id=p.turn_id JOIN qq_conversations c ON c.id=t.conversation_id WHERE c.app_id=$1 AND p.status='sending' AND c.lease_until>clock_timestamp()")
            .bind(&t.app_id).fetch_one(&mut *tx).await?;
        if gate > time || in_flight >= APP_IN_FLIGHT {
            let at = gate
                .max(time + Duration::milliseconds(APP_INTERVAL_MS))
                .min(deadline);
            dispatch::reschedule(s, &mut tx, l, at).await?;
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query("UPDATE qq_app_delivery_limits SET next_send_at=$2 WHERE app_id=$1")
            .bind(&t.app_id)
            .bind(time + Duration::milliseconds(APP_INTERVAL_MS))
            .execute(&mut *tx)
            .await?;
        let reserved_count = segment.attempt_count + 1;
        let attempt_no: i32 = sqlx::query_scalar("SELECT COALESCE(max(attempt_no),0)+1 FROM qq_send_attempts WHERE turn_id=$1 AND msg_seq=$2")
            .bind(t.id).bind(segment.msg_seq).fetch_one(&mut *tx).await?;
        let attempt_id = s.ids.next_raw();
        sqlx::query("UPDATE qq_reply_segments SET status='sending',attempt_count=$3,attempt_epoch=$4,retry_at=NULL,updated_at=clock_timestamp() WHERE turn_id=$1 AND msg_seq=$2")
            .bind(t.id).bind(segment.msg_seq).bind(reserved_count).bind(l.epoch).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO qq_send_attempts(id,turn_id,msg_seq,attempt_no,lease_epoch,payload_hash,payload_version,format) VALUES($1,$2,$3,$4,$5,$6,$7,$8)")
            .bind(attempt_id).bind(t.id).bind(segment.msg_seq).bind(attempt_no).bind(l.epoch).bind(&segment.payload_hash).bind(segment.payload_version).bind(segment.format).execute(&mut *tx).await?;
        dispatch::fence(&mut tx, l).await?;
        tx.commit().await?;

        let timeout = (deadline - time)
            .to_std()
            .unwrap_or_default()
            .saturating_sub(budget_checked_at.elapsed())
            .min(StdDuration::from_secs(10));
        let attempt = tokio::time::timeout(
            timeout,
            s.client.send_message_once(
                &t.app_id,
                &t.scene,
                &t.openid,
                t.qq_msg_id.as_deref().unwrap_or_default(),
                segment.msg_seq,
                Message::new(segment.format, &segment.content),
            ),
        )
        .await
        .unwrap_or_else(|_| timeout_attempt());
        // Token invalidation is in-memory IO and never runs while holding a DB transaction.
        if attempt.class == ErrorClass::Auth && !segment.auth_corrected {
            if let Some(generation) = attempt.token_generation {
                s.client.invalidate_token(&t.app_id, generation).await;
            }
        }
        let mut tx = s.db.begin().await?;
        // Authorization must lock user/identity before the turn row.
        let authorized = crate::authorization::validate(&mut tx, &frozen).await?
            && role_enabled(&mut tx, t.ai_user_id).await?;
        let Some(current) = repo::turn(&mut tx, t.id).await? else {
            return Ok(());
        };
        let authorized = authorized && current.allowed;
        if !dispatch::valid(&mut tx, l).await? {
            return Ok(());
        }
        let result = if !attempt.message_attempted {
            // Token failures did not submit a message. Keep their own ledger
            // entry, but release the conservatively reserved POST attempt.
            sqlx::query("UPDATE qq_reply_segments SET attempt_count=GREATEST(attempt_count-1,0) WHERE turn_id=$1 AND msg_seq=$2")
                .bind(t.id).bind(segment.msg_seq).execute(&mut *tx).await?;
            "not_sent"
        } else {
            match attempt.outcome {
                SendOutcome::Accepted => "accepted",
                SendOutcome::Pending => "pending_review",
                SendOutcome::Rejected => "rejected",
                SendOutcome::Unknown => "unknown",
            }
        };
        let endpoint = match attempt.meta.endpoint {
            ApiEndpoint::Token => "token",
            ApiEndpoint::Message => "message",
            ApiEndpoint::GroupMessage => "group_message",
            ApiEndpoint::C2cMessage => "c2c_message",
        };
        let code = attempt
            .meta
            .err_code
            .filter(|n| *n != 0)
            .or(attempt.meta.code);
        sqlx::query("UPDATE qq_send_attempts SET finished_at=clock_timestamp(),result=$2,endpoint=$3,http_status=$4,platform_code=$5,code=$6,err_code=$7,trace_id=$8,malformed=$9,audit_id=$10 WHERE id=$1")
            .bind(attempt_id).bind(result).bind(endpoint).bind(attempt.meta.http_status.map(i32::from)).bind(code).bind(attempt.meta.code).bind(attempt.meta.err_code).bind(&attempt.meta.trace_id).bind(attempt.meta.malformed).bind(&attempt.audit_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE qq_reply_segments SET last_http_status=$3,last_platform_code=$4,trace_id=$5,updated_at=clock_timestamp() WHERE turn_id=$1 AND msg_seq=$2")
            .bind(t.id).bind(segment.msg_seq).bind(attempt.meta.http_status.map(i32::from)).bind(code).bind(&attempt.meta.trace_id).execute(&mut *tx).await?;
        if attempt.outcome == SendOutcome::Accepted {
            sqlx::query("UPDATE qq_reply_segments SET status='accepted',qq_message_id=$3,accepted_at=clock_timestamp() WHERE turn_id=$1 AND msg_seq=$2")
                .bind(t.id).bind(segment.msg_seq).bind(&attempt.message_id).execute(&mut *tx).await?;
            if authorized {
                sqlx::query("INSERT INTO qq_messages(id,app_id,identity_id,group_id,ai_user_id,direction,content,reply_to_id,qq_msg_id) VALUES($1,$2,$3,$4,$5,'out',$6,$7,$8)")
                    .bind(s.ids.next_raw()).bind(&t.app_id).bind(t.identity_id).bind(t.group_id).bind(t.ai_user_id).bind(&segment.content).bind(t.message_id).bind(&attempt.message_id).execute(&mut *tx).await?;
            } else {
                // A deletion/unbind may have cleared the persisted text during HTTP.
                // Record the real receipt without resurrecting that private content.
                sqlx::query("UPDATE qq_reply_segments SET content='' WHERE turn_id=$1")
                    .bind(t.id)
                    .execute(&mut *tx)
                    .await?;
            }
            sqlx::query("UPDATE qq_turns SET segments_sent=(SELECT count(*)::integer FROM qq_reply_segments WHERE turn_id=$1 AND status='accepted') WHERE id=$1")
                .bind(t.id).execute(&mut *tx).await?;
            if !authorized {
                if let Some(current) = repo::turn(&mut tx, t.id).await? {
                    dispatch::finish(s, &mut tx, l, &current, "failed", "authorization_changed")
                        .await?;
                }
                tx.commit().await?;
                return Ok(());
            }
            dispatch::fence(&mut tx, l).await?;
            tx.commit().await?;
            continue;
        }
        let unknown = attempt.message_attempted
            && matches!(attempt.outcome, SendOutcome::Unknown | SendOutcome::Pending);
        sqlx::query("UPDATE qq_reply_segments SET status=$3 WHERE turn_id=$1 AND msg_seq=$2")
            .bind(t.id)
            .bind(segment.msg_seq)
            .bind(
                if attempt.message_attempted && attempt.outcome == SendOutcome::Pending {
                    "pending_review"
                } else if unknown {
                    "unknown"
                } else {
                    "pending"
                },
            )
            .execute(&mut *tx)
            .await?;
        if !authorized {
            stop(s, &mut tx, l, &current, "authorization_changed").await?;
            tx.commit().await?;
            return Ok(());
        }
        let time = dispatch::now(&mut tx).await?;
        let mut correction = false;
        if attempt.class == ErrorClass::Auth && !segment.auth_corrected {
            sqlx::query(
                "UPDATE qq_reply_segments SET auth_corrected=TRUE WHERE turn_id=$1 AND msg_seq=$2",
            )
            .bind(t.id)
            .bind(segment.msg_seq)
            .execute(&mut *tx)
            .await?;
            correction = true;
        }
        if attempt.class == ErrorClass::Url
            && attempt.outcome == SendOutcome::Rejected
            && !segment.url_corrected
            && !uncertain(&mut tx, t.id).await?
        {
            let clean = crate::text::without_links(&segment.content, segment.format);
            if !clean.trim().is_empty() && clean != segment.content {
                let hash = segment.format.hash(&clean);
                sqlx::query("UPDATE qq_reply_segments SET content=$3,payload_hash=$4,payload_version=payload_version+1,url_corrected=TRUE WHERE turn_id=$1 AND msg_seq=$2")
                    .bind(t.id).bind(segment.msg_seq).bind(clean).bind(hash).execute(&mut *tx).await?;
                correction = true;
            }
        }
        if attempt.class == ErrorClass::Markdown
            && attempt.outcome == SendOutcome::Rejected
            && segment.format == MessageFormat::Markdown
            && !uncertain(&mut tx, t.id).await?
        {
            let plain = crate::text::format_content(
                &segment.content,
                crate::text::Links {
                    mode: "allow",
                    site: &s.site_url,
                },
                MessageFormat::Plain,
                segment.content.chars().count(),
            );
            if !plain.trim().is_empty() {
                let hash = MessageFormat::Plain.hash(&plain);
                sqlx::query("UPDATE qq_reply_segments SET content=$3,format='plain',payload_hash=$4,payload_version=payload_version+1 WHERE turn_id=$1 AND msg_seq=$2")
                    .bind(t.id).bind(segment.msg_seq).bind(plain).bind(hash).execute(&mut *tx).await?;
                correction = true;
            }
        }
        let temporary = matches!(
            attempt.class,
            ErrorClass::RateLimited | ErrorClass::Transient
        );
        let message_attempts = segment.attempt_count + i32::from(attempt.message_attempted);
        if (temporary || correction) && message_attempts < MAX_ATTEMPTS && time < deadline {
            let local_delay = if correction {
                Duration::milliseconds(APP_INTERVAL_MS)
            } else {
                retry_delay(attempt_no)
            };
            let retry_at = match attempt.meta.retry_after {
                Some(platform_delay) => Duration::from_std(platform_delay)
                    .ok()
                    .and_then(|delay| time.checked_add_signed(delay.max(local_delay)))
                    .unwrap_or(deadline),
                None => time.checked_add_signed(local_delay).unwrap_or(deadline),
            };
            let at = retry_at.min(deadline);
            if attempt.class == ErrorClass::RateLimited {
                sqlx::query("UPDATE qq_app_delivery_limits SET blocked_until=GREATEST(blocked_until,$2) WHERE app_id=$1").bind(&t.app_id).bind(retry_at).execute(&mut *tx).await?;
            }
            sqlx::query("UPDATE qq_reply_segments SET retry_at=$3 WHERE turn_id=$1 AND msg_seq=$2")
                .bind(t.id)
                .bind(segment.msg_seq)
                .bind(at)
                .execute(&mut *tx)
                .await?;
            dispatch::reschedule(s, &mut tx, l, at).await?;
            tx.commit().await?;
            return Ok(());
        }
        if !unknown {
            sqlx::query(
                "UPDATE qq_reply_segments SET status='rejected' WHERE turn_id=$1 AND msg_seq=$2",
            )
            .bind(t.id)
            .bind(segment.msg_seq)
            .execute(&mut *tx)
            .await?;
        }
        stop(s, &mut tx, l, &current, attempt.reason()).await?;
        tx.commit().await?;
        return Ok(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retry_policy_is_bounded_and_timeout_stays_unknown() {
        assert!((2000..=2500).contains(&retry_delay(1).num_milliseconds()));
        assert!((5000..=5500).contains(&retry_delay(2).num_milliseconds()));
        let timed_out = timeout_attempt();
        assert_eq!(timed_out.outcome, SendOutcome::Unknown);
        assert!(timed_out.message_attempted);
    }
}
