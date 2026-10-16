//! 管理后台接口：权限分级（普通用户 / 版主 / 管理员）、题库、AI 配置、审计日志。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::Router;
use axum::body::Body;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use ik_kernel::config::{Config, Secret};
use ik_server::{AppState, build_app};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

const ORIGIN: &str = "https://interk.net";
const ADMIN: &str = "iks_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const MODERATOR: &str = "iks_BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const USER: &str = "iks_CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC";

async fn setup(db: &PgPool) -> Router {
    ik_kernel::jobs::Jobs::new(db.clone())
        .migrate()
        .await
        .unwrap();
    sqlx::query!(
        "INSERT INTO users (id, username, email, role, level, exam_passed_at)
         VALUES (1, 'boss', 'boss@example.com', 'admin', 1, now()),
                (2, 'mod', 'mod@example.com', 'moderator', 1, now()),
                (3, 'plain', 'plain@example.com', 'user', 1, now()),
                (4, 'fairy', 'fairy@example.com', 'user', 1, now())"
    )
    .execute(db)
    .await
    .unwrap();
    for (id, token) in [(1_i64, ADMIN), (2, MODERATOR), (3, USER)] {
        sqlx::query!(
            "INSERT INTO sessions (id, user_id, token_hash, login_method, expires_at, max_expires_at)
             VALUES ($1, $1, sha256(convert_to($2, 'UTF8')), 'password',
                     now() + interval '1 day', now() + interval '1 day')",
            id,
            token
        )
        .execute(db)
        .await
        .unwrap();
    }
    let mut config = Config::default();
    config.database.url = Secret::new("postgres://unused");
    config.http.allowed_origins = vec![ORIGIN.to_owned()];
    config.hot.enabled = false;
    let state = AppState::new(config, db.clone()).unwrap();
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
        .header("user-agent", "admin-test/1.0")
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
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

fn question(text: &str) -> Value {
    json!({
        "question": text,
        "kind": "single",
        "options": [{ "key": "A", "text": "对" }, { "key": "B", "text": "错" }],
        "correctKeys": ["A"],
        "explanation": "  ",
        "weight": 0,
        "tag": "规则"
    })
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn exam_bank_permissions_crud_and_audit(db: PgPool) {
    let app = setup(&db).await;
    let base = "/v1/admin/exam/questions";

    // 普通用户看不到；版主能看不能改。
    assert_eq!(
        call(&app, USER, "GET", base, Value::Null).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, MODERATOR, "GET", base, Value::Null).await.0,
        StatusCode::OK
    );
    let (status, body) = call(&app, MODERATOR, "POST", base, question("版主出题")).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    // 管理员新建：weight 非法按 1、空解析存 NULL（EXAMADM-005）；答案与解析在后台可见。
    let (status, created) = call(&app, ADMIN, "POST", base, question("绳网是什么？")).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["weight"], 1);
    assert_eq!(created["explanation"], Value::Null);
    assert_eq!(created["correctKeys"], json!(["A"]));
    let id = created["id"].as_str().unwrap().to_owned();

    // 校验（EXAMADM-004）：多选至少 3 个选项。
    let mut bad = question("坏题");
    bad["kind"] = json!("multiple");
    let (status, body) = call(&app, ADMIN, "POST", base, bad).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["errors"][0]["detail"], "多选题至少需要 3 个选项");

    call(&app, ADMIN, "POST", base, question("第二题")).await;
    // 筛选与分页。
    let (_, page) = call(
        &app,
        MODERATOR,
        "GET",
        &format!("{base}?q=绳网&limit=10"),
        Value::Null,
    )
    .await;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["id"], id.as_str());
    let (_, page) = call(
        &app,
        ADMIN,
        "GET",
        &format!("{base}?page=2&limit=1"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (
            page["total"].as_i64(),
            page["page"].as_i64(),
            page["items"].as_array().unwrap().len()
        ),
        (Some(2), Some(2), 1)
    );

    // 修改（整体覆盖）、启停、统计。
    let mut edited = question("绳网是什么地方？");
    edited["enabled"] = json!(false);
    let (status, body) = call(&app, ADMIN, "PUT", &format!("{base}/{id}"), edited).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["enabled"], false);
    let (_, body) = call(
        &app,
        ADMIN,
        "POST",
        &format!("{base}/{id}/toggle"),
        Value::Null,
    )
    .await;
    assert_eq!(body["enabled"], true);
    let (_, stats) = call(
        &app,
        MODERATOR,
        "GET",
        &format!("{base}/stats"),
        Value::Null,
    )
    .await;
    assert_eq!(
        (
            stats["total"].as_i64(),
            stats["enabled"].as_i64(),
            stats["single"].as_i64()
        ),
        (Some(2), Some(2), Some(2))
    );
    assert!(stats["questionCount"].as_u64().unwrap() > 0);
    assert_eq!(
        call(&app, ADMIN, "GET", &format!("{base}/123"), Value::Null)
            .await
            .0,
        StatusCode::NOT_FOUND
    );

    // 批量删除：只计实际删除的。
    let (status, body) = call(
        &app,
        ADMIN,
        "POST",
        &format!("{base}/bulk-delete"),
        json!({ "ids": [id, "999", id] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deletedCount"], 1);

    // 每个写操作一条审计：操作者、动作、目标、IP、UA（失败的写入不记）。
    let rows = sqlx::query!(
        "SELECT actor_id, action, target_id, user_agent, ip IS NOT NULL AS has_ip
         FROM admin_audit_log ORDER BY created_at, id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    let actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
    assert_eq!(
        actions,
        vec![
            "exam.question.create",
            "exam.question.create",
            "exam.question.update",
            "exam.question.toggle",
            "exam.question.delete",
        ]
    );
    assert!(rows.iter().all(|r| r.actor_id == Some(1)));
    assert_eq!(rows[0].user_agent.as_deref(), Some("admin-test/1.0"));
    assert_eq!(rows[0].has_ip, Some(true));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn ai_config_is_admin_only_and_round_trips(db: PgPool) {
    let app = setup(&db).await;
    assert_eq!(
        call(&app, MODERATOR, "GET", "/v1/admin/ai/settings", Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );

    let (status, settings) = call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/ai/settings",
        json!({ "globalEnabled": false, "dailyTokensPerUser": 5000, "dailyTokens": 0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["globalEnabled"], false);
    assert_eq!(settings["dailyTokensPerUser"], 5000);

    let model = |key: &str, default: bool| {
        json!({ "key": key, "displayName": key.to_uppercase(), "provider": "nowhere",
                "providerModel": "m", "costWeight": 1.5, "isDefault": default })
    };
    let (status, body) = call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/ai/models/base",
        model("base", true),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["providerConfigured"], false);
    call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/ai/models/pro",
        model("pro", false),
    )
    .await;
    // 路径与请求体的 key 不一致。
    let (status, _) = call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/ai/models/x",
        model("pro", false),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, card) = call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/ai/characters/fairy",
        json!({ "slug": "fairy", "displayName": "Fairy", "username": "fairy", "enabled": true,
                "systemPrompt": "你是 Fairy。", "defaultModelKey": "pro",
                "allowedModelKeys": ["base", "pro"],
                "examples": [{ "user": "你好", "assistant": "你好，绳匠。" }] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{card}");
    assert_eq!(card["username"], "fairy");
    assert_eq!(card["allowedModelKeys"], json!(["base", "pro"]));
    // 绑定的账号被标为 AI 账号。
    let is_ai: bool = sqlx::query_scalar!("SELECT is_ai_agent FROM users WHERE id = 4")
        .fetch_one(&db)
        .await
        .unwrap();
    assert!(is_ai);

    // 默认模型不能删；删别的模型时从角色卡里摘掉。
    let (status, body) = call(
        &app,
        ADMIN,
        "DELETE",
        "/v1/admin/ai/models/base",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "AI_MODEL_IS_DEFAULT");
    let (status, _) = call(
        &app,
        ADMIN,
        "DELETE",
        "/v1/admin/ai/models/pro",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, cards) = call(&app, ADMIN, "GET", "/v1/admin/ai/characters", Value::Null).await;
    assert_eq!(cards[0]["defaultModelKey"], Value::Null);
    assert_eq!(cards[0]["allowedModelKeys"], json!(["base"]));

    let (status, _) = call(
        &app,
        ADMIN,
        "DELETE",
        "/v1/admin/ai/characters/fairy",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, cards) = call(&app, ADMIN, "GET", "/v1/admin/ai/characters", Value::Null).await;
    assert_eq!(cards, json!([]));

    let (status, jobs) = call(
        &app,
        ADMIN,
        "GET",
        "/v1/admin/ai/jobs?status=failed",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jobs["total"], 0);

    let audited: Vec<String> =
        sqlx::query_scalar!("SELECT action FROM admin_audit_log ORDER BY created_at, id")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(
        audited,
        vec![
            "ai.settings.update",
            "ai.model.save",
            "ai.model.save",
            "ai.character.save",
            "ai.model.delete",
            "ai.character.delete",
        ]
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn categories_and_tags(db: PgPool) {
    let app = setup(&db).await;
    let input = json!({ "slug": "guide", "name": "攻略区", "description": "攻略", "sortOrder": 2,
                        "allowCommentAccept": true });
    assert_eq!(
        call(
            &app,
            MODERATOR,
            "POST",
            "/v1/admin/categories",
            input.clone()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, created) = call(&app, ADMIN, "POST", "/v1/admin/categories", input.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["postCount"], 0);
    let id = created["id"].as_str().unwrap().to_owned();
    let (status, body) = call(&app, ADMIN, "POST", "/v1/admin/categories", input).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // 停用：公开分区列表里消失，后台仍能看到。
    let (status, updated) = call(
        &app,
        ADMIN,
        "PUT",
        &format!("/v1/admin/categories/{id}"),
        json!({ "slug": "guide", "name": "攻略", "isActive": false }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["isActive"], false);
    let (_, public) = call(&app, USER, "GET", "/v1/categories", Value::Null).await;
    assert!(public.to_string().find("guide").is_none(), "{public}");
    let (_, all) = call(&app, MODERATOR, "GET", "/v1/admin/categories", Value::Null).await;
    assert_eq!(all[0]["name"], "攻略");

    // 有帖子的分区不能删。
    sqlx::query!(
        "INSERT INTO posts (id, author_id, title, body, status, category_id, published_at,
                            first_published_at, bumped_at)
         VALUES (900, 1, '帖', '正文', 'published', $1, now(), now(), now())",
        id.parse::<i64>().unwrap()
    )
    .execute(&db)
    .await
    .unwrap();
    let (status, body) = call(
        &app,
        ADMIN,
        "DELETE",
        &format!("/v1/admin/categories/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "CATEGORY_IN_USE");
    sqlx::query!("DELETE FROM posts WHERE id = 900")
        .execute(&db)
        .await
        .unwrap();
    let (status, _) = call(
        &app,
        ADMIN,
        "DELETE",
        &format!("/v1/admin/categories/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // 标签：改名时 slug 随名称重新派生；封禁。
    sqlx::query!(
        "INSERT INTO tags (id, name, slug) VALUES (1, '零号空洞', '零号空洞'), (2, 'C++', 'c++')"
    )
    .execute(&db)
    .await
    .unwrap();
    let (status, tag) = call(
        &app,
        ADMIN,
        "PATCH",
        "/v1/admin/tags/1",
        json!({ "name": "  零号  空洞 " }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{tag}");
    assert_eq!(
        (tag["name"].as_str(), tag["slug"].as_str()),
        (Some("零号 空洞"), Some("零号-空洞"))
    );
    let (status, _) = call(
        &app,
        ADMIN,
        "PATCH",
        "/v1/admin/tags/2",
        json!({ "name": "零号空洞" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = call(
        &app,
        ADMIN,
        "PATCH",
        "/v1/admin/tags/1",
        json!({ "name": "零号空洞" }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (_, tag) = call(
        &app,
        ADMIN,
        "PATCH",
        "/v1/admin/tags/1",
        json!({ "isBlocked": true }),
    )
    .await;
    assert_eq!(tag["isBlocked"], true);
    let (_, page) = call(
        &app,
        MODERATOR,
        "GET",
        "/v1/admin/tags?blocked=true",
        Value::Null,
    )
    .await;
    assert_eq!(page["total"], 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn users_roles_economy_and_bans(db: PgPool) {
    let app = setup(&db).await;
    sqlx::query!(
        "INSERT INTO users (id, username, email, role, level) VALUES (5, 'newbie', 'newbie@example.com', 'user', 0)"
    )
    .execute(&db)
    .await
    .unwrap();

    // 邮箱只有管理员看到全文。
    let (_, page) = call(
        &app,
        MODERATOR,
        "GET",
        "/v1/admin/users?q=newbie",
        Value::Null,
    )
    .await;
    assert_eq!(page["total"], 1);
    assert_eq!(page["items"][0]["email"], "n***@example.com");
    let (_, page) = call(&app, ADMIN, "GET", "/v1/admin/users?q=5", Value::Null).await;
    assert_eq!(page["items"][0]["email"], "newbie@example.com");
    let (_, page) = call(
        &app,
        ADMIN,
        "GET",
        "/v1/admin/users?role=moderator",
        Value::Null,
    )
    .await;
    assert_eq!(page["items"][0]["username"], "mod");

    // 角色：不能改自己；版主不能改别人。
    let (status, body) = call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/users/1/role",
        json!({ "role": "user" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["code"], "CANNOT_CHANGE_SELF");
    assert_eq!(
        call(
            &app,
            MODERATOR,
            "PUT",
            "/v1/admin/users/5/role",
            json!({ "role": "admin" })
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (_, detail) = call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/users/5/role",
        json!({ "role": "moderator" }),
    )
    .await;
    assert_eq!(detail["user"]["role"], "moderator");
    call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/users/5/role",
        json!({ "role": "user" }),
    )
    .await;
    let (_, detail) = call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/users/5/ai-agent",
        json!({ "isAiAgent": true }),
    )
    .await;
    assert_eq!(detail["user"]["isAiAgent"], true);
    call(
        &app,
        ADMIN,
        "PUT",
        "/v1/admin/users/5/ai-agent",
        json!({ "isAiAgent": false }),
    )
    .await;

    // 手动通过考试：等级不再锁 0。
    let (status, detail) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/users/5/exam-pass",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detail}");
    assert!(detail["user"]["examPassedAt"].is_string());
    assert!(detail["user"]["level"].as_i64().unwrap() >= 1);

    // 丁尼：发放、扣除、余额不足；经验发放不占每日上限。
    let (status, economy) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/users/5/denny",
        json!({ "amount": 100, "reason": "活动奖励" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{economy}");
    let after_grant = economy["denny"].as_i64().unwrap();
    let (_, economy) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/users/5/denny",
        json!({ "amount": -50, "reason": "误发" }),
    )
    .await;
    assert_eq!(economy["denny"].as_i64().unwrap(), after_grant - 50);
    let (status, body) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/users/5/denny",
        json!({ "amount": -9999, "reason": "扣光" }),
    )
    .await;
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    let (status, _) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/users/5/denny",
        json!({ "amount": 0, "reason": "x" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, economy) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/users/5/exp",
        json!({ "amount": 5000, "reason": "补发" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{economy}");
    assert_eq!(economy["exp"], 5000);
    assert!(economy["level"].as_i64().unwrap() > 1);
    let logs: Vec<String> = sqlx::query_scalar!(
        "SELECT action FROM denny_logs WHERE user_id = 5 ORDER BY created_at, id"
    )
    .fetch_all(&db)
    .await
    .unwrap();
    assert_eq!(logs, vec!["admin_grant", "admin_deduct"]);

    // 封禁：立即下线；不能封禁员工；解封。
    let (status, body) = call(
        &app,
        MODERATOR,
        "POST",
        "/v1/admin/users/3/block",
        json!({ "note": "刷屏" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["blocked"], true);
    assert_eq!(
        call(&app, USER, "GET", "/v1/me", Value::Null).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (_, body) = call(
        &app,
        MODERATOR,
        "POST",
        "/v1/admin/users/3/block",
        json!({}),
    )
    .await;
    assert_eq!(body["blocked"], false);
    let (status, body) = call(
        &app,
        MODERATOR,
        "POST",
        "/v1/admin/users/1/block",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["code"], "CANNOT_SANCTION_STAFF");
    let (_, body) = call(
        &app,
        MODERATOR,
        "POST",
        "/v1/admin/users/3/unblock",
        Value::Null,
    )
    .await;
    assert_eq!(body["unblocked"], true);

    // 审计日志：只有管理员能看，带操作者名字；可按动作前缀筛选。
    assert_eq!(
        call(&app, MODERATOR, "GET", "/v1/admin/audit", Value::Null)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let (_, audit) = call(
        &app,
        ADMIN,
        "GET",
        "/v1/admin/audit?action=user.block&limit=5",
        Value::Null,
    )
    .await;
    assert_eq!(audit["total"], 1);
    assert_eq!(audit["items"][0]["actorName"], "mod");
    assert_eq!(audit["items"][0]["targetId"], "3");
    let (_, audit) = call(
        &app,
        ADMIN,
        "GET",
        "/v1/admin/audit?targetType=user&targetId=5",
        Value::Null,
    )
    .await;
    assert_eq!(audit["total"], 8);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn cosmetics_dashboard_and_search_ops(db: PgPool) {
    let app = setup(&db).await;
    sqlx::query!(
        "INSERT INTO media_files (id, object_key, mime, size_bytes, sha256, receipt)
         VALUES (70, 'a/70.png', 'image/png', 10, repeat('a', 64), repeat('c', 64)),
                (71, 'a/71.png', 'image/png', 10, repeat('b', 64), repeat('d', 64))"
    )
    .execute(&db)
    .await
    .unwrap();
    let (status, avatar) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/cosmetics/avatars",
        json!({ "name": "邦布", "kind": "character", "imageId": "70" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{avatar}");
    let avatar_id = avatar["id"].as_str().unwrap().to_owned();
    // 有人装备着它：换图同步，删除时卸下。
    sqlx::query!(
        "UPDATE users SET equipped_avatar_id = $1, equipped_avatar_media_id = 70 WHERE id = 3",
        avatar_id.parse::<i64>().unwrap()
    )
    .execute(&db)
    .await
    .unwrap();
    let (status, _) = call(
        &app,
        ADMIN,
        "PUT",
        &format!("/v1/admin/cosmetics/avatars/{avatar_id}"),
        json!({ "name": "邦布", "kind": "city", "imageId": "71" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let media: Option<i64> =
        sqlx::query_scalar!("SELECT equipped_avatar_media_id FROM users WHERE id = 3")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(media, Some(71));
    let (status, body) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/cosmetics/avatars",
        json!({ "name": "缺图", "imageId": "999" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let (status, _) = call(
        &app,
        ADMIN,
        "DELETE",
        &format!("/v1/admin/cosmetics/avatars/{avatar_id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let equipped: Option<i64> =
        sqlx::query_scalar!("SELECT equipped_avatar_id FROM users WHERE id = 3")
            .fetch_one(&db)
            .await
            .unwrap();
    assert_eq!(equipped, None);

    let (status, card) = call(
        &app,
        ADMIN,
        "POST",
        "/v1/admin/cosmetics/cards",
        json!({ "name": "新艾利都", "kind": "city", "description": "城市", "story": "# 故事", "imageId": "70" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{card}");
    let (_, cards) = call(
        &app,
        MODERATOR,
        "GET",
        "/v1/admin/cosmetics/cards",
        Value::Null,
    )
    .await;
    assert_eq!(cards[0]["story"], "# 故事");

    let (status, dashboard) =
        call(&app, MODERATOR, "GET", "/v1/admin/dashboard", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{dashboard}");
    assert_eq!(dashboard["users"]["total"], 4);
    assert_eq!(dashboard["reports"]["pending"], 0);
    assert!(dashboard["exam"]["questionCount"].is_number());
    assert_eq!(dashboard["ai"]["globalEnabled"], true);

    let (_, search) = call(&app, ADMIN, "GET", "/v1/admin/search", Value::Null).await;
    assert_eq!(search["engineConfigured"], false);
    assert_eq!(search["indexedPosts"], Value::Null);
    let (status, body) = call(&app, ADMIN, "POST", "/v1/admin/search/reindex", Value::Null).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["code"], "SEARCH_ENGINE_NOT_CONFIGURED");
}
