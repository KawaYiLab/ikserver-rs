//! `Idempotency-Key`：已登录用户的 POST 请求带上这个头时，同一用户同一个键 24 小时内只处理一次，
//! 之后的重放返回第一次的结果（状态码、响应体与 content-type / etag / location 头），并带
//! `Idempotent-Replayed: true`。
//!
//! - 只保存成功（2xx）的响应。失败的请求随事务回滚、没有副作用，所以删掉记录，客户端可以用同一个键重试；
//!   这样也不会把 429、403 之类会随时间变化的结果锁死 24 小时。
//! - 同一个键用在不同的请求上（方法、路径、查询、请求体任一不同）→ 422 `IDEMPOTENCY_KEY_REUSED`；
//!   第一次请求还在处理 → 409 `IDEMPOTENCY_IN_PROGRESS`。
//! - 处理放在独立任务里：客户端断开、请求超时都不会中断它，结果照样记下。占用在请求超时后再过 30 秒
//!   失效，覆盖实例崩溃的情况；那时如果业务已经提交而结果没记下，重试会再处理一次（只在崩溃时发生）。
//! - 未登录的请求不做幂等处理（没有「同一用户」）；格式不对的键一律 400 `IDEMPOTENCY_KEY_INVALID`。

use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{FromRequestParts, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use ik_auth::MaybeUser;
use ik_kernel::Problem;
use ik_kernel::http::{current_request_id, with_request_id};
use ik_kernel::jobs::{IntoTaskHandlerResult, TaskHandler, WorkerContext};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tracing::Instrument;

use crate::AppState;

pub const HEADER: &str = "idempotency-key";
pub const REPLAYED_HEADER: &str = "idempotent-replayed";
/// 键的有效期。
const RETENTION: chrono::Duration = chrono::Duration::hours(24);
/// 超过这个大小的成功响应不保存（不能重放，同一个键的重试会再处理一次）。创建类接口的响应远小于它。
const MAX_STORED_BODY: usize = 256 * 1024;
/// 处理中的占用在请求超时之后再保留多久。
const LOCK_MARGIN: Duration = Duration::from_secs(30);
const REPLAYED: [HeaderName; 3] = [header::CONTENT_TYPE, header::ETAG, header::LOCATION];

pub async fn layer(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if request.method() != Method::POST {
        return next.run(request).await;
    }
    let Some(raw) = request.headers().get(HEADER) else {
        return next.run(request).await;
    };
    let Some(key) = raw.to_str().ok().and_then(parse_key) else {
        return Problem::new(
            StatusCode::BAD_REQUEST,
            "IDEMPOTENCY_KEY_INVALID",
            "幂等键格式不正确",
        )
        .into_response();
    };
    let (mut parts, body) = request.into_parts();
    let user_id = match MaybeUser::from_request_parts(&mut parts, &state).await {
        Ok(MaybeUser(Some(principal))) => principal.user_id.get(),
        Ok(MaybeUser(None)) => return next.run(Request::from_parts(parts, body)).await,
        Err(problem) => return problem.into_response(),
    };
    let Ok(bytes) = axum::body::to_bytes(body, state.config.http.body_limit_bytes).await else {
        return Problem::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "PAYLOAD_TOO_LARGE",
            "请求体过大",
        )
        .into_response();
    };
    let fingerprint = fingerprint(&parts.method, &parts.uri, &bytes);
    let now = micros(Utc::now());
    let lock = now
        + chrono::Duration::from_std(
            Duration::from_secs(state.config.http.request_timeout_secs) + LOCK_MARGIN,
        )
        .unwrap_or(chrono::Duration::minutes(1));
    let claim = Claim {
        user_id,
        key,
        fingerprint,
        created_at: now,
    };
    match claim.acquire(&state.db, lock).await {
        Ok(Outcome::Claimed) => {}
        Ok(Outcome::Replay(stored)) => return stored.into_response(),
        Ok(Outcome::InProgress) => {
            return Problem::conflict("IDEMPOTENCY_IN_PROGRESS", "相同的请求正在处理，请稍后再试")
                .with_retry_after(1)
                .into_response();
        }
        Ok(Outcome::Mismatch) => {
            return Problem::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "IDEMPOTENCY_KEY_REUSED",
                "这个幂等键已经用在了另一个请求上",
            )
            .into_response();
        }
        Err(error) => return Problem::from(error).into_response(),
    }

    let request = Request::from_parts(parts, Body::from(bytes));
    let db = state.db.clone();
    let work = async move {
        let response = next.run(request).await;
        claim.finish(&db, response).await
    };
    let request_id = current_request_id();
    let span = tracing::Span::current();
    let task = tokio::spawn(
        async move {
            match request_id {
                Some(id) => with_request_id(id, work).await,
                None => work.await,
            }
        }
        .instrument(span),
    );
    match task.await {
        Ok(response) => response,
        Err(error) => {
            tracing::error!(?error, "幂等请求的处理任务异常结束");
            Problem::internal().into_response()
        }
    }
}

