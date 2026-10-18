//! 审核侧（RPTADM）：按目标聚合的列表、领取与释放、整组结案、解封。需要版主或管理员。
//!
//! 同一目标的未结案举报是一组：领取、释放、结案都先锁住整组（按 id 升序），在锁内重读状态。
//! 删除内容与结案在同一个事务里完成——新系统删除帖子、评论只是数据库写入（索引同步在提交后），
//! 不再需要旧系统两阶段的删除任务表（RPTADM-070～080）。

use std::collections::HashMap;

use chrono::Utc;
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::interop::{BlockOutcome, UserStatus};
use ik_kernel::bus::Effects;
use ik_kernel::{Id, Problem};
use sqlx::PgConnection;

use crate::ModerationState;
use crate::dto::{
    Action, BlockRequest, BlockResult, ClaimResult, ContentAction, ContentActionRequest,
    ContentActionResult, GroupPage, GroupQuery, ModerationEntry, Reason, ReceivedReport,
    ReceivedStats, ReportGroup, ReportRow, ReportStatus, Resolution, ResolveRequest, ResolveResult,
    Tab, TargetType, TargetView, UnblockResult, UnclaimResult, UserModeration, UserRef,
};
use crate::repo::{self, Closing, GroupFilter, GroupRow, NewLog, OpenRow};
use crate::service::{Snapshot, post_of};

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 100;
const NOTE_MAX_CHARS: usize = 500;
/// 关键词匹配举报人用户名时，最多带上这么多个账号。
const REPORTER_MATCH_LIMIT: i64 = 200;

pub fn require_staff(principal: &Principal) -> Result<(), Problem> {
    if principal.role.is_staff() {
        Ok(())
    } else {
        Err(Problem::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "需要版主或管理员权限",
        ))
    }
}

fn statuses(tab: Tab) -> Vec<String> {
    match tab {
        Tab::Pending => vec!["pending".to_owned()],
        Tab::Reviewing => vec!["reviewing".to_owned()],
        Tab::Closed => vec!["accepted".to_owned(), "rejected".to_owned()],
    }
}

fn all_statuses() -> Vec<String> {
    ["pending", "reviewing", "accepted", "rejected"]
        .map(str::to_owned)
        .to_vec()
}

