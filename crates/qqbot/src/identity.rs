//! 一次对话首次执行时的授权快照；重试不能随当前绑定改变历史归属。
use ik_kernel::Problem;
use sqlx::PgConnection;

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Snapshot {
    pub identity_id: i64,
    pub owner_user_id: Option<i64>,
    pub binding_version: i64,
}

/// 普通对话在首次执行时调用；身份变更命令在提交变更的同一事务末调用。
/// `owner=NULL, version=Some` 是游客；version=NULL 的旧记录不会被提炼任务猜测归属。
pub async fn freeze(conn: &mut PgConnection, message_id: i64) -> Result<Snapshot, Problem> {
    let identity_id: i64 = sqlx::query_scalar("SELECT identity_id FROM qq_messages WHERE id=$1")
        .bind(message_id)
        .fetch_one(&mut *conn)
        .await?;
    lock_identity(conn, identity_id).await?;
    // 迁移统一排除了旧历史；尚未执行的普通排队消息在新协议首次授权后仍应参与提炼。
    // 已冻结重试、已有生成记录及命令不能清掉原有处理标记。
    sqlx::query("UPDATE qq_turns t SET personal_memory_processed_at=NULL,group_memory_processed_at=NULL FROM qq_messages m WHERE m.id=$1 AND t.message_id=m.id AND m.binding_version IS NULL AND m.direction='in' AND m.content NOT LIKE '/%' AND t.status='queued' AND t.hold_id IS NULL AND t.ai_user_id IS NULL AND t.output IS NULL")
        .bind(message_id).execute(&mut *conn).await?;
    sqlx::query("UPDATE qq_messages m SET owner_user_id=i.user_id, binding_version=i.binding_version
        FROM qq_identities i WHERE m.id=$1 AND m.identity_id=i.id AND m.direction='in' AND m.binding_version IS NULL")
        .bind(message_id).execute(&mut *conn).await?;
    sqlx::query_as::<_, Snapshot>("SELECT identity_id, owner_user_id, binding_version FROM qq_messages WHERE id=$1 AND binding_version IS NOT NULL")
        .bind(message_id).fetch_optional(conn).await?.ok_or_else(Problem::not_found)
}

/// 在获取 turn/message 写锁之前，统一按账号 → 身份的顺序锁定。
pub async fn lock_turn_identity(conn: &mut PgConnection, turn_id: i64) -> Result<(), Problem> {
    let identity_id: Option<i64> =
        sqlx::query_scalar("SELECT identity_id FROM qq_turns WHERE id=$1")
            .bind(turn_id)
            .fetch_optional(&mut *conn)
            .await?;
    if let Some(identity_id) = identity_id {
        lock_identity(conn, identity_id).await?;
    }
    Ok(())
}

async fn lock_identity(conn: &mut PgConnection, identity_id: i64) -> Result<(), Problem> {
    let owner: Option<i64> = sqlx::query_scalar("SELECT user_id FROM qq_identities WHERE id=$1")
        .bind(identity_id)
        .fetch_one(&mut *conn)
        .await?;
    if let Some(owner) = owner {
        ik_auth::interop::lock_user_status(conn, owner).await?;
    }
    crate::authorization::lock_channel(conn, identity_id).await?;
    let current: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM qq_identities WHERE id=$1 FOR UPDATE")
            .bind(identity_id)
            .fetch_one(conn)
            .await?;
    if current != owner {
        return Err(Problem::service_unavailable());
    }
    Ok(())
}

pub async fn freeze_after_command(
    conn: &mut PgConnection,
    message_id: i64,
) -> Result<Snapshot, Problem> {
    freeze(conn, message_id).await
}

/// server 注入 auth.lifecycle 的组合 hook；账号注销事务已经持有用户行锁。
pub struct AccountDeletionHook;

impl ik_auth::lifecycle::AccountLifecycle for AccountDeletionHook {
    fn deleting<'a>(
        &'a self,
        conn: &'a mut PgConnection,
        user_id: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> ik_auth::lifecycle::DeleteFuture<'a> {
        Box::pin(async move {
            // 锁与授权检查一致：账号 → 身份 → turn。不要删除仍需结算的 turn。
            let identities: Vec<i64> = sqlx::query_scalar(
                "SELECT id FROM qq_identities WHERE user_id=$1 ORDER BY id FOR UPDATE",
            )
            .bind(user_id)
            .fetch_all(&mut *conn)
            .await?;
            sqlx::query("UPDATE qq_identities SET user_id=NULL,bound_at=NULL,binding_version=binding_version+1,nickname='',context_reset_at=NULL WHERE id=ANY($1)")
                .bind(&identities).execute(&mut *conn).await?;
            sqlx::query("DELETE FROM qq_bind_codes WHERE user_id=$1")
                .bind(user_id)
                .execute(&mut *conn)
                .await?;
            // 尚未首次执行的旧队列消息随这次授权撤销，不能注销后再以游客身份复活。
            sqlx::query("UPDATE qq_turns t SET status='skipped',reason='account_deleted',finished_at=$2,personal_memory_processed_at=$2,group_memory_processed_at=$2 FROM qq_messages m WHERE m.id=t.message_id AND m.binding_version IS NULL AND t.identity_id=ANY($1) AND t.status='queued'")
                .bind(&identities).bind(now).execute(&mut *conn).await?;
            let turns: Vec<i64> = sqlx::query_scalar("SELECT t.id FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id WHERE m.owner_user_id=$1 OR t.payer_user_id=$1 OR (t.identity_id=ANY($2) AND t.reason='account_deleted') ORDER BY t.id FOR UPDATE OF t")
                .bind(user_id).bind(&identities).fetch_all(&mut *conn).await?;
            sqlx::query("UPDATE qq_turns SET output=NULL,personal_memory_processed_at=$2,group_memory_processed_at=$2 WHERE id=ANY($1)")
                .bind(&turns).bind(now).execute(&mut *conn).await?;
            sqlx::query("UPDATE qq_reply_segments SET content='' WHERE turn_id=ANY($1)")
                .bind(&turns)
                .execute(&mut *conn)
                .await?;
            sqlx::query("UPDATE qq_messages m SET content='' WHERE m.owner_user_id=$1 OR EXISTS(SELECT 1 FROM qq_turns t WHERE t.id=ANY($2) AND (m.id=t.message_id OR m.reply_to_id=t.message_id))")
                .bind(user_id).bind(&turns).execute(&mut *conn).await?;
            sqlx::query("DELETE FROM qq_memory_progress WHERE user_id=$1")
                .bind(user_id)
                .execute(&mut *conn)
                .await?;
            ik_ai::memory::clear_for_deletion(conn, user_id, now).await?;
            Ok(())
        })
    }
}
