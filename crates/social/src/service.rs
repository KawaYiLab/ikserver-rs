//! 关注与拉黑。业务规则：`docs/business-rules/social-economy.md` 第 3、4 节。
//!
//! 同一对用户之间的关注 / 拉黑写入用 [`repo::lock_pair`] 串行化。

use chrono::{DateTime, Utc};
use governor::clock::{Clock, DefaultClock};
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::interop::UserRef;
use ik_kernel::cursor::{self, Page};
use ik_kernel::{Id, Problem};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::SocialState;
use crate::dto::{BlockResult, FollowResult, PageQuery, RelationView, UserCard};
use crate::interop::{any_block, user_blocked};
use crate::repo::{self, RelationRow};

const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 50;
/// 批量查询关系时最多取前 100 个 id（COMMON-002）。
const MAX_RELATION_IDS: usize = 100;

// ---------- 错误 ----------

fn user_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "用户不存在")
}

fn follow_self() -> Problem {
    Problem::new(StatusCode::BAD_REQUEST, "FOLLOW_SELF", "不能关注自己")
}

fn block_self() -> Problem {
    Problem::new(StatusCode::BAD_REQUEST, "BLOCK_SELF", "不能拉黑自己")
}

fn block_system_account() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "BLOCK_SYSTEM_ACCOUNT",
        "不能拉黑系统账号",
    )
}

/// 关注 / 拉黑的目标必须存在且没有注销。
async fn target(conn: &mut PgConnection, user_id: i64) -> Result<UserRef, Problem> {
    ik_auth::interop::user_ref(conn, user_id)
        .await?
        .filter(|u| !u.gone)
        .ok_or_else(user_not_found)
}

// ---------- 关注 ----------

/// 关注（幂等）。需要通过入站考试；双方存在拉黑时不能新建关注（FOLLOW-004）。
/// 关注本身不发通知、不加经验（FOLLOW-007）。
pub async fn follow(
    state: &SocialState,
    principal: &Principal,
    user_id: i64,
) -> Result<FollowResult, Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    if me == user_id {
        return Err(follow_self());
    }
    let mut tx = state.db.begin().await?;
    target(&mut tx, user_id).await?;
    repo::lock_pair(&mut tx, me, user_id).await?;
    if !repo::is_following(&mut tx, me, user_id).await? {
        if any_block(&mut tx, me, user_id).await? {
            return Err(user_blocked());
        }
        repo::insert_follow(&mut tx, me, user_id, Utc::now()).await?;
    }
    let follower_count = repo::follower_count(&mut tx, user_id).await?;
    tx.commit().await?;
    Ok(FollowResult {
        following: true,
        follower_count,
    })
}

/// 取消关注（幂等）。不检查拉黑；和删除自己的内容一样不要求通过考试。
pub async fn unfollow(
    state: &SocialState,
    principal: &Principal,
    user_id: i64,
) -> Result<FollowResult, Problem> {
    let me = principal.user_id.get();
    let mut tx = state.db.begin().await?;
    repo::lock_pair(&mut tx, me, user_id).await?;
    repo::delete_follow(&mut tx, me, user_id).await?;
    let follower_count = repo::follower_count(&mut tx, user_id).await?;
    tx.commit().await?;
    Ok(FollowResult {
        following: false,
        follower_count,
    })
}

// ---------- 拉黑 ----------

/// 每分钟 10 次、每天 100 次。和旧系统一样在参数校验之前计数，取消拉黑也消耗额度（BLOCK-006）。
fn check_block_rate(state: &SocialState, principal: &Principal) -> Result<(), Problem> {
    let key = principal.user_id.get();
    let limits = &state.block_limits;
    limits
        .per_minute
        .check_key(&key)
        .and_then(|()| limits.per_day.check_key(&key))
        .map_err(|denied| {
            let wait = denied.wait_time_from(DefaultClock::default().now());
            Problem::rate_limited().with_retry_after(wait.as_secs().saturating_add(1))
        })
}

/// 拉黑（幂等）。不要求通过考试（BLOCK-004）；不能拉黑自己和 AI 账号（BLOCK-005）。
/// 同一事务里双向删除关注，取消拉黑不会恢复（BLOCK-008）。
pub async fn block(
    state: &SocialState,
    principal: &Principal,
    user_id: i64,
) -> Result<BlockResult, Problem> {
    check_block_rate(state, principal)?;
    let me = principal.user_id.get();
    if me == user_id {
        return Err(block_self());
    }
    let mut tx = state.db.begin().await?;
    let target = target(&mut tx, user_id).await?;
    if target.is_ai_agent {
        return Err(block_system_account());
    }
    repo::lock_pair(&mut tx, me, user_id).await?;
    repo::insert_block(&mut tx, me, user_id, Utc::now()).await?;
    repo::delete_follows_between(&mut tx, me, user_id).await?;
    tx.commit().await?;
    Ok(BlockResult { blocked: true })
}

