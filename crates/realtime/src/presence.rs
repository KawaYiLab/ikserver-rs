//! 在线人数（PRES-001～006）。
//!
//! 在线 = 有实时连接（网页与 App 打开期间一直连着 `/v1/realtime`）。旧系统靠每个页面每 20 秒 HTTP 心跳；
//! 这里直接数连接，不需要额外请求。每个实例每 15 秒把自己的连接快照写进 `realtime_presence`
//! （一行一个实例），读的时候合并 45 秒内更新过的行：多实例下同一用户只算一次，实例宕机后它的行自然过期。

use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;

use crate::Hub;

/// 快照写入间隔。
const PUBLISH_EVERY: Duration = Duration::from_secs(15);
/// 多久没更新的实例视为下线（PRES-003 的 45 秒在线窗口）。
const STALE_AFTER_SECS: f64 = 45.0;
/// 最近在线用户最多取多少个（调用方再按可见性筛出头像）。
const RECENT_USERS: i64 = 64;

/// 全站在线情况。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Online {
    /// 在线人数：登录用户去重 + 游客连接数。
    pub count: i64,
    /// 最近建立连接的登录用户（最多 64 个，新的在前）。
    pub recent_user_ids: Vec<i64>,
}

/// 写入本实例的快照。
pub async fn publish(db: &PgPool, node_id: u16, hub: &Hub) -> sqlx::Result<()> {
    let presence = hub.presence();
    let guests = i32::try_from(presence.guests).unwrap_or(i32::MAX);
    sqlx::query!(
        "INSERT INTO realtime_presence (node_id, user_ids, guests, updated_at)
         VALUES ($1, $2, $3, now())
         ON CONFLICT (node_id) DO UPDATE
         SET user_ids = EXCLUDED.user_ids, guests = EXCLUDED.guests, updated_at = EXCLUDED.updated_at",
        i32::from(node_id),
        &presence.user_ids,
        guests,
    )
    .execute(db)
    .await?;
    Ok(())
}

/// 停机时撤下本实例的快照（否则要等 45 秒才过期）。
pub async fn withdraw(db: &PgPool, node_id: u16) -> sqlx::Result<()> {
    sqlx::query!(
        "DELETE FROM realtime_presence WHERE node_id = $1",
        i32::from(node_id)
    )
    .execute(db)
    .await?;
    Ok(())
}

/// 在当前 tokio 运行时上每 15 秒写一次快照。写失败只记日志（读的一方看到的是上一次的快照，过期后不计）。
pub fn spawn_publisher(db: PgPool, node_id: u16, hub: Arc<Hub>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(PUBLISH_EVERY);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            if let Err(error) = publish(&db, node_id, &hub).await {
                tracing::warn!(%error, "写入在线快照失败");
            }
        }
    })
}

/// 合并所有存活实例的快照。
pub async fn online(db: &PgPool) -> sqlx::Result<Online> {
    let row = sqlx::query!(
        r#"WITH live AS (
               SELECT user_ids, guests FROM realtime_presence
               WHERE updated_at > now() - make_interval(secs => $1)
           ),
           users AS (
               SELECT u.user_id, min(u.n) AS rank
               FROM live, unnest(live.user_ids) WITH ORDINALITY AS u(user_id, n)
               GROUP BY u.user_id
           )
           SELECT (SELECT count(*) FROM users) + (SELECT coalesce(sum(guests), 0) FROM live)
                      AS "count!",
                  coalesce((SELECT array_agg(user_id ORDER BY rank, user_id)
                            FROM (SELECT user_id, rank FROM users ORDER BY rank, user_id LIMIT $2) r),
                           '{}') AS "recent!""#,
        STALE_AFTER_SECS,
        RECENT_USERS,
    )
    .fetch_one(db)
    .await?;
    Ok(Online {
        count: row.count,
        recent_user_ids: row.recent,
    })
}