/// ILIKE 的包含匹配模式：转义通配符。
fn contains_pattern(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

/// 举报列表（按目标聚合，RPTADM-010～019）。组按最新举报时间倒序，`page` 从 1 开始。
pub async fn list(
    state: &ModerationState,
    principal: &Principal,
    query: GroupQuery,
) -> Result<GroupPage, Problem> {
    require_staff(principal)?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let page = query.page.unwrap_or(1).max(1);
    let statuses = statuses(query.tab);
    let q = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let mut conn = state.db.acquire().await?;
    let reporter_ids = match q {
        Some(q) => {
            ik_auth::interop::ids_by_username_fragment(&mut conn, q, REPORTER_MATCH_LIMIT).await?
        }
        None => Vec::new(),
    };
    let pattern = q.map(contains_pattern);
    let groups = repo::groups(
        &mut conn,
        &GroupFilter {
            statuses: &statuses,
            target_type: query.target_type.map(TargetType::as_str),
            target_id: None,
            reason: query.reason.map(Reason::as_str),
            from: query.from,
            to: query.to,
            assigned_to: (query.tab == Tab::Reviewing && query.mine)
                .then(|| principal.user_id.get()),
            pattern: pattern.as_deref(),
            reporter_ids: &reporter_ids,
            limit: i64::from(limit),
            offset: i64::from(page - 1) * i64::from(limit),
        },
    )
    .await?;
    let total = groups.first().map_or(0, |g| g.total);
    let items = assemble(state, &mut conn, groups, &statuses).await?;
    Ok(GroupPage {
        items,
        total,
        page,
        limit,
    })
}

/// 一个目标的全部举报（各种状态），用于详情。没有举报时 404。
pub async fn detail(
    state: &ModerationState,
    principal: &Principal,
    target_type: TargetType,
    target_id: i64,
) -> Result<ReportGroup, Problem> {
    require_staff(principal)?;
    let statuses = all_statuses();
    let mut conn = state.db.acquire().await?;
    let groups = repo::groups(
        &mut conn,
        &GroupFilter {
            statuses: &statuses,
            target_type: Some(target_type.as_str()),
            target_id: Some(target_id),
            reason: None,
            from: None,
            to: None,
            assigned_to: None,
            pattern: None,
            reporter_ids: &[],
            limit: 1,
            offset: 0,
        },
    )
    .await?;
    assemble(state, &mut conn, groups, &statuses)
        .await?
        .into_iter()
        .next()
        .ok_or_else(Problem::not_found)
}

/// 把组行补全成展示用的组：举报明细、相关账号、目标现状、快照图片。
async fn assemble(
    state: &ModerationState,
    conn: &mut PgConnection,
    groups: Vec<GroupRow>,
    statuses: &[String],
) -> Result<Vec<ReportGroup>, Problem> {
    if groups.is_empty() {
        return Ok(Vec::new());
    }
    let types: Vec<String> = groups.iter().map(|g| g.target_type.clone()).collect();
    let ids: Vec<i64> = groups.iter().map(|g| g.target_id).collect();
    let rows = repo::rows_of(conn, &types, &ids, statuses).await?;

    let snapshots: Vec<Snapshot> = groups
        .iter()
        .map(|g| Snapshot::parse(g.snapshot.clone()))
        .collect();
    let mut user_ids: Vec<i64> = rows
        .iter()
        .flat_map(|r| [Some(r.reporter_id), r.assigned_to, r.handled_by])
        .chain(groups.iter().map(|g| g.owner_user_id))
        .flatten()
        .collect();
    user_ids.sort_unstable();
    user_ids.dedup();
    let users = ik_auth::interop::user_statuses(conn, &user_ids).await?;
    let post_ids: Vec<i64> = groups
        .iter()
        .filter(|g| g.target_type == "post")
        .map(|g| g.target_id)
        .collect();
    let comment_ids: Vec<i64> = groups
        .iter()
        .filter(|g| g.target_type == "comment")
        .map(|g| g.target_id)
        .collect();
    let posts = ik_content::interop::hidden_states(conn, &post_ids).await?;
    let comments = ik_comments::interop::hidden_states(conn, &comment_ids).await?;
    let mut image_ids: Vec<i64> = snapshots.iter().flat_map(|s| s.image_ids.clone()).collect();
    image_ids.sort_unstable();
    image_ids.dedup();
    let media = state.media.views(&image_ids).await?;

    let user_ref = |id: Option<i64>| -> Option<UserRef> {
        let user: &UserStatus = users.get(&id?)?;
        Some(UserRef {
            id: Id::new(user.id),
            username: user.username.clone(),
            status: user.status.clone(),
        })
    };
    let mut by_target: HashMap<(String, i64), Vec<ReportRow>> = HashMap::new();
    for row in &rows {
        by_target
            .entry((row.target_type.clone(), row.target_id))
            .or_default()
            .push(ReportRow {
                id: Id::new(row.id),
                reporter: user_ref(Some(row.reporter_id)),
                reason: Reason::parse(&row.reason),
                detail: row.detail.clone(),
                status: ReportStatus::parse(&row.status),
                resolution: row.resolution.as_deref().map(Resolution::parse),
                resolution_note: row.resolution_note.clone(),
                target_resolution_note: row.target_resolution_note.clone(),
                assigned_to: user_ref(row.assigned_to),
                assigned_at: row.assigned_at,
                handled_by: user_ref(row.handled_by),
                handled_at: row.handled_at,
                created_at: row.created_at,
            });
    }

    Ok(groups
        .into_iter()
        .zip(snapshots)
        .map(|(group, snapshot)| {
            let target_type = TargetType::parse(&group.target_type);
            let owner = group.owner_user_id.and_then(|id| users.get(&id));
            let (exists, hidden) = match target_type {
                TargetType::Post => (
                    posts.contains_key(&group.target_id),
                    posts.get(&group.target_id) == Some(&true),
                ),
                TargetType::Comment => (
                    comments.contains_key(&group.target_id),
                    comments.get(&group.target_id) == Some(&true),
                ),
                TargetType::User => (owner.is_some_and(|u| !u.gone()), false),
            };
            let target = TargetView {
                exists,
                hidden,
                anonymous: snapshot.anonymous,
                // 匿名内容不暴露作者（AGENTS.md 规则 10）。
                owner: if snapshot.anonymous {
                    None
                } else {
                    user_ref(group.owner_user_id)
                },
                owner_blocked: owner.is_some_and(|u| u.status == "blocked"),
                preview: snapshot.preview(),
                post_id: snapshot.post_id.map(Id::new),
                post_title: snapshot.post_title.clone(),
                images: snapshot
                    .image_ids
                    .iter()
                    .filter_map(|id| media.get(id).cloned())
                    .collect(),
            };
            ReportGroup {
                target_type,
                target_id: Id::new(group.target_id),
                report_count: group.report_count,
                first_report_at: group.first_at,
                latest_report_at: group.latest_at,
                status: ReportStatus::parse(&group.status),
                assigned_to: user_ref(group.assigned_to),
                target,
                reports: by_target
                    .remove(&(group.target_type.clone(), group.target_id))
                    .unwrap_or_default(),
            }
        })
        .collect())
}

fn closed() -> Problem {
    Problem::conflict("REPORT_CLOSED", "该举报已结案")
}

/// 组里有被别人领取的举报时拒绝（不做部分领取、部分结案）。
async fn ensure_not_claimed_by_others(
    conn: &mut PgConnection,
    rows: &[OpenRow],
    staff_id: i64,
) -> Result<(), Problem> {
    let Some(other) = rows
        .iter()
        .filter(|r| r.status == "reviewing")
        .find_map(|r| r.assigned_to.filter(|a| *a != staff_id))
    else {
        return Ok(());
    };
    let name = ik_auth::interop::user_statuses(conn, &[other])
        .await?
        .remove(&other)
        .map_or_else(|| "其他人".to_owned(), |u| u.username);
    Err(Problem::conflict(
        "REPORT_ALREADY_CLAIMED",
        format!("该举报已被 {name} 领取，仅处理人可以处理"),
    ))
}

/// 领取（RPTADM-030～035）：组里任一条被别人领取就整体拒绝 409；重复领取自己的是幂等的。
pub async fn claim(
    state: &ModerationState,
    principal: &Principal,
    target_type: TargetType,
    target_id: i64,
) -> Result<ClaimResult, Problem> {
    require_staff(principal)?;
    let staff_id = principal.user_id.get();
    let mut tx = state.db.begin().await?;
    let rows = repo::lock_open(&mut tx, target_type.as_str(), target_id).await?;
    if rows.is_empty() {
        return Err(closed());
    }
    ensure_not_claimed_by_others(&mut tx, &rows, staff_id).await?;
    let claimed = repo::claim(
        &mut tx,
        target_type.as_str(),
        target_id,
        staff_id,
        Utc::now(),
    )
    .await?;
    tx.commit().await?;
    Ok(ClaimResult {
        claimed_count: i64::try_from(claimed).unwrap_or(i64::MAX),
    })
}

/// 释放（RPTADM-040）：自己领取的改回待处理。
pub async fn unclaim(
    state: &ModerationState,
    principal: &Principal,
    target_type: TargetType,
    target_id: i64,
) -> Result<UnclaimResult, Problem> {
    require_staff(principal)?;
    let mut tx = state.db.begin().await?;
    repo::lock_open(&mut tx, target_type.as_str(), target_id).await?;
    let released = repo::unclaim(
        &mut tx,
        target_type.as_str(),
        target_id,
        principal.user_id.get(),
    )
    .await?;
    tx.commit().await?;
    Ok(UnclaimResult {
        released_count: i64::try_from(released).unwrap_or(i64::MAX),
    })
}

fn clean_note(note: Option<String>) -> Option<String> {
    note.map(|n| n.trim().chars().take(NOTE_MAX_CHARS).collect::<String>())
        .filter(|n| !n.is_empty())
}

fn with_note(message: String, note: Option<&str>) -> String {
    match note {
        Some(note) => format!("{message}\n处理备注：{note}"),
        None => message,
    }
}

/// 结案（RPTADM-050～067）：整组一起结案。待处理的、或复核中没人领取的，任何版主都能直接结案；
/// 被别人领取的只有领取人能结案。
///
/// - `reject`：驳回；内容如果是被自动隐藏的，恢复可见（管理员隐藏的不恢复）。不通知被举报人。
/// - `hide_content` / `delete_content`：隐藏 / 删除内容，通知作者。
/// - `warn_user`：只通知作者。
/// - `block_user`：封禁作者或被举报的用户，立即下线（D-059 方案 A）；不能封禁版主与管理员。
///
/// 举报人每人收到一条回执（附处理备注）。内容的处理、举报结案、通知、审核日志在同一个事务里。
pub async fn resolve(
    state: &ModerationState,
    principal: &Principal,
    target_type: TargetType,
    target_id: i64,
    request: ResolveRequest,
) -> Result<ResolveResult, Problem> {
    require_staff(principal)?;
    let action = request.action;
    if target_type == TargetType::User
        && matches!(action, Action::HideContent | Action::DeleteContent)
    {
        return Err(Problem::field(
            "action",
            "INVALID",
            "对用户的举报不支持内容处理动作",
        ));
    }
    let note = clean_note(request.note);
    let target_note = clean_note(request.target_note);
    let staff_id = principal.user_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();

    let mut tx = state.db.begin().await?;
    let rows = repo::lock_open(&mut tx, target_type.as_str(), target_id).await?;
    let Some(latest) = rows.last() else {
        return Err(closed());
    };
    ensure_not_claimed_by_others(&mut tx, &rows, staff_id).await?;
    let snapshot = Snapshot::parse(latest.snapshot.clone());
    let owner = rows.iter().find_map(|r| r.owner_user_id);
    let label = snapshot.label(target_type);
    let post_id = post_of(target_type, target_id, &snapshot);
    let moderator = ik_content::interop::HIDDEN_BY_MODERATOR;

    let owner_message = match action {
        Action::Reject => {
            let reason = ik_content::interop::HIDDEN_BY_REPORTS;
            match target_type {
                TargetType::Post => {
                    ik_content::interop::unhide_post(&mut tx, target_id, reason, &mut effects)
                        .await?;
                }
                TargetType::Comment => {
                    ik_comments::interop::unhide_comment(&mut tx, target_id, reason, &mut effects)
                        .await?;
                }
                TargetType::User => {}
            }
            None
        }
        Action::HideContent => {
            match target_type {
                TargetType::Post => {
                    ik_content::interop::hide_post(
                        &mut tx,
                        target_id,
                        Some(staff_id),
                        moderator,
                        now,
                        &mut effects,
                    )
                    .await?;
                }
                TargetType::Comment => {
                    ik_comments::interop::hide_comment(
                        &mut tx,
                        target_id,
                        Some(staff_id),
                        moderator,
                        now,
                        &mut effects,
                    )
                    .await?;
                }
                TargetType::User => {}
            }
            Some(format!(
                "你的{label}因违反绳网社区规范已被隐藏。如有异议请联系管理员。"
            ))
        }
        Action::DeleteContent => {
            match target_type {
                TargetType::Post => {
                    ik_content::interop::delete_post(&mut tx, target_id, &mut effects).await?;
                }
                TargetType::Comment => {
                    ik_comments::interop::delete_comment(&mut tx, target_id, &mut effects).await?;
                }
                TargetType::User => {}
            }
            Some(format!(
                "你的{label}因违反绳网社区规范已被删除。如有异议请联系管理员。"
            ))
        }
        Action::WarnUser => Some(format!(
            "请注意：你的{label}收到举报且经核实存在不当之处，请遵守绳网社区规范。"
        )),
        Action::BlockUser => {
            if let Some(owner) = owner {
                match ik_auth::interop::block_user(&mut tx, owner, now).await? {
                    BlockOutcome::Staff => {
                        return Err(Problem::new(
                            StatusCode::FORBIDDEN,
                            "CANNOT_SANCTION_STAFF",
                            "不能封禁版主或管理员",
                        ));
                    }
                    BlockOutcome::Blocked => {
                        effects.publish(ik_auth::service::sessions_changed_event(owner)?);
                    }
                    BlockOutcome::AlreadyBlocked | BlockOutcome::Gone => {}
                }
            }
            Some("你的账号因违反绳网社区规范已被封禁。如有异议请联系管理员。".to_owned())
        }
    };

    let accepted = action != Action::Reject;
    let resolved = repo::close(
        &mut tx,
        target_type.as_str(),
        target_id,
        &Closing {
            status: if accepted { "accepted" } else { "rejected" },
            resolution: action.resolution().as_str(),
            note: note.as_deref(),
            target_note: target_note.as_deref(),
            handled_by: staff_id,
            now,
        },
    )
    .await?;

    // 删除后帖子不在了，通知不再关联它。
    let notice_post = if action == Action::DeleteContent && target_type == TargetType::Post {
        None
    } else {
        post_id
    };
    if let (Some(message), Some(owner)) = (owner_message, owner) {
        ik_notify::interop::system(
            &mut tx,
            &state.ids,
            &[owner],
            &with_note(message, target_note.as_deref()),
            notice_post,
            now,
            &mut effects,
        )
        .await?;
    }
    let mut reporters: Vec<i64> = rows.iter().map(|r| r.reporter_id).collect();
    reporters.sort_unstable();
    reporters.dedup();
    let receipt = if accepted {
        format!("你举报的{label}已处理完毕，感谢你维护绳网环境！")
    } else {
        format!("你举报的{label}经核实暂未发现违规，感谢你维护绳网环境。")
    };
    ik_notify::interop::system(
        &mut tx,
        &state.ids,
        &reporters,
        &with_note(receipt, note.as_deref()),
        notice_post,
        now,
        &mut effects,
    )
    .await?;
    repo::log(
        &mut tx,
        NewLog {
            id: state.ids.next_raw(),
            actor_id: Some(staff_id),
            action: action.as_str(),
            target_type: target_type.as_str(),
            target_id,
            report_count: i32::try_from(resolved).unwrap_or(i32::MAX),
            note: note.as_deref(),
            subject_user_id: if target_type == TargetType::User {
                Some(target_id)
            } else {
                owner
            },
            subject_anonymous: target_type != TargetType::User && snapshot.anonymous,
            now,
        },
    )
    .await?;
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(ResolveResult {
        action,
        resolved_count: i64::try_from(resolved).unwrap_or(i64::MAX),
    })
}

/// 解封（D-057，至少方案 B）。只有封禁中的账号会解封；写审核日志与后台审计。
pub async fn unblock(
    state: &ModerationState,
    op: &ik_auth::Operator,
    user_id: i64,
) -> Result<UnblockResult, Problem> {
    require_staff(&op.principal)?;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let unblocked = ik_auth::interop::unblock_user(&mut tx, user_id, now).await?;
    if unblocked {
        repo::log(
            &mut tx,
            NewLog {
                id: state.ids.next_raw(),
                actor_id: Some(op.user_id()),
                action: "unblock_user",
                target_type: "user",
                target_id: user_id,
                report_count: 0,
                note: None,
                subject_user_id: Some(user_id),
                subject_anonymous: false,
                now,
            },
        )
        .await?;
        ik_auth::admin::record(
            &mut tx,
            &state.ids,
            op,
            ik_auth::admin::Audit {
                action: "user.unblock",
                target_type: "user",
                target_id: Some(user_id.to_string()),
                details: serde_json::json!({}),
            },
            now,
        )
        .await?;
    }
    tx.commit().await?;
    Ok(UnblockResult { unblocked })
}

/// 直接封禁（不经举报，管理后台用户页）：立即下线（D-059 方案 A）；不能封禁版主与管理员；
/// 已封禁的不重复记录。写审核日志与后台审计。
pub async fn block(
    state: &ModerationState,
    op: &ik_auth::Operator,
    user_id: i64,
    request: BlockRequest,
) -> Result<BlockResult, Problem> {
    require_staff(&op.principal)?;
    let note = clean_note(request.note);
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let blocked = match ik_auth::interop::block_user(&mut tx, user_id, now).await? {
        BlockOutcome::Staff => {
            return Err(Problem::new(
                StatusCode::FORBIDDEN,
                "CANNOT_SANCTION_STAFF",
                "不能封禁版主或管理员",
            ));
        }
        BlockOutcome::Gone => return Err(Problem::not_found()),
        BlockOutcome::AlreadyBlocked => false,
        BlockOutcome::Blocked => {
            effects.publish(ik_auth::service::sessions_changed_event(user_id)?);
            true
        }
    };
    if blocked {
        repo::log(
            &mut tx,
            NewLog {
                id: state.ids.next_raw(),
                actor_id: Some(op.user_id()),
                action: "block_user",
                target_type: "user",
                target_id: user_id,
                report_count: 0,
                note: note.as_deref(),
                subject_user_id: Some(user_id),
                subject_anonymous: false,
                now,
            },
        )
        .await?;
        ik_auth::admin::record(
            &mut tx,
            &state.ids,
            op,
            ik_auth::admin::Audit {
                action: "user.block",
                target_type: "user",
                target_id: Some(user_id.to_string()),
                details: serde_json::json!({ "note": note }),
            },
            now,
        )
        .await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(BlockResult { blocked })
}

/// 一次直接处置最多这么多条。
const MAX_BATCH: usize = 50;

/// 锁住的处置目标。
struct Locked {
    author_id: i64,
    anonymous: bool,
    /// 隐藏原因；未隐藏为 None。
    hidden_reason: Option<String>,
    /// 通知里关联的帖子（帖子本身或评论所在的帖子）。
    post_id: i64,
    label: String,
}

async fn lock_target(
    conn: &mut PgConnection,
    target_type: TargetType,
    id: i64,
) -> Result<Option<Locked>, Problem> {
    Ok(match target_type {
        TargetType::Post => ik_content::interop::lock_moderation_target(conn, id)
            .await?
            .map(|t| Locked {
                author_id: t.author_id,
                anonymous: t.is_anonymous,
                hidden_reason: t.hidden_reason,
                post_id: id,
                label: format!("委托《{}》", t.title),
            }),
        TargetType::Comment => ik_comments::interop::lock_moderation_target(conn, id)
            .await?
            .map(|t| Locked {
                author_id: t.author_id,
                anonymous: t.is_anonymous,
                hidden_reason: t.hidden_reason,
                post_id: t.post_id,
                label: format!("在委托《{}》下的评论", t.post_title),
            }),
        TargetType::User => None,
    })
}

/// 不经举报直接隐藏、恢复、删除帖子或评论（管理后台的帖子 / 评论管理）。
///
/// - 按 id 升序逐条加锁后再判断状态，并发重复提交时后到的看到已是目标状态，不重复通知、不重复记录。
/// - 有未结案举报的目标跳过（在 `pendingReports` 里返回），请到举报处理里结案。
/// - 恢复可见对两种隐藏原因（版主隐藏、多人举报自动隐藏）都生效。
/// - 每条改动写一行审核日志；后台审计每次请求一行（批量时 id 列表在 details 里）。
/// - `notifyAuthor` 时给作者发系统通知（恢复可见不发），删除帖子后通知不再关联它。
pub async fn act(
    state: &ModerationState,
    op: &ik_auth::Operator,
    request: ContentActionRequest,
) -> Result<ContentActionResult, Problem> {
    require_staff(&op.principal)?;
    let target_type = request.target_type;
    if target_type == TargetType::User {
        return Err(Problem::field(
            "targetType",
            "INVALID",
            "只能处置帖子或评论",
        ));
    }
    let mut ids: Vec<i64> = request.ids.iter().map(|id| id.get()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.is_empty() || ids.len() > MAX_BATCH {
        return Err(Problem::field("ids", "INVALID", "一次处置 1–50 条"));
    }
    let action = request.action;
    let note = clean_note(request.note);
    let staff_id = op.user_id();
    let now = Utc::now();
    let mut effects = Effects::new();

    let mut tx = state.db.begin().await?;
    let open: std::collections::HashSet<i64> =
        repo::open_targets(&mut tx, target_type.as_str(), &ids)
            .await?
            .into_iter()
            .collect();
    let mut affected = Vec::new();
    let mut pending = Vec::new();
    let mut unchanged = Vec::new();
    for id in ids {
        if open.contains(&id) {
            pending.push(Id::new(id));
            continue;
        }
        let Some(target) = lock_target(&mut tx, target_type, id).await? else {
            unchanged.push(Id::new(id));
            continue;
        };
        let moderator = ik_content::interop::HIDDEN_BY_MODERATOR;
        let changed = match (action, target_type) {
            (ContentAction::Hide, _) if target.hidden_reason.as_deref() == Some(moderator) => false,
            (ContentAction::Hide, TargetType::Post) => {
                ik_content::interop::hide_post(
                    &mut tx,
                    id,
                    Some(staff_id),
                    moderator,
                    now,
                    &mut effects,
                )
                .await?
            }
            (ContentAction::Hide, _) => {
                ik_comments::interop::hide_comment(
                    &mut tx,
                    id,
                    Some(staff_id),
                    moderator,
                    now,
                    &mut effects,
                )
                .await?
            }
            (ContentAction::Unhide, _) => match target.hidden_reason.as_deref() {
                None => false,
                Some(reason) if target_type == TargetType::Post => {
                    ik_content::interop::unhide_post(&mut tx, id, reason, &mut effects).await?
                }
                Some(reason) => {
                    ik_comments::interop::unhide_comment(&mut tx, id, reason, &mut effects).await?
                }
            },
            (ContentAction::Delete, TargetType::Post) => {
                ik_content::interop::delete_post(&mut tx, id, &mut effects).await?
            }
            (ContentAction::Delete, _) => {
                ik_comments::interop::delete_comment(&mut tx, id, &mut effects).await?
            }
        };
        if !changed {
            unchanged.push(Id::new(id));
            continue;
        }
        repo::log(
            &mut tx,
            NewLog {
                id: state.ids.next_raw(),
                actor_id: Some(staff_id),
                action: action.log_action(),
                target_type: target_type.as_str(),
                target_id: id,
                report_count: 0,
                note: note.as_deref(),
                subject_user_id: Some(target.author_id),
                subject_anonymous: target.anonymous,
                now,
            },
        )
        .await?;
        let message = match action {
            ContentAction::Hide => Some(format!(
                "你的{}因违反绳网社区规范已被隐藏。如有异议请联系管理员。",
                target.label
            )),
            ContentAction::Delete => Some(format!(
                "你的{}因违反绳网社区规范已被删除。如有异议请联系管理员。",
                target.label
            )),
            ContentAction::Unhide => None,
        };
        if let (true, Some(message)) = (request.notify_author, message) {
            let deleted_post = action == ContentAction::Delete && target_type == TargetType::Post;
            ik_notify::interop::system(
                &mut tx,
                &state.ids,
                &[target.author_id],
                &with_note(message, note.as_deref()),
                (!deleted_post).then_some(target.post_id),
                now,
                &mut effects,
            )
            .await?;
        }
        affected.push(id);
    }
    if !affected.is_empty() {
        let single = affected.len() == 1;
        ik_auth::admin::record(
            &mut tx,
            &state.ids,
            op,
            ik_auth::admin::Audit {
                action: match (target_type, action) {
                    (TargetType::Post, ContentAction::Hide) => "content.post.hide",
                    (TargetType::Post, ContentAction::Unhide) => "content.post.unhide",
                    (TargetType::Post, ContentAction::Delete) => "content.post.delete",
                    (_, ContentAction::Hide) => "content.comment.hide",
                    (_, ContentAction::Unhide) => "content.comment.unhide",
                    (_, ContentAction::Delete) => "content.comment.delete",
                },
                target_type: target_type.as_str(),
                target_id: single.then(|| affected[0].to_string()),
                details: serde_json::json!({
                    "ids": if single { None } else {
                        Some(affected.iter().map(i64::to_string).collect::<Vec<_>>())
                    },
                    "note": note,
                    "notifyAuthor": request.notify_author,
                }),
            },
            now,
        )
        .await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(ContentActionResult {
        action,
        affected: i64::try_from(affected.len()).unwrap_or(i64::MAX),
        pending_reports: pending,
        unchanged,
    })
}

/// 账号的处罚记录与收到的举报（管理后台账号详情）。匿名内容相关的一律排除。
pub async fn user_history(
    state: &ModerationState,
    user_id: i64,
) -> Result<UserModeration, Problem> {
    let mut conn = state.db.acquire().await?;
    let stats = repo::received_stats(&mut conn, user_id).await?;
    let reports = repo::received_reports(&mut conn, user_id, 20).await?;
    let logs = repo::subject_logs(&mut conn, user_id, 50).await?;
    let mut actor_ids: Vec<i64> = logs.iter().filter_map(|l| l.actor_id).collect();
    actor_ids.sort_unstable();
    actor_ids.dedup();
    let actors = ik_auth::interop::user_statuses(&mut conn, &actor_ids).await?;
    Ok(UserModeration {
        stats: ReceivedStats {
            total: stats.total,
            open: stats.open,
            accepted: stats.accepted,
            rejected: stats.rejected,
        },
        reports: reports
            .into_iter()
            .map(|r| ReceivedReport {
                target_type: TargetType::parse(&r.target_type),
                target_id: Id::new(r.target_id),
                reason: Reason::parse(&r.reason),
                detail: r.detail,
                status: ReportStatus::parse(&r.status),
                resolution: r.resolution.as_deref().map(Resolution::parse),
                preview: Snapshot::parse(r.snapshot).preview(),
                created_at: r.created_at,
            })
            .collect(),
        actions: logs
            .into_iter()
            .map(|l| ModerationEntry {
                action: l.action,
                target_type: TargetType::parse(&l.target_type),
                target_id: Id::new(l.target_id),
                actor: l.actor_id.and_then(|id| actors.get(&id)).map(|u| UserRef {
                    id: Id::new(u.id),
                    username: u.username.clone(),
                    status: u.status.clone(),
                }),
                report_count: l.report_count,
                note: l.note,
                created_at: l.created_at,
            })
            .collect(),
    })
}

/// 待处理、复核中的举报目标数（管理后台仪表盘）。
pub async fn open_counts(state: &ModerationState) -> Result<(i64, i64), Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(repo::open_target_counts(&mut conn).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_patterns_escape_wildcards() {
        assert_eq!(contains_pattern("abc"), "%abc%");
        assert_eq!(contains_pattern("50%_\\"), "%50\\%\\_\\\\%");
    }

    #[test]
    fn notes_are_trimmed_and_capped() {
        assert_eq!(clean_note(Some("  ".into())), None);
        assert_eq!(clean_note(Some(" 好 ".into())).as_deref(), Some("好"));
        assert_eq!(
            clean_note(Some("字".repeat(600))).unwrap().chars().count(),
            500
        );
        assert_eq!(
            with_note("已处理".into(), Some("备注")),
            "已处理\n处理备注：备注"
        );
    }
}
