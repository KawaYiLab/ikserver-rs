//! Durable delivery against real local HTTP failures and a failing database acknowledgement.
use super::*;
use std::collections::VecDeque;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct ReplyAi {
    text: &'static str,
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl ChatProvider for ReplyAi {
    async fn stream(&self, _: &ChatRequest) -> Result<EventStream, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(futures_util::stream::iter(vec![
            Ok(StreamEvent::Content(self.text.to_owned())),
            Ok(StreamEvent::Usage(Usage {
                prompt_tokens: 10,
                completion_tokens: 10,
            })),
        ])))
    }
}

#[derive(Clone, Copy)]
enum WireReply {
    Accept,
    /// The request body has arrived; hold its success response until the test releases it.
    BlockedAccept,
    Reject(i64),
    Limited(&'static str),
    /// The platform accepted the message, but the response disconnects after its 200 headers.
    TruncatedAcceptance,
}

#[derive(Default)]
struct WireState {
    replies: Mutex<VecDeque<WireReply>>,
    token_replies: Mutex<VecDeque<WireReply>>,
    token_attempts: AtomicUsize,
    attempts: Mutex<Vec<Value>>,
    accepted: Mutex<Vec<Value>>,
    request_received: tokio::sync::Notify,
    release_response: tokio::sync::Notify,
}

struct WireServer {
    state: Arc<WireState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for WireServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn wire_request(mut stream: tokio::net::TcpStream, state: Arc<WireState>) {
    let mut bytes = Vec::new();
    let body_start;
    let is_token;
    loop {
        if stream.read_buf(&mut bytes).await.unwrap() == 0 {
            return;
        }
        if let Some(end) = bytes.windows(4).position(|value| value == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            if bytes.len() >= end + 4 + length {
                body_start = end + 4;
                is_token = headers.starts_with("post /token ");
                break;
            }
        }
    }
    let body: Value = serde_json::from_slice(&bytes[body_start..]).unwrap();
    let reply = if is_token {
        state.token_attempts.fetch_add(1, Ordering::SeqCst);
        state
            .token_replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(WireReply::Accept)
    } else {
        state.attempts.lock().unwrap().push(body.clone());
        state
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(WireReply::Reject(40034006))
    };
    let retry_header = if let WireReply::Limited(delay) = reply {
        format!("Retry-After: {delay}\r\n")
    } else {
        String::new()
    };
    let (status, body) = match reply {
        WireReply::Accept if is_token => (
            "200 OK",
            json!({"access_token":"test-token","expires_in":7200}).to_string(),
        ),
        WireReply::Accept | WireReply::BlockedAccept => {
            if matches!(reply, WireReply::BlockedAccept) {
                state.request_received.notify_one();
                state.release_response.notified().await;
            }
            state.accepted.lock().unwrap().push(body);
            (
                "200 OK",
                json!({"id":format!("accepted-{}",state.accepted.lock().unwrap().len())})
                    .to_string(),
            )
        }
        WireReply::Reject(code) if is_token => ("200 OK", json!({"code":code}).to_string()),
        WireReply::Reject(code) => ("400 Bad Request", json!({"err_code":code}).to_string()),
        WireReply::Limited(_) => ("429 Too Many Requests", "{}".to_owned()),
        WireReply::TruncatedAcceptance => {
            state.accepted.lock().unwrap().push(body);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{\"id\":").await.unwrap();
            stream.shutdown().await.unwrap();
            return;
        }
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{retry_header}Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.unwrap();
    stream.shutdown().await.unwrap();
}

async fn fault_setup(
    db: PgPool,
    text: &'static str,
    replies: Vec<WireReply>,
) -> (
    QqState,
    Arc<ReplyAi>,
    WireServer,
    tokio::task::JoinHandle<()>,
) {
    let (mut s, _, _, original) = setup(db).await;
    let llm = Arc::new(ReplyAi {
        text,
        calls: AtomicUsize::new(0),
    });
    s.ai.providers = Providers::default().with("fake", llm.clone());
    s.ai.catalog = ik_ai::catalog::CatalogCache::new(s.db.clone(), s.ai.providers.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*s.config).clone();
    let base = format!("http://{}", listener.local_addr().unwrap());
    config.token_url = format!("{base}/token");
    config.api_base = Some(base);
    s.client = Arc::new(crate::client::ApiClient::new(&config).unwrap());
    s.config = Arc::new(config);
    let state = Arc::new(WireState {
        replies: Mutex::new(replies.into()),
        ..WireState::default()
    });
    let serving = state.clone();
    let task = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let state = serving.clone();
            let _request = tokio::spawn(wire_request(stream, state));
        }
    });
    (s, llm, WireServer { state, task }, original)
}

