//! 举报与审核：真实数据库，经 content / comments / auth / notify 完成处置。

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use ik_auth::Principal;
use ik_comments::CommentsState;
use ik_comments::dto::CreateCommentRequest;
use ik_content::ContentState;
use ik_content::dto::PostInput;
use ik_kernel::bus::LocalBus;
use ik_kernel::{Id, Role, Snowflake};
use ik_moderation::dto::{
    Action, CheckQuery, CreateReportRequest, GroupQuery, PageQuery, Reason, ReportStatus,
    Resolution, ResolveRequest, Tab, TargetType,
};
use ik_moderation::{ModerationState, admin, service};
use sqlx::PgPool;

fn operator(principal: &Principal) -> ik_auth::Operator {
    ik_auth::Operator {
        principal: principal.clone(),
        meta: ik_auth::service::RequestMeta {
            ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            user_agent: None,
        },
    }
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

struct Harness {
    state: ModerationState,
    content: ContentState,
    comments: CommentsState,
    bus: Arc<LocalBus>,
    db: PgPool,
}

fn harness(db: PgPool, threshold: i64) -> Harness {
    let ids = Arc::new(Snowflake::new(1).unwrap());
    let bus = Arc::new(LocalBus::new(256));
    let media = ik_media::MediaState::for_tests(db.clone(), ids.clone());
    Harness {
        state: ModerationState::new(
            db.clone(),
            ids.clone(),
            bus.clone(),
            media.clone(),
            threshold,
        ),
        content: ContentState::new(db.clone(), ids.clone(), bus.clone(), media.clone()),
        comments: CommentsState::new(db.clone(), ids, bus.clone(), media),
        bus,
        db,
    }
}

fn principal(id: i64, role: Role) -> Principal {
    Principal {
        user_id: Id::new(id),
        session_id: Id::new(id),
        role,
        exam_passed: true,
        is_ai_agent: false,
        level: 1,
    }
}

fn report(
    target_type: TargetType,
    id: i64,
    reason: Reason,
    detail: Option<&str>,
) -> CreateReportRequest {
    CreateReportRequest {
        target_type,
        target_id: Id::new(id),
        reason,
        detail: detail.map(Into::into),
    }
}

fn resolve(action: Action, note: Option<&str>, target_note: Option<&str>) -> ResolveRequest {
    ResolveRequest {
        action,
        note: note.map(Into::into),
        target_note: target_note.map(Into::into),
    }
}

impl Harness {
    async fn user(&self, id: i64, role: Role) -> Principal {
        sqlx::query!(
            "INSERT INTO users (id, username, email, role, level, exam_passed_at)
             VALUES ($1, $2, $3, $4, 1, now())",
            id,
            format!("user{id}"),
            format!("user{id}@example.com"),
            role as Role,
        )
        .execute(&self.db)
        .await
        .unwrap();
        principal(id, role)
    }

    async fn post(&self, author: &Principal, title: &str, anonymous: bool) -> i64 {
        ik_content::service::create(
            &self.content,
            author,
            PostInput {
                title: title.into(),
                body: "正文内容".into(),
                category_id: None,
                tags: Vec::new(),
                is_anonymous: anonymous,
                external_videos: Vec::new(),
                editor_state: None,
                cover_ids: Vec::new(),
            },
            true,
        )
        .await
        .unwrap()
        .id
        .get()
    }

    async fn comment(&self, author: &Principal, post: i64) -> i64 {
        ik_comments::service::create(
            &self.comments,
            author,
            post,
            CreateCommentRequest {
                body: "一条评论".into(),
                reply_to_id: None,
                is_anonymous: false,
                image_ids: Vec::new(),
            },
        )
        .await
        .unwrap()
        .id
        .get()
    }

    async fn notices(&self, recipient: i64) -> Vec<String> {
        sqlx::query_scalar!(
            "SELECT excerpt FROM notifications WHERE recipient_id = $1 AND kind = 'system' ORDER BY id",
            recipient
        )
        .fetch_all(&self.db)
        .await
        .unwrap()
    }

    async fn post_public(&self, id: i64) -> Option<bool> {
        let mut conn = self.db.acquire().await.unwrap();
        ik_content::interop::reportable_post(&mut conn, id)
            .await
            .unwrap()
            .map(|p| p.is_public)
    }

    async fn statuses(&self, target_type: TargetType, id: i64) -> Vec<String> {
        sqlx::query_scalar!(
            "SELECT status FROM reports WHERE target_type = $1 AND target_id = $2 ORDER BY id",
            target_type.as_str(),
            id
        )
        .fetch_all(&self.db)
        .await
        .unwrap()
    }
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn reporting_rules(db: PgPool) {
    let h = harness(db, 5);
    let author = h.user(1, Role::User).await;
    let me = h.user(2, Role::User).await;
    let post = h.post(&author, "被举报的帖子", false).await;

    let err = service::report(
        &h.state,
        &author,
        report(TargetType::Post, post, Reason::Spam, None),
    )
    .await
    .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "SELF");
    let err = service::report(
        &h.state,
        &me,
        report(TargetType::Post, post, Reason::Other, Some("  ")),
    )
    .await
    .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "REQUIRED");
    let long = "长".repeat(501);
    let err = service::report(
        &h.state,
        &me,
        report(TargetType::Post, post, Reason::Spam, Some(&long)),
    )
    .await
    .unwrap_err();
    assert_eq!(err.field_errors()[0].code, "TOO_LONG");
    let err = service::report(
        &h.state,
        &me,
        report(TargetType::Post, 999, Reason::Spam, None),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND");

    let created = service::report(
        &h.state,
        &me,
        report(TargetType::Post, post, Reason::Spam, Some(" 广告 ")),
    )
    .await
    .unwrap();
    assert_eq!(created.status, ReportStatus::Pending);
    let err = service::report(
        &h.state,
        &me,
        report(TargetType::Post, post, Reason::Abuse, None),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "REPORT_DUPLICATE");

    // 用户目标：封禁的可以举报，注销的不行（D-063 方案 A）。
    h.user(3, Role::User).await;
    h.user(4, Role::User).await;
    sqlx::query!("UPDATE users SET status = 'blocked' WHERE id = 3; ")
        .execute(&h.db)
        .await
        .unwrap();
    sqlx::query!("UPDATE users SET status = 'deleted' WHERE id = 4")
        .execute(&h.db)
        .await
        .unwrap();
    service::report(
        &h.state,
        &me,
        report(TargetType::User, 3, Reason::Abuse, None),
    )
    .await
    .unwrap();
    let err = service::report(
        &h.state,
        &me,
        report(TargetType::User, 4, Reason::Abuse, None),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "NOT_FOUND");

    // 每分钟 3 次：前面失败的不算，已经成功了 2 次。
    let comment = h.comment(&author, post).await;
    service::report(
        &h.state,
        &me,
        report(TargetType::Comment, comment, Reason::Spam, None),
    )
    .await
    .unwrap();
    let other = h.post(&author, "另一篇", false).await;
    let err = service::report(
        &h.state,
        &me,
        report(TargetType::Post, other, Reason::Spam, None),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "RATE_LIMITED");

    let checked = service::check(
        &h.state,
        &me,
        CheckQuery {
            target_type: TargetType::Post,
            ids: format!("{post}, {other},abc"),
        },
    )
    .await
    .unwrap();
    assert!(checked.reported[&post.to_string()]);
    assert!(!checked.reported[&other.to_string()]);
    let mine = service::mine(
        &h.state,
        &me,
        PageQuery {
            cursor: None,
            limit: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(mine.items.len(), 3);
    assert_eq!(mine.items[2].target.title.as_deref(), Some("被举报的帖子"));
    assert_eq!(mine.items[2].detail.as_deref(), Some("广告"));
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn auto_hide_then_reject_restores_the_post(db: PgPool) {
    let h = harness(db, 3);
    let author = h.user(1, Role::User).await;
    let staff = h.user(9, Role::Moderator).await;
    let post = h.post(&author, "热门帖", false).await;
    for id in 2..=4 {
        let reporter = h.user(id, Role::User).await;
        service::report(
            &h.state,
            &reporter,
            report(TargetType::Post, post, Reason::Spam, None),
        )
        .await
        .unwrap();
    }
    assert_eq!(
        h.post_public(post).await,
        Some(false),
        "第三个人举报后自动隐藏"
    );
    assert_eq!(h.statuses(TargetType::Post, post).await, ["reviewing"; 3]);
    assert_eq!(
        h.notices(1).await,
        ["你的委托《热门帖》因收到多人举报已被暂时隐藏，等待管理员复核。"]
    );
    // 已隐藏的不能再举报。
    let late = h.user(5, Role::User).await;
    assert_eq!(
        service::report(
            &h.state,
            &late,
            report(TargetType::Post, post, Reason::Spam, None)
        )
        .await
        .unwrap_err()
        .code(),
        "NOT_FOUND"
    );

    // 自动隐藏转入的复核中没有领取人，可以领取；驳回恢复可见。
    let claimed = admin::claim(&h.state, &staff, TargetType::Post, post)
        .await
        .unwrap();
    assert_eq!(claimed.claimed_count, 3);
    let resolved = admin::resolve(
        &h.state,
        &staff,
        TargetType::Post,
        post,
        resolve(Action::Reject, Some("没问题"), None),
    )
    .await
    .unwrap();
    assert_eq!(resolved.resolved_count, 3);
    assert_eq!(h.post_public(post).await, Some(true));
    assert_eq!(h.statuses(TargetType::Post, post).await, ["rejected"; 3]);
    assert_eq!(
        h.notices(2).await,
        ["你举报的委托《热门帖》经核实暂未发现违规，感谢你维护绳网环境。\n处理备注：没问题"]
    );
    assert_eq!(h.notices(1).await.len(), 1, "驳回不通知被举报人");
    let err = admin::resolve(
        &h.state,
        &staff,
        TargetType::Post,
        post,
        resolve(Action::Reject, None, None),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "REPORT_CLOSED");
    let actions = sqlx::query_scalar!(
        "SELECT action FROM moderation_log WHERE target_id = $1 ORDER BY id",
        post
    )
    .fetch_all(&h.db)
    .await
    .unwrap();
    assert_eq!(actions, ["auto_hide", "reject"]);
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn claims_are_exclusive(db: PgPool) {
    let h = harness(db, 5);
    let author = h.user(1, Role::User).await;
    let reporter = h.user(2, Role::User).await;
    let alice = h.user(8, Role::Moderator).await;
    let bob = h.user(9, Role::Admin).await;
    let post = h.post(&author, "帖子", false).await;
    service::report(
        &h.state,
        &reporter,
        report(TargetType::Post, post, Reason::Spam, None),
    )
    .await
    .unwrap();

    assert_eq!(
        admin::claim(&h.state, &reporter, TargetType::Post, post)
            .await
            .unwrap_err()
            .code(),
        "FORBIDDEN"
    );
    let (a, b) = tokio::join!(
        admin::claim(&h.state, &alice, TargetType::Post, post),
        admin::claim(&h.state, &bob, TargetType::Post, post),
    );
    let winner = match (&a, &b) {
        (Ok(r), Err(e)) | (Err(e), Ok(r)) => {
            assert_eq!(r.claimed_count, 1);
            assert_eq!(e.code(), "REPORT_ALREADY_CLAIMED");
            if a.is_ok() { &alice } else { &bob }
        }
        other => panic!("应当恰好一人领取成功：{other:?}"),
    };
    let loser = if std::ptr::eq(winner, &alice) {
        &bob
    } else {
        &alice
    };
    // 领取人重复领取是幂等的；别人不能结案。
    assert_eq!(
        admin::claim(&h.state, winner, TargetType::Post, post)
            .await
            .unwrap()
            .claimed_count,
        0
    );
    assert_eq!(
        admin::resolve(
            &h.state,
            loser,
            TargetType::Post,
            post,
            resolve(Action::WarnUser, None, None)
        )
        .await
        .unwrap_err()
        .code(),
        "REPORT_ALREADY_CLAIMED"
    );
    // 别人释放不了；领取人释放后别人可以领取。
    assert_eq!(
        admin::unclaim(&h.state, loser, TargetType::Post, post)
            .await
            .unwrap()
            .released_count,
        0
    );
    assert_eq!(
        admin::unclaim(&h.state, winner, TargetType::Post, post)
            .await
            .unwrap()
            .released_count,
        1
    );
    assert_eq!(
        admin::claim(&h.state, loser, TargetType::Post, post)
            .await
            .unwrap()
            .claimed_count,
        1
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn resolve_actions(db: PgPool) {
    let h = harness(db, 5);
    let author = h.user(1, Role::User).await;
    let reporter = h.user(2, Role::User).await;
    let staff = h.user(9, Role::Moderator).await;
    let other_staff = h.user(10, Role::Admin).await;
    let post = h.post(&author, "帖子", false).await;
    let comment = h.comment(&author, post).await;

    // 隐藏评论：评论不可见，作者与举报人收到附备注的通知。
    service::report(
        &h.state,
        &reporter,
        report(TargetType::Comment, comment, Reason::Abuse, None),
    )
    .await
    .unwrap();
    admin::resolve(
        &h.state,
        &staff,
        TargetType::Comment,
        comment,
        resolve(Action::HideContent, Some("已隐藏"), Some("注意言辞")),
    )
    .await
    .unwrap();
    let mut conn = h.db.acquire().await.unwrap();
    let hidden = ik_comments::interop::hidden_states(&mut conn, &[comment])
        .await
        .unwrap();
    assert!(hidden[&comment]);
    drop(conn);
    assert_eq!(
        h.notices(1).await,
        ["你的评论因违反绳网社区规范已被隐藏。如有异议请联系管理员。\n处理备注：注意言辞"]
    );
    assert_eq!(
        h.notices(2).await,
        ["你举报的评论已处理完毕，感谢你维护绳网环境！\n处理备注：已隐藏"]
    );

    // 删除帖子：帖子没了，举报成立。
    service::report(
        &h.state,
        &reporter,
        report(TargetType::Post, post, Reason::Spam, None),
    )
    .await
    .unwrap();
    admin::resolve(
        &h.state,
        &staff,
        TargetType::Post,
        post,
        resolve(Action::DeleteContent, None, None),
    )
    .await
    .unwrap();
    assert_eq!(h.post_public(post).await, None);
    let resolution = sqlx::query_scalar!(
        "SELECT resolution FROM reports WHERE target_type = 'post' AND target_id = $1",
        post
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(
        resolution.as_deref(),
        Some(Resolution::ContentDeleted.as_str())
    );

    // 对用户的举报：不能用内容动作；封禁立即下线；不能封禁工作人员；解封。
    service::report(
        &h.state,
        &reporter,
        report(TargetType::User, 1, Reason::Abuse, None),
    )
    .await
    .unwrap();
    let err = admin::resolve(
        &h.state,
        &staff,
        TargetType::User,
        1,
        resolve(Action::HideContent, None, None),
    )
    .await
    .unwrap_err();
    assert_eq!(err.field_errors()[0].field, "action");
    sqlx::query!(
        "INSERT INTO sessions (id, user_id, token_hash, login_method, expires_at, max_expires_at)
         VALUES (77, 1, '\\x01', 'password', now() + interval '1 day', now() + interval '1 day')"
    )
    .execute(&h.db)
    .await
    .unwrap();
    let mut events = h.bus.subscribe();
    admin::resolve(
        &h.state,
        &staff,
        TargetType::User,
        1,
        resolve(Action::BlockUser, None, None),
    )
    .await
    .unwrap();
    let status = sqlx::query_scalar!("SELECT status FROM users WHERE id = 1")
        .fetch_one(&h.db)
        .await
        .unwrap();
    assert_eq!(status, "blocked");
    let live = sqlx::query_scalar!(
        r#"SELECT count(*) AS "n!" FROM sessions WHERE user_id = 1 AND revoked_at IS NULL"#
    )
    .fetch_one(&h.db)
    .await
    .unwrap();
    assert_eq!(live, 0);
    let kinds: Vec<&str> = std::iter::from_fn(|| events.try_recv().ok())
        .map(|e| e.kind)
        .collect();
    assert!(kinds.contains(&ik_auth::service::SESSIONS_CHANGED));
    assert_eq!(
        h.notices(1).await.last().map(String::as_str),
        Some("你的账号因违反绳网社区规范已被封禁。如有异议请联系管理员。")
    );

    // 同一人一分钟只能举报 3 次，换个举报人。
    let second = h.user(3, Role::User).await;
    service::report(
        &h.state,
        &second,
        report(TargetType::User, 10, Reason::Abuse, None),
    )
    .await
    .unwrap();
    let err = admin::resolve(
        &h.state,
        &staff,
        TargetType::User,
        10,
        resolve(Action::BlockUser, None, None),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "CANNOT_SANCTION_STAFF");
    assert_eq!(
        h.statuses(TargetType::User, 10).await,
        ["pending"],
        "失败时整体回滚"
    );

    assert!(
        admin::unblock(&h.state, &operator(&other_staff), 1)
            .await
            .unwrap()
            .unblocked
    );
    assert!(
        !admin::unblock(&h.state, &operator(&other_staff), 1)
            .await
            .unwrap()
            .unblocked
    );
}

#[sqlx::test(migrator = "MIGRATOR")]
async fn grouped_list_filters_and_hides_anonymous_authors(db: PgPool) {
    let h = harness(db, 10);
    let author = h.user(1, Role::User).await;
    let staff = h.user(9, Role::Moderator).await;
    let named = h.post(&author, "实名帖子", false).await;
    let anon = h.post(&author, "匿名帖子", true).await;
    for id in 2..=4 {
        let reporter = h.user(id, Role::User).await;
        service::report(
            &h.state,
            &reporter,
            report(TargetType::Post, named, Reason::Spam, None),
        )
        .await
        .unwrap();
    }
    let reporter = principal(2, Role::User);
    service::report(
        &h.state,
        &reporter,
        report(TargetType::Post, anon, Reason::Porn, None),
    )
    .await
    .unwrap();

    let page = admin::list(&h.state, &staff, GroupQuery::default())
        .await
        .unwrap();
    assert_eq!(page.total, 2);
    let named_group = page
        .items
        .iter()
        .find(|g| g.target_id.get() == named)
        .unwrap();
    assert_eq!(named_group.report_count, 3);
    assert_eq!(named_group.reports.len(), 3);
    assert_eq!(named_group.target.owner.as_ref().unwrap().username, "user1");
    let anon_group = page
        .items
        .iter()
        .find(|g| g.target_id.get() == anon)
        .unwrap();
    assert!(anon_group.target.anonymous);
    assert!(anon_group.target.owner.is_none(), "匿名内容不暴露作者");
    // 组按最新举报时间倒序。
    assert_eq!(page.items[0].target_id.get(), anon);

    let query = |q: &str| GroupQuery {
        q: Some(q.into()),
        ..GroupQuery::default()
    };
    let by_title = admin::list(&h.state, &staff, query("实名")).await.unwrap();
    assert_eq!(by_title.total, 1);
    let by_reporter = admin::list(&h.state, &staff, query("user3")).await.unwrap();
    assert_eq!(by_reporter.items[0].target_id.get(), named);
    let by_reason = admin::list(
        &h.state,
        &staff,
        GroupQuery {
            reason: Some(Reason::Porn),
            ..GroupQuery::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(by_reason.total, 1);

    admin::claim(&h.state, &staff, TargetType::Post, named)
        .await
        .unwrap();
    let reviewing = admin::list(
        &h.state,
        &staff,
        GroupQuery {
            tab: Tab::Reviewing,
            mine: true,
            ..GroupQuery::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(reviewing.total, 1);
    assert_eq!(
        reviewing.items[0].assigned_to.as_ref().unwrap().username,
        "user9"
    );
    let pending = admin::list(&h.state, &staff, GroupQuery::default())
        .await
        .unwrap();
    assert_eq!(pending.total, 1);

    admin::resolve(
        &h.state,
        &staff,
        TargetType::Post,
        named,
        resolve(Action::WarnUser, None, None),
    )
    .await
    .unwrap();
    let detail = admin::detail(&h.state, &staff, TargetType::Post, named)
        .await
        .unwrap();
    assert_eq!(detail.status, ReportStatus::Accepted);
    assert!(
        detail
            .reports
            .iter()
            .all(|r| r.handled_by.as_ref().unwrap().username == "user9")
    );
    assert_eq!(
        admin::detail(&h.state, &staff, TargetType::Post, 123)
            .await
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
}
