//! 用户侧：举报、是否已举报、我的举报（RPT）。多人举报自动隐藏在举报的同一个事务里完成。

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_auth::Principal;
use ik_kernel::bus::Effects;
use ik_kernel::cursor::{self, Page};
use ik_kernel::db::is_unique_violation;
use ik_kernel::{FieldError, Id, Problem};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::ModerationState;
use crate::dto::{
    CheckQuery, CheckResult, CreateReportRequest, MyReport, PageQuery, Reason, ReportCreated,
    ReportStatus, Resolution, TargetPreview, TargetType,
};
use crate::repo::{self, NewLog, NewReport};

const DETAIL_MAX_CHARS: usize = 500;
const EXCERPT_MAX_CHARS: usize = 200;
const SNAPSHOT_MAX_IMAGES: usize = 9;
const CHECK_MAX_IDS: usize = 100;
const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 50;

/// 举报时的目标快照（RPT-013），存进 `reports.snapshot`。内容被删除后后台仍能看到。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excerpt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default)]
    pub image_ids: Vec<i64>,
    #[serde(default)]
    pub anonymous: bool,
    /// 评论所在的帖子。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_title: Option<String>,
}

impl Snapshot {
    pub(crate) fn parse(value: serde_json::Value) -> Self {
        serde_json::from_value(value).unwrap_or_default()
    }

    pub(crate) fn preview(&self) -> TargetPreview {
        TargetPreview {
            title: self.title.clone(),
            excerpt: self.excerpt.clone(),
            username: self.username.clone(),
        }
    }

    /// 通知文案里的目标（RPTADM-066）。
    pub(crate) fn label(&self, target_type: TargetType) -> String {
        match target_type {
            TargetType::Post => format!(
                "委托《{}》",
                self.title
                    .as_deref()
                    .or(self.excerpt.as_deref())
                    .unwrap_or_default()
            ),
            TargetType::Comment => "评论".to_owned(),
            TargetType::User => "账号".to_owned(),
        }
    }
}

/// 摘要：折叠空白，超过 200 字截断并加省略号（RPT-014）。
fn excerpt(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > EXCERPT_MAX_CHARS {
        let mut cut: String = collapsed.chars().take(EXCERPT_MAX_CHARS).collect();
        cut.push('…');
        cut
    } else {
        collapsed
    }
}

fn target_not_found() -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "举报的内容不存在或不可见",
    )
}

/// 举报目标的现状：作者与快照。不可举报（不存在、不公开、账号已注销）时 404。
struct Target {
    owner_user_id: i64,
    snapshot: Snapshot,
}

async fn load_target(
    conn: &mut PgConnection,
    target_type: TargetType,
    id: i64,
) -> Result<Target, Problem> {
    match target_type {
        TargetType::Post => {
            let post = ik_content::interop::reportable_post(conn, id)
                .await?
                .filter(|p| p.is_public)
                .ok_or_else(target_not_found)?;
            Ok(Target {
                owner_user_id: post.author_id,
                snapshot: Snapshot {
                    title: Some(post.title),
                    excerpt: Some(excerpt(&post.excerpt)),
                    image_ids: post
                        .cover_ids
                        .into_iter()
                        .take(SNAPSHOT_MAX_IMAGES)
                        .collect(),
                    anonymous: post.is_anonymous,
                    ..Snapshot::default()
                },
            })
        }
        TargetType::Comment => {
            let comment = ik_comments::interop::reportable_comment(conn, id)
                .await?
                .filter(|c| c.is_public)
                .ok_or_else(target_not_found)?;
            Ok(Target {
                owner_user_id: comment.author_id,
                snapshot: Snapshot {
                    excerpt: Some(excerpt(&comment.text)),
                    image_ids: comment
                        .image_ids
                        .into_iter()
                        .take(SNAPSHOT_MAX_IMAGES)
                        .collect(),
                    anonymous: comment.is_anonymous,
                    post_id: Some(comment.post_id),
                    post_title: Some(comment.post_title),
                    ..Snapshot::default()
                },
            })
        }
        TargetType::User => {
            // 已注销、已合并的账号不能举报（D-063 方案 A）；封禁的可以。
            let user = ik_auth::interop::user_statuses(conn, &[id])
                .await?
                .remove(&id)
                .filter(|u| !u.gone())
                .ok_or_else(target_not_found)?;
            Ok(Target {
                owner_user_id: user.id,
                snapshot: Snapshot {
                    username: Some(user.username),
                    ..Snapshot::default()
                },
            })
        }
    }
}

