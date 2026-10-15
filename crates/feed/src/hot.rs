//! 热榜（HOT-001～007，对应 Discourse TopicHotScore）。
//!
//! 计分是纯函数 [`score`]；[`compute`] 读取候选帖与窗口内的点赞、评论活动，过滤无效行为人后打分排序，
//! 得到完整榜单（热门浏览流）与展示位榜单（首页注入，同作者限流）。

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use ik_content::interop::{Activity, HotPost};
use ik_kernel::Problem;
use ik_kernel::config::Hot;
use sqlx::PgPool;

/// 窗口外补查候选帖时每批的 id 数。
const BACKFILL_BATCH: usize = 500;

/// 一次计算的结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HotBoard {
    /// 完整榜单（热门浏览流，不限流），最多 `feed_size` 篇。
    pub full: Vec<i64>,
    /// 展示位（首页注入），最多 `count` 篇，同一作者最多 `max_per_author` 篇，匿名帖不参与限流。
    pub display: Vec<i64>,
    pub computed_at: Option<DateTime<Utc>>,
}

/// 一篇帖子的热度输入。
#[derive(Debug, Clone, Copy)]
pub struct ScoreInput {
    /// 帖子「创建时间」：首次发布时间。
    pub created_at: DateTime<Utc>,
    /// 累计点赞数（老帖用）。
    pub like_count: i32,
    /// 窗口内点赞的不同有效用户数（Lr）。
    pub recent_likers: usize,
    /// 窗口内评论的不同有效用户数（Cr）。
    pub recent_commenters: usize,
    /// 窗口内最早一条有效评论的时间。
    pub first_comment_at: Option<DateTime<Utc>>,
}

/// 计分结果：分数与参与排序的 f（最早一次近期参与）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Score {
    pub value: f64,
    pub first_activity: Option<DateTime<Utc>>,
}

fn age_hours(now: DateTime<Utc>, t: DateTime<Utc>) -> f64 {
    let ms = (now - t).num_milliseconds().max(0);
    // i64 → f64 只在超过 2^53 毫秒（约 28 万年）时损失精度。
    #[allow(clippy::cast_precision_loss)]
    let ms = ms as f64;
    ms / 3_600_000.0 + 2.0
}

#[allow(clippy::cast_precision_loss)]
fn count(n: usize) -> f64 {
    n as f64
}

/// HOT-002：
///
/// ```text
/// h(t)  = max(0, now − t) / 1h + 2
/// o     = 1 若 c 在窗口内，否则 0；  Pr = Cr + o
/// likes = c 在窗口内 ? Lr : 累计点赞数
/// f     = min({c | o=1} ∪ {窗口内最早一条有效评论时间})
/// score = likes / h(c)^g + [f 存在] · (Lr + Pr − 1) / h(f)^g
/// ```
pub fn score(input: &ScoreInput, now: DateTime<Utc>, since: DateTime<Utc>, gravity: f64) -> Score {
    let c = input.created_at;
    let opened_in_window = since <= c && c <= now;
    let lr = count(input.recent_likers);
    let pr = count(input.recent_commenters) + if opened_in_window { 1.0 } else { 0.0 };
    let likes = if c > since {
        lr
    } else {
        f64::from(input.like_count.max(0))
    };
    let first_activity = match (opened_in_window.then_some(c), input.first_comment_at) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };
    let mut value = likes / age_hours(now, c).powf(gravity);
    if let Some(f) = first_activity {
        value += (lr + pr - 1.0) / age_hours(now, f).powf(gravity);
    }
    Score {
        value,
        first_activity,
    }
}

struct Ranked {
    id: i64,
    author_id: i64,
    is_anonymous: bool,
    bumped_at: DateTime<Utc>,
    score: Score,
}

/// HOT-003：score DESC → f DESC（无 f 最低）→ bumpedAt DESC → id DESC。
fn rank_order(a: &Ranked, b: &Ranked) -> Ordering {
    b.score
        .value
        .partial_cmp(&a.score.value)
        .unwrap_or(Ordering::Equal)
        .then_with(|| b.score.first_activity.cmp(&a.score.first_activity))
        .then_with(|| b.bumped_at.cmp(&a.bumped_at))
        .then_with(|| b.id.cmp(&a.id))
}

