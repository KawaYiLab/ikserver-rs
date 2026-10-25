//! AI 记忆（ADR 0007）：长期记忆跨会话、跨会话衔接、会话滚动摘要、对话里的记住 / 忘掉 / 翻聊天记录、
//! 事件检索与待跟进、久别重逢、每晚整理、管理后台的设置与按用户管理。上游用脚本化的假提供方，不连网络。

// clippy.toml 的测试豁免只覆盖 #[test] 函数本身，不覆盖集成测试里的辅助函数。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use ik_ai::AiState;
use ik_ai::dm::{self, RunOutcome};
use ik_ai::dto::{
    ClearMemoriesRequest, CreateSessionRequest, ImportFile, MemoryKind, MemoryOrigin,
    MemorySettingsInput,
};
use ik_ai::llm::{ChatMessage, ChatRequest, Providers, StreamEvent};
use ik_ai::memory_jobs::{self, Extracted};
use ik_ai::{memory, memory_admin, service, sessions};
use ik_auth::Principal;
use ik_auth::admin::Operator;
use ik_auth::service::RequestMeta;
use ik_kernel::bus::LocalBus;
use ik_kernel::config::Ai;
use ik_kernel::{Id, Role, Snowflake};
use ik_messaging::MessagingState;
use ik_messaging::dto::SendMessageRequest;
use serde_json::Value;
use sqlx::PgPool;

mod common;
use common::{Scripted, Step, content, usage};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const HUMAN: i64 = 100;
const FAIRY: i64 = 200;
const OTHER: i64 = 300;
const ADMIN: i64 = 900;

struct Harness {
    db: PgPool,
    state: AiState,
    fake: Arc<Scripted>,
    me: Principal,
}

fn principal(id: i64) -> Principal {
    Principal {
        user_id: Id::new(id),
        session_id: Id::new(id),
        role: Role::User,
        exam_passed: true,
        is_ai_agent: false,
        level: 1,
    }
}

async fn insert_user(db: &PgPool, id: i64, ai: bool) {
    sqlx::query!(
        "INSERT INTO users (id, username, email, role, level, is_ai_agent, exam_passed_at)
         VALUES ($1, $2, $3, 'user', 1, $4, now())",
        id,
        format!("user{id}"),
        format!("user{id}@example.com"),
        ai,
    )
    .execute(db)
    .await
    .unwrap();
}

async fn harness_with(db: PgPool, config: Ai) -> Harness {
    ik_kernel::jobs::Jobs::new(db.clone())
        .migrate()
        .await
        .unwrap();
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let bus = Arc::new(LocalBus::new(1024));
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    let messaging = MessagingState::new(db.clone(), ids.clone(), bus.clone(), media.clone());
    let forum = ik_ai::Forum::for_tests(db.clone(), ids, bus, media).unwrap();
    let fake = Arc::new(Scripted::default());
    let providers = Providers::default().with("fake", fake.clone());
    let state = AiState::new(messaging, forum, config, providers);
    assert!(state.install_hooks());
    insert_user(&db, HUMAN, false).await;
    insert_user(&db, FAIRY, true).await;
    insert_user(&db, OTHER, true).await;
    let file: ImportFile = serde_json::from_value(serde_json::json!({
        "settings": { "globalEnabled": true, "dailyTokensPerUser": 1_000_000, "dailyTokens": 0 },
        "models": [
            { "key": "base", "displayName": "基础", "provider": "fake",
              "providerModel": "fake-base", "isDefault": true, "sortOrder": 1 }
        ],
        "characters": [
            { "slug": "fairy", "displayName": "Fairy", "username": format!("user{FAIRY}"),
              "enabled": true, "systemPrompt": "你是 Fairy。" },
            { "slug": "other", "displayName": "Other", "username": format!("user{OTHER}"),
              "enabled": true, "systemPrompt": "你是 Other。" }
        ]
    }))
    .unwrap();
    service::import(&state, file).await.unwrap();
    Harness {
        db,
        state,
        fake,
        me: principal(HUMAN),
    }
}

async fn harness(db: PgPool) -> Harness {
    harness_with(db, Ai::default()).await
}

fn system_of(request: &ChatRequest) -> &str {
    let ChatMessage::System { content } = &request.messages[0] else {
        panic!("第一条不是 system")
    };
    content
}

fn code<T: std::fmt::Debug>(result: Result<T, ik_kernel::Problem>) -> (u16, String) {
    let problem = result.unwrap_err();
    (problem.status().as_u16(), problem.code().to_owned())
}

fn tool_call(id: &str, name: &str, arguments: &str) -> Step {
    Step::Emit(StreamEvent::ToolCallDelta {
        index: 0,
        id: Some(id.into()),
        name: Some(name.into()),
        arguments: Some(arguments.into()),
    })
}

