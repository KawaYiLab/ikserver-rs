use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;

use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use ik_comments::CommentsState;
use ik_content::ContentState;
use ik_kernel::Snowflake;
use ik_media::MediaState;
use sqlx::PgPool;

/// 用户搜索与 @ 候选：登录用户每秒 10 次（PROFILE-023/024）。
const SEARCHES_PER_SECOND_USER: NonZeroU32 = match NonZeroU32::new(10) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};
/// 游客按 IP 每秒 3 次（PROFILE-024）。
const SEARCHES_PER_SECOND_IP: NonZeroU32 = match NonZeroU32::new(3) {
    Some(n) => n,
    None => NonZeroU32::MIN,
};

#[derive(Clone)]
pub struct ProfileState {
    pub db: PgPool,
    pub ids: Arc<Snowflake>,
    pub content: ContentState,
    pub comments: CommentsState,
    pub media: MediaState,
    pub(crate) limits: Arc<SearchLimits>,
}

pub(crate) struct SearchLimits {
    pub per_user: DefaultKeyedRateLimiter<i64>,
    pub per_ip: DefaultKeyedRateLimiter<IpAddr>,
}

impl ProfileState {
    pub fn new(
        db: PgPool,
        ids: Arc<Snowflake>,
        content: ContentState,
        comments: CommentsState,
        media: MediaState,
    ) -> Self {
        Self {
            db,
            ids,
            content,
            comments,
            media,
            limits: Arc::new(SearchLimits {
                per_user: RateLimiter::keyed(Quota::per_second(SEARCHES_PER_SECOND_USER)),
                per_ip: RateLimiter::keyed(Quota::per_second(SEARCHES_PER_SECOND_IP)),
            }),
        }
    }
}