/// 展示位：同一作者最多 `max_per_author` 篇（0 = 不限），匿名帖不参与限流（否则能从
/// 「谁被挤掉」反推匿名作者，HOT-007）。
fn display_slots(ranked: &[Ranked], count: usize, max_per_author: usize) -> Vec<i64> {
    let mut per_author: HashMap<i64, usize> = HashMap::new();
    let mut out = Vec::with_capacity(count);
    for post in ranked {
        if out.len() >= count {
            break;
        }
        if max_per_author > 0 && !post.is_anonymous {
            let used = per_author.entry(post.author_id).or_default();
            if *used >= max_per_author {
                continue;
            }
            *used += 1;
        }
        out.push(post.id);
    }
    out
}

/// 计算热榜。
pub async fn compute(db: &PgPool, settings: &Hot, now: DateTime<Utc>) -> Result<HotBoard, Problem> {
    let since = now - Duration::days(i64::from(settings.recent_days));
    let mut conn = db.acquire().await?;

    let mut posts: HashMap<i64, HotPost> =
        ik_content::interop::hot_recent_posts(&mut conn, settings.candidates)
            .await?
            .into_iter()
            .map(|p| (p.id, p))
            .collect();
    let likes = ik_content::interop::like_activity(&mut conn, since, now).await?;
    let comments = ik_comments::interop::comment_activity(&mut conn, since, now).await?;

    // 窗口内有活动、但不在最近活跃里的帖子也是候选（HOT-004）。
    let mut missing: Vec<i64> = likes
        .iter()
        .chain(comments.iter())
        .map(|a| a.post_id)
        .filter(|id| !posts.contains_key(id))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    for chunk in missing.chunks(BACKFILL_BATCH) {
        for post in ik_content::interop::hot_posts(&mut conn, chunk).await? {
            posts.insert(post.id, post);
        }
    }

    // 有效行为人：状态正常、不是 AI（HOT-005）。
    let mut actors: Vec<i64> = likes
        .iter()
        .chain(comments.iter())
        .map(|a| a.user_id)
        .collect();
    actors.sort_unstable();
    actors.dedup();
    let valid: HashSet<i64> = ik_auth::interop::active_users(&mut conn, &actors)
        .await?
        .into_iter()
        .filter(|u| !u.is_ai_agent)
        .map(|u| u.id)
        .collect();
    drop(conn);

    // 只统计候选帖上、有效行为人、且不是作者本人的行为。
    let counts = |activity: &[Activity]| {
        let mut by_post: HashMap<i64, (usize, Option<DateTime<Utc>>)> = HashMap::new();
        for a in activity {
            let Some(post) = posts.get(&a.post_id) else {
                continue;
            };
            if a.user_id == post.author_id || !valid.contains(&a.user_id) {
                continue;
            }
            let entry = by_post.entry(a.post_id).or_insert((0, None));
            entry.0 += 1;
            entry.1 = Some(entry.1.map_or(a.first_at, |t| t.min(a.first_at)));
        }
        by_post
    };
    let like_counts = counts(&likes);
    let comment_counts = counts(&comments);

    let mut ranked: Vec<Ranked> = posts
        .values()
        .filter_map(|post| {
            let (recent_likers, _) = like_counts.get(&post.id).copied().unwrap_or((0, None));
            let (recent_commenters, first_comment_at) =
                comment_counts.get(&post.id).copied().unwrap_or((0, None));
            let score = score(
                &ScoreInput {
                    created_at: post.created_at,
                    like_count: post.like_count,
                    recent_likers,
                    recent_commenters,
                    first_comment_at,
                },
                now,
                since,
                settings.gravity,
            );
            (score.value > 0.0).then_some(Ranked {
                id: post.id,
                author_id: post.author_id,
                is_anonymous: post.is_anonymous,
                bumped_at: post.bumped_at,
                score,
            })
        })
        .collect();
    ranked.sort_by(rank_order);
    ranked.truncate(settings.feed_size);

    Ok(HotBoard {
        display: display_slots(&ranked, settings.count, settings.max_per_author),
        full: ranked.iter().map(|r| r.id).collect(),
        computed_at: Some(now),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn at(hours_ago: i64, now: DateTime<Utc>) -> DateTime<Utc> {
        now - Duration::hours(hours_ago)
    }

    fn input(created_hours_ago: i64, now: DateTime<Utc>) -> ScoreInput {
        ScoreInput {
            created_at: at(created_hours_ago, now),
            like_count: 0,
            recent_likers: 0,
            recent_commenters: 0,
            first_comment_at: None,
        }
    }

    #[test]
    fn new_post_uses_recent_likers_and_opening_counts_as_participation() {
        let now = Utc::now();
        let since = now - Duration::days(7);
        let mut post = input(10, now);
        post.recent_likers = 3;
        post.like_count = 100; // 新帖不用累计点赞
        let s = score(&post, now, since, 1.2);
        let h = 12.0_f64.powf(1.2);
        // likes/h(c) + (Lr + Cr + 1 − 1)/h(c)
        assert!((s.value - (3.0 / h + 3.0 / h)).abs() < 1e-9, "{}", s.value);
        assert_eq!(s.first_activity, Some(post.created_at));
    }

    #[test]
    fn old_post_uses_total_likes_and_needs_recent_comments_for_second_term() {
        let now = Utc::now();
        let since = now - Duration::days(7);
        let mut post = input(24 * 30, now);
        post.like_count = 50;
        let quiet = score(&post, now, since, 1.2);
        assert!(quiet.first_activity.is_none());
        let first = 50.0 / (24.0 * 30.0 + 2.0_f64).powf(1.2);
        assert!((quiet.value - first).abs() < 1e-9);

        // 老帖被新回复带回榜单（hot-rank.test.ts）。
        post.recent_commenters = 2;
        post.first_comment_at = Some(at(1, now));
        let revived = score(&post, now, since, 1.2);
        assert!(revived.value > quiet.value);
        let second = (0.0 + 2.0 - 1.0) / 3.0_f64.powf(1.2);
        assert!((revived.value - (first + second)).abs() < 1e-9);
    }

    #[test]
    fn untouched_old_post_without_likes_scores_zero() {
        let now = Utc::now();
        let s = score(&input(24 * 30, now), now, now - Duration::days(7), 1.2);
        assert!(s.value <= 0.0);
    }

    fn ranked(id: i64, author: i64, anonymous: bool, value: f64) -> Ranked {
        Ranked {
            id,
            author_id: author,
            is_anonymous: anonymous,
            bumped_at: Utc::now(),
            score: Score {
                value,
                first_activity: None,
            },
        }
    }

    #[test]
    fn display_slots_cap_named_authors_but_not_anonymous_posts() {
        let list = vec![
            ranked(1, 10, false, 9.0),
            ranked(2, 10, false, 8.0),
            ranked(3, 10, true, 7.0),
            ranked(4, 10, true, 6.0),
            ranked(5, 20, false, 5.0),
        ];
        assert_eq!(display_slots(&list, 4, 1), vec![1, 3, 4, 5]);
        assert_eq!(display_slots(&list, 4, 0), vec![1, 2, 3, 4]);
        assert_eq!(display_slots(&list, 2, 2), vec![1, 2]);
    }

    #[test]
    fn ties_break_by_activity_then_bump_then_id() {
        let now = Utc::now();
        let mut a = ranked(1, 1, false, 1.0);
        let mut b = ranked(2, 2, false, 1.0);
        a.score.first_activity = Some(at(2, now));
        b.score.first_activity = None;
        assert_eq!(rank_order(&a, &b), Ordering::Less, "有近期活动的排前");
        b.score.first_activity = a.score.first_activity;
        b.bumped_at = a.bumped_at;
        assert_eq!(rank_order(&a, &b), Ordering::Greater, "最后按 id 倒序");
    }

    proptest! {
        #[test]
        fn more_recent_likers_never_lower_the_score(base in 0usize..50, extra in 1usize..50, hours in 0i64..400) {
            let now = Utc::now();
            let since = now - Duration::days(7);
            let mut post = input(hours, now);
            post.recent_likers = base;
            let low = score(&post, now, since, 1.2).value;
            post.recent_likers = base + extra;
            let high = score(&post, now, since, 1.2).value;
            prop_assert!(high >= low);
        }

        #[test]
        fn score_is_finite_and_non_negative(likes in 0i32..100_000, likers in 0usize..1000, commenters in 0usize..1000, hours in -5i64..10_000) {
            let now = Utc::now();
            let mut post = input(hours, now);
            post.like_count = likes;
            post.recent_likers = likers;
            post.recent_commenters = commenters;
            let s = score(&post, now, now - Duration::days(7), 1.2);
            prop_assert!(s.value.is_finite());
            prop_assert!(s.value >= 0.0);
        }
    }
}
