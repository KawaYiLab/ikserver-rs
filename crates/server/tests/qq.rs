//! QQ 机器人的站内接口（ADR 0008 第二、三期）：绑定码、我的绑定、管理后台的身份与群共享记忆。走完整中间件栈。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use ik_kernel::config::{Config, QqbotApp, Secret};
use ik_server::{AppState, build_app};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const ORIGIN: &str = "https://interk.net";
const ADMIN: &str = "iks_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const USER: &str = "iks_CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";
const OTHER: &str = "iks_DDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDDD";

async fn setup(db: &PgPool) -> Router {
    ik_kernel::jobs::Jobs::new(db.clone())
        .migrate()
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, email, role, level, exam_passed_at, is_ai_agent)
         VALUES (1, 'boss', 'boss@example.com', 'admin', 1, now(), FALSE),
                (3, 'plain', 'plain@example.com', 'user', 1, now(), FALSE),
                (4, 'fairy', 'fairy@example.com', 'user', 1, now(), TRUE),
                (5, 'other', 'other@example.com', 'user', 1, now(), FALSE)",
    )
    .execute(db)
    .await
    .unwrap();
    for (id, token) in [(1_i64, ADMIN), (3, USER), (5, OTHER)] {
        sqlx::query(
            "INSERT INTO sessions (id, user_id, token_hash, login_method, expires_at, max_expires_at)
             VALUES ($1, $1, sha256(convert_to($2, 'UTF8')), 'password',
                     now() + interval '1 day', now() + interval '1 day')",
        )
        .bind(id)
        .bind(token)
        .execute(db)
        .await
        .unwrap();
    }
    let mut config = Config::default();
    config.database.url = Secret::new("postgres://unused");
    config.http.allowed_origins = vec![ORIGIN.to_owned()];
    config.hot.enabled = false;
    config.qqbot.enabled = true;
    config.qqbot.apps.insert(
        "main".to_owned(),
        QqbotApp {
            app_id: "123".to_owned(),
            secret: Secret::new("test-secret"),
        },
    );
    let state = AppState::new(config, db.clone()).unwrap();
    state.qqbot.initialize().await.unwrap();
    build_app(state).unwrap()
}

