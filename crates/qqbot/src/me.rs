//! 站内的 QQ 绑定（ADR 0008 §4.2）：登录用户生成一次性绑定码，在 QQ 里发 `/绑定 <码>` 把发消息的那个 QQ 身份绑上。
//! 码由站内账号生成、由用户自己的消息最先用掉；反方向（QQ 出码、站内填）会让旁观者把别人的 QQ 绑到自己账号上。
use axum::extract::{FromRef, State};
use chrono::{DateTime, Duration, Utc};
use http::StatusCode;
use ik_auth::{AuthState, CurrentUser};
use ik_kernel::{
    Id, Problem,
    error::ProblemBody,
    extract::{Json, Path},
};
use ring::{hmac, rand::SecureRandom};
use serde::Serialize;
use sqlx::PgConnection;
use utoipa::ToSchema;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::QqState;

/// 码的字符：去掉易混的 0/O、1/I/L。
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LEN: usize = 8;
/// 有效期。
const CODE_TTL: Duration = Duration::minutes(10);
/// 同一账号同时有效的码最多几个。
const ACTIVE_CODES: i64 = 3;
/// 同一账号每小时最多生成几个。
const HOURLY_CODES: i64 = 10;

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BindCode {
    /// 8 位绑定码（大写字母与数字）。
    pub code: String,
    pub expires_at: DateTime<Utc>,
    /// 给用户看的用法说明。
    pub instruction: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    #[schema(value_type = String)]
    pub id: Id,
    /// `group` 群 / `c2c` 单聊。
    pub scene: String,
    /// 群的展示名（管理员在后台填，没有时为「QQ 群 #后四位」）；单聊为空。
    pub group_name: Option<String>,
    pub bound_at: Option<DateTime<Utc>>,
    pub last_active_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BindingList {
    pub items: Vec<Binding>,
}

pub(crate) fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    QqState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(create_code))
        .routes(routes!(bindings))
        .routes(routes!(unbind))
}

fn disabled() -> Problem {
    Problem::new(StatusCode::CONFLICT, "QQ_BOT_DISABLED", "QQ 机器人没有开启")
}

fn code_limit() -> Problem {
    Problem::new(
        StatusCode::TOO_MANY_REQUESTS,
        "QQ_BIND_CODE_LIMIT",
        "绑定码生成得太频繁了，请稍后再试",
    )
}

/// HMAC(key, "qq-bind:<码>")。库里只存它。
pub(crate) fn code_hash(key: &[u8], code: &str) -> Vec<u8> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::sign(&key, format!("qq-bind:{code}").as_bytes())
        .as_ref()
        .to_vec()
}

/// 用户在 QQ 里发来的码：去空白、转大写；格式不对时为 None。
pub(crate) fn normalize_code(raw: &str) -> Option<String> {
    let code: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    (code.len() == CODE_LEN && code.bytes().all(|b| ALPHABET.contains(&b))).then_some(code)
}

fn random_code() -> Result<String, Problem> {
    let rng = ring::rand::SystemRandom::new();
    let mut bytes = [0u8; CODE_LEN];
    let mut code = String::with_capacity(CODE_LEN);
    while code.len() < CODE_LEN {
        rng.fill(&mut bytes).map_err(|_| Problem::internal())?;
        // 拒绝采样：256 不是 31 的倍数，丢掉会带来偏差的那几个值。
        for b in bytes {
            let limit = 256 - 256 % ALPHABET.len();
            if usize::from(b) < limit && code.len() < CODE_LEN {
                code.push(char::from(ALPHABET[usize::from(b) % ALPHABET.len()]));
            }
        }
    }
    Ok(code)
}