/// 取消拉黑（幂等）。
pub async fn unblock(
    state: &SocialState,
    principal: &Principal,
    user_id: i64,
) -> Result<BlockResult, Problem> {
    check_block_rate(state, principal)?;
    let me = principal.user_id.get();
    let mut tx = state.db.begin().await?;
    repo::lock_pair(&mut tx, me, user_id).await?;
    repo::delete_block(&mut tx, me, user_id).await?;
    tx.commit().await?;
    Ok(BlockResult { blocked: false })
}

// ---------- 读取 ----------

#[derive(Serialize, Deserialize)]
struct RelationCursor {
    t: DateTime<Utc>,
    i: i64,
}

fn page_limit(limit: Option<u32>) -> usize {
    usize::try_from(limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE)).unwrap_or(20)
}

fn decode_cursor(raw: Option<&str>) -> Result<Option<(DateTime<Utc>, i64)>, Problem> {
    Ok(raw
        .map(cursor::decode::<RelationCursor>)
        .transpose()?
        .map(|c| (c.t, c.i)))
}

/// 把关系行变成用户卡片。已注销的账号不展示，但游标照常按原始行推进。
async fn user_cards(
    state: &SocialState,
    mut rows: Vec<RelationRow>,
    limit: usize,
) -> Result<Page<UserCard>, Problem> {
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    let next_cursor = match (has_more, rows.last()) {
        (true, Some(last)) => Some(cursor::encode(&RelationCursor {
            t: last.created_at,
            i: last.user_id,
        })?),
        _ => None,
    };
    let ids: Vec<i64> = rows.iter().map(|r| r.user_id).collect();
    let authors = ik_auth::service::authors(&state.db, &ids).await?;
    let items = rows
        .into_iter()
        .filter_map(|row| {
            authors
                .get(&row.user_id)
                .filter(|a| !a.gone)
                .map(|a| UserCard {
                    id: a.id,
                    username: a.username.clone(),
                    level: a.level,
                    since: row.created_at,
                })
        })
        .collect();
    Ok(Page { items, next_cursor })
}

/// 我关注的人（新关注的在前）。
pub async fn following(
    state: &SocialState,
    principal: &Principal,
    query: PageQuery,
) -> Result<Page<UserCard>, Problem> {
    let limit = page_limit(query.limit);
    let after = decode_cursor(query.cursor.as_deref())?;
    let mut conn = state.db.acquire().await?;
    let rows = repo::following_page(
        &mut conn,
        principal.user_id.get(),
        after,
        i64::try_from(limit).unwrap_or(20) + 1,
    )
    .await?;
    drop(conn);
    user_cards(state, rows, limit).await
}

/// 我拉黑的人（新拉黑的在前）。
pub async fn blocks(
    state: &SocialState,
    principal: &Principal,
    query: PageQuery,
) -> Result<Page<UserCard>, Problem> {
    let limit = page_limit(query.limit);
    let after = decode_cursor(query.cursor.as_deref())?;
    let mut conn = state.db.acquire().await?;
    let rows = repo::blocks_page(
        &mut conn,
        principal.user_id.get(),
        after,
        i64::try_from(limit).unwrap_or(20) + 1,
    )
    .await?;
    drop(conn);
    user_cards(state, rows, limit).await
}

/// 解析逗号分隔的 id 列表：去空白、去重、保序，最多 100 个（COMMON-002）。
fn parse_ids(raw: &str) -> Result<Vec<i64>, Problem> {
    let mut ids: Vec<i64> = Vec::new();
    for part in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let id = part
            .parse::<Id<()>>()
            .map_err(|_| Problem::field("userIds", "INVALID", "用户 id 格式不正确"))?
            .get();
        if !ids.contains(&id) {
            ids.push(id);
            if ids.len() == MAX_RELATION_IDS {
                break;
            }
        }
    }
    Ok(ids)
}

/// 我与一批用户的关系。每个请求的 id 都在结果里，顺序与请求一致。
pub async fn relations(
    state: &SocialState,
    principal: &Principal,
    user_ids: &str,
) -> Result<Vec<RelationView>, Problem> {
    let ids = parse_ids(user_ids)?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let me = principal.user_id.get();
    let mut conn = state.db.acquire().await?;
    let following = repo::following_among(&mut conn, me, &ids).await?;
    let blocking = repo::blocking_among(&mut conn, me, &ids).await?;
    Ok(ids
        .into_iter()
        .map(|id| RelationView {
            user_id: Id::new(id),
            following: following.contains(&id),
            blocking: blocking.contains(&id),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_trimmed_deduplicated_and_capped() {
        assert_eq!(parse_ids(" 3, 1,,3 ,2 ").unwrap(), vec![3, 1, 2]);
        assert!(parse_ids("").unwrap().is_empty());
        assert!(parse_ids("1,abc").is_err());
        let many: Vec<String> = (1..=150).map(|i| i.to_string()).collect();
        assert_eq!(parse_ids(&many.join(",")).unwrap().len(), MAX_RELATION_IDS);
    }
}
