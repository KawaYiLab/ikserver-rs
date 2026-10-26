//! 外部渠道复用角色和工具，但游客不获得私信记忆；超时仍返回账单。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use ik_ai::external::{self, ExternalLine, ExternalTurn, RunOutcome};
use ik_ai::llm::Providers;
use ik_ai::{AiState, service};
use ik_kernel::Snowflake;
use ik_kernel::bus::LocalBus;
use ik_kernel::config::Ai;
use sqlx::PgPool;

mod common;
use common::{Scripted, Step, content, usage};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

async fn harness(db: PgPool) -> (AiState, Arc<Scripted>) {
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let bus = Arc::new(LocalBus::new(16));
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    let messaging =
        ik_messaging::MessagingState::new(db.clone(), ids.clone(), bus.clone(), media.clone());
    let forum = ik_ai::Forum::for_tests(db.clone(), ids, bus, media).unwrap();
    let fake = Arc::new(Scripted::default());
    let state = AiState::new(
        messaging,
        forum,
        Ai::default(),
        Providers::default().with("fake", fake.clone()),
    );
    sqlx::query("INSERT INTO users (id, username, email, role, level, is_ai_agent) VALUES (200, 'external_fairy', 'external@example.com', 'user', 1, true)")
        .execute(&db).await.unwrap();
    service::import(&state, serde_json::from_value(serde_json::json!({
        "models": [
            {"key":"base", "displayName":"基础", "provider":"fake", "providerModel":"base", "isDefault":true},
            {"key":"allowed", "displayName":"角色模型", "provider":"fake", "providerModel":"allowed", "costWeight":2}
        ],
        "characters":[{"slug":"fairy", "displayName":"Fairy", "username":"external_fairy", "enabled":true,
            "qqEnabled":true, "systemPrompt":"你是 Fairy。", "allowedModelKeys":["allowed"]}]
    })).unwrap()).await.unwrap();
    (state, fake)
}

fn turn() -> ExternalTurn {
    ExternalTurn {
        channel: "qq",
        card_slug: "fairy".into(),
        model_key: Some("base".into()),
        viewer: None,
        history: vec![ExternalLine {
            assistant: false,
            content: "群友 A：今天玩什么？".into(),
        }],
        system_note: "\n这是 QQ 群聊，只用公开资料。".into(),
        max_chars: 600,
        output_mode: ik_ai::catalog::OutputMode::Plain,
        timeout_secs: 1,
        memory: None,
        shared_scope: None,
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn external_guest_uses_character_tools_and_returns_usage_on_timeout(db: PgPool) {
    let (state, fake) = harness(db.clone()).await;
    let prepared = external::prepare(&state, turn()).await.unwrap();
    assert_eq!(prepared.ai_user_id, 200);
    // 请求了角色白名单之外的模型，仍选择白名单内的那一个。
    assert_eq!(prepared.model_key, "allowed");
    assert!(external::estimate(&prepared) > 600);
    fake.push(vec![content("一起去六分街逛逛吧。"), usage(12, 8)]);
    let reply = external::reply(&state, &prepared, 1).await;
    assert_eq!(reply.outcome, RunOutcome::Completed);
    assert_eq!(reply.segments, vec!["一起去六分街逛逛吧。"]);
    assert_eq!(reply.charge(10_000), 40);
    let requests = fake.requests();
    assert_eq!(requests[0].model, "allowed");
    assert!(
        requests[0]
            .tools
            .iter()
            .any(|tool| tool.name == "search_posts")
    );
    assert!(
        !requests[0]
            .tools
            .iter()
            .any(|tool| matches!(tool.name.as_ref(), "remember" | "forget" | "recall_history"))
    );
    let prompt = serde_json::to_string(&requests[0].messages).unwrap();
    assert!(prompt.contains("站内私信或其他群"));
    assert!(!prompt.contains("external_fairy"));

    fake.push(vec![usage(10, 5), Step::Hang]);
    let reply = external::reply(&state, &prepared, 2).await;
    assert!(matches!(reply.outcome, RunOutcome::Failed(_)));
    assert!(reply.segments.is_empty());
    assert!(reply.started);
    assert_eq!(reply.charge(10_000), 30);
    assert_eq!(fake.requests().len(), 2);

    // 默认关闭的角色不能从 QQ 入口绕过开关。
    sqlx::query("UPDATE ai_role_cards SET qq_enabled = false WHERE slug = 'fairy'")
        .execute(&db)
        .await
        .unwrap();
    state.catalog.invalidate().await;
    assert!(external::prepare(&state, turn()).await.is_err());
    assert_eq!(fake.requests().len(), 2);
}

#[test]
fn external_charge_keeps_unknown_spend_and_guest_context_has_no_account() {
    let mut reply = external::ExternalReply {
        segments: vec![],
        usage: Default::default(),
        model_key: "base".into(),
        weight: 2.0,
        started: true,
        outcome: RunOutcome::Failed("connection lost".into()),
    };
    assert_eq!(reply.charge(900), 900);
    reply.started = false;
    assert_eq!(reply.charge(900), 0);
    let guest = ik_ai::tools::ToolContext::guest();
    assert_eq!(guest.viewer_id, None);
    assert!(guest.excluded.is_empty());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn qq_output_mode_is_frozen_and_markdown_reaches_presentation_intact(db: PgPool) {
    let (state, fake) = harness(db.clone()).await;
    for mode in [
        ik_ai::catalog::OutputMode::Markdown,
        ik_ai::catalog::OutputMode::Plain,
    ] {
        let mut input = turn();
        input.output_mode = mode;
        input.max_chars = 8;
        let prepared = external::prepare(&state, input).await.unwrap();
        let raw = "## 标题\n\n**重点** [攻略](/post/123) <https://example.com/long/path>";
        fake.push(vec![content(raw), usage(10, 10)]);
        let reply = external::reply(&state, &prepared, 1).await;
        assert_eq!(reply.segments, [raw]);
        assert_eq!(reply.outcome, RunOutcome::Completed);
    }
    let requests = fake.requests();
    let markdown = serde_json::to_string(&requests[0].messages).unwrap();
    let plain = serde_json::to_string(&requests[1].messages).unwrap();
    assert!(markdown.contains("适量使用一、二级标题"));
    assert!(!markdown.contains("只输出纯文本"));
    assert!(plain.contains("只输出纯文本"));
}
