//! QQ 身份快照的执行与事务内复核。
use crate::identity::Snapshot;
use ik_ai::authorization::{Authorization, CheckFuture};
use ik_kernel::Problem;
use sqlx::PgConnection;

#[derive(Debug, Clone)]
pub struct QqAuthorization {
    pub snapshot: Snapshot,
    role: Option<String>,
}

impl QqAuthorization {
    pub fn new(snapshot: Snapshot) -> Self {
        Self {
            snapshot,
            role: None,
        }
    }
    pub fn with_role(mut self, slug: String) -> Self {
        self.role = Some(slug);
        self
    }
}

impl Authorization for QqAuthorization {
    fn check<'a>(&'a self, conn: &'a mut PgConnection) -> CheckFuture<'a> {
        Box::pin(async move {
            if !validate(conn, &self.snapshot).await? {
                return Ok(false);
            }
            if let Some(slug) = &self.role {
                return ik_ai::external::role_enabled_in(conn, slug).await;
            }
            Ok(true)
        })
    }
}

/// 固定顺序：账号 → QQ 身份。注销与授权写操作也遵循这个顺序。
pub async fn validate(conn: &mut PgConnection, snapshot: &Snapshot) -> Result<bool, Problem> {
    // 已撤销的旧版本无需再锁旧账号，避免重试在持有当前身份锁时反向等待旧账号。
    let current: Option<(Option<i64>, i64)> =
        sqlx::query_as("SELECT user_id,binding_version FROM qq_identities WHERE id=$1")
            .bind(snapshot.identity_id)
            .fetch_optional(&mut *conn)
            .await?;
    if current != Some((snapshot.owner_user_id, snapshot.binding_version)) {
        return Ok(false);
    }
    if let Some(owner) = snapshot.owner_user_id {
        let status = ik_auth::interop::lock_user_status(conn, owner).await?;
        if status.as_deref() != Some("active") {
            return Ok(false);
        }
    }
    let channel_allowed = lock_channel(conn, snapshot.identity_id).await?;
    let row:Option<(Option<i64>,i64,bool)>=sqlx::query_as("SELECT user_id,binding_version,NOT blocked AND available FROM qq_identities WHERE id=$1 FOR SHARE")
        .bind(snapshot.identity_id).fetch_optional(conn).await?;
    Ok(row.is_some_and(|(owner, version, allowed)| {
        channel_allowed
            && allowed
            && owner == snapshot.owner_user_id
            && version == snapshot.binding_version
    }))
}

/// 与入站的 group → identity 顺序一致；锁仅随调用方短事务存活。
pub(crate) async fn lock_channel(
    conn: &mut PgConnection,
    identity_id: i64,
) -> Result<bool, Problem> {
    let channel: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT app_id,group_id FROM qq_identities WHERE id=$1")
            .bind(identity_id)
            .fetch_optional(&mut *conn)
            .await?;
    let Some((app, group)) = channel else {
        return Ok(false);
    };
    let settings: bool =
        sqlx::query_scalar("SELECT enabled FROM qq_settings WHERE singleton FOR SHARE")
            .fetch_one(&mut *conn)
            .await?;
    let bot: bool = sqlx::query_scalar("SELECT enabled FROM qq_bots WHERE app_id=$1 FOR SHARE")
        .bind(app)
        .fetch_one(&mut *conn)
        .await?;
    let group_allowed = if let Some(group) = group {
        sqlx::query_scalar::<_,bool>("SELECT enabled AND NOT blocked AND left_at IS NULL FROM qq_groups WHERE id=$1 FOR SHARE").bind(group).fetch_optional(&mut *conn).await?.unwrap_or(false)
    } else {
        true
    };
    Ok(settings && bot && group_allowed)
}
