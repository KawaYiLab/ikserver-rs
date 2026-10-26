//! 身份切换、完整 turn 提炼与注销回归。假模型与假 QQ 均复用父模块。
use super::*;
use crate::{
    authorization::{self, QqAuthorization},
    identity, memory_jobs,
};
use ik_auth::lifecycle::AccountLifecycle;

struct ControlledAi {
    fake: Arc<FakeAi>,
    pause_next: AtomicBool,
    started: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

impl ControlledAi {
    fn arm(&self) {
        self.pause_next.store(true, Ordering::SeqCst);
    }

    async fn wait_started(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(30), self.started.notified())
            .await
            .expect("模型应在超时前进入受控等待");
    }
}

#[async_trait::async_trait]
impl ChatProvider for ControlledAi {
    async fn stream(&self, request: &ChatRequest) -> Result<EventStream, LlmError> {
        let response = self.fake.stream(request).await?;
        if self.pause_next.swap(false, Ordering::SeqCst) {
            self.started.notify_one();
            self.release.notified().await;
        }
        Ok(response)
    }
}

async fn unbind_while_model_waits(s: &QqState, identity: i64) {
    let mut conn = s.db.acquire().await.unwrap();
    let released = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::me::release(&mut conn, identity, Some(SITE_USER)),
    )
    .await
    .expect("外部模型等待期间不能持有阻塞解绑的数据库锁")
    .unwrap();
    assert!(released);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn unbinding_during_generation_and_extraction_discards_in_flight_results(db: PgPool) {
    let (mut s, llm, qq, server) = setup(db).await;
    let controlled = Arc::new(ControlledAi {
        fake: llm.clone(),
        pause_next: AtomicBool::new(false),
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    s.ai.providers = Providers::default().with("fake", controlled.clone());
    s.ai.catalog = ik_ai::catalog::CatalogCache::new(s.db.clone(), s.ai.providers.clone());
    site_user(&s, SITE_USER, "in_flight_owner").await;
    bind_code(&s, SITE_USER, "ABCD2345").await;
    say(&s, "in-flight-bind", "in-flight-qq", None, "/绑定 ABCD2345").await;
    let identity: i64 =
        sqlx::query_scalar("SELECT id FROM qq_identities WHERE openid='in-flight-qq'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    let attempts_before = qq.attempts.lock().unwrap().len();
    let turn = enqueue(
        &s,
        "in-flight-generation",
        "in-flight-qq",
        None,
        "这次生成中途会解绑",
        Duration::zero(),
    )
    .await
    .unwrap();
    controlled.arm();
    let generating = {
        let s = s.clone();
        tokio::spawn(async move { drive_reply(&s, turn).await })
    };
    controlled.wait_started().await;
    unbind_while_model_waits(&s, identity).await;
    controlled.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(30), generating)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (status, settled, charged): (String, bool, i64) = sqlx::query_as(
        "SELECT status,cost_finalized_at IS NOT NULL,prompt_tokens+completion_tokens FROM qq_turns WHERE id=$1",
    ).bind(turn).fetch_one(&s.db).await.unwrap();
    assert_eq!(status, "failed");
    assert!(settled && charged > 0, "已开始的模型调用必须结算成本");
    assert_eq!(
        count(&s, "SELECT used FROM ai_quota_counters WHERE user_id=200").await,
        charged
    );
    assert_eq!(count(&s, "SELECT count(*) FROM ai_quota_holds").await, 0);
    assert_eq!(
        count(&s, "SELECT count(*) FROM ai_quota_settlements").await,
        1
    );
    assert_eq!(qq.attempts.lock().unwrap().len(), attempts_before);
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_reply_segments").await,
        1,
        "仅保留先前的绑定命令回复"
    );
    memory_jobs::extract_personal(&s, SITE_USER, 100)
        .await
        .unwrap();
    assert_eq!(llm.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        count(&s, "SELECT count(*) FROM ai_memories WHERE user_id=200").await,
        0
    );

    // 同一批混合两份身份授权：任一份在模型调用中撤销，就丢弃整份方案。
    bind_code(&s, SITE_USER, "ABCD2346").await;
    say(&s, "extract-rebind", "in-flight-qq", None, "/绑定 ABCD2346").await;
    say(
        &s,
        "extract-revoked",
        "in-flight-qq",
        None,
        "这一句提炼中会被撤销",
    )
    .await;
    bind_code(&s, SITE_USER, "ABCD2347").await;
    say(
        &s,
        "extract-valid-bind",
        "still-bound-qq",
        None,
        "/绑定 ABCD2347",
    )
    .await;
    say(
        &s,
        "extract-valid",
        "still-bound-qq",
        None,
        "这一句授权保持有效",
    )
    .await;
    let pending: Vec<i64> = sqlx::query_scalar("SELECT t.id FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id WHERE m.qq_msg_id IN ('extract-revoked','extract-valid') ORDER BY m.qq_msg_id")
        .fetch_all(&s.db).await.unwrap();
    assert_eq!(pending.len(), 2);
    controlled.arm();
    let extracting = {
        let s = s.clone();
        tokio::spawn(async move { memory_jobs::extract_personal(&s, SITE_USER, 100).await })
    };
    controlled.wait_started().await;
    assert!(last_prompt(&llm).contains("这一句提炼中会被撤销"));
    assert!(last_prompt(&llm).contains("这一句授权保持有效"));
    unbind_while_model_waits(&s, identity).await;
    controlled.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(30), extracting)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        count(&s, "SELECT count(*) FROM ai_memories WHERE user_id=200").await,
        0,
        "含撤销发言的整份提炼方案必须丢弃"
    );
    let processed: Vec<bool> = sqlx::query_scalar("SELECT personal_memory_processed_at IS NOT NULL FROM qq_turns WHERE id=ANY($1) ORDER BY id")
        .bind(&pending).fetch_all(&s.db).await.unwrap();
    assert_eq!(
        processed,
        vec![true, false],
        "仅推进已撤销来源，其余完整 turn 留待续跑"
    );
    assert!(count(&s, "SELECT count(*) FROM graphile_worker.jobs WHERE task_identifier='qq.extract_memories' AND run_at<=now()").await > 0);
    memory_jobs::extract_personal(&s, SITE_USER, 100)
        .await
        .unwrap();
    assert_eq!(
        count(&s, "SELECT count(*) FROM ai_memories WHERE user_id=200").await,
        1
    );
    assert!(last_prompt(&llm).contains("这一句授权保持有效"));
    assert!(!last_prompt(&llm).contains("这一句提炼中会被撤销"));
    let pending_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM qq_turns WHERE id=ANY($1) AND personal_memory_processed_at IS NULL",
    )
    .bind(&pending)
    .fetch_one(&s.db)
    .await
    .unwrap();
    assert_eq!(pending_count, 0);
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn queued_bind_then_chat_freezes_new_version_and_rebinding_cannot_reauthorize_history(
    db: PgPool,
) {
    let (s, _llm, qq, server) = setup(db).await;
    site_user(&s, SITE_USER, "epoch_owner").await;
    bind_code(&s, SITE_USER, "ABCD2345").await;
    let bind = enqueue(
        &s,
        "epoch-bind",
        "epoch-qq",
        None,
        "/绑定 ABCD2345",
        Duration::zero(),
    )
    .await
    .unwrap();
    let chat = enqueue(
        &s,
        "epoch-chat",
        "epoch-qq",
        None,
        "新绑定后的第一句",
        Duration::zero(),
    )
    .await
    .unwrap();
    // 模拟迁移前已经入队、但从未执行的消息：迁移排除旧历史时为它也写过 processed。
    sqlx::query("UPDATE qq_turns SET personal_memory_processed_at=now(),group_memory_processed_at=now() WHERE id=$1")
        .bind(chat).execute(&s.db).await.unwrap();
    drive_reply(&s, bind).await.unwrap();
    drive_reply(&s, chat).await.unwrap();
    let pending: (bool, bool) = sqlx::query_as("SELECT personal_memory_processed_at IS NULL,group_memory_processed_at IS NULL FROM qq_turns WHERE id=$1")
        .bind(chat).fetch_one(&s.db).await.unwrap();
    assert_eq!(
        pending,
        (true, false),
        "首次授权后的单聊应恢复个人提炼资格，并排除群提炼"
    );
    assert!(qq.sent.lock().unwrap().iter().any(|v| {
        v["content"]
            .as_str()
            .is_some_and(|t| t.contains("绑定成功"))
    }));
    let message: i64 = sqlx::query_scalar("SELECT message_id FROM qq_turns WHERE id=$1")
        .bind(chat)
        .fetch_one(&s.db)
        .await
        .unwrap();
    let snapshot = {
        let mut tx = s.db.begin().await.unwrap();
        let snapshot = identity::freeze(&mut tx, message).await.unwrap();
        tx.commit().await.unwrap();
        snapshot
    };
    assert_eq!(snapshot.owner_user_id, Some(SITE_USER));
    say(&s, "epoch-unbind", "epoch-qq", None, "/解绑").await;
    bind_code(&s, SITE_USER, "ABCD2346").await;
    say(&s, "epoch-rebind", "epoch-qq", None, "/绑定 ABCD2346").await;
    let mut tx = s.db.begin().await.unwrap();
    assert_eq!(identity::freeze(&mut tx, message).await.unwrap(), snapshot);
    assert!(!authorization::validate(&mut tx, &snapshot).await.unwrap());
    tx.rollback().await.unwrap();
    memory_jobs::extract_personal(&s, SITE_USER, 100)
        .await
        .unwrap();
    assert_eq!(
        count(&s, "SELECT count(*) FROM ai_memories WHERE user_id=200").await,
        0,
        "旧版本对话不得在重新绑定后提炼"
    );
    server.abort();
}

