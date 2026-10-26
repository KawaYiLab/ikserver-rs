//! Failure injection for the durable outbox; uses the real ledger and HTTP fixture.
use super::*;

#[derive(Default)]
struct GatedQq {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    calls: AtomicUsize,
}

async fn gated_client(s: &mut QqState) -> (Arc<GatedQq>, tokio::task::JoinHandle<()>) {
    let gate = Arc::new(GatedQq::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*s.config).clone();
    config.api_base = Some(format!("http://{}", listener.local_addr().unwrap()));
    let router = Router::new()
        .route(
            "/v2/users/{openid}/messages",
            post(|State(gate): State<Arc<GatedQq>>| async move {
                if gate.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    gate.entered.notify_one();
                    gate.release.notified().await;
                    (
                        http::StatusCode::OK,
                        Json(json!({"id":"accepted-upstream"})),
                    )
                } else {
                    (
                        http::StatusCode::BAD_REQUEST,
                        Json(json!({"err_code":40054005})),
                    )
                }
            }),
        )
        .with_state(gate.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    s.client = Arc::new(crate::client::ApiClient::new(&config).unwrap());
    (gate, server)
}

async fn due(s: &QqState) {
    // Advance only scheduler/rate timestamps, never lease ownership or accounting.
    sqlx::query("UPDATE qq_conversations SET wake_at=clock_timestamp() WHERE lease_until IS NULL")
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE qq_reply_segments SET retry_at=clock_timestamp() WHERE status IN ('pending','unknown')").execute(&s.db).await.unwrap();
    sqlx::query("UPDATE qq_app_delivery_limits SET next_send_at=clock_timestamp(),blocked_until=clock_timestamp()").execute(&s.db).await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn queued_followup_sees_the_previous_accepted_reply_even_when_it_arrives_later(db: PgPool) {
    let (s, llm, _, server) = setup(db).await;
    for (prefix, group, first_text, second_text) in [
        (
            "c2c",
            None,
            "第一问：周末去哪",
            "第二问：刚才那个地方怎么去",
        ),
        (
            "group",
            Some("history-group"),
            "<@123> 第一问：周末去哪",
            "<@123> 第二问：刚才那个地方怎么去",
        ),
    ] {
        let first = enqueue(
            &s,
            &format!("{prefix}-first"),
            prefix,
            group,
            first_text,
            Duration::zero(),
        )
        .await
        .unwrap();
        let second = enqueue(
            &s,
            &format!("{prefix}-second"),
            prefix,
            group,
            second_text,
            Duration::zero(),
        )
        .await
        .unwrap();
        // Intake timestamps originate at the host, while outgoing timestamps
        // originate in PostgreSQL. Pin this ordering fixture to the DB clock.
        sqlx::query("UPDATE qq_messages m SET created_at=clock_timestamp()-CASE WHEN t.id=$1 THEN interval '2 seconds' ELSE interval '1 second' END FROM qq_turns t WHERE t.message_id=m.id AND t.id=ANY($2)")
            .bind(first).bind(&[first, second]).execute(&s.db).await.unwrap();
        sqlx::query("UPDATE qq_turns t SET platform_deadline_at=m.created_at+CASE WHEN t.group_id IS NULL THEN interval '60 minutes' ELSE interval '5 minutes' END FROM qq_messages m WHERE m.id=t.message_id AND t.id=ANY($1)")
            .bind(&[first, second]).execute(&s.db).await.unwrap();
        // Both inputs exist before either reply. Waking the later job still
        // drives the oldest conversation sequence before preparing its successor.
        drive_reply(&s, second).await.unwrap();
        let output_after_input: bool = sqlx::query_scalar("SELECT output.created_at>input.created_at FROM qq_messages output JOIN qq_turns first ON first.message_id=output.reply_to_id JOIN qq_turns second ON second.id=$2 JOIN qq_messages input ON input.id=second.message_id WHERE first.id=$1 AND output.direction='out'")
            .bind(first).bind(second).fetch_one(&s.db).await.unwrap();
        assert!(output_after_input);
        let prompts = llm.prompts.lock().unwrap();
        let pair = &prompts[prompts.len() - 2..];
        assert!(
            !pair[0].contains("第二问"),
            "后来的输入不能泄入第一轮提示词"
        );
        let first_position = pair[1].find("第一问").unwrap();
        let output_position = pair[1].find("你好，今天去六分街逛逛吧。").unwrap();
        let second_position = pair[1].find("第二问").unwrap();
        assert!(first_position < output_position && output_position < second_position);
    }
    assert_eq!(llm.calls.load(Ordering::SeqCst), 4);
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn intake_during_pause_attaches_older_legacy_turns_before_the_new_turn(db: PgPool) {
    let (s, llm, _, server) = setup(db).await;
    sqlx::query("UPDATE qq_settings SET dispatch_paused=TRUE")
        .execute(&s.db)
        .await
        .unwrap();
    let first = enqueue(
        &s,
        "legacy-queued",
        "alice",
        None,
        "最早的问题",
        Duration::zero(),
    )
    .await
    .unwrap();
    let interrupted = enqueue(
        &s,
        "legacy-processing",
        "alice",
        None,
        "旧执行器已经开始",
        Duration::zero(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE qq_turns SET conversation_id=NULL,conversation_seq=NULL,protocol_version=0 WHERE id=ANY($1)")
        .bind(&[first, interrupted]).execute(&s.db).await.unwrap();
    sqlx::query("UPDATE qq_turns SET status='processing' WHERE id=$1")
        .bind(interrupted)
        .execute(&s.db)
        .await
        .unwrap();
    let fresh = enqueue(
        &s,
        "fresh-during-pause",
        "alice",
        None,
        "新的问题",
        Duration::zero(),
    )
    .await
    .unwrap();
    let ordered: Vec<(i64, i16)> =
        sqlx::query_as("SELECT id,protocol_version FROM qq_turns ORDER BY conversation_seq")
            .fetch_all(&s.db)
            .await
            .unwrap();
    assert_eq!(ordered, vec![(first, 0), (interrupted, 0), (fresh, 2)]);
    jobs::run_reply(&s, fresh).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    sqlx::query("UPDATE qq_settings SET dispatch_paused=FALSE")
        .execute(&s.db)
        .await
        .unwrap();
    drive_reply(&s, fresh).await.unwrap();
    assert_eq!(
        llm.calls.load(Ordering::SeqCst),
        2,
        "旧 processing 不能重新生成"
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_turns WHERE reason='legacy_delivery_unknown'"
        )
        .await,
        1
    );
    let prompts = llm.prompts.lock().unwrap();
    assert!(prompts[0].contains("最早的问题"));
    assert!(!prompts[0].contains("新的问题"));
    assert!(prompts[1].contains("新的问题"));
    server.abort();
}

struct CapacityGate {
    calls: AtomicUsize,
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}
impl CapacityGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        })
    }
    async fn wait_for_entries(&self, count: u32) {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.entered.acquire_many(count),
        )
        .await
        .unwrap()
        .unwrap()
        .forget();
    }
    async fn hold(&self) -> usize {
        let number = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        number
    }
}
struct HeldAi {
    gate: Arc<CapacityGate>,
    inner: Arc<FakeAi>,
}
#[async_trait::async_trait]
impl ChatProvider for HeldAi {
    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, LlmError> {
        self.gate.hold().await;
        self.inner.stream(request).await
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn only_two_generations_are_admitted_and_the_third_releases_its_worker(db: PgPool) {
    let (mut s, llm, _, server) = setup(db).await;
    let gate = CapacityGate::new();
    s.ai.providers = Providers::default().with(
        "fake",
        Arc::new(HeldAi {
            gate: gate.clone(),
            inner: llm.clone(),
        }),
    );
    s.ai.catalog = ik_ai::catalog::CatalogCache::new(s.db.clone(), s.ai.providers.clone());
    let mut turns = Vec::new();
    for i in 0..3 {
        turns.push(
            enqueue(
                &s,
                &format!("generation-{i}"),
                &format!("user-{i}"),
                None,
                "你好",
                Duration::zero(),
            )
            .await
            .unwrap(),
        );
    }
    let mut workers = Vec::new();
    for &id in &turns[..2] {
        let state = s.clone();
        workers.push(tokio::spawn(
            async move { jobs::run_reply(&state, id).await },
        ));
    }
    gate.wait_for_entries(2).await;
    jobs::run_reply(&s, turns[2]).await.unwrap();
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    let deferred: bool = sqlx::query_scalar("SELECT t.phase='queued' AND t.attempts=0 AND c.lease_until IS NULL AND c.wake_at IS NOT NULL FROM qq_turns t JOIN qq_conversations c ON c.id=t.conversation_id WHERE t.id=$1")
        .bind(turns[2]).fetch_one(&s.db).await.unwrap();
    assert!(deferred);
    gate.release.add_permits(3);
    for worker in workers {
        worker.await.unwrap().unwrap();
    }
    for turn in turns {
        drive_reply(&s, turn).await.unwrap();
    }
    assert_eq!(gate.calls.load(Ordering::SeqCst), 3);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 3);
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn app_rate_and_four_inflight_sends_release_waiting_workers(db: PgPool) {
    let (mut s, llm, _, server) = setup(db).await;
    let gate = CapacityGate::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = (*s.config).clone();
    config.api_base = Some(format!("http://{}", listener.local_addr().unwrap()));
    let router = Router::new()
        .route(
            "/v2/users/{openid}/messages",
            post(|State(gate): State<Arc<CapacityGate>>| async move {
                let number = gate.hold().await;
                Json(json!({"id":format!("accepted-{number}")}))
            }),
        )
        .with_state(gate.clone());
    let gated_server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    s.client = Arc::new(crate::client::ApiClient::new(&config).unwrap());
    let mut turns = Vec::new();
    for i in 0..5 {
        turns.push(
            enqueue(
                &s,
                &format!("send-{i}"),
                &format!("sender-{i}"),
                None,
                "你好",
                Duration::zero(),
            )
            .await
            .unwrap(),
        );
    }
    sqlx::query("INSERT INTO qq_app_delivery_limits(app_id,next_send_at) VALUES('123',clock_timestamp()+interval '1 hour')")
        .execute(&s.db).await.unwrap();
    // Complete preparation before opening the HTTP gates: otherwise serial
    // generation can outlast the first request's real 10s transport timeout.
    for &turn in &turns {
        jobs::run_reply(&s, turn).await.unwrap();
    }
    assert_eq!(llm.calls.load(Ordering::SeqCst), 5);
    assert_eq!(
        gate.calls.load(Ordering::SeqCst),
        0,
        "AppID rate gate must defer before HTTP"
    );
    let mut workers = Vec::new();
    for &id in &turns[..4] {
        // Advance only rate timestamps so this check isolates the in-flight cap.
        due(&s).await;
        let state = s.clone();
        workers.push(tokio::spawn(
            async move { jobs::run_reply(&state, id).await },
        ));
        gate.wait_for_entries(1).await;
    }
    due(&s).await;
    jobs::run_reply(&s, turns[4]).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 5);
    assert_eq!(gate.calls.load(Ordering::SeqCst), 4);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments p JOIN qq_turns t ON t.id=p.turn_id JOIN qq_conversations c ON c.id=t.conversation_id WHERE p.status='sending' AND c.lease_until>clock_timestamp()"
        )
        .await,
        4
    );
    let waiting: bool = sqlx::query_scalar("SELECT p.status='pending' AND p.attempt_count=0 AND c.lease_until IS NULL FROM qq_reply_segments p JOIN qq_turns t ON t.id=p.turn_id JOIN qq_conversations c ON c.id=t.conversation_id WHERE t.id=$1")
        .bind(turns[4]).fetch_one(&s.db).await.unwrap();
    assert!(waiting);
    gate.release.add_permits(5);
    for worker in workers {
        worker.await.unwrap().unwrap();
    }
    due(&s).await;
    drive_reply(&s, turns[4]).await.unwrap();
    assert_eq!(gate.calls.load(Ordering::SeqCst), 5);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 5);
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_turns WHERE status='completed'").await,
        5
    );
    gated_server.abort();
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn retry_keeps_payload_sequence_and_model_cost(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    qq.transient.store(true, Ordering::SeqCst);
    let id = enqueue(&s, "durable-retry", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_send_attempts WHERE result='rejected'"
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='pending'"
        )
        .await,
        1
    );
    due(&s).await;
    jobs::run_reply(&s, id).await.unwrap();
    let attempts = qq.attempts.lock().unwrap().clone();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0], attempts[1]);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(count(&s,"SELECT count(*) FROM qq_turns WHERE status='completed' AND quota_finalized_at IS NOT NULL").await,1);
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        1
    );
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unresolved_delivery_releases_queue_and_refunds_once(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    qq.transient.store(true, Ordering::SeqCst);
    let first = enqueue(&s, "unknown-first", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    let second = enqueue(&s, "queued-second", "alice", None, "再聊", Duration::zero())
        .await
        .unwrap();
    jobs::run_reply(&s, first).await.unwrap();
    jobs::run_reply(&s, second).await.unwrap();
    assert_eq!(
        llm.calls.load(Ordering::SeqCst),
        1,
        "后来的消息不能越过前轮退避"
    );
    // Equivalent to three unconfirmed attempts followed by a restarted worker.
    sqlx::query("UPDATE qq_reply_segments SET status='unknown',attempt_count=3 WHERE turn_id=$1")
        .bind(first)
        .execute(&s.db)
        .await
        .unwrap();
    due(&s).await;
    jobs::run_reply(&s, first).await.unwrap();
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
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    jobs::run_reply(&s, second).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        1
    );
    jobs::run_reply(&s, first).await.unwrap();
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        1,
        "旧wake不能再退款"
    );
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn pause_keeps_ingestion_and_resume_does_not_reuse_a_live_lease(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    sqlx::query("UPDATE qq_settings SET dispatch_paused=TRUE")
        .execute(&s.db)
        .await
        .unwrap();
    let id = enqueue(&s, "paused", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_turns WHERE status='queued'").await,
        1
    );
    sqlx::query("UPDATE qq_settings SET dispatch_paused=FALSE")
        .execute(&s.db)
        .await
        .unwrap();
    sqlx::query("UPDATE qq_conversations SET active_turn_id=$1,lease_epoch=42,lease_until=clock_timestamp()+interval '30 seconds'").bind(id).execute(&s.db).await.unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(
        llm.calls.load(Ordering::SeqCst),
        0,
        "有效lease不可被第二个worker抢走"
    );
    sqlx::query("UPDATE qq_conversations SET lease_until=clock_timestamp()-interval '1 second'")
        .execute(&s.db)
        .await
        .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(qq.sent.lock().unwrap().len(), 1);
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn heartbeat_keeps_polling_the_transaction_that_holds_its_renewal_lock(db: PgPool) {
    let (s, _, _, server) = setup(db).await;
    let turn = enqueue(
        &s,
        "heartbeat-lock",
        "alice",
        None,
        "你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE qq_conversations SET active_turn_id=$1,lease_epoch=1,lease_until=clock_timestamp()+interval '30 seconds'")
        .bind(turn).execute(&s.db).await.unwrap();
    let work = async {
        let mut tx = s.db.begin().await?;
        sqlx::query("SELECT c.id FROM qq_conversations c JOIN qq_turns t ON t.conversation_id=c.id WHERE t.id=$1 FOR UPDATE OF c")
            .bind(turn).fetch_one(&mut *tx).await?;
        // The real renewal interval is 10s. Its UPDATE must be allowed to wait
        // while this future keeps being polled and commits after the timer.
        tokio::time::sleep(std::time::Duration::from_secs(11)).await;
        tx.commit().await?;
        Ok(())
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(14),
        jobs::test_heartbeat(&s, turn, work),
    )
    .await
    .expect("renewal must not suspend the transaction holding its row lock")
    .unwrap();
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn generation_recovery_never_calls_the_model_twice(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    qq.transient.store(true, Ordering::SeqCst);
    let id = enqueue(
        &s,
        "generation-recovery",
        "alice",
        None,
        "你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    sqlx::query("UPDATE qq_turns SET phase='generating',output=NULL WHERE id=$1")
        .bind(id)
        .execute(&s.db)
        .await
        .unwrap();
    due(&s).await;
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_turns WHERE reason='generation_interrupted'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        0
    );
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn an_expired_delivery_budget_does_not_send_or_extend_on_retry(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    qq.transient.store(true, Ordering::SeqCst);
    let id = enqueue(
        &s,
        "expired-delivery",
        "alice",
        None,
        "你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    sqlx::query("UPDATE qq_turns SET delivery_deadline_at=clock_timestamp()-interval '1 second' WHERE id=$1").bind(id).execute(&s.db).await.unwrap();
    due(&s).await;
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(qq.attempts.lock().unwrap().len(), 1);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_turns WHERE status='failed' AND quota_finalized_at IS NOT NULL"
        )
        .await,
        1
    );
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        0
    );
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn fully_confirmed_delivery_recovers_after_deadline_without_resending(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    let id = enqueue(
        &s,
        "confirmed-before-recovery",
        "alice",
        None,
        "你好",
        Duration::zero(),
    )
    .await
    .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(qq.sent.lock().unwrap().len(), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='accepted'"
        )
        .await,
        1
    );

    // Keep the real accepted receipt and model charge, but restore the state
    // just before the final turn transaction, as if the worker died there.
    sqlx::query("UPDATE qq_turns SET status='processing',phase='delivering',reason=NULL,finished_at=NULL,quota_finalized_at=NULL,delivery_deadline_at=clock_timestamp()-interval '1 second' WHERE id=$1")
        .bind(id).execute(&s.db).await.unwrap();
    sqlx::query("UPDATE qq_conversations SET active_turn_id=$1,lease_until=clock_timestamp()-interval '1 second',wake_at=NULL WHERE id=(SELECT conversation_id FROM qq_turns WHERE id=$1)")
        .bind(id).execute(&s.db).await.unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_turns WHERE status='completed' AND reason='delivered' AND segments_sent=1 AND quota_finalized_at IS NOT NULL").await,
        1,
        "confirmed delivery must finish successfully even when recovery is past its send deadline"
    );
    // A repeated wake must retain the same confirmation, charge and guest turn.
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(qq.attempts.lock().unwrap().len(), 1);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        1
    );
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    assert_eq!(
        count(&s, "SELECT sum(turns)::bigint FROM qq_guest_usage").await,
        1
    );
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn stale_worker_receipt_cannot_overwrite_recovery_or_fabricate_confirmation(db: PgPool) {
    let (mut s, llm, _, server) = setup(db).await;
    let (gate, gated_server) = gated_client(&mut s).await;
    let id = enqueue(&s, "lost-receipt", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    let worker_state = s.clone();
    let worker = tokio::spawn(async move { jobs::run_reply(&worker_state, id).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), gate.entered.notified())
        .await
        .unwrap();
    // Simulate a recovery owner taking the expired lease while the old response is delayed.
    sqlx::query("UPDATE qq_conversations SET lease_epoch=lease_epoch+1,lease_until=clock_timestamp()-interval '1 second'").execute(&s.db).await.unwrap();
    gate.release.notify_one();
    worker.await.unwrap().unwrap();
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_messages WHERE direction='out'").await,
        0
    );
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='sending'"
        )
        .await,
        1
    );
    due(&s).await;
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(
            &s,
            "SELECT count(*) FROM qq_reply_segments WHERE status='accepted'"
        )
        .await,
        0,
        "去重不是成功回执"
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
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    gated_server.abort();
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn a_confirmed_prefix_keeps_exactly_one_guest_turn(db: PgPool) {
    let (s, llm, qq, server) = setup(db).await;
    qq.transient.store(true, Ordering::SeqCst);
    let id = enqueue(&s, "partial", "alice", None, "你好", Duration::zero())
        .await
        .unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    sqlx::query("INSERT INTO qq_reply_segments(turn_id,msg_seq,content,payload_hash,retry_at) VALUES($1,2,'第二段',decode(repeat('00',32),'hex'),clock_timestamp()+interval '1 hour')").bind(id).execute(&s.db).await.unwrap();
    sqlx::query("UPDATE qq_turns SET output=array_append(output,'第二段') WHERE id=$1")
        .bind(id)
        .execute(&s.db)
        .await
        .unwrap();
    due(&s).await;
    sqlx::query("UPDATE qq_reply_segments SET retry_at=clock_timestamp()+interval '1 hour' WHERE turn_id=$1 AND msg_seq=2").bind(id).execute(&s.db).await.unwrap();
    jobs::run_reply(&s, id).await.unwrap();
    assert_eq!(
        count(&s, "SELECT sum(segments_sent)::bigint FROM qq_turns").await,
        1
    );
    sqlx::query("UPDATE qq_reply_segments SET status='unknown',attempt_count=3 WHERE turn_id=$1 AND msg_seq=2").bind(id).execute(&s.db).await.unwrap();
    due(&s).await;
    jobs::run_reply(&s, id).await.unwrap();
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
        1
    );
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=0").await,
        20
    );
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    server.abort();
}
