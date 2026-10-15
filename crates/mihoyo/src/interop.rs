//! 给其他域用的接口。

use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_kernel::Problem;
use sqlx::PgConnection;

use crate::dto::{QrMode, ZzzProfile};
use crate::repo;
use crate::service::{Outcome, id_hash, parse_mode, user_mismatch};

/// 作者主页展示的绝区零角色（PROFILE-017）。没有绑定或没有角色资料时为 None。
pub async fn zzz_profile(
    conn: &mut PgConnection,
    user_id: i64,
) -> Result<Option<ZzzProfile>, Problem> {
    Ok(repo::zzz_of(conn, user_id).await?.and_then(|row| {
        Some(ZzzProfile {
            uid: row.zzz_uid?,
            nickname: row.zzz_nickname.unwrap_or_default(),
            level: row.zzz_level.unwrap_or(0),
            region_name: row.zzz_region_name.unwrap_or_default(),
        })
    }))
}

/// 账号是否绑定了米游社（账号安全概览）。
pub async fn is_bound(conn: &mut PgConnection, user_id: i64) -> Result<bool, Problem> {
    Ok(repo::binding_of(conn, user_id).await?.is_some())
}

/// 删除绑定（注销账号时，DEL-006）。
pub async fn remove_binding(conn: &mut PgConnection, user_id: i64) -> Result<(), Problem> {
    repo::delete_binding(conn, user_id).await?;
    Ok(())
}

/// 在注销事务里核对并消费扫码注销凭证（MHY-017、MHY-018）：结果不存在或过期 → 400
/// `MIHOYO_DELETE_TICKET_EXPIRED`；不是注销核验 → 400 `MIHOYO_DELETE_TICKET_INVALID`；
/// 不是本人发起 → 403 `MIHOYO_BIND_USER_MISMATCH`；扫的不是绑定的米游社账号 → 403
/// `MIHOYO_DELETE_ACCOUNT_MISMATCH`。通过后删除扫码会话：与注销同一事务，回滚时凭证还在，可以重试。
pub async fn take_delete_verification(
    conn: &mut PgConnection,
    user_id: i64,
    qr_id: &str,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    let expired = || {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "MIHOYO_DELETE_TICKET_EXPIRED",
            "扫码验证已过期，请重新扫码",
        )
    };
    let hash = id_hash(qr_id);
    let row = repo::lock_qr(conn, &hash).await?.ok_or_else(expired)?;
    if parse_mode(&row.mode) != QrMode::Delete {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "MIHOYO_DELETE_TICKET_INVALID",
            "这不是注销用的扫码验证",
        ));
    }
    if row.user_id != Some(user_id) {
        return Err(user_mismatch());
    }
    let (Some(result), Some(until)) = (row.result, row.result_expires_at) else {
        return Err(expired());
    };
    if until <= now {
        return Err(expired());
    }
    let outcome: Outcome =
        serde_json::from_value(result).map_err(|e| Problem::internal_from(&e))?;
    if !matches!(outcome, Outcome::DeleteChecked { matched: true, .. }) {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "MIHOYO_DELETE_ACCOUNT_MISMATCH",
            "扫码的米游社账号与当前账号不一致，请使用当前账号绑定的米游社扫码",
        ));
    }
    repo::delete_qr(conn, &hash).await?;
    Ok(())
}