impl Harness {
    async fn session(&self, character: &str) -> i64 {
        sessions::create(
            &self.state,
            &self.me,
            CreateSessionRequest {
                character: character.into(),
                title: None,
            },
        )
        .await
        .unwrap()
        .id
        .get()
    }

    async fn send(&self, conversation_id: i64, text: &str) -> i64 {
        let (_, view) = ik_messaging::service::send(
            &self.state.messaging,
            &self.me,
            conversation_id,
            SendMessageRequest {
                content: text.into(),
                reply_to_id: None,
                client_id: None,
                image_ids: Vec::new(),
            },
        )
        .await
        .unwrap();
        view.id.get()
    }

    async fn job_for_trigger(&self, trigger: i64) -> i64 {
        sqlx::query_scalar!(
            "SELECT id FROM ai_jobs WHERE execution_key = $1",
            format!("dm:{trigger}")
        )
        .fetch_one(&self.db)
        .await
        .unwrap()
    }

    /// 发一句话并让 AI 回 `reply`，返回这一次请求。
    async fn turn(&self, conversation_id: i64, text: &str, reply: &str) -> ChatRequest {
        let trigger = self.send(conversation_id, text).await;
        self.fake.push(vec![content(reply), usage(10, 5)]);
        let job = self.job_for_trigger(trigger).await;
        assert_eq!(
            dm::run_job(&self.state, job, 1, false).await.unwrap(),
            RunOutcome::Completed
        );
        self.fake.requests().last().unwrap().clone()
    }

