//! 米游社扫码登录与绑定：用假的上游跑完整流程。

// clippy.toml 的测试豁免只覆盖 #[test] 函数本身，不覆盖集成测试里的辅助函数。
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use ik_auth::service::RequestMeta;
use ik_auth::{AuthSettings, AuthState, Principal};
use ik_kernel::bus::LocalBus;
use ik_kernel::mail::DevLogMailer;
use ik_kernel::{Id, Role, Snowflake};
use ik_mihoyo::MihoyoState;
use ik_mihoyo::dto::{CreateQrRequest, PollQrRequest, QrMode, QrStatus};
use ik_mihoyo::service::{self, Polled};
use ik_mihoyo::upstream::{
    self, Confirmed, Cookies, Device, GameRole, MihoyoApi, QrCode, UpstreamError,
};
use parking_lot::Mutex;
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

/// 按 ticket 返回预设状态的上游。
#[derive(Default)]
struct FakeApi {
    statuses: Mutex<HashMap<String, upstream::QrStatus>>,
    roles: Mutex<Option<Vec<GameRole>>>,
    devices: Mutex<HashMap<String, Device>>,
    created: AtomicUsize,
    queries: AtomicUsize,
}

#[async_trait]
impl MihoyoApi for FakeApi {
    async fn create_qr(&self, device: &Device) -> Result<QrCode, UpstreamError> {
        let n = self.created.fetch_add(1, Ordering::SeqCst);
        let ticket = format!("ticket-{n}");
        self.statuses
            .lock()
            .insert(ticket.clone(), upstream::QrStatus::Created);
        self.devices.lock().insert(ticket.clone(), device.clone());
        Ok(QrCode {
            url: format!("https://user.mihoyo.com/qr?ticket={ticket}"),
            ticket,
            expires_at: None,
        })
    }

    async fn query_qr(
        &self,
        ticket: &str,
        device: &Device,
    ) -> Result<upstream::QrStatus, UpstreamError> {
        self.queries.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            self.devices.lock().get(ticket),
            Some(device),
            "轮询的设备必须与创建时一致"
        );
        Ok(self
            .statuses
            .lock()
            .get(ticket)
            .cloned()
            .unwrap_or(upstream::QrStatus::Expired))
    }

    async fn game_roles(&self, cookies: &Cookies) -> Result<Vec<GameRole>, UpstreamError> {
        assert_eq!(cookies.0[0].0, "ltoken_v2");
        self.roles
            .lock()
            .clone()
            .ok_or(UpstreamError::Malformed("角色接口挂了"))
    }
}

struct Harness {
    state: MihoyoState,
    api: Arc<FakeApi>,
    db: PgPool,
}

fn harness_with(db: PgPool, registration_open: bool) -> Harness {
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let auth = AuthState::new(
        db.clone(),
        ids,
        Arc::new(DevLogMailer),
        AuthSettings {
            cookie_name: "ik_session".into(),
            cookie_secure: false,
            session_ttl: chrono::Duration::days(30),
            session_max_lifetime: chrono::Duration::days(180),
            registration_open,
            trusted_proxy_hops: 0,
            code_key: vec![1; 32],
        },
        Arc::new(LocalBus::new(16)),
    );
    let api = Arc::new(FakeApi::default());
    *api.roles.lock() = Some(vec![role("10000001", "邦布", 45)]);
    Harness {
        state: MihoyoState::new(auth, api.clone(), true, vec![9; 32]),
        api,
        db,
    }
}

fn harness(db: PgPool) -> Harness {
    harness_with(db, true)
}

fn role(uid: &str, nickname: &str, level: i32) -> GameRole {
    GameRole {
        game_uid: uid.into(),
        nickname: nickname.into(),
        level,
        region: "prod_gf_cn".into(),
        region_name: "新艾利都".into(),
        is_chosen: false,
    }
}

fn meta() -> RequestMeta {
    RequestMeta {
        ip: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
        user_agent: Some("Mozilla/5.0 (Windows NT 10.0) Chrome/124.0".into()),
    }
}

fn principal(user_id: i64, session_id: i64) -> Principal {
    Principal {
        user_id: Id::new(user_id),
        session_id: Id::new(session_id),
        role: Role::User,
        exam_passed: false,
        is_ai_agent: false,
        level: 0,
    }
}