async fn due(s: &QqState) {
    sqlx::query("UPDATE qq_conversations SET wake_at=clock_timestamp() WHERE lease_until IS NULL")
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE qq_reply_segments SET retry_at=clock_timestamp() WHERE status IN ('pending','unknown')").execute(&s.db).await.unwrap();
    sqlx::query("UPDATE qq_app_delivery_limits SET next_send_at=clock_timestamp(),blocked_until=clock_timestamp()").execute(&s.db).await.unwrap();
}

async fn finish_due(s: &QqState, id: i64) {
    for _ in 0..8 {
        due(s).await;
        jobs::run_reply(s, id).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM qq_turns WHERE id=$1")
            .bind(id)
            .fetch_one(&s.db)
            .await
            .unwrap();
        if matches!(status.as_str(), "completed" | "failed" | "skipped") {
            return;
        }
    }
    panic!("delivery did not reach a terminal state within its bounded attempts");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deletion_during_an_in_flight_send_keeps_receipt_and_charge_without_restoring_private_text(
    db: PgPool,
) {
    use ik_auth::lifecycle::AccountLifecycle;

    let (s, llm, wire, original) = fault_setup(
        db,
        "收到请求。正在整理。",
        vec![WireReply::BlockedAccept, WireReply::Accept],
    )
    .await;
    site_user(&s, SITE_USER, "in_flight_owner").await;
    let id = enqueue(
        &s,
        "delete-in-flight",
        "delete-qq",
        None,
        "这段私下对话应当删除",
        Duration::zero(),
    )
    .await
    .unwrap();
    // Establish the binding before this queued turn freezes its authorization snapshot.
    sqlx::query("UPDATE qq_identities SET user_id=$1,bound_at=clock_timestamp(),binding_version=binding_version+1 WHERE openid='delete-qq'")
        .bind(SITE_USER).execute(&s.db).await.unwrap();
    let s = Arc::new(s);
    let sending_state = s.clone();
    let sending = tokio::spawn(async move { jobs::run_reply(&sending_state, id).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        wire.state.request_received.notified(),
    )
    .await
    .unwrap();
    assert_eq!(wire.state.attempts.lock().unwrap().len(), 1);
    assert_eq!(
        wire.state.attempts.lock().unwrap()[0]["markdown"]["content"],
        "收到请求。"
    );
    assert_eq!(count(&s, "SELECT count(*) FROM qq_reply_segments").await, 2);
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=200").await,
        20
    );

    // Complete the real deletion transaction while the HTTP success is still blocked.
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let now = Utc::now();
        let mut tx = s.db.begin().await.unwrap();
        ik_auth::interop::anonymize_deleted(&mut tx, SITE_USER, now)
            .await
            .unwrap();
        crate::identity::AccountDeletionHook
            .deleting(&mut tx, SITE_USER, now)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    })
    .await
    .unwrap();
    assert!(!sending.is_finished());
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE content<>''").await,
        0
    );
    wire.state.release_response.notify_one();
    sending.await.unwrap().unwrap();

    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_send_attempts WHERE result='accepted' AND http_status=200"
        )
        .await,
        1
    );
    assert_eq!(count(&s, "SELECT count(*) FROM qq_reply_segments WHERE msg_seq=1 AND status='accepted' AND qq_message_id='accepted-1' AND accepted_at IS NOT NULL").await, 1);
    assert_eq!(count(&s, "SELECT count(*) FROM qq_turns WHERE payer_user_id=200 AND segments_sent=1 AND prompt_tokens=10 AND completion_tokens=10 AND cost_finalized_at IS NOT NULL AND quota_finalized_at IS NOT NULL AND status='failed' AND reason='authorization_changed'").await, 1);
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=200").await,
        20
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_messages WHERE direction='out' OR content<>''"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE content<>''"
        )
        .await,
        0
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_turns WHERE output IS NOT NULL").await,
        0
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE msg_seq=2 AND status='cancelled'"
        )
        .await,
        1
    );
    due(&s).await;
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(wire.state.attempts.lock().unwrap().len(), 1);
    assert_eq!(wire.state.accepted.lock().unwrap().len(), 1);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn token_cooldown_does_not_consume_message_post_attempts(db: PgPool) {
    let (mut s, llm, wire, original) = fault_setup(db, "你好。", vec![WireReply::Accept]).await;
    wire.state
        .token_replies
        .lock()
        .unwrap()
        .push_back(WireReply::Reject(100001));
    let id = enqueue(
        &s,
        "token-cooldown",
        "alice",
        None,
        "你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    for _ in 0..5 {
        due(&s).await;
        jobs::run_reply(&s, id).await.unwrap();
        assert_eq!(
            count(&s, "SELECT attempt_count::bigint FROM qq_reply_segments").await,
            0
        );
    }
    assert_eq!(wire.state.token_attempts.load(Ordering::SeqCst), 1);
    assert!(wire.state.attempts.lock().unwrap().is_empty());
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_send_attempts WHERE result='not_sent' AND endpoint='token'"
        )
        .await,
        5
    );
    // A restarted process loses only the in-memory token cooldown, never the persisted reply.
    s.client = Arc::new(crate::client::ApiClient::new(&s.config).unwrap());
    finish_due(&s, id).await;
    assert_eq!(wire.state.attempts.lock().unwrap().len(), 1);
    assert_eq!(
        count(&s, "SELECT attempt_count::bigint FROM qq_reply_segments").await,
        1
    );
    let numbers: Vec<i32> = sqlx::query_scalar(
        "SELECT attempt_no FROM qq_send_attempts WHERE turn_id=$1 ORDER BY attempt_no",
    )
    .bind(id)
    .fetch_all(&s.db)
    .await
    .unwrap();
    assert_eq!(numbers, vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn short_retry_after_keeps_the_local_retry_delay(db: PgPool) {
    let (s, _, wire, original) = fault_setup(
        db,
        "你好。",
        vec![WireReply::Limited("0"), WireReply::Accept],
    )
    .await;
    let id = enqueue(
        &s,
        "short-retry-after",
        "alice",
        None,
        "你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    let delays: i64 = sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM (p.retry_at-a.finished_at))*1000)::bigint FROM qq_reply_segments p JOIN qq_send_attempts a ON a.turn_id=p.turn_id AND a.msg_seq=p.msg_seq WHERE p.turn_id=$1")
        .bind(id).fetch_one(&s.db).await.unwrap();
    assert!(
        delays >= 1900,
        "local two-second backoff was shortened to {delays}ms"
    );
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(wire.state.attempts.lock().unwrap().len(), 1);
    finish_due(&s, id).await;
    assert_eq!(wire.state.accepted.lock().unwrap().len(), 1);
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn huge_retry_after_waits_until_the_delivery_deadline_without_overflow(db: PgPool) {
    let (s, _, wire, original) = fault_setup(
        db,
        "你好。",
        vec![WireReply::Limited("18446744073709551615")],
    )
    .await;
    let id = enqueue(
        &s,
        "huge-retry-after",
        "alice",
        None,
        "你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    let at_deadline: bool = sqlx::query_scalar("SELECT p.retry_at=t.delivery_deadline_at FROM qq_reply_segments p JOIN qq_turns t ON t.id=p.turn_id WHERE t.id=$1")
        .bind(id).fetch_one(&s.db).await.unwrap();
    assert!(at_deadline);
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(wire.state.attempts.lock().unwrap().len(), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='accepted'"
        )
        .await,
        0
    );
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn url_correction_is_persisted_before_retry_and_keeps_sequence(db: PgPool) {
    let (s, llm, wire, original) = fault_setup(
        db,
        "攻略 https://interk.net/post/123",
        vec![WireReply::Reject(304003), WireReply::Accept],
    )
    .await;
    sqlx::query("UPDATE qq_settings SET link_mode='domain'")
        .execute(&s.db)
        .await
        .unwrap();
    let id = enqueue(
        &s,
        "persist-url",
        "alice",
        None,
        "给我攻略",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    let (content,version,corrected,status):(String,i32,bool,String)=sqlx::query_as("SELECT content,payload_version,url_corrected,status FROM qq_reply_segments WHERE turn_id=$1 AND msg_seq=1")
        .bind(id).fetch_one(&s.db).await.unwrap();
    assert!(!content.contains("https://"));
    assert_eq!(version, 2);
    assert!(corrected);
    assert_eq!(status, "pending");
    assert_eq!(wire.state.attempts.lock().unwrap().len(), 1);
    finish_due(&s, id).await;
    let attempts = wire.state.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["msg_id"], attempts[1]["msg_id"]);
    assert_eq!(attempts[0]["msg_seq"], attempts[1]["msg_seq"]);
    assert_eq!(attempts[0]["msg_type"], 2);
    assert_eq!(attempts[1]["msg_type"], 2);
    assert_ne!(
        attempts[0]["markdown"]["content"],
        attempts[1]["markdown"]["content"]
    );
    assert_eq!(attempts[1]["markdown"]["content"], content);
    let versions: Vec<i32> = sqlx::query_scalar(
        "SELECT payload_version FROM qq_send_attempts WHERE turn_id=$1 ORDER BY attempt_no",
    )
    .bind(id)
    .fetch_all(&s.db)
    .await
    .unwrap();
    assert_eq!(versions, vec![1, 2]);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deduplication_is_unknown_and_does_not_send_the_next_segment(db: PgPool) {
    let (s, llm, wire, original) = fault_setup(
        db,
        "收到请求。正在整理。",
        vec![WireReply::Reject(40054005), WireReply::Accept],
    )
    .await;
    let id = enqueue(&s, "dedup-first", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    finish_due(&s, id).await;
    assert_eq!(wire.state.attempts.lock().unwrap().len(), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='unknown'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='cancelled'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        0
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_turns WHERE reason='delivery_unknown'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        0
    );
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_later_rejection_keeps_the_first_accepted_segment_and_guest_turn(db: PgPool) {
    let (s, llm, wire, original) = fault_setup(
        db,
        "收到请求。正在整理。",
        vec![WireReply::Accept, WireReply::Reject(40034006)],
    )
    .await;
    let id = enqueue(&s, "partial", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    finish_due(&s, id).await;
    assert_eq!(wire.state.attempts.lock().unwrap().len(), 2);
    assert_eq!(wire.state.accepted.lock().unwrap().len(), 1);
    assert_eq!(
        count(&s, "SELECT segments_sent::bigint FROM qq_turns").await,
        1
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='accepted'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        1
    );
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        1
    );
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(wire.state.attempts.lock().unwrap().len(), 2);
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn accepted_upstream_then_disconnect_never_regenerates_or_fabricates_a_receipt(db: PgPool) {
    let (s, llm, wire, original) = fault_setup(
        db,
        "收到请求。正在整理。",
        vec![WireReply::TruncatedAcceptance, WireReply::Reject(40054005)],
    )
    .await;
    let id = enqueue(
        &s,
        "wire-disconnect",
        "alice",
        None,
        "你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_send_attempts WHERE result='unknown' AND http_status=200"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='accepted'"
        )
        .await,
        0
    );
    finish_due(&s, id).await;
    let attempts = wire.state.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 2);
    assert_eq!(
        attempts[0], attempts[1],
        "an uncertain send may only retry the persisted identical payload"
    );
    assert_eq!(wire.state.accepted.lock().unwrap().len(), 1);
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        0
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_turns WHERE reason='delivery_unknown'"
        )
        .await,
        1
    );
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_database_confirmation_recovers_without_regeneration_or_duplicate_confirmation(
    db: PgPool,
) {
    let (s, llm, wire, original) = fault_setup(
        db,
        "收到请求。正在整理。",
        vec![WireReply::Accept, WireReply::Reject(40054005)],
    )
    .await;
    sqlx::query("CREATE FUNCTION fail_qq_confirmation() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected confirmation failure'; END $$")
        .execute(&s.db).await.unwrap();
    sqlx::query("CREATE TRIGGER qq_confirmation_failure BEFORE UPDATE ON qq_send_attempts FOR EACH ROW WHEN (NEW.result='accepted') EXECUTE FUNCTION fail_qq_confirmation()")
        .execute(&s.db).await.unwrap();
    let id = enqueue(
        &s,
        "db-confirmation",
        "alice",
        None,
        "你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    assert!(jobs::run_reply(&s, id).await.is_err());
    assert_eq!(wire.state.accepted.lock().unwrap().len(), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='sending'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        0
    );
    sqlx::query("DROP TRIGGER qq_confirmation_failure ON qq_send_attempts")
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE qq_conversations SET lease_until=clock_timestamp()-interval '1 second',wake_at=clock_timestamp()").execute(&s.db).await.unwrap();
    finish_due(&s, id).await;
    let attempts = wire.state.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0], attempts[1]);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='accepted'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_turns WHERE reason='delivery_unknown'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn markdown_rejections_preserve_sequence_cost_and_only_record_delivered_text(db: PgPool) {
    let codes = [40034011, 40034124, 304036, 40034127];
    let replies = codes
        .into_iter()
        .flat_map(|code| [WireReply::Reject(code), WireReply::Accept])
        .collect();
    let (s, llm, wire, original) =
        fault_setup(db, "**攻略** [网页](https://example.com)", replies).await;
    for index in 0..4 {
        let id = enqueue(
            &s,
            &format!("markdown-{index}"),
            "alice",
            if index % 2 == 0 { Some("group") } else { None },
            "<@123> 给我攻略",
            Duration::zero(),
        )
        .await
        .unwrap();
        finish_due(&s, id).await;
        let (format, content, version, status): (String, String, i32, String) = sqlx::query_as(
            "SELECT format,content,payload_version,status FROM qq_reply_segments WHERE turn_id=$1",
        )
        .bind(id)
        .fetch_one(&s.db)
        .await
        .unwrap();
        assert_eq!(
            (format.as_str(), version, status.as_str()),
            ("plain", 2, "accepted")
        );
        assert_eq!(content, "攻略 网页 https://example.com");
        let formats: Vec<String> = sqlx::query_scalar(
            "SELECT format FROM qq_send_attempts WHERE turn_id=$1 ORDER BY attempt_no",
        )
        .bind(id)
        .fetch_all(&s.db)
        .await
        .unwrap();
        assert_eq!(formats, ["markdown", "plain"]);
        let history: Vec<String> = sqlx::query_scalar("SELECT m.content FROM qq_messages m JOIN qq_turns t ON m.reply_to_id=t.message_id WHERE t.id=$1 AND m.direction='out'").bind(id).fetch_all(&s.db).await.unwrap();
        assert_eq!(history, [content]);
    }
    let attempts = wire.state.attempts.lock().unwrap().clone();
    for pair in attempts.chunks_exact(2) {
        assert_eq!(pair[0]["msg_type"], 2);
        assert_eq!(pair[1]["msg_type"], 0);
        assert_eq!(pair[0]["msg_seq"], pair[1]["msg_seq"]);
        assert_eq!(pair[0]["msg_id"], pair[1]["msg_id"]);
    }
    assert_eq!(attempts.len(), 8);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        80
    );
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        4
    );
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn format_and_url_corrections_share_three_attempt_budget(db: PgPool) {
    let (s, llm, wire, original) = fault_setup(
        db,
        "**攻略** [网页](https://example.com)",
        vec![
            WireReply::Reject(40034011),
            WireReply::Reject(304003),
            WireReply::Accept,
            WireReply::Reject(304003),
            WireReply::Reject(304036),
            WireReply::Accept,
            WireReply::Reject(40034124),
            WireReply::Reject(40034124),
        ],
    )
    .await;
    for index in 0..3 {
        let id = enqueue(
            &s,
            &format!("correction-{index}"),
            "alice",
            None,
            "攻略",
            Duration::zero(),
        )
        .await
        .unwrap();
        finish_due(&s, id).await;
    }
    let attempts = wire.state.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 8);
    assert_eq!(
        attempts
            .iter()
            .map(|v| v["msg_type"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [2, 0, 0, 2, 2, 0, 2, 0]
    );
    assert_eq!(attempts[2]["content"], "攻略 网页");
    assert_eq!(attempts[4]["markdown"]["content"], "**攻略** 网页");
    assert_eq!(attempts[5]["content"], "攻略 网页");
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        2
    );
    assert_eq!(llm.calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        60
    );
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn uncertain_markdown_payload_cannot_change_after_format_or_url_rejection(db: PgPool) {
    let (s, _, wire, original) = fault_setup(
        db,
        "**攻略** [网页](https://example.com)",
        vec![
            WireReply::TruncatedAcceptance,
            WireReply::Reject(40034011),
            WireReply::TruncatedAcceptance,
            WireReply::Reject(304003),
        ],
    )
    .await;
    for index in 0..2 {
        let id = enqueue(
            &s,
            &format!("uncertain-markdown-{index}"),
            "alice",
            None,
            "攻略",
            Duration::zero(),
        )
        .await
        .unwrap();
        finish_due(&s, id).await;
        let (format, version, reason): (String, i32, String) = sqlx::query_as("SELECT p.format,p.payload_version,t.reason FROM qq_reply_segments p JOIN qq_turns t ON t.id=p.turn_id WHERE t.id=$1").bind(id).fetch_one(&s.db).await.unwrap();
        assert_eq!(
            (format.as_str(), version, reason.as_str()),
            ("markdown", 1, "delivery_unknown")
        );
    }
    let attempts = wire.state.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 4);
    assert_eq!(attempts[0], attempts[1]);
    assert_eq!(attempts[2], attempts[3]);
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        0
    );
    original.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn retry_uses_saved_format_and_legacy_plain_segments_remain_plain(db: PgPool) {
    let (s, llm, wire, original) = fault_setup(
        db,
        "**攻略** [网页](https://example.com)",
        vec![
            WireReply::Limited("1"),
            WireReply::Accept,
            WireReply::Accept,
        ],
    )
    .await;
    let id = enqueue(
        &s,
        "saved-markdown",
        "alice",
        None,
        "攻略",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    sqlx::query("UPDATE qq_settings SET reply_format='plain',link_mode='strip'")
        .execute(&s.db)
        .await
        .unwrap();
    s.invalidate().await;
    finish_due(&s, id).await;
    let attempts = wire.state.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0], attempts[1]);
    assert_eq!(attempts[1]["msg_type"], 2);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);

    // A pre-upgrade row has no format field in the INSERT and a body-only legacy hash.
    let legacy = enqueue(
        &s,
        "legacy-plain",
        "alice",
        None,
        "旧消息",
        Duration::zero(),
    )
    .await
    .unwrap();
    let mut tx = s.db.begin().await.unwrap();
    let turn = repo::turn(&mut tx, legacy).await.unwrap().unwrap();
    crate::identity::freeze(&mut tx, turn.message_id)
        .await
        .unwrap();
    sqlx::query("UPDATE qq_turns SET status='processing',phase='delivering',output=ARRAY['**原样纯文本**'],delivery_deadline_at=clock_timestamp()+interval '60 seconds',cost_finalized_at=clock_timestamp() WHERE id=$1").bind(legacy).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO qq_reply_segments(turn_id,msg_seq,content,payload_hash) VALUES($1,1,'**原样纯文本**',sha256(convert_to('**原样纯文本**','UTF8')))").bind(legacy).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    sqlx::query("UPDATE qq_settings SET reply_format='markdown',link_mode='allow'")
        .execute(&s.db)
        .await
        .unwrap();
    s.invalidate().await;
    finish_due(&s, legacy).await;
    let attempts = wire.state.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 3);
    assert_eq!(attempts[2]["content"], "**原样纯文本**");
    assert_eq!(attempts[2]["msg_type"], 0);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    original.abort();
}