    async fn queued(&self, key: String) -> i64 {
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "n!" FROM graphile_worker.jobs WHERE key = $1"#,
            key
        )
        .fetch_one(&self.db)
        .await
        .unwrap()
    }

    async fn items(&self) -> Vec<ik_ai::dto::MemoryView> {
        memory::user_memories(&self.state, HUMAN).await.unwrap()
    }

    async fn insert_memory(&self, id: i64, ai: Option<i64>, kind: &str, content: &str) {
        sqlx::query!(
            "INSERT INTO ai_memories (id, user_id, ai_user_id, kind, content, origin)
             VALUES ($1, $2, $3, $4, $5, 'told')",
            id,
            HUMAN,
            ai,
            kind,
            content,
        )
        .execute(&self.db)
        .await
        .unwrap();
    }

    async fn set_memory(&self, enabled: bool, embedding: bool) {
        let (provider, model) = if embedding {
            (Some("fake"), Some("fake-embed"))
        } else {
            (None, None)
        };
        sqlx::query!(
            "INSERT INTO ai_memory_settings (id, enabled, embedding_provider, embedding_model)
             VALUES (1, $1, $2, $3)
             ON CONFLICT (id) DO UPDATE SET enabled = EXCLUDED.enabled,
                 embedding_provider = EXCLUDED.embedding_provider,
                 embedding_model = EXCLUDED.embedding_model",
            enabled,
            provider,
            model,
        )
        .execute(&self.db)
        .await
        .unwrap();
        self.state.catalog.invalidate().await;
    }

    async fn contents(&self) -> Vec<(String, String)> {
        let mut items: Vec<(String, String)> = self
            .items()
            .await
            .into_iter()
            .map(|m| {
                (
                    match m.kind {
                        MemoryKind::Profile => "profile".to_owned(),
                        MemoryKind::Relation | MemoryKind::Event => m.character.unwrap_or_default(),
                    },
                    m.content,
                )
            })
            .collect();
        items.sort();
        items
    }

    async fn workflow(&self, job: i64) -> Vec<Value> {
        let stored: Value = sqlx::query_scalar!(
            "SELECT m.ai_workflow FROM ai_jobs j JOIN messages m ON m.id = j.reply_message_id
             WHERE j.id = $1",
            job
        )
        .fetch_one(&self.db)
        .await
        .unwrap()
        .unwrap();
        stored.as_array().unwrap().clone()
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn long_term_memory_and_recent_turns_follow_into_a_new_session(db: PgPool) {
    let h = harness(db).await;
    let first = h.session("fairy").await;
    let request = h
        .turn(first, "我叫阿哲，住在上海，明天要考试", "加油阿哲！")
        .await;
    // 第一次见面：没有任何记忆段。
    assert!(!system_of(&request).contains("【记忆】"));
    // 回复定稿时排了一个推迟执行的提炼任务（MEM-020）。
    assert_eq!(h.queued(format!("ai-memory:{first}")).await, 1);

    // 提炼：模型给出操作，写进档案与关系记忆。
    h.fake.push(vec![content(
        r#"{"ops":[
            {"op":"add","kind":"profile","content":"对方叫阿哲"},
            {"op":"add","kind":"profile","content":"对方住在上海"},
            {"op":"add","kind":"relation","content":"对方明天考试，你给他打了气"}
        ]}"#,
    )]);
    let extracted = memory_jobs::extract(&h.state, first).await.unwrap();
    assert_eq!(
        extracted,
        Extracted {
            added: 3,
            ..Extracted::default()
        }
    );
    // 提炼请求里有已有记忆（无）与带日期的对话；不带工具。
    let prompt = h.fake.requests().last().unwrap().clone();
    let input = prompt.messages[1].text();
    assert!(input.contains("已有记忆：\n（无）"), "{input}");
    assert!(
        input.contains("对方：我叫阿哲，住在上海，明天要考试\n"),
        "{input}"
    );
    assert!(input.ends_with("你：加油阿哲！"), "{input}");
    assert!(prompt.tools.is_empty());
    // 没有新消息时不再调用模型。
    let calls = h.fake.requests().len();
    assert_eq!(
        memory_jobs::extract(&h.state, first).await.unwrap(),
        Extracted::default()
    );
    assert_eq!(h.fake.requests().len(), calls);

    // 换一个会话：带上档案、关系记忆，以及另一个会话里最近的几轮（MEM-005）。
    let second = h.session("fairy").await;
    let request = h.turn(second, "考完了！", "考得怎么样？").await;
    let system = system_of(&request);
    assert!(
        system.contains("关于对方：\n- 对方住在上海\n- 对方叫阿哲")
            || system.contains("关于对方：\n- 对方叫阿哲\n- 对方住在上海"),
        "{system}"
    );
    assert!(
        system.contains("你们之间：\n- 对方明天考试，你给他打了气"),
        "{system}"
    );
    assert!(
        system.contains("【最近在另一个对话里】")
            && system.contains("\n对方：我叫阿哲，住在上海，明天要考试\n你：加油阿哲！"),
        "{system}"
    );
    // 历史轮次只有本会话的。
    assert_eq!(
        &request.messages[1..],
        &[ChatMessage::user("user100 说：考完了！")]
    );

    // 另一个角色：共享档案，但看不到与 Fairy 的关系记忆，也不带 Fairy 会话里的衔接。
    let other = h.session("other").await;
    let request = h.turn(other, "你好", "你好呀").await;
    let system = system_of(&request);
    assert!(system.contains("对方叫阿哲"), "{system}");
    assert!(!system.contains("你们之间"), "{system}");
    assert!(!system.contains("【最近在另一个对话里】"), "{system}");

    // 置顶的不会被自动整理删掉或改写；冲突的说法用 update 改那一条。
    let items = h.items().await;
    let shanghai = items
        .iter()
        .find(|m| m.content == "对方住在上海")
        .unwrap()
        .id;
    let name = items.iter().find(|m| m.content == "对方叫阿哲").unwrap().id;
    sqlx::query!(
        "UPDATE ai_memories SET pinned = TRUE WHERE id = $1",
        name.get()
    )
    .execute(&h.db)
    .await
    .unwrap();
    h.turn(second, "我搬到北京了", "北京冬天冷，记得多穿点")
        .await;
    // 传给模型的已有记忆：置顶的在前，其余最近更新的在前（同一时刻写入的，后写的在前）。
    h.fake.push(vec![content(
        r#"{"ops":[{"op":"delete","id":"m1"},{"op":"update","id":"m3","content":"对方住在北京"}]}"#,
    )]);
    let extracted = memory_jobs::extract(&h.state, second).await.unwrap();
    assert_eq!(
        extracted,
        Extracted {
            updated: 1,
            ..Extracted::default()
        }
    );
    let input = h.fake.requests().last().unwrap().messages[1]
        .text()
        .to_owned();
    assert!(
        input.contains("m1 [profile，置顶] 对方叫阿哲\nm2 [relation] 对方明天考试，你给他打了气\nm3 [profile] 对方住在上海\n"),
        "{input}"
    );
    let contents = h.contents().await;
    assert!(
        contents.contains(&("profile".into(), "对方叫阿哲".into())),
        "{contents:?}"
    );
    let updated = h
        .items()
        .await
        .into_iter()
        .find(|m| m.id == shanghai)
        .unwrap();
    assert_eq!(updated.content, "对方住在北京");
    assert_eq!(updated.origin, MemoryOrigin::Extracted);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn rolling_summary_covers_what_falls_out_of_the_window(db: PgPool) {
    let h = harness_with(
        db,
        Ai {
            max_history_messages: 4,
            ..Ai::default()
        },
    )
    .await;
    let conversation = h.session("fairy").await;
    h.turn(conversation, "我们聊聊配队", "好呀").await;
    h.turn(conversation, "主C 用谁", "推荐艾莲").await;
    assert_eq!(h.queued(format!("ai-summary:{conversation}")).await, 0);
    // 第三句时窗口满了（4 条）：排一个摘要任务（MEM-002）。
    h.turn(conversation, "辅助呢", "莱卡恩不错").await;
    assert_eq!(h.queued(format!("ai-summary:{conversation}")).await, 1);

    h.fake
        .push(vec![content("对方在问配队，你推荐了艾莲当主C。")]);
    assert!(
        memory_jobs::summarize(&h.state, conversation)
            .await
            .unwrap()
    );
    // 最近半个窗口（2 条）不压；压的是它之前的 4 条。
    let input = h.fake.requests().last().unwrap().messages[1]
        .text()
        .to_owned();
    assert!(
        input.contains("对方：我们聊聊配队") && input.ends_with("你：推荐艾莲"),
        "{input}"
    );
    assert!(!input.contains("已有摘要"), "{input}");

    let request = h.turn(conversation, "那就这么定了", "好的").await;
    let system = system_of(&request);
    assert!(
        system.contains("【更早的对话摘要】")
            && system.contains("对方在问配队，你推荐了艾莲当主C。"),
        "{system}"
    );
    // 窗口里仍是最近 4 条原文。
    assert_eq!(request.messages.len(), 5);

    // 重置后旧摘要不再使用（MEM-001）。
    sessions::reset(&h.state, &h.me, conversation)
        .await
        .unwrap();
    let request = h.turn(conversation, "重新开始", "好").await;
    assert!(!system_of(&request).contains("【更早的对话摘要】"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn remember_and_forget_take_effect_during_the_reply(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session("fairy").await;
    let trigger = h.send(conversation, "记住，我的生日是 3 月 14 日").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        tool_call(
            "call_1",
            "remember",
            r#"{"content":"对方的生日是 3 月 14 日","about":"user","userId":"999"}"#,
        ),
        Step::Emit(StreamEvent::Finish("tool_calls".into())),
        usage(10, 5),
    ]);
    h.fake.push(vec![content("记住啦！"), usage(10, 5)]);
    assert_eq!(
        dm::run_job(&h.state, job, 1, false).await.unwrap(),
        RunOutcome::Completed
    );
    let requests = h.fake.requests();
    let ChatMessage::Tool {
        content: result, ..
    } = requests[1].messages.last().unwrap()
    else {
        panic!()
    };
    assert_eq!(result, "已记住：对方的生日是 3 月 14 日");
    // 记在触发者名下（参数里的 userId 被忽略，TOOL-004），来源 told。
    let items = h.items().await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].origin, MemoryOrigin::Told);
    assert_eq!(items[0].kind, MemoryKind::Profile);
    // 时间线：通用工具事件 + 「记住了」提示（WF-011）。
    let events = h.workflow(job).await;
    let start = events.iter().find(|e| e["type"] == "tool.start").unwrap();
    assert_eq!(
        start["data"],
        serde_json::json!({ "tool": "remember", "title": "记下这件事", "source": "memory" })
    );
    let saved = events.iter().find(|e| e["type"] == "memory.saved").unwrap();
    assert_eq!(saved["data"]["memoryId"], items[0].id.get().to_string());
    assert_eq!(saved["data"]["content"], "对方的生日是 3 月 14 日");
    assert_eq!(saved["data"]["kind"], "profile");

    let trigger = h.send(conversation, "算了，把我生日忘掉吧").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        tool_call("call_2", "forget", r#"{"content":"生日是 3 月 14 日"}"#),
        Step::Emit(StreamEvent::Finish("tool_calls".into())),
        usage(10, 5),
    ]);
    h.fake.push(vec![content("好，忘掉了。"), usage(10, 5)]);
    dm::run_job(&h.state, job, 1, false).await.unwrap();
    assert!(h.items().await.is_empty());
    let events = h.workflow(job).await;
    let forgotten = events
        .iter()
        .find(|e| e["type"] == "memory.forgotten")
        .unwrap();
    assert_eq!(
        forgotten["data"]["contents"],
        serde_json::json!(["对方的生日是 3 月 14 日"])
    );
}

