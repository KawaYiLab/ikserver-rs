//! 在线人数：多实例快照的合并与过期。

#![allow(clippy::unwrap_used)]

use ik_realtime::Hub;
use ik_realtime::presence::{self, Online};
use sqlx::PgPool;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");

#[sqlx::test(migrator = "MIGRATOR")]
async fn merges_live_nodes_and_ignores_stale_ones(db: PgPool) {
    assert_eq!(presence::online(&db).await.unwrap(), Online::default());

    // 实例 2、3 存活（用户 7 同时连在两个实例上，只算一次）；实例 4 已经 1 分钟没更新。
    sqlx::query!(
        "INSERT INTO realtime_presence (node_id, user_ids, guests, updated_at) VALUES
             (2, '{7, 5}', 3, now()),
             (3, '{8, 7}', 1, now() - interval '10 seconds'),
             (4, '{9}', 50, now() - interval '1 minute')"
    )
    .execute(&db)
    .await
    .unwrap();
    // 本实例没有连接。
    presence::publish(&db, 1, &Hub::new()).await.unwrap();

    let online = presence::online(&db).await.unwrap();
    assert_eq!(online.count, 3 + 4);
    // 各实例里排第一的（最新的）在前。
    assert_eq!(online.recent_user_ids, vec![7, 8, 5]);

    presence::withdraw(&db, 2).await.unwrap();
    let online = presence::online(&db).await.unwrap();
    assert_eq!(online.count, 2 + 1);
    assert_eq!(online.recent_user_ids, vec![8, 7]);
}
