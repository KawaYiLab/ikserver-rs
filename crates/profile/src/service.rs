//! 个人资料与作者主页。业务规则：`docs/business-rules/social-economy.md` 第 11 节。

use std::collections::HashSet;
use std::net::IpAddr;

use chrono::Utc;
use governor::clock::{Clock, DefaultClock};
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::interop::ProfileUser;
use ik_economy::service::DennyAction;
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::cursor::Page;
use ik_kernel::{FieldError, Id, Problem};
use sqlx::PgConnection;

use crate::ProfileState;
use crate::dto::{
    AvatarResult, MentionCandidate, MyProfileView, PageQuery, PinnedPostsRequest, PostsScope,
    ProfilePostsPage, ProfilePostsQuery, ProfileStats, ProfileView, RenameRequest, RenameResult,
    SearchQuery, SetAvatarRequest, UpdateProfileRequest, UserSearchItem,
};
use crate::repo::{self, ProfileRow};

const BIO_MAX_CHARS: usize = 100;
const USERNAME_MIN_CHARS: usize = 3;
const USERNAME_MAX_CHARS: usize = 20;
/// 改名收费（PROFILE-005）。
const RENAME_COST: i32 = 10;
/// 设置自定义头像的费用（AVA-002）。
const AVATAR_COST: i32 = 10;
const MAX_PINNED: usize = 6;
const PROFILE_POSTS_DEFAULT: u32 = 10;
const SEARCH_MAX_QUERY_CHARS: usize = 32;
const SEARCH_DEFAULT: u32 = 20;
const SEARCH_MAX: u32 = 20;
const MENTION_DEFAULT: u32 = 8;
const MENTION_MAX: u32 = 10;
/// 搜索结果里签名的最大长度。
const SEARCH_BIO_CHARS: usize = 120;

fn user_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "用户不存在")
}

fn rate_limited<K: std::hash::Hash + Eq + Clone>(
    limiter: &governor::DefaultKeyedRateLimiter<K>,
    key: &K,
) -> Result<(), Problem> {
    limiter.check_key(key).map_err(|denied| {
        let wait = denied.wait_time_from(DefaultClock::default().now());
        Problem::rate_limited().with_retry_after(wait.as_secs().saturating_add(1))
    })
}

fn defaults() -> ProfileRow {
    ProfileRow {
        bio: String::new(),
        hidden: false,
        pinned_post_ids: None,
    }
}

async fn profile_row(conn: &mut PgConnection, user_id: i64) -> Result<ProfileRow, Problem> {
    Ok(repo::get(conn, user_id).await?.unwrap_or_else(defaults))
}

/// 主页对访客的可见程度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Full,
    Hidden,
    Blocked,
}

struct Target {
    user: ProfileUser,
    profile: ProfileRow,
    access: Access,
    is_self: bool,
    blocking: bool,
    blocked_by: bool,
}

/// 读出主页主人并判断访客能看到什么。已注销、已合并的账号 404。
async fn target(
    conn: &mut PgConnection,
    viewer: Option<&Principal>,
    user_id: i64,
) -> Result<Target, Problem> {
    let user = ik_auth::interop::profile_user(conn, user_id)
        .await?
        .filter(|u| !u.gone)
        .ok_or_else(user_not_found)?;
    let profile = profile_row(conn, user_id).await?;
    let viewer_id = viewer.map(|p| p.user_id.get());
    let is_self = viewer_id == Some(user_id);
    let blocks = match viewer_id {
        Some(viewer_id) if !is_self => {
            ik_social::interop::block_status(conn, viewer_id, user_id).await?
        }
        _ => ik_social::interop::BlockStatus::default(),
    };
    // 拉黑优先于隐藏（PROFILE-021）。
    let access = if is_self {
        Access::Full
    } else if blocks.any() {
        Access::Blocked
    } else if profile.hidden {
        Access::Hidden
    } else {
        Access::Full
    };
    Ok(Target {
        user,
        profile,
        access,
        is_self,
        blocking: blocks.blocking,
        blocked_by: blocks.blocked_by,
    })
}

