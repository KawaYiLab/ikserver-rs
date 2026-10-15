//! 账号安全：概览与注销（邮箱验证码、米游社扫码两种核验）。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use async_trait::async_trait;
use ik_account::dto::{DeleteAccountRequest, DeleteMethod};
use ik_account::service;
use ik_auth::dto::{ClientKind, RegisterRequest, SendCodeRequest};
use ik_auth::service::{RequestMeta, Resolution};
use ik_auth::{AuthSettings, AuthState, Principal};
use ik_kernel::Snowflake;
use ik_kernel::bus::{LocalBus, internal};
use ik_kernel::mail::{MailError, Mailer, OutgoingMail};
use ik_mihoyo::MihoyoState;
use ik_mihoyo::dto::{CreateQrRequest, PollQrRequest, QrMode};
use ik_mihoyo::upstream::{
    self, Confirmed, Cookies, Device, GameRole, MihoyoApi, QrCode, UpstreamError,
};
use parking_lot::Mutex;
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[derive(Default)]
struct Mail(Mutex<Vec<OutgoingMail>>);

#[async_trait]
impl Mailer for Mail {
    async fn send(&self, mail: OutgoingMail) -> Result<(), MailError> {
        self.0.lock().push(mail);
        Ok(())
    }
}

impl Mail {
    fn last(&self, to: &str) -> OutgoingMail {
        self.0
            .lock()
            .iter()
            .rev()
            .find(|m| m.to == to)
            .cloned()
            .expect("没有邮件")
    }

    fn last_code(&self, to: &str) -> String {
        let chars: Vec<char> = self.last(to).text.chars().collect();
        chars
            .windows(6)
            .find(|w| w.iter().all(char::is_ascii_digit))
            .map(|w| w.iter().collect())
            .expect("没有验证码")
    }
}

/// 每个 ticket 确认为预设的米游社账号。
#[derive(Default)]
struct FakeApi {
    confirms: Mutex<HashMap<String, String>>,
    created: Mutex<u32>,
}

#[async_trait]
impl MihoyoApi for FakeApi {
    async fn create_qr(&self, _: &Device) -> Result<QrCode, UpstreamError> {
        let mut n = self.created.lock();
        *n += 1;
        Ok(QrCode {
            url: format!("https://qr/?ticket=t{n}"),
            ticket: format!("t{n}"),
            expires_at: None,
        })
    }

    async fn query_qr(
        &self,
        ticket: &str,
        _: &Device,
    ) -> Result<upstream::QrStatus, UpstreamError> {
        Ok(match self.confirms.lock().get(ticket) {
            Some(aid) => upstream::QrStatus::Confirmed(Confirmed {
                aid: aid.clone(),
                mid: None,
                cookies: Cookies(vec![("ltoken_v2".into(), "x".into())]),
            }),
            None => upstream::QrStatus::Created,
        })
    }

    async fn game_roles(&self, _: &Cookies) -> Result<Vec<GameRole>, UpstreamError> {
        Ok(Vec::new())
    }
}

struct Harness {
    auth: AuthState,
    mihoyo: MihoyoState,
    api: Arc<FakeApi>,
    mail: Arc<Mail>,
    bus: Arc<LocalBus>,
    db: PgPool,
}

fn meta() -> RequestMeta {
    RequestMeta {
        ip: IpAddr::V4(Ipv4Addr::new(10, 1, 1, 1)),
        user_agent: None,
    }
}

fn harness(db: PgPool) -> Harness {
    let mail = Arc::new(Mail::default());
    let bus = Arc::new(LocalBus::new(64));
    let auth = AuthState::new(
        db.clone(),
        Arc::new(Snowflake::new(1).unwrap()),
        mail.clone(),
        AuthSettings {
            cookie_name: "ik_session".into(),
            cookie_secure: false,
            session_ttl: chrono::Duration::days(30),
            session_max_lifetime: chrono::Duration::days(180),
            registration_open: true,
            trusted_proxy_hops: 0,
            code_key: vec![3; 32],
        },
        bus.clone(),
    );
    let api = Arc::new(FakeApi::default());
    let mihoyo = MihoyoState::new(auth.clone(), api.clone(), true, vec![5; 32]);
    Harness {
        auth,
        mihoyo,
        api,
        mail,
        bus,
        db,
    }
}