fn admin_operator() -> Operator {
    Operator {
        principal: Principal {
            user_id: Id::new(ADMIN),
            session_id: Id::new(ADMIN),
            role: Role::Admin,
            exam_passed: true,
            is_ai_agent: false,
            level: 1,
        },
        meta: RequestMeta {
            ip: std::net::IpAddr::from([127, 0, 0, 1]),
            user_agent: None,
        },
    }
}

fn settings_input(enabled: bool) -> MemorySettingsInput {
    MemorySettingsInput {
        enabled,
        model_key: None,
        extract_delay_secs: 180,
        bridge_hours: 48,
        embedding_provider: None,
        embedding_model: None,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn memory_jobs_use_disabled_models_and_fall_back_when_unavailable(db: PgPool) {
    let h = harness(db).await;
    insert_user(&h.db, ADMIN, false).await;
    sqlx::query(
        "INSERT INTO ai_models (id, key, display_name, provider, provider_model, enabled)
         VALUES (2, 'memory-only', '记忆专用', 'fake', 'fake-memory', false)",
    )
    .execute(&h.db)
    .await
    .unwrap();

    // 后台可以保存停用的专用模型，但用户的可选模型列表仍只显示启用模型。
    let mut input = settings_input(true);
    input.model_key = Some("memory-only".into());
    let saved = memory_admin::update_settings(&h.state, &admin_operator(), input)
        .await
        .unwrap();
    assert_eq!(saved.model_key.as_deref(), Some("memory-only"));
    let visible = service::models(&h.state).await.unwrap();
    assert_eq!(visible.items.len(), 1);
    assert_eq!(visible.items[0].key, "base");

    let conversation = h.session("fairy").await;
    for (change, expected_model) in [
        (None, "fake-memory"),
        (
            Some("UPDATE ai_models SET provider = 'missing' WHERE key = 'memory-only'"),
            "fake-base",
        ),
        (
            Some("DELETE FROM ai_models WHERE key = 'memory-only'"),
            "fake-base",
        ),
    ] {
        if let Some(sql) = change {
            sqlx::query(sql).execute(&h.db).await.unwrap();
            h.state.catalog.invalidate().await;
        }
        // 普通聊天始终用全站默认模型；提炼优先用专用模型，仅在提供方不可用或模型被删时回退。
        let reply = h.turn(conversation, "我喜欢看书", "最近读了什么？").await;
        assert_eq!(reply.model, "fake-base");
        let calls = h.fake.requests().len();
        h.fake.push(vec![content(r#"{"ops":[]}"#)]);
        assert_eq!(
            memory_jobs::extract(&h.state, conversation).await.unwrap(),
            Extracted::default()
        );
        let requests = h.fake.requests();
        assert_eq!(requests.len(), calls + 1);
        assert_eq!(requests.last().unwrap().model, expected_model);
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn admins_manage_settings_and_each_users_memories(db: PgPool) {
    let h = harness(db).await;
    insert_user(&h.db, ADMIN, false).await;
    let op = admin_operator();

    // 默认设置：开着，用默认模型，不算向量。
    let view = memory_admin::settings(&h.state).await.unwrap();
    assert!(view.enabled);
    assert_eq!(view.updated_at, None);
    assert_eq!((view.extract_delay_secs, view.bridge_hours), (180, 48));

    // 校验：模型要存在，向量的提供方与模型名要么都填要么都不填，范围。
    let bad = |f: fn(&mut MemorySettingsInput)| {
        let mut input = settings_input(true);
        f(&mut input);
        input
    };
    for input in [
        bad(|i| i.model_key = Some("nope".into())),
        bad(|i| i.embedding_provider = Some("fake".into())),
        bad(|i| i.embedding_model = Some("bge-m3".into())),
        bad(|i| {
            i.embedding_provider = Some("nowhere".into());
            i.embedding_model = Some("bge-m3".into());
        }),
        bad(|i| i.extract_delay_secs = -1),
        bad(|i| i.bridge_hours = 721),
    ] {
        assert_eq!(
            code(memory_admin::update_settings(&h.state, &op, input).await),
            (400, "VALIDATION_FAILED".into())
        );
    }
    let mut input = settings_input(true);
    input.model_key = Some("base".into());
    input.embedding_provider = Some("fake".into());
    input.embedding_model = Some(" fake-embed ".into());
    let saved = memory_admin::update_settings(&h.state, &op, input)
        .await
        .unwrap();
    assert_eq!(saved.model_key.as_deref(), Some("base"));
    assert_eq!(saved.embedding_model.as_deref(), Some("fake-embed"));
    assert!(saved.updated_at.is_some());
    let audited: i64 = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM admin_audit_log WHERE action = 'ai.memory.settings.update'"#
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(audited, 1);

    // 关掉：不带记忆、不提供记忆工具、不排提炼任务。
    h.insert_memory(1, None, "profile", "对方叫阿哲").await;
    memory_admin::update_settings(&h.state, &op, settings_input(false))
        .await
        .unwrap();
    let conversation = h.session("fairy").await;
    let request = h.turn(conversation, "你还记得我吗", "当然").await;
    assert!(!system_of(&request).contains("【记忆】"));
    assert_eq!(request.tools.len(), 7);
    assert_eq!(h.queued(format!("ai-memory:{conversation}")).await, 0);
    let calls = h.fake.requests().len();
    assert_eq!(
        memory_jobs::extract(&h.state, conversation).await.unwrap(),
        Extracted::default()
    );
    assert_eq!(h.fake.requests().len(), calls);

    // 打开：记忆回来了。
    memory_admin::update_settings(&h.state, &op, settings_input(true))
        .await
        .unwrap();
    let request = h.turn(conversation, "现在呢", "记得").await;
    assert!(system_of(&request).contains("- 对方叫阿哲"));
    assert_eq!(request.tools.len(), 10);

    // 按 id 或用户名（不区分大小写，可带 @）查某个用户的记忆。
    h.insert_memory(2, Some(FAIRY), "relation", "你叫对方阿哲")
        .await;
    h.insert_memory(3, Some(OTHER), "relation", "你们约好周五开黑")
        .await;
    let found = memory_admin::user_memories(&h.state, &HUMAN.to_string())
        .await
        .unwrap();
    assert_eq!(found.username, format!("user{HUMAN}"));
    assert_eq!(found.items.len(), 3);
    let by_name = memory_admin::user_memories(&h.state, &format!("@USER{HUMAN}"))
        .await
        .unwrap();
    assert_eq!(by_name.user_id, found.user_id);
    assert_eq!(
        code(memory_admin::user_memories(&h.state, "nobody").await),
        (404, "NOT_FOUND".into())
    );
    let stats = memory_admin::settings(&h.state).await.unwrap().stats;
    assert_eq!((stats.users, stats.profile, stats.relation), (1, 1, 2));

    // 删一条、按角色清、全部清，都记审计。
    memory_admin::delete(&h.state, &op, 1).await.unwrap();
    assert_eq!(
        code(memory_admin::delete(&h.state, &op, 1).await),
        (404, "NOT_FOUND".into())
    );
    let cleared = memory_admin::clear(
        &h.state,
        &op,
        ClearMemoriesRequest {
            user_id: Id::new(HUMAN),
            character: Some("fairy".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(cleared, 1);
    assert_eq!(
        h.contents().await,
        vec![("other".into(), "你们约好周五开黑".into())]
    );
    assert_eq!(
        code(
            memory_admin::clear(
                &h.state,
                &op,
                ClearMemoriesRequest {
                    user_id: Id::new(HUMAN),
                    character: Some("nobody".into()),
                },
            )
            .await
        ),
        (404, "AI_CHARACTER_NOT_FOUND".into())
    );
    memory_admin::clear(
        &h.state,
        &op,
        ClearMemoriesRequest {
            user_id: Id::new(HUMAN),
            character: None,
        },
    )
    .await
    .unwrap();
    assert!(h.items().await.is_empty());
    let audited: i64 = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM admin_audit_log
           WHERE action IN ('ai.memory.delete', 'ai.memory.clear')"#
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(audited, 3);
}

fn beijing_today() -> chrono::NaiveDate {
    (chrono::Utc::now() + chrono::Duration::hours(8)).date_naive()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn events_come_back_by_topic_and_due_follow_ups_open_the_next_chat(db: PgPool) {
    let h = harness(db).await;
    h.set_memory(true, true).await;
    let today = beijing_today();
    let first = h.session("fairy").await;
    h.turn(
        first,
        "我明天考驾照科目二，好紧张；我家猫团子最近也生病了",
        "别紧张，你可以的！",
    )
    .await;
    h.fake.push(vec![content(&format!(
        r#"{{"ops":[
            {{"op":"add","kind":"event","content":"对方考驾照科目二","date":"{today}","followUp":"{today}","importance":4}},
            {{"op":"add","kind":"event","content":"对方的猫团子生病了","date":"{early}","importance":3}}
        ]}}"#,
        early = today - chrono::Duration::days(20),
    ))]);
    let extracted = memory_jobs::extract(&h.state, first).await.unwrap();
    assert_eq!(extracted.added, 2);
    // 新事件立即补算向量（MEM-054）。
    let embedded = h.fake.embedded();
    assert!(
        embedded.contains(&"对方考驾照科目二".to_owned()),
        "{embedded:?}"
    );
    assert!(
        embedded.contains(&"对方的猫团子生病了".to_owned()),
        "{embedded:?}"
    );
    let items = h.items().await;
    let exam = items
        .iter()
        .find(|m| m.content == "对方考驾照科目二")
        .unwrap();
    assert_eq!(exam.kind, MemoryKind::Event);
    assert_eq!(exam.follow_up_on, Some(today));
    assert_eq!(exam.character.as_deref(), Some("fairy"));

    // 新会话开场：到期的待跟进让角色主动问起（MEM-055）。
    let second = h.session("fairy").await;
    let request = h.turn(second, "在吗", "在呀，科目二考得怎么样？").await;
    let system = system_of(&request);
    assert!(
        system.contains("【待跟进】") && system.contains(&format!("- {today} 对方考驾照科目二")),
        "{system}"
    );
    // 回复完成后记为已问起（MEM-056），之后不再提。
    let exam = h
        .items()
        .await
        .into_iter()
        .find(|m| m.content == "对方考驾照科目二")
        .unwrap();
    assert!(exam.followed_up_at.is_some());

    // 聊到猫：相关往事按话题取回（MEM-052），查询算了一次向量。
    let request = h
        .turn(second, "团子又生病了，不吃饭", "带它去看看医生吧")
        .await;
    let system = system_of(&request);
    assert!(!system.contains("【待跟进】"), "{system}");
    assert!(
        system.contains("【相关往事】") && system.contains("对方的猫团子生病了"),
        "{system}"
    );
    assert_eq!(
        h.fake.embedded().last().map(String::as_str),
        Some("在吗 团子又生病了，不吃饭")
    );

    // 另一个角色看不到与 Fairy 的事件。
    let other = h.session("other").await;
    let request = h.turn(other, "团子生病了", "抱抱").await;
    assert!(!system_of(&request).contains("【相关往事】"));
    assert!(!system_of(&request).contains("【待跟进】"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn remember_can_record_an_event_with_a_follow_up(db: PgPool) {
    let h = harness(db).await;
    let conversation = h.session("fairy").await;
    let trigger = h
        .send(conversation, "记住，我 11 月 3 日面试，到时候问问我")
        .await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        tool_call(
            "call_1",
            "remember",
            r#"{"content":"对方 11 月 3 日面试","about":"event","date":"2026-11-03","follow_up":"2026-11-04"}"#,
        ),
        Step::Emit(StreamEvent::Finish("tool_calls".into())),
        usage(10, 5),
    ]);
    h.fake
        .push(vec![content("记下啦，到时候问你！"), usage(10, 5)]);
    dm::run_job(&h.state, job, 1, false).await.unwrap();
    let items = h.items().await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, MemoryKind::Event);
    assert_eq!(
        items[0].happened_on.map(|d| d.to_string()).as_deref(),
        Some("2026-11-03")
    );
    assert_eq!(
        items[0].follow_up_on.map(|d| d.to_string()).as_deref(),
        Some("2026-11-04")
    );
    assert_eq!(items[0].character.as_deref(), Some("fairy"));
    let events = h.workflow(job).await;
    let saved = events.iter().find(|e| e["type"] == "memory.saved").unwrap();
    assert_eq!(saved["data"]["kind"], "event");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn recall_history_reads_old_turns_after_each_reset(db: PgPool) {
    let h = harness(db).await;
    let first = h.session("fairy").await;
    h.turn(
        first,
        "第一次来，想聊聊零号空洞的打法",
        "好呀，先说说你的配队",
    )
    .await;
    let secret = h.session("fairy").await;
    h.turn(secret, "零号空洞里我其实有个秘密", "嗯，我听着")
        .await;
    // 重置过的会话只翻重置之后的（CTX-021：AI 不再记得重置之前的）。
    sessions::reset(&h.state, &h.me, secret).await.unwrap();

    let now_chat = h.session("fairy").await;
    let trigger = h.send(now_chat, "我们第一次聊的是什么来着？").await;
    let job = h.job_for_trigger(trigger).await;
    h.fake.push(vec![
        tool_call(
            "call_1",
            "recall_history",
            r#"{"query":"零号空洞","order":"oldest"}"#,
        ),
        Step::Emit(StreamEvent::Finish("tool_calls".into())),
        usage(10, 5),
    ]);
    h.fake.push(vec![
        content("第一次你是来聊零号空洞的打法的。"),
        usage(10, 5),
    ]);
    dm::run_job(&h.state, job, 1, false).await.unwrap();
    let requests = h.fake.requests();
    let ChatMessage::Tool {
        content: result, ..
    } = requests[requests.len() - 1].messages.last().unwrap()
    else {
        panic!()
    };
    // 别的会话里的原文按时间给出，带日期与说话人（MEM-095）；重置之前的不在里面。
    assert!(
        result.contains("] 对方：第一次来，想聊聊零号空洞的打法"),
        "{result}"
    );
    assert!(!result.contains("秘密"), "{result}");
    let events = h.workflow(job).await;
    let start = events.iter().find(|e| e["type"] == "tool.start").unwrap();
    assert_eq!(
        start["data"],
        serde_json::json!({ "tool": "recall_history", "title": "翻看聊天记录", "source": "memory" })
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn coming_back_after_a_week_is_a_reunion(db: PgPool) {
    let h = harness(db).await;
    let first = h.session("fairy").await;
    h.turn(first, "晚安", "晚安～").await;
    sqlx::query!(
        "UPDATE messages SET created_at = created_at - interval '10 days' WHERE conversation_id = $1",
        first
    )
    .execute(&h.db)
    .await
    .unwrap();
    let second = h.session("fairy").await;
    let request = h.turn(second, "我回来啦", "好久不见！").await;
    let system = system_of(&request);
    assert!(
        system.contains("【久别重逢】") && system.contains("已经过了 10 天"),
        "{system}"
    );
    // 接着聊就不再提。
    let request = h.turn(second, "最近好忙", "辛苦了").await;
    assert!(!system_of(&request).contains("【久别重逢】"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn nightly_forgets_small_old_things_and_consolidates_the_rest(db: PgPool) {
    let h = harness(db).await;
    // 25 条一个多月前提炼的普通事件，加一条半年多前、从没被想起的小事，再加一条重要的老事。
    sqlx::query!(
        "INSERT INTO ai_memories (id, user_id, ai_user_id, kind, content, origin, importance,
                                  happened_on, created_at, updated_at)
         SELECT 900000 + n, $1, $2, 'event', '对方第 ' || n || ' 次聊到考研复习', 'extracted', 3,
                (now() - interval '40 days')::date, now() - interval '40 days', now() - interval '40 days'
         FROM generate_series(1, 25) AS n",
        HUMAN,
        FAIRY,
    )
    .execute(&h.db)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO ai_memories (id, user_id, ai_user_id, kind, content, origin, importance,
                                  created_at, updated_at)
         VALUES (990001, $1, $2, 'event', '对方那天午饭吃了拉面', 'extracted', 1,
                 now() - interval '200 days', now() - interval '200 days'),
                (990002, $1, $2, 'event', '对方的外婆去世了', 'extracted', 5,
                 now() - interval '400 days', now() - interval '400 days')",
        HUMAN,
        FAIRY,
    )
    .execute(&h.db)
    .await
    .unwrap();

    let (forgotten, pairs) = memory_jobs::nightly(&h.state).await.unwrap();
    assert_eq!((forgotten, pairs), (1, 1));
    assert_eq!(h.queued(format!("ai-consolidate:{HUMAN}:{FAIRY}")).await, 1);
    let contents = h.contents().await;
    assert!(
        !contents.iter().any(|(_, c)| c.contains("拉面")),
        "小事被遗忘"
    );
    assert!(
        contents.iter().any(|(_, c)| c.contains("外婆")),
        "重要的不忘"
    );

    h.fake.push(vec![content(
        r#"{"merge": [{"ids": ["e1", "e2", "e3"], "content": "对方一个多月前一直在复习考研", "importance": 4}],
            "insights": [{"kind": "profile", "content": "对方在准备考研"}],
            "drop": ["e4"]}"#,
    )]);
    let done = memory_jobs::consolidate(&h.state, HUMAN, FAIRY)
        .await
        .unwrap();
    assert_eq!(
        done,
        memory_jobs::Consolidated {
            merged: 1,
            insights: 1,
            dropped: 1,
        }
    );
    let input = h.fake.requests().last().unwrap().messages[1]
        .text()
        .to_owned();
    assert!(input.contains("e1 ["), "{input}");
    // 重要的老事也给模型看（可以合并、归纳），提示词要求不要丢。
    assert!(input.contains("外婆"), "{input}");
    let items = h.items().await;
    let events: Vec<_> = items
        .iter()
        .filter(|m| m.kind == MemoryKind::Event)
        .collect();
    // 25 + 外婆 - 3（合并）+ 1（合并结果）- 1（丢弃）= 23。
    assert_eq!(events.len(), 23);
    let merged = events
        .iter()
        .find(|m| m.content == "对方一个多月前一直在复习考研")
        .unwrap();
    assert_eq!(merged.importance, 4);
    assert!(
        items
            .iter()
            .any(|m| m.kind == MemoryKind::Profile && m.content == "对方在准备考研")
    );
    // 整理过的一周内不再排。
    assert_eq!(memory_jobs::nightly(&h.state).await.unwrap(), (0, 0));
}