impl Harness {
    async fn qr(&self, viewer: Option<&Principal>, mode: QrMode) -> (String, String) {
        let created = service::create_qr(&self.state, viewer, CreateQrRequest { mode }, &meta())
            .await
            .unwrap();
        assert_eq!(created.expires_in, 180);
        let ticket = created.url.rsplit('=').next().unwrap().to_owned();
        (created.qr_id, ticket)
    }

    fn confirm(&self, ticket: &str, aid: &str, mid: Option<&str>) {
        self.api.statuses.lock().insert(
            ticket.to_owned(),
            upstream::QrStatus::Confirmed(Confirmed {
                aid: aid.into(),
                mid: mid.map(Into::into),
                cookies: Cookies(vec![("ltoken_v2".into(), "secret".into())]),
            }),
        );
    }

    async fn poll(
        &self,
        viewer: Option<&Principal>,
        qr_id: &str,
    ) -> Result<Polled, ik_kernel::Problem> {
        // 跳过 2 秒的状态微缓存。
        sqlx::query!("UPDATE mihoyo_qr_logins SET last_polled_at = now() - interval '5 seconds'")
            .execute(&self.db)
            .await
            .unwrap();
        let request = PollQrRequest {
            qr_id: qr_id.to_owned(),
            client: ik_auth::dto::ClientKind::App,
        };
        service::poll(&self.state, viewer, &request, &meta()).await
    }