impl Harness {
    async fn principal(&self, token: &str) -> Principal {
        match ik_auth::service::resolve(&self.auth, token).await.unwrap() {
            Resolution::Active(p) => p,
            other => panic!("令牌应当有效：{other:?}"),
        }
    }

    async fn register(&self, email: &str) -> (Principal, String) {
        let request: SendCodeRequest =
            serde_json::from_value(serde_json::json!({ "email": email, "purpose": "register" }))
                .unwrap();
        ik_auth::service::send_code(&self.auth, request, &meta())
            .await
            .unwrap();
        let issued = ik_auth::service::register(
            &self.auth,
            RegisterRequest {
                email: email.into(),
                code: self.mail.last_code(email),
                password: "secret-1".into(),
                client: ClientKind::App,
            },
            &meta(),
        )
        .await
        .unwrap();
        (self.principal(&issued.token).await, issued.token)
    }

    /// 扫码核验 / 登录：二维码确认为米游社账号 `aid`，返回 (qrId, 轮询结果)。
    async fn scan(
        &self,
        viewer: Option<&Principal>,
        mode: QrMode,
        aid: &str,
    ) -> (String, ik_mihoyo::service::Polled) {
        let created =
            ik_mihoyo::service::create_qr(&self.mihoyo, viewer, CreateQrRequest { mode }, &meta())
                .await
                .unwrap();
        let ticket = created.url.rsplit('=').next().unwrap().to_owned();
        self.api.confirms.lock().insert(ticket, aid.to_owned());
        let polled = ik_mihoyo::service::poll(
            &self.mihoyo,
            viewer,
            &PollQrRequest {
                qr_id: created.qr_id.clone(),
                client: ClientKind::App,
            },
            &meta(),
        )
        .await
        .unwrap();
        (created.qr_id, polled)
    }

    async fn status(&self, user_id: i64) -> (String, String, Option<String>) {
        let row = sqlx::query!(
            "SELECT status, username, email FROM users WHERE id = $1",
            user_id
        )
        .fetch_one(&self.db)
        .await
        .unwrap();
        (row.status, row.username, row.email)
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn delete_with_email_code_anonymizes_and_signs_out(db: PgPool) {
    let h = harness(db);
    let (me, token) = h.register("dora@example.com").await;
    let user_id = me.user_id.get();
    sqlx::query!(
        "INSERT INTO user_profiles (user_id, bio, pinned_post_ids) VALUES ($1, '签名', '{}');
         ",
        user_id
    )
    .execute(&h.db)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO mihoyo_bindings (user_id, aid) VALUES ($1, 'aid-dora')",
        user_id
    )
    .execute(&h.db)
    .await
    .unwrap();

    let view = service::security(&h.auth, &me).await.unwrap();
    assert_eq!(view.email.as_deref(), Some("dora@example.com"));
    assert!(view.has_password && view.mihoyo_bound);
    assert_eq!(view.delete_method, DeleteMethod::EmailCode);

    let request = |code: Option<String>| DeleteAccountRequest { code, qr_id: None };
    let err = service::delete_account(&h.auth, &me, request(None))
        .await
        .unwrap_err();
    assert_eq!(err.field_errors()[0].field, "code");
    service::send_delete_code(&h.auth, &me, &meta())
        .await
        .unwrap();
    assert!(h.mail.last("dora@example.com").text.contains("不可撤回"));
    let err = service::delete_account(&h.auth, &me, request(Some("000000".into())))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "CODE_INVALID");
    assert_eq!(h.status(user_id).await.0, "active", "核验失败不动账号");

    let mut events = h.bus.subscribe();
    let code = h.mail.last_code("dora@example.com");
    service::delete_account(&h.auth, &me, request(Some(code)))
        .await
        .unwrap();