async fn terminal_fixture(
    s: &QqState,
    identity: i64,
    version: i64,
    at: chrono::DateTime<Utc>,
    with_reply: bool,
) -> i64 {
    let message = s.ids.next_raw();
    let turn = s.ids.next_raw();
    let mut tx = s.db.begin().await.unwrap();
    sqlx::query("INSERT INTO qq_messages(id,app_id,identity_id,ai_user_id,direction,mentioned,content,qq_msg_id,created_at,owner_user_id,binding_version) VALUES($1,'123',$2,100,'in',TRUE,'批次用户的话',$3,$4,200,$5)")
        .bind(message).bind(identity).bind(format!("memory-{message}")).bind(at).bind(version).execute(&mut *tx).await.unwrap();
    sqlx::query("INSERT INTO qq_turns(id,execution_key,message_id,identity_id,ai_user_id,payer_user_id,status,segments_sent,finished_at) VALUES($1,$2,$3,$4,100,200,$5,$6,$7)")
        .bind(turn).bind(format!("memory-{turn}")).bind(message).bind(identity).bind(if with_reply{"completed"}else{"processing"}).bind(if with_reply{1}else{0}).bind(with_reply.then_some(at)).execute(&mut *tx).await.unwrap();
    if with_reply {
        sqlx::query("INSERT INTO qq_messages(id,app_id,identity_id,ai_user_id,direction,content,reply_to_id,created_at) VALUES($1,'123',$2,100,'out','完整问答的回复',$3,$4)")
            .bind(s.ids.next_raw()).bind(identity).bind(message).bind(at).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    turn
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn complete_turn_batches_keep_ties_late_completion_and_schedule_the_remaining_backlog(
    db: PgPool,
) {
    let (s, llm, _qq, server) = setup(db).await;
    site_user(&s, SITE_USER, "batch_owner").await;
    bind_code(&s, SITE_USER, "ABCD2345").await;
    say(&s, "batch-bind", "batch-qq", None, "/绑定 ABCD2345").await;
    let (identity, version): (i64, i64) =
        sqlx::query_as("SELECT id,binding_version FROM qq_identities WHERE openid='batch-qq'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    let at: chrono::DateTime<Utc> =
        sqlx::query_scalar("SELECT clock_timestamp()-interval '2 minutes'")
            .fetch_one(&s.db)
            .await
            .unwrap();
    for _ in 0..13 {
        terminal_fixture(&s, identity, version, at, true).await;
    }
    let late = terminal_fixture(&s, identity, version, at - Duration::hours(2), false).await;
    memory_jobs::extract_personal(&s, SITE_USER, 100)
        .await
        .unwrap();
    assert_eq!(count(&s,"SELECT count(*) FROM qq_turns WHERE payer_user_id=200 AND personal_memory_processed_at IS NOT NULL").await,12);
    assert!(count(&s,"SELECT count(*) FROM graphile_worker.jobs WHERE task_identifier='qq.extract_memories' AND run_at<=now()").await>0,"满批必须自行续跑");
    memory_jobs::extract_personal(&s, SITE_USER, 100)
        .await
        .unwrap();
    assert_eq!(count(&s,"SELECT count(*) FROM qq_turns WHERE payer_user_id=200 AND personal_memory_processed_at IS NOT NULL").await,13);
    assert_eq!(llm.calls.load(Ordering::SeqCst), 2);
    let mut tx = s.db.begin().await.unwrap();
    sqlx::query("INSERT INTO qq_messages(id,app_id,identity_id,ai_user_id,direction,content,reply_to_id,created_at) SELECT $1,'123',identity_id,100,'out','迟到但确认的回复',message_id,$3 FROM qq_turns WHERE id=$2")
        .bind(s.ids.next_raw()).bind(late).bind(at-Duration::hours(2)).execute(&mut *tx).await.unwrap();
    sqlx::query(
        "UPDATE qq_turns SET status='completed',segments_sent=1,finished_at=$2 WHERE id=$1",
    )
    .bind(late)
    .bind(at - Duration::hours(2))
    .execute(&mut *tx)
    .await
    .unwrap();
    let now = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    memory_jobs::finish_turn(&s, &mut tx, late, now)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    memory_jobs::extract_personal(&s, SITE_USER, 100)
        .await
        .unwrap();
    assert_eq!(count(&s,"SELECT count(*) FROM qq_turns WHERE payer_user_id=200 AND personal_memory_processed_at IS NOT NULL").await,14);
    assert!(last_prompt(&llm).contains("迟到但确认的回复"));
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn deletion_revokes_prepared_tools_and_memory_plans_and_erases_pending_text(db: PgPool) {
    let (s, _llm, _qq, server) = setup(db).await;
    site_user(&s, SITE_USER, "delete_owner").await;
    bind_code(&s, SITE_USER, "ABCD2345").await;
    say(&s, "delete-bind", "delete-qq", None, "/绑定 ABCD2345").await;
    say(&s, "delete-chat", "delete-qq", None, "注销前的私下对话").await;
    let snapshot:identity::Snapshot=sqlx::query_as("SELECT identity_id,owner_user_id,binding_version FROM qq_messages WHERE qq_msg_id='delete-chat'").fetch_one(&s.db).await.unwrap();
    let toolbox = ik_ai::toolbox::Toolbox::forum()
        .with_memory(SITE_USER, 100, ik_ai::memory::MemoryScope::default())
        .with_authorization(Arc::new(QqAuthorization::new(snapshot.clone())));
    let plan = ik_ai::memory_jobs::prepare_external(
        &s.ai,
        SITE_USER,
        100,
        None,
        &[ik_ai::memory_jobs::TranscriptLine {
            from_ai: false,
            speaker: None,
            at: Utc::now(),
            text: "注销前等待提炼的内容".into(),
        }],
    )
    .await
    .unwrap()
    .unwrap();
    let mut tx = s.db.begin().await.unwrap();
    sqlx::query("UPDATE users SET status='deleted' WHERE id=200")
        .execute(&mut *tx)
        .await
        .unwrap();
    identity::AccountDeletionHook
        .deleting(&mut tx, SITE_USER, Utc::now())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let result = toolbox
        .execute(
            &s.ai,
            &ik_ai::tools::ToolContext::guest(),
            &s.ai.mcp,
            "remember",
            r#"{"content":"注销后不得写回"}"#,
            tokio::time::Instant::now() + std::time::Duration::from_secs(10),
        )
        .await;
    assert!(result.is_error);
    let mut tx = s.db.begin().await.unwrap();
    assert!(
        ik_ai::memory_jobs::apply_external(&mut tx, &s.ai, plan)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        count(&s, "SELECT count(*) FROM ai_memories WHERE user_id=200").await,
        0
    );
    assert_eq!(
        count(&s, "SELECT count(*) FROM qq_identities WHERE user_id=200").await,
        0
    );
    assert_eq!(count(&s,"SELECT count(*) FROM qq_messages m WHERE m.content<>'' AND (m.owner_user_id=200 OR EXISTS(SELECT 1 FROM qq_messages p WHERE p.id=m.reply_to_id AND p.owner_user_id=200))").await,0);
    assert_eq!(count(&s,"SELECT count(*) FROM qq_reply_segments r JOIN qq_turns t ON t.id=r.turn_id WHERE t.payer_user_id=200 AND r.content<>''").await,0);
    server.abort();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn memory_disabled_during_generation_stays_excluded_and_recovery_respects_group_delay(
    db: PgPool,
) {
    let (s, llm, _qq, server) = setup(db).await;
    site_user(&s, SITE_USER, "delay_owner").await;
    bind_code(&s, SITE_USER, "ABCD2345").await;
    say(
        &s,
        "delay-bind",
        "delay-qq",
        Some("delay-group"),
        "<@123> /绑定 ABCD2345",
    )
    .await;
    sqlx::query("INSERT INTO ai_memory_settings(id,enabled) VALUES(1,FALSE) ON CONFLICT(id) DO UPDATE SET enabled=FALSE").execute(&s.db).await.unwrap();
    s.ai.catalog.invalidate().await;
    say(
        &s,
        "memory-off",
        "delay-qq",
        Some("delay-group"),
        "<@123> 关闭记忆期间说的话",
    )
    .await;
    sqlx::query("UPDATE ai_memory_settings SET enabled=TRUE WHERE id=1")
        .execute(&s.db)
        .await
        .unwrap();
    s.ai.catalog.invalidate().await;
    memory_jobs::extract_personal(&s, SITE_USER, 100)
        .await
        .unwrap();
    assert_eq!(
        llm.calls.load(Ordering::SeqCst),
        1,
        "关闭时生成的旧对话不因重新开启而追溯提炼"
    );
    say(
        &s,
        "memory-on",
        "delay-qq",
        Some("delay-group"),
        "<@123> 重新开启记忆后的对话",
    )
    .await;
    memory_jobs::recover(&s).await.unwrap();
    let delay:f64=sqlx::query_scalar("SELECT extract(epoch FROM (run_at-now()))::float8 FROM graphile_worker.jobs WHERE task_identifier='qq.extract_group_memories'").fetch_one(&s.db).await.unwrap();
    assert!(
        delay > 540.0,
        "恢复任务不能把群提炼十分钟防抖改成立即执行，剩余{delay}秒"
    );
    server.abort();
}