// ---------- 作者主页 ----------

/// 作者主页（PROFILE-014～022）。
pub async fn profile(
    state: &ProfileState,
    viewer: Option<&Principal>,
    user_id: i64,
) -> Result<ProfileView, Problem> {
    let mut conn = state.db.acquire().await?;
    let t = target(&mut conn, viewer, user_id).await?;
    let viewer_id = viewer.map(|p| p.user_id.get());

    let (follower_count, following_count, is_following) = if t.access == Access::Blocked {
        (0, 0, false)
    } else {
        let (followers, following) = ik_social::interop::follow_counts(&mut conn, user_id).await?;
        let is_following = match viewer_id {
            Some(viewer_id) if !t.is_self => {
                !ik_social::interop::following_among(&mut conn, viewer_id, &[user_id])
                    .await?
                    .is_empty()
            }
            _ => false,
        };
        (followers, following, is_following)
    };

    let (exp, stats) = if t.access == Access::Full {
        let exp = ik_economy::service::exp_of(&mut conn, user_id).await?;
        drop(conn);
        let posts = ik_content::service::author_stats(&state.content, user_id).await?;
        let comment_count =
            ik_comments::service::author_comment_count(&state.comments, user_id).await?;
        (
            Some(exp),
            Some(ProfileStats {
                post_count: posts.post_count,
                comment_count,
                total_views: posts.total_views,
                total_comments: posts.total_comments,
                total_likes: posts.total_likes,
            }),
        )
    } else {
        (None, None)
    };

    let avatar = avatar_view(state, t.user.avatar_media_id.filter(|_| !t.user.gone)).await?;
    // 隐藏主页时不给装扮（PROFILE-022）；拉黑时保留（PROFILE-021）；已注销的不给（COSMETIC-008）。
    let show_cosmetics = !t.user.gone && t.access != Access::Hidden;
    let (equipped_avatar, equipped_card) = if show_cosmetics {
        equipped(state, t.user.equipped_avatar_id, t.user.equipped_card_id).await?
    } else {
        (None, None)
    };
    Ok(ProfileView {
        avatar,
        equipped_avatar,
        equipped_card,
        id: Id::new(t.user.id),
        username: t.user.username,
        level: t.user.level,
        is_ai_agent: t.user.is_ai_agent,
        joined_at: t.user.created_at,
        bio: if t.access == Access::Full {
            t.profile.bio
        } else {
            String::new()
        },
        exp,
        is_self: t.is_self,
        is_hidden: t.access == Access::Hidden,
        is_blocked_by_me: t.blocking,
        has_blocked_me: t.blocked_by,
        is_following,
        follower_count,
        following_count,
        stats,
    })
}

/// 作者主页的帖子。配置了精选且没有要求 `scope=all` 时只返回精选（PROFILE-019）。
/// 拉黑或隐藏时返回空页。被隐藏的帖子只有本人能看到（精选也一样，D-026 方案 A）。
pub async fn posts(
    state: &ProfileState,
    viewer: Option<&Principal>,
    user_id: i64,
    query: ProfilePostsQuery,
) -> Result<ProfilePostsPage, Problem> {
    let mut conn = state.db.acquire().await?;
    let t = target(&mut conn, viewer, user_id).await?;
    drop(conn);
    if t.access != Access::Full {
        return Ok(ProfilePostsPage {
            pinned: false,
            items: Vec::new(),
            next_cursor: None,
        });
    }
    if query.scope == PostsScope::Default
        && let Some(pinned) = &t.profile.pinned_post_ids
    {
        let items = ik_content::service::author_posts_by_ids(
            &state.content,
            viewer,
            user_id,
            pinned,
            t.is_self,
        )
        .await?;
        return Ok(ProfilePostsPage {
            pinned: true,
            items,
            next_cursor: None,
        });
    }
    let page = ik_content::service::author_posts(
        &state.content,
        viewer,
        user_id,
        t.is_self,
        query.cursor.as_deref(),
        Some(query.limit.unwrap_or(PROFILE_POSTS_DEFAULT)),
    )
    .await?;
    Ok(ProfilePostsPage {
        pinned: false,
        items: page.items,
        next_cursor: page.next_cursor,
    })
}