fn check_detail(reason: Reason, detail: Option<String>) -> Result<Option<String>, Problem> {
    let detail = detail
        .map(|d| d.trim().to_owned())
        .filter(|d| !d.is_empty());
    let mut errors = Vec::new();
    if reason == Reason::Other && detail.is_none() {
        errors.push(FieldError {
            field: "detail".to_owned(),
            code: "REQUIRED".to_owned(),
            detail: "理由为「其他」时需要填写说明".to_owned(),
        });
    }
    if detail
        .as_ref()
        .is_some_and(|d| d.chars().count() > DETAIL_MAX_CHARS)
    {
        errors.push(FieldError {
            field: "detail".to_owned(),
            code: "TOO_LONG".to_owned(),
            detail: "补充说明最多 500 字".to_owned(),
        });
    }
    if errors.is_empty() {
        Ok(detail)
    } else {
        Err(Problem::validation(errors))
    }
}

fn check_rate(state: &ModerationState, user_id: i64) -> Result<(), Problem> {
    let limited = || {
        Problem::new(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMITED",
            "举报太频繁了，请稍后再试",
        )
    };
    state
        .limits
        .per_minute
        .check_key(&user_id)
        .map_err(|_| limited().with_retry_after(60))?;
    state
        .limits
        .per_day
        .check_key(&user_id)
        .map_err(|_| limited().with_retry_after(3600))
}