    let (status, username, email) = h.status(user_id).await;
    assert_eq!(status, "deleted");
    assert_eq!(username, format!("已注销用户_{user_id}"));
    assert!(email.is_none());
    assert!(matches!(
        ik_auth::service::resolve(&h.auth, &token).await.unwrap(),
        Resolution::Invalid | Resolution::Blocked
    ));
    let leftovers = sqlx::query!(
        r#"SELECT
             (SELECT count(*) FROM user_passwords WHERE user_id = $1) AS "passwords!",
             (SELECT count(*) FROM mihoyo_bindings WHERE user_id = $1) AS "bindings!",
             (SELECT count(*) FROM sessions WHERE user_id = $1 AND revoked_at IS NULL) AS "sessions!",
             (SELECT bio FROM user_profiles WHERE user_id = $1) AS "bio!",
             (SELECT hidden FROM user_profiles WHERE user_id = $1) AS "hidden!""#,
        user_id
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(
        (leftovers.passwords, leftovers.bindings, leftovers.sessions),
        (0, 0, 0)
    );
    assert_eq!(leftovers.bio, "");
    assert!(leftovers.hidden);
    let kinds: Vec<&str> = std::iter::from_fn(|| events.try_recv().ok())
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        kinds,
        [ik_auth::service::SESSIONS_CHANGED, internal::USER_RENAMED]
    );

    // 邮箱释放了，可以重新注册。
    h.register("dora@example.com").await;
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn emailless_accounts_delete_by_scanning_their_own_mihoyo(db: PgPool) {
    let h = harness(db);
    let (_, login) = h.scan(None, QrMode::Login, "aid-eve").await;
    let issued = login.issued.unwrap();
    let me = h.principal(&issued.token).await;
    let user_id = me.user_id.get();
    let view = service::security(&h.auth, &me).await.unwrap();
    assert!(view.email.is_none() && !view.has_password && view.mihoyo_bound);
    assert_eq!(view.delete_method, DeleteMethod::MihoyoQr);
    let err = service::send_delete_code(&h.auth, &me, &meta())
        .await
        .unwrap_err();
    assert_eq!(err.code(), "DELETE_ACCOUNT_NO_EMAIL");

    let by_qr = |qr_id: &str| DeleteAccountRequest {
        code: None,
        qr_id: Some(qr_id.to_owned()),
    };
    // 扫的不是自己绑定的米游社账号：核验不通过，也不建号。
    let users_before = sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM users"#)
        .fetch_one(&h.db)
        .await
        .unwrap();
    let (wrong, polled) = h.scan(Some(&me), QrMode::Delete, "aid-stranger").await;
    assert_eq!(polled.matched, Some(false));
    assert_eq!(
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM users"#)
            .fetch_one(&h.db)
            .await
            .unwrap(),
        users_before
    );
    let err = service::delete_account(&h.auth, &me, by_qr(&wrong))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "MIHOYO_DELETE_ACCOUNT_MISMATCH");
    // 登录用的二维码不能拿来注销。
    let (login_qr, _) = h.scan(None, QrMode::Login, "aid-eve").await;
    let err = service::delete_account(&h.auth, &me, by_qr(&login_qr))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "MIHOYO_DELETE_TICKET_INVALID");
    let err = service::delete_account(&h.auth, &me, by_qr("nonexistent"))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "MIHOYO_DELETE_TICKET_EXPIRED");

    // 别人发起的核验不能用。
    let (other, other_token) = h.register("frank@example.com").await;
    let (theirs, _) = h.scan(Some(&other), QrMode::Delete, "aid-eve").await;
    let err = service::delete_account(&h.auth, &me, by_qr(&theirs))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "MIHOYO_BIND_USER_MISMATCH");
    drop(other_token);

    let (good, polled) = h.scan(Some(&me), QrMode::Delete, "aid-eve").await;
    assert_eq!(polled.matched, Some(true));
    service::delete_account(&h.auth, &me, by_qr(&good))
        .await
        .unwrap();
    assert_eq!(h.status(user_id).await.0, "deleted");
    // 绑定删了：同一个米游社账号再扫码登录是一个新账号。
    let (_, again) = h.scan(None, QrMode::Login, "aid-eve").await;
    assert_eq!(again.is_new_user, Some(true));
    assert_ne!(again.issued.unwrap().user.id.get(), user_id);
}