/// 作者主页的评论（PROFILE-020）。拉黑或隐藏时返回空页。
pub async fn comments(
    state: &ProfileState,
    viewer: Option<&Principal>,
    user_id: i64,
    query: PageQuery,
) -> Result<Page<ik_comments::dto::AuthorCommentView>, Problem> {
    let mut conn = state.db.acquire().await?;
    let t = target(&mut conn, viewer, user_id).await?;
    drop(conn);
    if t.access != Access::Full {
        return Ok(Page {
            items: Vec::new(),
            next_cursor: None,
        });
    }
    ik_comments::service::author_comments(
        &state.comments,
        viewer,
        user_id,
        ik_comments::dto::PageQuery {
            cursor: query.cursor,
            limit: query.limit,
        },
    )
    .await
}

// ---------- 自己的资料 ----------

async fn avatar_view(
    state: &ProfileState,
    media_id: Option<i64>,
) -> Result<Option<ik_media::dto::MediaView>, Problem> {
    let Some(id) = media_id else {
        return Ok(None);
    };
    Ok(state.media.views(&[id]).await?.remove(&id))
}

type Equipped = (
    Option<ik_cosmetics::dto::AvatarView>,
    Option<ik_cosmetics::dto::CardView>,
);

/// 装备的系统头像与名片的展示数据。
async fn equipped(
    state: &ProfileState,
    avatar_id: Option<i64>,
    card_id: Option<i64>,
) -> Result<Equipped, Problem> {
    let avatar = match avatar_id {
        Some(id) => ik_cosmetics::service::avatars_by_ids(&state.cosmetics, &[id])
            .await?
            .remove(&id),
        None => None,
    };
    let card = match card_id {
        Some(id) => ik_cosmetics::service::cards_by_ids(&state.cosmetics, &[id])
            .await?
            .remove(&id),
        None => None,
    };
    Ok((avatar, card))
}

async fn my_view(
    state: &ProfileState,
    user_id: i64,
    row: ProfileRow,
) -> Result<MyProfileView, Problem> {
    let mut conn = state.db.acquire().await?;
    let user = ik_auth::interop::profile_user(&mut conn, user_id)
        .await?
        .ok_or_else(Problem::not_found)?;
    drop(conn);
    let (equipped_avatar, equipped_card) =
        equipped(state, user.equipped_avatar_id, user.equipped_card_id).await?;
    Ok(MyProfileView {
        avatar: avatar_view(state, user.avatar_media_id).await?,
        equipped_avatar,
        equipped_card,
        bio: row.bio,
        hidden: row.hidden,
        pinned_post_ids: row
            .pinned_post_ids
            .map(|ids| ids.into_iter().map(Id::new).collect()),
    })
}

pub async fn my_profile(
    state: &ProfileState,
    principal: &Principal,
) -> Result<MyProfileView, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = profile_row(&mut conn, principal.user_id.get()).await?;
    drop(conn);
    my_view(state, principal.user_id.get(), row).await
}

/// 修改签名、隐藏主页。不收费、不要求通过考试（PROFILE-007/008）。
pub async fn update_profile(
    state: &ProfileState,
    principal: &Principal,
    request: UpdateProfileRequest,
) -> Result<MyProfileView, Problem> {
    let bio = request.bio.as_deref().map(str::trim);
    if bio.is_some_and(|b| b.chars().count() > BIO_MAX_CHARS) {
        return Err(Problem::field(
            "bio",
            "TOO_LONG",
            format!("签名最多 {BIO_MAX_CHARS} 个字"),
        ));
    }
    let mut conn = state.db.acquire().await?;
    let row = repo::update_settings(
        &mut conn,
        principal.user_id.get(),
        bio,
        request.hidden,
        Utc::now(),
    )
    .await?;
    my_view(state, principal.user_id.get(), row).await
}

