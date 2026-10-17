//! 认证 service 层集成测试：真实 PostgreSQL，每个测试一个独立临时库（`#[sqlx::test]`）。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ik_auth::dto::{
    ClientKind, CodeLoginRequest, PasswordLoginRequest, RegisterRequest, ResetPasswordRequest,
    SendCodeRequest,
};
use ik_auth::service::{self, RequestMeta, Resolution};
use ik_auth::{AuthSettings, AuthState};
use ik_kernel::Snowflake;
use ik_kernel::mail::{MailError, Mailer, OutgoingMail};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[derive(Default)]
struct CapturingMailer {
    sent: Mutex<Vec<OutgoingMail>>,
    fail: std::sync::atomic::AtomicBool,
}

#[async_trait]
impl Mailer for CapturingMailer {
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(MailError("smtp down".into()));
        }
        self.sent.lock().unwrap().push(mail);
        Ok(())
    }
}

impl CapturingMailer {
    fn count(&self) -> usize {
        self.sent.lock().unwrap().len()
    }

    /// 取最近一封发给 `to` 的邮件里的 6 位验证码。
    fn last_code(&self, to: &str) -> String {
        let sent = self.sent.lock().unwrap();
        let mail = sent
            .iter()
            .rev()
            .find(|m| m.to == to)
            .expect("没有发给该邮箱的邮件");
        let digits: Vec<char> = mail.text.chars().collect();
        digits
            .windows(6)
            .find(|w| w.iter().all(char::is_ascii_digit))
            .map(|w| w.iter().collect())
            .expect("邮件里没有 6 位验证码")
    }
}

struct Harness {
    state: AuthState,
    mailer: Arc<CapturingMailer>,
    meta: RequestMeta,
    bus: Arc<ik_kernel::bus::LocalBus>,
}

fn harness(db: PgPool) -> Harness {
    let mailer = Arc::new(CapturingMailer::default());
    let settings = AuthSettings {
        cookie_name: "ik_session".into(),
        cookie_secure: false,
        cookie_cross_site: false,
        session_ttl: chrono::Duration::days(30),
        session_max_lifetime: chrono::Duration::days(180),
        registration_open: true,
        trusted_proxy_hops: 0,
        code_key: vec![7u8; 32],
    };
    let bus = Arc::new(ik_kernel::bus::LocalBus::new(16));
    let state = AuthState::new(
        db,
        Arc::new(Snowflake::new(1).unwrap()),
        mailer.clone(),
        settings,
        bus.clone(),
    );
    Harness {
        state,
        mailer,
        bus,
        meta: RequestMeta {
            ip: IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)),
            user_agent: Some("tests".into()),
        },
    }
}

impl Harness {
    async fn send(&self, email: &str, purpose: &str) -> Result<(), ik_kernel::Problem> {
        let request: SendCodeRequest =
            serde_json::from_value(serde_json::json!({ "email": email, "purpose": purpose }))
                .unwrap();
        service::send_code(&self.state, request, &self.meta)
            .await
            .map(|_| ())
    }

    async fn register(&self, email: &str, password: &str) -> service::Issued {
        self.send(email, "register").await.unwrap();
        // 邮件发往规范化后的地址。
        let code = self.mailer.last_code(&email.trim().to_lowercase());
        service::register(
            &self.state,
            RegisterRequest {
                email: email.into(),
                code,
                password: password.into(),
                client: ClientKind::App,
            },
            &self.meta,
        )
        .await
        .unwrap()
    }

    async fn login(
        &self,
        identifier: &str,
        password: &str,
    ) -> Result<service::Issued, ik_kernel::Problem> {
        service::login_password(
            &self.state,
            PasswordLoginRequest {
                identifier: identifier.into(),
                password: password.into(),
                client: ClientKind::App,
            },
            &self.meta,
        )
        .await
    }