/// 生成 QQ 绑定码
///
/// 一次性、10 分钟有效。在 QQ 里对机器人发 `/绑定 <码>`（单聊或在群里 @机器人）就把发消息的那个 QQ 身份绑到当前账号；
/// 同一个人在不同群里是不同的身份，每个群要各绑一次。同时最多 3 个有效码，每小时最多生成 10 个。
#[utoipa::path(
    post, operation_id = "qq_create_code",
    path = "/v1/me/qq/bind-codes",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = BindCode),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
        (status = 409, description = "QQ_BOT_DISABLED", body = ProblemBody, content_type = "application/problem+json"),
        (status = 429, description = "QQ_BIND_CODE_LIMIT", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn create_code(
    State(s): State<QqState>,
    CurrentUser(principal): CurrentUser,
) -> Result<(StatusCode, Json<BindCode>), Problem> {
    if !s.config.enabled || !s.settings().await?.enabled {
        return Err(disabled());
    }
    let user_id = principal.user_id.get();
    let now = Utc::now();
    let mut tx = s.db.begin().await?;
    // 同一账号串行生成，数量检查才可靠。
    sqlx::query!(
        "SELECT pg_advisory_xact_lock(hashtextextended('ik:qq_bind_codes:' || $1::bigint, 0))",
        user_id
    )
    .execute(&mut *tx)
    .await?;
    let counts = sqlx::query!(
        r#"SELECT count(*) FILTER (WHERE used_at IS NULL AND expires_at > $2) AS "active!",
                  count(*) FILTER (WHERE created_at > $3) AS "hourly!"
           FROM qq_bind_codes WHERE user_id = $1"#,
        user_id,
        now,
        now - Duration::hours(1)
    )
    .fetch_one(&mut *tx)
    .await?;
    if counts.active >= ACTIVE_CODES || counts.hourly >= HOURLY_CODES {
        return Err(code_limit());
    }
    let code = random_code()?;
    let expires_at = now + CODE_TTL;
    sqlx::query!(
        "INSERT INTO qq_bind_codes (code_hash, user_id, expires_at, created_at) VALUES ($1, $2, $3, $4)",
        code_hash(&s.code_key, &code),
        user_id,
        expires_at,
        now
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((
        StatusCode::CREATED,
        Json(BindCode {
            instruction: format!(
                "10 分钟内在 QQ 里对机器人发送「/绑定 {code}」（单聊，或在群里 @机器人）。在哪个群用机器人，就在哪个群绑一次。"
            ),
            code,
            expires_at,
        }),
    ))
}

/// 我绑定的 QQ 身份
#[utoipa::path(
    get, operation_id = "qq_bindings",
    path = "/v1/me/qq/bindings",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = BindingList),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn bindings(
    State(s): State<QqState>,
    CurrentUser(principal): CurrentUser,
) -> Result<Json<BindingList>, Problem> {
    let rows = sqlx::query!(
        r#"SELECT i.id, i.scene, i.bound_at, i.last_active_at, g.display_name AS "display_name?", g.group_openid AS "group_openid?"
           FROM qq_identities i LEFT JOIN qq_groups g ON g.id = i.group_id
           WHERE i.user_id = $1 ORDER BY i.bound_at DESC NULLS LAST, i.id DESC"#,
        principal.user_id.get()
    )
    .fetch_all(&s.db)
    .await?;
    Ok(Json(BindingList {
        items: rows
            .into_iter()
            .map(|r| Binding {
                id: Id::new(r.id),
                group_name: r.group_openid.map(|openid| {
                    crate::repo::group_name(r.display_name.unwrap_or_default(), &openid)
                }),
                scene: r.scene,
                bound_at: r.bound_at,
                last_active_at: r.last_active_at,
            })
            .collect(),
    }))
}

/// 解绑一个 QQ 身份
#[utoipa::path(
    delete, operation_id = "qq_unbind",
    path = "/v1/me/qq/bindings/{id}",
    tag = "me",
    security(("session_cookie" = []), ("bearer" = [])),
    params(("id" = String, Path)),
    responses(
        (status = 204),
        (status = 401, body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn unbind(
    State(s): State<QqState>,
    CurrentUser(principal): CurrentUser,
    Path(id): Path<Id>,
) -> Result<StatusCode, Problem> {
    let mut conn = s.db.acquire().await?;
    if !release(&mut conn, id.get(), Some(principal.user_id.get())).await? {
        return Err(Problem::not_found());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// 解除绑定。`owner` 不为空时只解这个账号的。返回有没有解掉。
pub(crate) async fn release(
    conn: &mut PgConnection,
    identity_id: i64,
    owner: Option<i64>,
) -> Result<bool, Problem> {
    Ok(sqlx::query(
        "UPDATE qq_identities SET user_id = NULL, bound_at = NULL,
             binding_version = binding_version + 1, context_reset_at = NULL
         WHERE id = $1 AND user_id IS NOT NULL AND ($2::bigint IS NULL OR user_id = $2)",
    )
    .bind(identity_id)
    .bind(owner)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn codes_are_unambiguous_and_normalize() {
        for _ in 0..50 {
            let code = random_code().unwrap();
            assert_eq!(code.len(), CODE_LEN);
            assert_eq!(normalize_code(&code.to_lowercase()), Some(code.clone()));
        }
        assert_eq!(normalize_code(" abcd-2345 "), Some("ABCD2345".to_owned()));
        // 含易混字符（0、O、1、I、L）或长度不对的不是合法的码。
        assert_eq!(normalize_code("ABCD0345"), None);
        assert_eq!(normalize_code("ABCDI345"), None);
        assert_eq!(normalize_code("ABC2345"), None);
        assert_ne!(code_hash(b"k", "ABCD2345"), code_hash(b"k2", "ABCD2345"));
    }
}