/// 用户名校验（D-018：区分大小写；字符集维持旧系统的宽松规则，只额外拒绝 `@` 与控制字符）。
fn validate_username(raw: &str) -> Result<String, Problem> {
    let name = raw.trim();
    let len = name.chars().count();
    let invalid = |detail: String| {
        Problem::validation(vec![FieldError {
            field: "username".to_owned(),
            code: "INVALID".to_owned(),
            detail,
        }])
    };
    if !(USERNAME_MIN_CHARS..=USERNAME_MAX_CHARS).contains(&len) {
        return Err(invalid(format!(
            "用户名长度需在 {USERNAME_MIN_CHARS}-{USERNAME_MAX_CHARS} 个字之间"
        )));
    }
    if name.contains('@') || name.chars().any(char::is_control) {
        return Err(invalid("用户名不能包含 @ 或控制字符".to_owned()));
    }
    Ok(name.to_owned())
}

/// 改名：每次 10 丁尼（含第一次），与原名相同时直接成功、不扣费（PROFILE-002～006）。
/// 先改名（拿到用户行的排他锁，名字冲突时立即失败），再扣费；任一步失败整体回滚，不扣费。
pub async fn rename(
    state: &ProfileState,
    principal: &Principal,
    request: RenameRequest,
) -> Result<RenameResult, Problem> {
    let username = validate_username(&request.username)?;
    let user_id = principal.user_id.get();
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let current = ik_auth::interop::profile_user(&mut tx, user_id)
        .await?
        .ok_or_else(Problem::unauthorized)?;
    if current.username == username {
        let wallet_balance = ik_economy::service::balance_of(&mut tx, user_id).await?;
        return Ok(RenameResult {
            username,
            balance: wallet_balance,
        });
    }
    ik_auth::interop::rename(&mut tx, user_id, &username, now).await?;
    let spent = ik_economy::service::spend_denny(
        &mut tx,
        &state.ids,
        user_id,
        DennyAction::SpendRename,
        RENAME_COST,
        &format!("改名为「{username}」"),
        now,
    )
    .await?;
    tx.commit().await?;
    // 内部事件：搜索索引里此人非匿名内容的作者名随之更新（D-039 方案 A）。
    let mut effects = Effects::new();
    effects.publish(Event::new(
        ik_kernel::bus::internal::USER_RENAMED,
        Audience::Internal,
        &serde_json::json!({ "userId": user_id }),
    )?);
    effects.dispatch(state.content.bus.as_ref());
    Ok(RenameResult {
        username,
        balance: spent.balance,
    })
}

/// 设置自定义头像（AVA-001～003）：每次 10 丁尼，同时卸下系统头像；换成当前同一张图不扣费。只能用自己图库里的图
/// （D-012 方案 A）。不要求通过考试（AVA-005）。改资料与扣费同一事务，余额不足整体回滚。
pub async fn set_avatar(
    state: &ProfileState,
    principal: &Principal,
    request: SetAvatarRequest,
) -> Result<AvatarResult, Problem> {
    let user_id = principal.user_id.get();
    let media_id = request.media_id.get();
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    // 锁住用户行，串行化同一用户的扣费。
    ik_auth::interop::lock_active_user(&mut tx, user_id)
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
    if ik_media::interop::owned_media(&mut tx, user_id, &[media_id])
        .await?
        .is_empty()
    {
        return Err(Problem::new(
            StatusCode::NOT_FOUND,
            "NOT_FOUND",
            "图片不存在或不在你的图库里",
        ));
    }
    let current = ik_auth::interop::profile_user(&mut tx, user_id)
        .await?
        .and_then(|u| u.avatar_media_id);
    // 换成当前同一张图不扣费，但照样卸下系统头像（COSMETIC-006）。
    ik_auth::interop::set_avatar(&mut tx, user_id, Some(media_id)).await?;
    let balance = if current == Some(media_id) {
        ik_economy::service::balance_of(&mut tx, user_id).await?
    } else {
        ik_economy::service::spend_denny(
            &mut tx,
            &state.ids,
            user_id,
            DennyAction::SpendAvatar,
            AVATAR_COST,
            "设置自定义头像",
            now,
        )
        .await?
        .balance
    };
    tx.commit().await?;
    Ok(AvatarResult {
        avatar: avatar_view(state, Some(media_id)).await?,
        balance,
    })
}