async fn call(
    app: &Router,
    token: &str,
    method: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header("origin", ORIGIN)
        .header("user-agent", "qq-test/1.0")
        .header("authorization", format!("Bearer {token}"))
        .body(if body.is_null() {
            Body::empty()
        } else {
            Body::from(body.to_string())
        })
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, value)
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bind_codes_and_my_bindings(db: PgPool) {
    let app = setup(&db).await;
    // 后台没开 QQ 时不发码。
    let (status, body) = call(&app, USER, "POST", "/v1/me/qq/bind-codes", Value::Null).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "QQ_BOT_DISABLED");
    sqlx::query("UPDATE qq_settings SET enabled = TRUE")
        .execute(&db)
        .await
        .unwrap();
    // 设置缓存 60 秒：测试里重建应用读最新设置。
    let app = build_fresh(&db).await;
    for _ in 0..3 {
        let (status, body) = call(&app, USER, "POST", "/v1/me/qq/bind-codes", Value::Null).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["code"].as_str().unwrap().len(), 8);
        assert!(body["instruction"].as_str().unwrap().contains("/绑定"));
    }
    let (status, body) = call(&app, USER, "POST", "/v1/me/qq/bind-codes", Value::Null).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body["code"], "QQ_BIND_CODE_LIMIT");
    // 码只存 HMAC。
    let plain: i64 =
        sqlx::query_scalar("SELECT count(*) FROM qq_bind_codes WHERE length(code_hash) = 32")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(plain, 3);

    // 绑定关系由 QQ 里的 /绑定 建立；这里直接写一条，看列表与解绑。
    sqlx::raw_sql(
        "INSERT INTO qq_groups (id, app_id, group_openid) VALUES (10, '123', 'GROUPOPENID1234');
         INSERT INTO qq_identities (id, app_id, scene, openid, group_id, user_id, bound_at)
         VALUES (20, '123', 'group', 'member', 10, 3, now())",
    )
    .execute(&db)
    .await
    .unwrap();
    let (status, body) = call(&app, USER, "GET", "/v1/me/qq/bindings", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
    assert_eq!(body["items"][0]["scene"], "group");
    assert_eq!(body["items"][0]["groupName"], "QQ 群 #1234");
    assert!(!body.to_string().contains("member"), "不返回 openid");
    let (status, _) = call(&app, OTHER, "DELETE", "/v1/me/qq/bindings/20", Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "不能解别人的绑定");
    let (status, _) = call(&app, USER, "DELETE", "/v1/me/qq/bindings/20", Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = call(&app, USER, "GET", "/v1/me/qq/bindings", Value::Null).await;
    assert!(body["items"].as_array().unwrap().is_empty());
}

async fn build_fresh(db: &PgPool) -> Router {
    let mut config = Config::default();
    config.database.url = Secret::new("postgres://unused");
    config.http.allowed_origins = vec![ORIGIN.to_owned()];
    config.hot.enabled = false;
    config.qqbot.enabled = true;
    config.qqbot.apps.insert(
        "main".to_owned(),
        QqbotApp {
            app_id: "123".to_owned(),
            secret: Secret::new("test-secret"),
        },
    );
    build_app(AppState::new(config, db.clone()).unwrap()).unwrap()
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn admins_manage_identities_and_group_memories(db: PgPool) {
    let app = setup(&db).await;
    sqlx::raw_sql(
        "INSERT INTO qq_groups (id, app_id, group_openid) VALUES (10, '123', 'GROUPOPENID1234');
         INSERT INTO qq_identities (id, app_id, scene, openid, group_id, user_id, bound_at)
         VALUES (20, '123', 'group', 'member-openid-abcdef', 10, 3, now());
         INSERT INTO ai_shared_memories (id, scope, ai_user_id, content, origin)
         VALUES (30, 'qq:group:10', 4, '这个群周五晚上一起打本', 'extracted')",
    )
    .execute(&db)
    .await
    .unwrap();
    let (status, _) = call(&app, USER, "GET", "/v1/admin/qq/identities", Value::Null).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = call(
        &app,
        ADMIN,
        "GET",
        "/v1/admin/qq/identities?user=plain",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 1);
    assert_eq!(body["items"][0]["openidTail"], "abcdef");
    let (status, _) = call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/qq/identities/20",
        json!({"blocked": true, "unbind": true}),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (blocked, user_id): (bool, Option<i64>) =
        sqlx::query_as("SELECT blocked, user_id FROM qq_identities WHERE id = 20")
            .fetch_one(&db)
            .await
            .unwrap();
    assert!(blocked);
    assert_eq!(user_id, None);

    let (status, body) = call(
        &app,
        ADMIN,
        "GET",
        "/v1/admin/qq/groups/10/memories",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["items"][0]["content"], "这个群周五晚上一起打本");
    let (status, body) = call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/qq/groups/10/memories/30",
        json!({"content": "这个群周六晚上一起打本", "pinned": true}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["origin"], "manual");
    assert_eq!(body["pinned"], true);
    // 别的群的路径删不到这条。
    sqlx::query("INSERT INTO qq_groups (id, app_id, group_openid) VALUES (11, '123', 'OTHER')")
        .execute(&db)
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        ADMIN,
        "DELETE",
        "/v1/admin/qq/groups/11/memories/30",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(
        &app,
        ADMIN,
        "DELETE",
        "/v1/admin/qq/groups/10/memories/30",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/qq/groups/10/memories",
        json!({"character": "nobody", "content": "x"}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_audit_log WHERE action IN ('qq.identity.update', 'qq.group_memory.update', 'qq.group_memory.delete')",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(audits, 3);
    let (status, body) = call(&app, ADMIN, "GET", "/v1/admin/qq/turns", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);

    let (status, mut settings) =
        call(&app, ADMIN, "GET", "/v1/admin/qq/settings", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    settings["dispatchPaused"] = json!(true);
    let (status, _) = call(&app, ADMIN, "PUT", "/v1/admin/qq/settings", settings).await;
    assert_eq!(status, StatusCode::OK);
    let (status, state) = call(&app, ADMIN, "GET", "/v1/admin/qq/dispatch", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(state["paused"], true);
    assert_eq!(state["activeLeases"], 0);

    sqlx::raw_sql("INSERT INTO qq_messages(id,app_id,identity_id,group_id,direction,content) VALUES(50,'123',20,10,'in','do-not-leak-input');
        INSERT INTO qq_conversations(id,app_id,scene,target_id,next_seq) VALUES(51,'123','group',10,1);
        INSERT INTO qq_turns(id,execution_key,message_id,identity_id,group_id,conversation_id,conversation_seq,status,phase,finished_at)
        VALUES(52,'detail-test',50,20,10,51,1,'failed','terminal',now());
        INSERT INTO qq_reply_segments(turn_id,msg_seq,content,payload_hash,status,attempt_count)
        VALUES(52,1,'do-not-leak-output',sha256('payload'::bytea),'unknown',1);
        INSERT INTO qq_send_attempts(id,turn_id,msg_seq,attempt_no,lease_epoch,payload_hash,payload_version,result,endpoint,http_status,code,err_code,trace_id)
        VALUES(53,52,1,1,1,sha256('payload'::bytea),1,'rejected','group',401,11201,40012001,'trace_example')")
        .execute(&db).await.unwrap();
    let (status, detail) = call(&app, ADMIN, "GET", "/v1/admin/qq/turns/52", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert_eq!(detail["segments"][0]["status"], "unknown");
    assert_eq!(detail["attempts"][0]["code"], 11201);
    assert_eq!(detail["attempts"][0]["errCode"], 40012001);
    assert_eq!(detail["attempts"][0]["traceId"], "trace_example");
    assert!(!detail.to_string().contains("do-not-leak"));
    assert!(!detail.to_string().contains("member-openid"));
    assert_eq!(
        call(&app, USER, "GET", "/v1/admin/qq/turns/52", Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, ADMIN, "GET", "/v1/admin/qq/turns/999", Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