    async fn user_count(&self) -> i64 {
        sqlx::query_scalar!(r#"SELECT count(*) AS "n!" FROM users"#)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    async fn user_with_email(&self, id: i64) -> Principal {
        sqlx::query!(
            "INSERT INTO users (id, username, email) VALUES ($1, $2, $3)",
            id,
            format!("user{id}"),
            format!("user{id}@example.com")
        )
        .execute(&self.db)
        .await
        .unwrap();
        principal(id, id)
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn first_login_creates_an_account_and_replays_the_same_session(db: PgPool) {
    let h = harness(db);
    let (qr_id, ticket) = h.qr(None, QrMode::Login).await;
    assert_eq!(
        h.poll(None, &qr_id).await.unwrap().status,
        QrStatus::Waiting
    );
    // 2 秒内重复轮询不查上游。
    let queries = h.api.queries.load(Ordering::SeqCst);
    let cached = service::poll(
        &h.state,
        None,
        &PollQrRequest {
            qr_id: qr_id.clone(),
            client: ik_auth::dto::ClientKind::App,
        },
        &meta(),
    )
    .await
    .unwrap();
    assert_eq!(cached.status, QrStatus::Waiting);
    assert_eq!(h.api.queries.load(Ordering::SeqCst), queries);

    h.confirm(&ticket, "aid-1", Some("mid-1"));
    let first = h.poll(None, &qr_id).await.unwrap();
    assert_eq!(first.status, QrStatus::Confirmed);
    assert_eq!(first.mode, Some(QrMode::Login));
    assert_eq!(first.is_new_user, Some(true));
    let issued = first.issued.unwrap();
    assert_eq!(issued.user.username, "邦布", "用绝区零昵称当用户名");
    assert!(issued.user.email.is_none(), "米游社首登没有邮箱（D-069）");
    assert_eq!(issued.session.login_method, "mihoyo");
    let zzz = first.binding.unwrap().zzz.unwrap();
    assert_eq!((zzz.uid.as_str(), zzz.level), ("10000001", 45));

    // 之后的轮询重放同一个结果、同一个会话，不再查上游。
    let queries = h.api.queries.load(Ordering::SeqCst);
    let again = h.poll(None, &qr_id).await.unwrap();
    let again_issued = again.issued.unwrap();
    assert_eq!(again_issued.token, issued.token);
    assert_eq!(again_issued.session.id, issued.session.id);
    assert_eq!(h.api.queries.load(Ordering::SeqCst), queries);
    assert_eq!(h.user_count().await, 1);

    // 登出之后，这个二维码不能再兑换出新会话。
    let user_id = issued.user.id.get();
    ik_auth::service::logout(&h.state.auth, &principal(user_id, issued.session.id.get()))
        .await
        .unwrap();
    let err = h.poll(None, &qr_id).await.unwrap_err();
    assert_eq!(err.code(), "QR_SESSION_CONSUMED");

    // 再扫一次：登录同一个账号，同步角色，昵称冲突不影响（不建号）。
    *h.api.roles.lock() = Some(vec![role("10000001", "邦布", 50), role("2", "小号", 60)]);
    let (qr_id, ticket) = h.qr(None, QrMode::Login).await;
    h.confirm(&ticket, "aid-1", None);
    let second = h.poll(None, &qr_id).await.unwrap();
    assert_eq!(second.is_new_user, Some(false));
    assert_eq!(second.issued.unwrap().user.id.get(), user_id);
    assert_eq!(
        second.binding.unwrap().zzz.unwrap().uid,
        "2",
        "没有 is_chosen 时取等级最高的"
    );
    assert_eq!(h.user_count().await, 1);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn concurrent_confirmed_polls_create_one_account(db: PgPool) {
    let h = harness(db);
    let (qr_id, ticket) = h.qr(None, QrMode::Login).await;
    h.confirm(&ticket, "aid-race", None);
    sqlx::query!("UPDATE mihoyo_qr_logins SET last_polled_at = NULL")
        .execute(&h.db)
        .await
        .unwrap();
    let request = PollQrRequest {
        qr_id,
        client: ik_auth::dto::ClientKind::App,
    };
    let meta = meta();
    let polls = (0..8).map(|_| service::poll(&h.state, None, &request, &meta));
    let results = futures_util::future::join_all(polls).await;
    let mut tokens = Vec::new();
    for result in results {
        let polled = result.unwrap();
        match polled.status {
            QrStatus::Confirmed => tokens.push(polled.issued.unwrap().token),
            QrStatus::Scanned => {}
            other => panic!("意外的状态 {other:?}"),
        }
    }
    assert!(!tokens.is_empty());
    assert!(
        tokens.windows(2).all(|w| w[0] == w[1]),
        "所有确认的轮询拿到同一个会话"
    );
    assert_eq!(h.user_count().await, 1);

    // 两个不同的二维码同时首登同一个米游社账号：也只建一个号。
    let (a, ta) = h.qr(None, QrMode::Login).await;
    let (b, tb) = h.qr(None, QrMode::Login).await;
    h.confirm(&ta, "aid-twice", None);
    h.confirm(&tb, "aid-twice", None);
    let (ra, rb) = tokio::join!(h.poll(None, &a), h.poll(None, &b));
    let (ra, rb) = (ra.unwrap(), rb.unwrap());
    assert_eq!(ra.issued.unwrap().user.id, rb.issued.unwrap().user.id);
    assert_eq!(h.user_count().await, 2);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn closed_registration_blocked_accounts_and_upstream_endings(db: PgPool) {
    let h = harness_with(db, false);
    let (qr_id, ticket) = h.qr(None, QrMode::Login).await;
    h.confirm(&ticket, "aid-new", None);
    assert_eq!(
        h.poll(None, &qr_id).await.unwrap_err().code(),
        "REGISTRATION_CLOSED"
    );
    // 失败结果也重放（MHY-008），不会变成「二维码已过期」。
    assert_eq!(
        h.poll(None, &qr_id).await.unwrap_err().code(),
        "REGISTRATION_CLOSED"
    );
    assert_eq!(h.user_count().await, 0);

    // 已绑定的账号照常登录（注册开关只管建号）；角色接口挂了也能登录，只是没有角色资料。
    let me = h.user_with_email(1).await;
    sqlx::query!("INSERT INTO mihoyo_bindings (user_id, aid) VALUES (1, 'aid-old')")
        .execute(&h.db)
        .await
        .unwrap();
    *h.api.roles.lock() = None;
    let (qr_id, ticket) = h.qr(None, QrMode::Login).await;
    h.confirm(&ticket, "aid-old", None);
    let polled = h.poll(None, &qr_id).await.unwrap();
    assert_eq!(polled.issued.unwrap().user.id, me.user_id);
    assert!(polled.binding.unwrap().zzz.is_none());

    // 封禁的账号不能扫码登录。
    sqlx::query!("UPDATE users SET status = 'blocked' WHERE id = 1")
        .execute(&h.db)
        .await
        .unwrap();
    let (qr_id, ticket) = h.qr(None, QrMode::Login).await;
    h.confirm(&ticket, "aid-old", None);
    assert_eq!(
        h.poll(None, &qr_id).await.unwrap_err().code(),
        "ACCOUNT_BLOCKED"
    );

    // 上游说过期 / 取消：扫码会话随之删除。
    let (qr_id, ticket) = h.qr(None, QrMode::Login).await;
    h.api
        .statuses
        .lock()
        .insert(ticket, upstream::QrStatus::Cancelled);
    assert_eq!(
        h.poll(None, &qr_id).await.unwrap().status,
        QrStatus::Cancelled
    );
    assert_eq!(
        h.poll(None, &qr_id).await.unwrap().status,
        QrStatus::Expired
    );
    assert_eq!(
        h.poll(None, "not-a-real-qr-id").await.unwrap().status,
        QrStatus::Expired
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn bind_rules_and_unbind(db: PgPool) {
    let h = harness(db);
    let alice = h.user_with_email(1).await;
    let bob = h.user_with_email(2).await;

    let err = service::create_qr(
        &h.state,
        None,
        CreateQrRequest { mode: QrMode::Bind },
        &meta(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "MIHOYO_BIND_REQUIRES_AUTH");

    let (qr_id, ticket) = h.qr(Some(&alice), QrMode::Bind).await;
    // 只有发起绑定的账号能轮询。
    assert_eq!(
        h.poll(Some(&bob), &qr_id).await.unwrap_err().code(),
        "MIHOYO_BIND_USER_MISMATCH"
    );
    assert_eq!(
        h.poll(None, &qr_id).await.unwrap_err().code(),
        "MIHOYO_BIND_USER_MISMATCH"
    );
    h.confirm(&ticket, "aid-a", Some("mid-a"));
    let bound = h.poll(Some(&alice), &qr_id).await.unwrap();
    assert_eq!(bound.mode, Some(QrMode::Bind));
    assert!(bound.issued.is_none(), "绑定不签发会话");
    assert_eq!(bound.binding.unwrap().aid, "aid-a");

    // 同一个米游社账号不能再绑到 bob 上；alice 也不能再绑另一个。
    let (qr_id, ticket) = h.qr(Some(&bob), QrMode::Bind).await;
    h.confirm(&ticket, "aid-a", None);
    assert_eq!(
        h.poll(Some(&bob), &qr_id).await.unwrap_err().code(),
        "IDENTITY_ALREADY_BOUND"
    );
    let (qr_id, ticket) = h.qr(Some(&bob), QrMode::Bind).await;
    h.confirm(&ticket, "aid-b", Some("mid-a"));
    assert_eq!(
        h.poll(Some(&bob), &qr_id).await.unwrap_err().code(),
        "IDENTITY_ALREADY_BOUND",
        "mid 指向别人也算"
    );
    let (qr_id, ticket) = h.qr(Some(&alice), QrMode::Bind).await;
    h.confirm(&ticket, "aid-c", None);
    assert_eq!(
        h.poll(Some(&alice), &qr_id).await.unwrap_err().code(),
        "USER_ALREADY_BOUND"
    );
    // 重复绑定同一个是幂等的。
    let (qr_id, ticket) = h.qr(Some(&alice), QrMode::Bind).await;
    h.confirm(&ticket, "aid-a", None);
    assert!(h.poll(Some(&alice), &qr_id).await.is_ok());

    assert_eq!(
        service::my_binding(&h.state, &alice)
            .await
            .unwrap()
            .binding
            .unwrap()
            .aid,
        "aid-a"
    );
    service::unbind(&h.state, &alice).await.unwrap();
    assert!(
        service::my_binding(&h.state, &alice)
            .await
            .unwrap()
            .binding
            .is_none()
    );
    assert_eq!(
        service::unbind(&h.state, &alice).await.unwrap_err().code(),
        "MIHOYO_NOT_BOUND"
    );

    // 扫码建的号没有邮箱，不能解绑。
    let (qr_id, ticket) = h.qr(None, QrMode::Login).await;
    h.confirm(&ticket, "aid-z", None);
    let issued = h.poll(None, &qr_id).await.unwrap().issued.unwrap();
    let qr_user = principal(issued.user.id.get(), issued.session.id.get());
    assert_eq!(
        service::unbind(&h.state, &qr_user)
            .await
            .unwrap_err()
            .code(),
        "MIHOYO_UNBIND_FORBIDDEN"
    );

    // 过期的扫码会话由定时任务清理。
    sqlx::query!(
        "UPDATE mihoyo_qr_logins SET expires_at = now() - interval '1 hour',
                result_expires_at = CASE WHEN result IS NULL THEN NULL ELSE now() - interval '1 hour' END"
    )
    .execute(&h.db)
    .await
    .unwrap();
    assert!(service::prune(&h.state).await.unwrap() > 0);
}