/// 举报（RPT-007～012）。不要求通过考试（RPT-020）。校验顺序：参数 → 目标可见 → 不是自己 →
/// 没有处理中的同目标举报 → 限流（前面的失败不消耗额度，RPT-043）。
pub async fn report(
    state: &ModerationState,
    principal: &Principal,
    request: CreateReportRequest,
) -> Result<ReportCreated, Problem> {
    let reporter_id = principal.user_id.get();
    let detail = check_detail(request.reason, request.detail)?;
    let target_type = request.target_type;
    let target_id = request.target_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();

    let mut tx = state.db.begin().await?;
    let target = load_target(&mut tx, target_type, target_id).await?;
    if target.owner_user_id == reporter_id {
        return Err(Problem::field("targetId", "SELF", "不能举报自己的内容"));
    }
    if !repo::open_targets_of(&mut tx, reporter_id, target_type.as_str(), &[target_id])
        .await?
        .is_empty()
    {
        return Err(duplicate());
    }
    check_rate(state, reporter_id)?;
    let id = state.ids.next_raw();
    let inserted = repo::insert(
        &mut tx,
        NewReport {
            id,
            reporter_id,
            target_type: target_type.as_str(),
            target_id,
            owner_user_id: Some(target.owner_user_id),
            reason: request.reason.as_str(),
            detail: detail.as_deref(),
            snapshot: serde_json::to_value(&target.snapshot)
                .map_err(|e| Problem::internal_from(&e))?,
            now,
        },
    )
    .await;
    match inserted {
        Ok(()) => {}
        // 并发的同一举报。
        Err(error) if is_unique_violation(&error, "reports_open_uq") => return Err(duplicate()),
        Err(error) => return Err(error.into()),
    }
    if target_type != TargetType::User {
        auto_hide(
            state,
            &mut tx,
            target_type,
            target_id,
            &target,
            now,
            &mut effects,
        )
        .await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(ReportCreated {
        id: Id::new(id),
        target_type,
        target_id: Id::new(target_id),
        reason: request.reason,
        status: ReportStatus::Pending,
        created_at: now,
    })
}

fn duplicate() -> Problem {
    Problem::conflict("REPORT_DUPLICATE", "你已举报过该内容，正在处理中")
}

/// 多人举报自动隐藏（RPT-050～052）：同一内容的待处理举报来自足够多的不同的人时，隐藏内容
/// （原因 `auto_report`，驳回时恢复），这些举报转入复核中（不写领取人，仍可领取），通知作者。
async fn auto_hide(
    state: &ModerationState,
    conn: &mut PgConnection,
    target_type: TargetType,
    target_id: i64,
    target: &Target,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<(), Problem> {
    let reporters = repo::pending_reporters(conn, target_type.as_str(), target_id).await?;
    if reporters < state.auto_hide_threshold().await {
        return Ok(());
    }
    let reason = ik_content::interop::HIDDEN_BY_REPORTS;
    let hidden = match target_type {
        TargetType::Post => {
            ik_content::interop::hide_post(conn, target_id, None, reason, now, effects).await?
        }
        TargetType::Comment => {
            ik_comments::interop::hide_comment(conn, target_id, None, reason, now, effects).await?
        }
        TargetType::User => false,
    };
    if !hidden {
        return Ok(());
    }
    let moved = repo::pending_to_reviewing(conn, target_type.as_str(), target_id).await?;
    let label = target.snapshot.label(target_type);
    ik_notify::interop::system(
        conn,
        &state.ids,
        &[target.owner_user_id],
        &format!("你的{label}因收到多人举报已被暂时隐藏，等待管理员复核。"),
        post_of(target_type, target_id, &target.snapshot),
        now,
        effects,
    )
    .await?;
    repo::log(
        conn,
        NewLog {
            id: state.ids.next_raw(),
            actor_id: None,
            action: "auto_hide",
            target_type: target_type.as_str(),
            target_id,
            report_count: i32::try_from(moved).unwrap_or(i32::MAX),
            note: None,
            subject_user_id: Some(target.owner_user_id),
            subject_anonymous: target.snapshot.anonymous,
            now,
        },
    )
    .await?;
    Ok(())
}

/// 通知里关联的帖子（帖子本身，或评论所在的帖子）。
pub(crate) fn post_of(target_type: TargetType, target_id: i64, snapshot: &Snapshot) -> Option<i64> {
    match target_type {
        TargetType::Post => Some(target_id),
        TargetType::Comment => snapshot.post_id,
        TargetType::User => None,
    }
}

/// 我对这些目标是否有处理中的举报（RPT-030/031）。最多 100 个 id，非法 id 忽略。
pub async fn check(
    state: &ModerationState,
    principal: &Principal,
    query: CheckQuery,
) -> Result<CheckResult, Problem> {
    let mut ids: Vec<i64> = query
        .ids
        .split(',')
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .filter(|id| *id > 0)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.truncate(CHECK_MAX_IDS);
    let mut conn = state.db.acquire().await?;
    let open = repo::open_targets_of(
        &mut conn,
        principal.user_id.get(),
        query.target_type.as_str(),
        &ids,
    )
    .await?;
    let reported: BTreeMap<String, bool> = ids
        .iter()
        .map(|id| (id.to_string(), open.contains(id)))
        .collect();
    Ok(CheckResult { reported })
}

#[derive(Serialize, Deserialize)]
struct MyCursor {
    id: i64,
}

/// 我的举报，新的在前（RPT-032）。
pub async fn mine(
    state: &ModerationState,
    principal: &Principal,
    query: PageQuery,
) -> Result<Page<MyReport>, Problem> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let before: Option<MyCursor> = query.cursor.as_deref().map(cursor::decode).transpose()?;
    let mut conn = state.db.acquire().await?;
    let rows = repo::mine(
        &mut conn,
        principal.user_id.get(),
        before.map(|c| c.id),
        i64::from(limit) + 1,
    )
    .await?;
    let items = rows
        .into_iter()
        .map(|row| MyReport {
            id: Id::new(row.id),
            target_type: TargetType::parse(&row.target_type),
            target_id: Id::new(row.target_id),
            reason: Reason::parse(&row.reason),
            detail: row.detail,
            status: ReportStatus::parse(&row.status),
            resolution: row.resolution.as_deref().map(Resolution::parse),
            created_at: row.created_at,
            handled_at: row.handled_at,
            target: Snapshot::parse(row.snapshot).preview(),
        })
        .collect();
    Page::from_rows(
        items,
        usize::try_from(limit).unwrap_or(20),
        |r: &MyReport| MyCursor { id: r.id.get() },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpts_collapse_and_cut_at_200_chars() {
        assert_eq!(excerpt("  a\n\n b  "), "a b");
        let long = "字".repeat(250);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), 201);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn labels_follow_the_old_wording() {
        let post = Snapshot {
            title: Some("标题".into()),
            ..Snapshot::default()
        };
        assert_eq!(post.label(TargetType::Post), "委托《标题》");
        assert_eq!(post.label(TargetType::Comment), "评论");
        assert_eq!(post.label(TargetType::User), "账号");
    }
}