/// 只接受标准 UUID 文本（8-4-4-4-12 位十六进制），统一成小写。
fn parse_key(raw: &str) -> Option<String> {
    let groups: Vec<&str> = raw.split('-').collect();
    let ok = groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()));
    ok.then(|| raw.to_ascii_lowercase())
}

fn fingerprint(method: &Method, uri: &http::Uri, body: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(method.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(uri.path_and_query().map_or("", |p| p.as_str()).as_bytes());
    hasher.update([0]);
    hasher.update(body);
    hasher.finalize().to_vec()
}

/// 数据库只存到微秒；占用的身份靠 `created_at` 比对，所以先截断。
fn micros(at: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(at.timestamp_micros()).unwrap_or(at)
}

enum Outcome {
    Claimed,
    Replay(Stored),
    InProgress,
    Mismatch,
}

struct Stored {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl IntoResponse for Stored {
    fn into_response(self) -> Response {
        let mut response = (self.status, self.body).into_response();
        response.headers_mut().extend(self.headers);
        response
            .headers_mut()
            .insert(REPLAYED_HEADER, HeaderValue::from_static("true"));
        response
    }
}

/// 一次占用。`created_at` 同时是占用的身份：只有自己的占用才能完成或释放。
struct Claim {
    user_id: i64,
    key: String,
    fingerprint: Vec<u8>,
    created_at: DateTime<Utc>,
}

impl Claim {
    async fn acquire(&self, db: &PgPool, lock: DateTime<Utc>) -> sqlx::Result<Outcome> {
        let mut conn = db.acquire().await?;
        // 没有记录、记录已过期、或占用已失效（实例崩溃）时占用成功。
        let claimed = sqlx::query_scalar!(
            r#"INSERT INTO idempotency_keys (user_id, key, fingerprint, locked_until, created_at)
               VALUES ($1, $2::text::uuid, $3, $4, $5)
               ON CONFLICT (user_id, key) DO UPDATE
               SET fingerprint = EXCLUDED.fingerprint, locked_until = EXCLUDED.locked_until,
                   status = NULL, headers = NULL, body = NULL, created_at = EXCLUDED.created_at
               WHERE idempotency_keys.created_at < $6 OR idempotency_keys.locked_until < $5
               RETURNING 1 AS "claimed!""#,
            self.user_id,
            self.key,
            self.fingerprint,
            lock,
            self.created_at,
            self.created_at - RETENTION,
        )
        .fetch_optional(&mut *conn)
        .await?;
        if claimed.is_some() {
            return Ok(Outcome::Claimed);
        }
        let Some(row) = sqlx::query!(
            "SELECT fingerprint, locked_until, status, headers, body FROM idempotency_keys
             WHERE user_id = $1 AND key = $2::text::uuid",
            self.user_id,
            self.key,
        )
        .fetch_optional(&mut *conn)
        .await?
        else {
            // 刚好被释放：让客户端再试一次。
            return Ok(Outcome::InProgress);
        };
        if row.fingerprint != self.fingerprint {
            return Ok(Outcome::Mismatch);
        }
        let (Some(status), Some(headers), Some(body)) = (row.status, row.headers, row.body) else {
            return Ok(Outcome::InProgress);
        };
        Ok(Outcome::Replay(Stored {
            status: u16::try_from(status)
                .ok()
                .and_then(|s| StatusCode::from_u16(s).ok())
                .unwrap_or(StatusCode::OK),
            headers: decode_headers(&headers),
            body,
        }))
    }

    /// 记下成功的响应，或者释放占用。不论记录成败，都把处理结果原样返回给客户端。
    async fn finish(self, db: &PgPool, response: Response) -> Response {
        let (parts, body) = response.into_parts();
        let bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::error!(?error, "读取响应体失败");
                self.release(db).await;
                return Problem::internal().into_response();
            }
        };
        let storable = parts.status.is_success()
            && bytes.len() <= MAX_STORED_BODY
            && !parts.headers.contains_key(header::SET_COOKIE);
        if storable {
            if let Err(error) = self
                .complete(db, parts.status, &parts.headers, &bytes)
                .await
            {
                tracing::warn!(?error, "保存幂等结果失败");
                self.release(db).await;
            }
        } else {
            self.release(db).await;
        }
        Response::from_parts(parts, Body::from(bytes))
    }

    async fn complete(
        &self,
        db: &PgPool,
        status: StatusCode,
        headers: &HeaderMap,
        body: &Bytes,
    ) -> sqlx::Result<()> {
        sqlx::query!(
            "UPDATE idempotency_keys SET locked_until = NULL, status = $4, headers = $5, body = $6
             WHERE user_id = $1 AND key = $2::text::uuid AND created_at = $3
               AND locked_until IS NOT NULL",
            self.user_id,
            self.key,
            self.created_at,
            i16::try_from(status.as_u16()).unwrap_or(200),
            encode_headers(headers),
            body.as_ref(),
        )
        .execute(db)
        .await?;
        Ok(())
    }

    async fn release(&self, db: &PgPool) {
        let result = sqlx::query!(
            "DELETE FROM idempotency_keys
             WHERE user_id = $1 AND key = $2::text::uuid AND created_at = $3
               AND locked_until IS NOT NULL",
            self.user_id,
            self.key,
            self.created_at,
        )
        .execute(db)
        .await;
        if let Err(error) = result {
            // 占用会在超时后自动失效。
            tracing::warn!(?error, "释放幂等键失败");
        }
    }
}