/// 清除自定义头像（恢复默认头像）。不收费。
pub async fn clear_avatar(
    state: &ProfileState,
    principal: &Principal,
) -> Result<AvatarResult, Problem> {
    let user_id = principal.user_id.get();
    let mut tx = state.db.begin().await?;
    ik_auth::interop::set_avatar(&mut tx, user_id, None).await?;
    let balance = ik_economy::service::balance_of(&mut tx, user_id).await?;
    tx.commit().await?;
    Ok(AvatarResult {
        avatar: None,
        balance,
    })
}

/// 保存主页精选（PROFILE-010～012）。去空白、去重、保序；超过 6 篇报错（在剔除失效项之前判断）；
/// 不是自己的、未发布、匿名的帖子静默剔除，被隐藏的保留（D-026 方案 A）。
pub async fn set_pinned(
    state: &ProfileState,
    principal: &Principal,
    request: PinnedPostsRequest,
) -> Result<MyProfileView, Problem> {
    let user_id = principal.user_id.get();
    let pinned = match request.post_ids {
        None => None,
        Some(raw) => {
            let mut wanted: Vec<i64> = Vec::new();
            for item in raw.iter().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                // 格式不对的 id 与「不存在的帖子」一样静默剔除。
                if let Ok(id) = item.parse::<Id<()>>()
                    && !wanted.contains(&id.get())
                {
                    wanted.push(id.get());
                }
            }
            if wanted.len() > MAX_PINNED {
                return Err(Problem::field(
                    "postIds",
                    "TOO_MANY",
                    format!("最多精选 {MAX_PINNED} 篇"),
                ));
            }
            let valid: HashSet<i64> =
                ik_content::service::pinnable_post_ids(&state.content, user_id, &wanted)
                    .await?
                    .into_iter()
                    .collect();
            Some(
                wanted
                    .into_iter()
                    .filter(|id| valid.contains(id))
                    .collect::<Vec<i64>>(),
            )
        }
    };
    let mut conn = state.db.acquire().await?;
    let row = repo::set_pinned(&mut conn, user_id, pinned.as_deref(), Utc::now()).await?;
    my_view(state, principal.user_id.get(), row).await
}

// ---------- 用户搜索 ----------

/// 搜索候选：排除隐藏主页的人与有拉黑关系的人（PROFILE-023/024）。
async fn search_candidates(
    conn: &mut PgConnection,
    viewer_id: Option<i64>,
    query: &str,
    limit: usize,
) -> Result<Vec<ik_auth::interop::FoundUser>, Problem> {
    // 多取几倍，过滤后再截断。
    let pool = i64::try_from(limit.saturating_mul(4)).unwrap_or(80);
    let found = ik_auth::interop::search_users(conn, query, pool).await?;
    let ids: Vec<i64> = found.iter().map(|u| u.id).collect();
    let hidden: HashSet<i64> = repo::hidden_among(conn, &ids).await?.into_iter().collect();
    let excluded: HashSet<i64> = ik_social::interop::excluded_authors(conn, viewer_id)
        .await?
        .into_iter()
        .collect();
    Ok(found
        .into_iter()
        .filter(|u| !hidden.contains(&u.id) && !excluded.contains(&u.id))
        .take(limit)
        .collect())
}

