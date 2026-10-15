//! 给其他域用的接口。

use ik_kernel::Problem;
use sqlx::PgConnection;

use crate::dto::ZzzProfile;
use crate::repo;

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