fn encode_headers(headers: &HeaderMap) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = REPLAYED
        .iter()
        .filter_map(|name| {
            let value = headers.get(name)?.to_str().ok()?;
            Some((name.as_str().to_owned(), value.into()))
        })
        .collect();
    map.into()
}

fn decode_headers(value: &serde_json::Value) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for name in &REPLAYED {
        if let Some(value) = value
            .get(name.as_str())
            .and_then(serde_json::Value::as_str)
            .and_then(|v| HeaderValue::from_str(v).ok())
        {
            headers.insert(name.clone(), value);
        }
    }
    headers
}

/// 每天清理一次过期的幂等键。失败不重试，第二天再来。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PruneIdempotencyKeys {}

impl TaskHandler for PruneIdempotencyKeys {
    const IDENTIFIER: &'static str = "server.prune_idempotency_keys";

    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(db) = ctx.get_ext::<PgPool>() else {
            return Err("执行器没有注册 PgPool".to_owned());
        };
        prune(db, Utc::now())
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

/// 删除 `now` 之前已经过期的键，返回删除数。
pub async fn prune(db: &PgPool, now: DateTime<Utc>) -> sqlx::Result<u64> {
    let deleted = sqlx::query!(
        "DELETE FROM idempotency_keys WHERE created_at < $1",
        now - RETENTION
    )
    .execute(db)
    .await?
    .rows_affected();
    if deleted > 0 {
        tracing::info!(deleted, "清理了过期的幂等键");
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_must_be_canonical_uuids() {
        assert_eq!(
            parse_key("0190F3E2-7A1B-7C3D-8E4F-0123456789AB").as_deref(),
            Some("0190f3e2-7a1b-7c3d-8e4f-0123456789ab")
        );
        assert!(parse_key("").is_none());
        assert!(parse_key("not-a-uuid").is_none());
        assert!(parse_key("0190f3e27a1b7c3d8e4f0123456789ab").is_none());
        assert!(parse_key("0190f3e2-7a1b-7c3d-8e4f-0123456789ag").is_none());
        assert!(parse_key("{0190f3e2-7a1b-7c3d-8e4f-0123456789ab}").is_none());
    }

    #[test]
    fn fingerprint_covers_method_path_query_and_body() {
        let uri: http::Uri = "/v1/posts?publish=true".parse().unwrap();
        let other: http::Uri = "/v1/posts?publish=false".parse().unwrap();
        let base = fingerprint(&Method::POST, &uri, b"{}");
        assert_eq!(base.len(), 32);
        assert_eq!(base, fingerprint(&Method::POST, &uri, b"{}"));
        assert_ne!(base, fingerprint(&Method::POST, &other, b"{}"));
        assert_ne!(base, fingerprint(&Method::POST, &uri, b"{ }"));
        assert_ne!(base, fingerprint(&Method::PUT, &uri, b"{}"));
    }

    #[test]
    fn only_whitelisted_headers_round_trip() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(header::ETAG, HeaderValue::from_static("\"rev:1\""));
        headers.insert(header::SET_COOKIE, HeaderValue::from_static("a=b"));
        let decoded = decode_headers(&encode_headers(&headers));
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[header::ETAG], "\"rev:1\"");
        assert!(!decoded.contains_key(header::SET_COOKIE));
    }
}