/// 用户搜索（公开）。关键词为空或超过 32 字时直接返回空列表（PROFILE-024）。
/// 限流：登录用户每秒 10 次，游客按 IP 每秒 3 次。
pub async fn search(
    state: &ProfileState,
    viewer: Option<&Principal>,
    ip: IpAddr,
    query: SearchQuery,
) -> Result<Vec<UserSearchItem>, Problem> {
    match viewer {
        Some(p) => rate_limited(&state.limits.per_user, &p.user_id.get())?,
        None => rate_limited(&state.limits.per_ip, &ip)?,
    }
    let q = query.q.trim();
    if q.is_empty() || q.chars().count() > SEARCH_MAX_QUERY_CHARS {
        return Ok(Vec::new());
    }
    let limit =
        usize::try_from(query.limit.unwrap_or(SEARCH_DEFAULT).clamp(1, SEARCH_MAX)).unwrap_or(20);
    let viewer_id = viewer.map(|p| p.user_id.get());
    let mut conn = state.db.acquire().await?;
    let found = search_candidates(&mut conn, viewer_id, q, limit).await?;
    let ids: Vec<i64> = found.iter().map(|u| u.id).collect();
    let bios: std::collections::HashMap<i64, String> = repo::bios(&mut conn, &ids)
        .await?
        .into_iter()
        .map(|row| (row.user_id, row.bio))
        .collect();
    let followers = ik_social::interop::follower_counts(&mut conn, &ids).await?;
    let following: HashSet<i64> = match viewer_id {
        Some(viewer_id) => ik_social::interop::following_among(&mut conn, viewer_id, &ids)
            .await?
            .into_iter()
            .collect(),
        None => HashSet::new(),
    };
    drop(conn);
    let avatars = avatar_urls(state, &ids).await?;
    Ok(found
        .into_iter()
        .map(|u| UserSearchItem {
            avatar_url: avatars.get(&u.id).cloned(),
            id: Id::new(u.id),
            bio: bios
                .get(&u.id)
                .map(|b| b.chars().take(SEARCH_BIO_CHARS).collect())
                .unwrap_or_default(),
            follower_count: followers.get(&u.id).copied().unwrap_or(0),
            is_following: following.contains(&u.id),
            username: u.username,
            level: u.level,
        })
        .collect())
}

/// 这些用户的头像缩略图地址。
async fn avatar_urls(
    state: &ProfileState,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, String>, Problem> {
    let authors = ik_auth::service::authors(&state.db, ids).await?;
    state.media.avatar_urls(authors.values()).await
}

/// @ 提及候选（需要登录，PROFILE-023）。关键词必须是 1–32 字。
pub async fn mention_candidates(
    state: &ProfileState,
    principal: &Principal,
    query: SearchQuery,
) -> Result<Vec<MentionCandidate>, Problem> {
    rate_limited(&state.limits.per_user, &principal.user_id.get())?;
    let q = query.q.trim();
    if q.is_empty() {
        return Err(Problem::field("q", "REQUIRED", "请输入关键词"));
    }
    if q.chars().count() > SEARCH_MAX_QUERY_CHARS {
        return Err(Problem::field("q", "TOO_LONG", "关键词过长"));
    }
    let limit =
        usize::try_from(query.limit.unwrap_or(MENTION_DEFAULT).clamp(1, MENTION_MAX)).unwrap_or(8);
    let mut conn = state.db.acquire().await?;
    let found = search_candidates(&mut conn, Some(principal.user_id.get()), q, limit).await?;
    drop(conn);
    let ids: Vec<i64> = found.iter().map(|u| u.id).collect();
    let avatars = avatar_urls(state, &ids).await?;
    Ok(found
        .into_iter()
        .map(|u| MentionCandidate {
            avatar_url: avatars.get(&u.id).cloned(),
            id: Id::new(u.id),
            username: u.username,
            level: u.level,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn username_rules() {
        assert_eq!(validate_username("  绳网小明 ").unwrap(), "绳网小明");
        assert!(validate_username("ab").is_err());
        assert!(validate_username(&"字".repeat(21)).is_err());
        assert!(validate_username(&"字".repeat(20)).is_ok());
        assert!(validate_username("a@bcd").is_err());
        assert!(validate_username("abc\u{0007}d").is_err());
        assert!(validate_username("Alice Smith").is_ok());
    }
}