    async fn is_active(&self, token: &str) -> bool {
        matches!(
            service::resolve(&self.state, token).await.unwrap(),
            Resolution::Active(_)
        )
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn register_then_login_by_email_and_username(db: PgPool) {
    let h = harness(db);
    let issued = h.register("Alice@Example.com ", "hunter22").await;
    assert_eq!(issued.user.email.as_deref(), Some("alice@example.com"));
    assert!(issued.user.username.starts_with("绳网用户"));
    assert!(!issued.user.exam_passed);
    assert!(h.is_active(&issued.token).await);

    assert!(h.login("ALICE@example.com", "hunter22").await.is_ok());
    assert!(h.login(&issued.user.username, "hunter22").await.is_ok());
    // D-018：用户名大小写敏感。
    let upper = issued.user.username.to_uppercase();
    if upper != issued.user.username {
        assert_eq!(
            h.login(&upper, "hunter22").await.unwrap_err().code(),
            "INVALID_CREDENTIALS"
        );
    }
    assert_eq!(
        h.login("alice@example.com", "wrong-pass")
            .await
            .unwrap_err()
            .code(),
        "INVALID_CREDENTIALS"
    );
    assert_eq!(
        h.login("nobody@example.com", "hunter22")
            .await
            .unwrap_err()
            .code(),
        "INVALID_CREDENTIALS"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn register_rejects_taken_email_and_cooldown(db: PgPool) {
    let h = harness(db);
    h.send("bob@example.com", "register").await.unwrap();
    assert_eq!(
        h.send("bob@example.com", "register")
            .await
            .unwrap_err()
            .code(),
        "CODE_COOLDOWN"
    );

    let code = h.mailer.last_code("bob@example.com");
    service::register(
        &h.state,
        RegisterRequest {
            email: "bob@example.com".into(),
            code,
            password: "hunter22".into(),
            client: ClientKind::App,
        },
        &h.meta,
    )
    .await
    .unwrap();
    assert_eq!(
        h.send("bob@example.com", "register")
            .await
            .unwrap_err()
            .code(),
        "EMAIL_TAKEN"
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn login_and_reset_codes_do_not_reveal_unknown_emails(db: PgPool) {
    let h = harness(db);
    h.send("ghost@example.com", "login").await.unwrap();
    h.send("ghost@example.com", "reset_password").await.unwrap();
    assert_eq!(h.mailer.count(), 0, "未注册邮箱不应收到邮件");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn wrong_code_five_times_burns_the_code(db: PgPool) {
    let h = harness(db);
    h.send("carol@example.com", "register").await.unwrap();
    let code = h.mailer.last_code("carol@example.com");
    let wrong = if code == "000000" { "111111" } else { "000000" };

    let attempt = |c: &str| {
        service::register(
            &h.state,
            RegisterRequest {
                email: "carol@example.com".into(),
                code: c.into(),
                password: "hunter22".into(),
                client: ClientKind::App,
            },
            &h.meta,
        )
    };
    for _ in 0..4 {
        assert_eq!(attempt(wrong).await.unwrap_err().code(), "CODE_INVALID");
    }
    assert_eq!(
        attempt(wrong).await.unwrap_err().code(),
        "CODE_TOO_MANY_ATTEMPTS"
    );
    // 正确的码也已作废。
    assert_eq!(attempt(&code).await.unwrap_err().code(), "CODE_INVALID");
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_register_with_same_code_creates_one_account(db: PgPool) {
    let h = harness(db.clone());
    h.send("dave@example.com", "register").await.unwrap();
    let code = h.mailer.last_code("dave@example.com");

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let state = h.state.clone();
        let meta = h.meta.clone();
        let code = code.clone();
        tasks.push(tokio::spawn(async move {
            service::register(
                &state,
                RegisterRequest {
                    email: "dave@example.com".into(),
                    code,
                    password: "hunter22".into(),
                    client: ClientKind::App,
                },
                &meta,
            )
            .await
        }));
    }
    let mut ok = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(_) => ok += 1,
            Err(problem) => assert!(
                matches!(problem.code(), "CODE_INVALID" | "EMAIL_TAKEN"),
                "{problem}"
            ),
        }
    }
    assert_eq!(ok, 1);
    let users = sqlx::query_scalar!("SELECT count(*) FROM users WHERE email = 'dave@example.com'")
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(users, Some(1));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_code_login_issues_one_session(db: PgPool) {
    let h = harness(db.clone());
    h.register("erin@example.com", "hunter22").await;
    h.send("erin@example.com", "login").await.unwrap();
    let code = h.mailer.last_code("erin@example.com");

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let state = h.state.clone();
        let meta = h.meta.clone();
        let code = code.clone();
        tasks.push(tokio::spawn(async move {
            service::login_email_code(
                &state,
                CodeLoginRequest {
                    email: "erin@example.com".into(),
                    code,
                    client: ClientKind::App,
                },
                &meta,
            )
            .await
        }));
    }
    let mut ok = 0;
    for task in tasks {
        if task.await.unwrap().is_ok() {
            ok += 1;
        }
    }
    assert_eq!(ok, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reset_password_revokes_every_session(db: PgPool) {
    let h = harness(db);
    let first = h.register("frank@example.com", "hunter22").await;
    let second = h.login("frank@example.com", "hunter22").await.unwrap();

    h.send("frank@example.com", "reset_password").await.unwrap();
    let code = h.mailer.last_code("frank@example.com");
    service::reset_password(
        &h.state,
        ResetPasswordRequest {
            email: "frank@example.com".into(),
            code,
            new_password: "newpass99".into(),
        },
    )
    .await
    .unwrap();

    assert!(!h.is_active(&first.token).await);
    assert!(!h.is_active(&second.token).await);
    assert!(h.login("frank@example.com", "hunter22").await.is_err());
    assert!(h.login("frank@example.com", "newpass99").await.is_ok());
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn blocked_account_cannot_log_in_or_use_sessions(db: PgPool) {
    let h = harness(db.clone());
    let issued = h.register("gina@example.com", "hunter22").await;
    sqlx::query!(
        "UPDATE users SET status = 'blocked' WHERE id = $1",
        issued.user.id.get()
    )
    .execute(&db)
    .await
    .unwrap();

    assert!(matches!(
        service::resolve(&h.state, &issued.token).await.unwrap(),
        Resolution::Blocked
    ));
    // 密码正确才透露封禁；密码错误仍是通用错误。
    assert_eq!(
        h.login("gina@example.com", "hunter22")
            .await
            .unwrap_err()
            .code(),
        "ACCOUNT_BLOCKED"
    );
    assert_eq!(
        h.login("gina@example.com", "nope-nope")
            .await
            .unwrap_err()
            .code(),
        "INVALID_CREDENTIALS"
    );
    // 封禁账号收不到登录验证码（静默）。
    let before = h.mailer.count();
    h.send("gina@example.com", "login").await.unwrap();
    assert_eq!(h.mailer.count(), before);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn legacy_bcrypt_hash_is_upgraded_on_login(db: PgPool) {
    let h = harness(db.clone());
    let issued = h.register("hank@example.com", "hunter22").await;
    let legacy = bcrypt::hash("old-secret", 4).unwrap();
    sqlx::query!(
        "UPDATE user_passwords SET hash = $2 WHERE user_id = $1",
        issued.user.id.get(),
        legacy
    )
    .execute(&db)
    .await
    .unwrap();

    h.login("hank@example.com", "old-secret").await.unwrap();
    let stored = sqlx::query_scalar!(
        "SELECT hash FROM user_passwords WHERE user_id = $1",
        issued.user.id.get()
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert!(stored.starts_with("$argon2id$"), "{stored}");
    h.login("hank@example.com", "old-secret").await.unwrap();
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn active_sessions_are_capped_at_100(db: PgPool) {
    let h = harness(db.clone());
    let issued = h.register("ivy@example.com", "hunter22").await;
    let user_id = issued.user.id.get();
    // 直接灌 120 条活跃会话，避免 120 次密码哈希。
    sqlx::query!(
        r#"INSERT INTO sessions (id, user_id, token_hash, login_method, created_at, last_seen_at, expires_at, max_expires_at)
           SELECT g, $1, sha256(g::text::bytea), 'password',
                  now() - make_interval(mins => g), now() - make_interval(mins => g),
                  now() + interval '1 day', now() + interval '2 days'
           FROM generate_series(1, 120) AS g"#,
        user_id
    )
    .execute(&db)
    .await
    .unwrap();

    let newest = h.login("ivy@example.com", "hunter22").await.unwrap();
    let active = sqlx::query_scalar!(
        "SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL",
        user_id
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(active, Some(100));
    assert!(h.is_active(&newest.token).await);
}

/// 同一邮箱同时发很多个发码请求：冷却期内只发出一封（CODE-004）。
#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_sends_respect_the_cooldown(db: PgPool) {
    let h = harness(db);
    let tasks: Vec<_> = (0..8)
        .map(|_| {
            let state = h.state.clone();
            let meta = h.meta.clone();
            tokio::spawn(async move {
                let request: SendCodeRequest = serde_json::from_value(
                    serde_json::json!({ "email": "flood@example.com", "purpose": "register" }),
                )
                .unwrap();
                service::send_code(&state, request, &meta).await.map(|_| ())
            })
        })
        .collect();
    let mut sent = 0;
    for task in tasks {
        match task.await.unwrap() {
            Ok(()) => sent += 1,
            Err(err) => assert_eq!(err.code(), "CODE_COOLDOWN"),
        }
    }
    assert_eq!(sent, 1);
    assert_eq!(h.mailer.count(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn failed_mail_does_not_hold_the_cooldown(db: PgPool) {
    let h = harness(db);
    h.mailer
        .fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        h.send("jack@example.com", "register")
            .await
            .unwrap_err()
            .code(),
        "MAIL_SEND_FAILED"
    );
    h.mailer
        .fail
        .store(false, std::sync::atomic::Ordering::SeqCst);
    h.send("jack@example.com", "register").await.unwrap();
    assert_eq!(h.mailer.count(), 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn revoke_single_and_other_sessions(db: PgPool) {
    let h = harness(db);
    let a = h.register("kate@example.com", "hunter22").await;
    let b = h.login("kate@example.com", "hunter22").await.unwrap();
    let c = h.login("kate@example.com", "hunter22").await.unwrap();

    let Resolution::Active(principal_c) = service::resolve(&h.state, &c.token).await.unwrap()
    else {
        panic!("c 应该有效");
    };
    let sessions = service::list_sessions(&h.state, &principal_c)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 3);
    assert_eq!(sessions.iter().filter(|s| s.current).count(), 1);

    let was_current = service::revoke_session(&h.state, &principal_c, a.session.id.get())
        .await
        .unwrap();
    assert!(!was_current);
    assert!(!h.is_active(&a.token).await);
    // 不能撤销不存在 / 别人的会话。
    assert_eq!(
        service::revoke_session(&h.state, &principal_c, 42)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );

    assert_eq!(
        service::revoke_other_sessions(&h.state, &principal_c)
            .await
            .unwrap(),
        1
    );
    assert!(!h.is_active(&b.token).await);
    assert!(h.is_active(&c.token).await);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn revocations_announce_session_changes(db: PgPool) {
    use ik_kernel::bus::Audience;

    let h = harness(db);
    let mut events = h.bus.subscribe();
    let first = h.register("henry@example.com", "hunter22").await;
    let second = h.login("henry@example.com", "hunter22").await.unwrap();
    let principal = |token: &str| {
        let state = h.state.clone();
        let token = token.to_owned();
        async move {
            match service::resolve(&state, &token).await.unwrap() {
                Resolution::Active(p) => p,
                _ => panic!("会话应当有效"),
            }
        }
    };
    let p1 = principal(&first.token).await;
    let p2 = principal(&second.token).await;
    let user_id = p1.user_id.get();
    assert!(events.try_recv().is_err(), "登录不发吊销事件");
    assert!(
        service::session_active(&h.state, user_id, p1.session_id.get())
            .await
            .unwrap()
    );

    // 下线其他设备：有会话被吊销才发。
    assert_eq!(
        service::revoke_other_sessions(&h.state, &p2).await.unwrap(),
        1
    );
    let event = events.try_recv().unwrap();
    assert_eq!(event.kind, service::SESSIONS_CHANGED);
    assert_eq!(event.audience, Audience::User(user_id));
    assert!(
        !service::session_active(&h.state, user_id, p1.session_id.get())
            .await
            .unwrap()
    );
    assert_eq!(
        service::revoke_other_sessions(&h.state, &p2).await.unwrap(),
        0
    );
    assert!(events.try_recv().is_err());

    service::logout(&h.state, &p2).await.unwrap();
    assert_eq!(events.try_recv().unwrap().kind, service::SESSIONS_CHANGED);

    h.send("henry@example.com", "reset_password").await.unwrap();
    service::reset_password(
        &h.state,
        ResetPasswordRequest {
            email: "henry@example.com".into(),
            code: h.mailer.last_code("henry@example.com"),
            new_password: "newpass99".into(),
        },
    )
    .await
    .unwrap();
    assert_eq!(events.try_recv().unwrap().audience, Audience::User(user_id));
}

// ---------- 账号安全：绑定邮箱、修改密码 ----------

impl Harness {
    async fn principal(&self, token: &str) -> ik_auth::Principal {
        match service::resolve(&self.state, token).await.unwrap() {
            Resolution::Active(p) => p,
            other => panic!("令牌应当有效：{other:?}"),
        }
    }

    /// 没有邮箱、没有密码的账号（米游社首登）及其会话。
    async fn emailless(&self, id: i64) -> ik_auth::Principal {
        let mut tx = self.state.db.begin().await.unwrap();
        assert!(
            ik_auth::interop::insert_user_without_email(
                &mut tx,
                id,
                &format!("扫码用户{id}"),
                chrono::Utc::now()
            )
            .await
            .unwrap()
        );
        let issued = service::issue_session_with_secret(
            &self.state,
            &mut tx,
            id,
            "mihoyo",
            &[id as u8; 32],
            &self.meta,
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        self.principal(&issued.token).await
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bind_and_change_email(db: PgPool) {
    use ik_auth::dto::{BindEmailCodeRequest, BindEmailRequest};
    let h = harness(db);
    let alice = h.register("alice@example.com", "secret-1").await;
    h.register("bob@example.com", "secret-2").await;
    let me = h.principal(&alice.token).await;
    let code_for = |email: &str| BindEmailCodeRequest {
        email: email.into(),
    };

    // 这两种用途不能经公开的发码接口申请。
    assert!(
        serde_json::from_value::<SendCodeRequest>(
            serde_json::json!({ "email": "x@example.com", "purpose": "bind_email" })
        )
        .is_err()
    );

    let err = service::send_bind_email_code(&h.state, &me, code_for("Alice@Example.com"), &h.meta)
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "UNCHANGED");
    let err = service::send_bind_email_code(&h.state, &me, code_for("bob@example.com"), &h.meta)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "EMAIL_TAKEN");
    service::send_bind_email_code(&h.state, &me, code_for("new@example.com"), &h.meta)
        .await
        .unwrap();
    let err = service::send_bind_email_code(&h.state, &me, code_for("new@example.com"), &h.meta)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "CODE_COOLDOWN");

    let bind = |code: String| BindEmailRequest {
        email: "new@example.com".into(),
        code,
    };
    let err = service::bind_email(&h.state, &me, bind("000000".into()))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "CODE_INVALID");
    let code = h.mailer.last_code("new@example.com");
    let view = service::bind_email(&h.state, &me, bind(code.clone()))
        .await
        .unwrap();
    assert_eq!(view.email.as_deref(), Some("new@example.com"));
    // 一码只用一次。
    assert_eq!(
        service::bind_email(&h.state, &me, bind(code))
            .await
            .unwrap_err()
            .code(),
        "CODE_INVALID"
    );
    // 新邮箱能登录，旧邮箱不能。
    assert!(h.login("new@example.com", "secret-1").await.is_ok());
    assert!(h.login("alice@example.com", "secret-1").await.is_err());

    // 米游社首登的账号绑定第一个邮箱。
    let qr_user = h.emailless(900).await;
    service::send_bind_email_code(&h.state, &qr_user, code_for("first@example.com"), &h.meta)
        .await
        .unwrap();
    let code = h.mailer.last_code("first@example.com");
    let view = service::bind_email(
        &h.state,
        &qr_user,
        BindEmailRequest {
            email: "first@example.com".into(),
            code,
        },
    )
    .await
    .unwrap();
    assert_eq!(view.email.as_deref(), Some("first@example.com"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn change_password_signs_out_everywhere_else(db: PgPool) {
    use ik_auth::dto::ChangePasswordRequest;
    let h = harness(db);
    let first = h.register("carol@example.com", "secret-1").await;
    let second = h.login("carol@example.com", "secret-1").await.unwrap();
    let me = h.principal(&first.token).await;
    let change = |current: &str, new: &str| ChangePasswordRequest {
        current_password: current.into(),
        new_password: new.into(),
        client: ClientKind::App,
    };

    let err = service::change_password(&h.state, &me, change("wrong-1", "secret-2"), &h.meta)
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].field, "currentPassword");
    let err = service::change_password(&h.state, &me, change("secret-1", "secret-1"), &h.meta)
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "UNCHANGED");
    let err = service::change_password(&h.state, &me, change("secret-1", "123"), &h.meta)
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].field, "newPassword");

    let mut events = h.bus.subscribe();
    let issued = service::change_password(&h.state, &me, change("secret-1", "secret-2"), &h.meta)
        .await
        .unwrap();
    assert_eq!(issued.session.login_method, "password_change");
    assert_eq!(events.try_recv().unwrap().kind, service::SESSIONS_CHANGED);
    for old in [&first.token, &second.token] {
        assert!(matches!(
            service::resolve(&h.state, old).await.unwrap(),
            Resolution::Invalid
        ));
    }
    assert!(matches!(
        service::resolve(&h.state, &issued.token).await.unwrap(),
        Resolution::Active(_)
    ));
    assert!(h.login("carol@example.com", "secret-1").await.is_err());
    assert!(h.login("carol@example.com", "secret-2").await.is_ok());

    // 没有密码的账号要走重置密码。
    let qr_user = h.emailless(901).await;
    let err = service::change_password(&h.state, &qr_user, change("x", "secret-3"), &h.meta)
        .await
        .unwrap_err();
    assert_eq!(err.code(), "PASSWORD_NOT_SET");
}
