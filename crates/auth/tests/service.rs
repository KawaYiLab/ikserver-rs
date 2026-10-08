//! 认证 service 层集成测试：真实 PostgreSQL，每个测试一个独立临时库（`#[sqlx::test]`）。

#![allow(clippy::unwrap_used, clippy::expect_used)]

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
}

fn harness(db: PgPool) -> Harness {
    let mailer = Arc::new(CapturingMailer::default());
    let settings = AuthSettings {
        cookie_name: "ik_session".into(),
        cookie_secure: false,
        session_ttl: chrono::Duration::days(30),
        session_max_lifetime: chrono::Duration::days(180),
        registration_open: true,
        trusted_proxy_hops: 0,
        code_key: vec![7u8; 32],
    };
    let state = AuthState::new(
        db,
        Arc::new(Snowflake::new(1).unwrap()),
        mailer.clone(),
        settings,
    );
    Harness {
        state,
        mailer,
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
        let code = self.mailer.last_code(email);
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
