//! 自定义中间件：请求 ID 与请求超时。其余横切能力直接用 tower-http。

use std::time::Duration;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::HeaderValue;
use ik_kernel::Problem;
use ik_kernel::http::with_request_id;

pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// 放进请求扩展里的请求 ID，供日志 span 读取。
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// 沿用客户端/网关传入的合法 `X-Request-Id`，否则生成新的；写回响应头，并设置到任务上下文
/// （`Problem` 序列化时据此填 `requestId`）。
pub async fn request_id(mut request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| is_valid_request_id(value))
        .map_or_else(generate_request_id, str::to_owned);
    request.extensions_mut().insert(RequestId(id.clone()));

    let mut response = with_request_id(id.clone(), next.run(request)).await;
    if let Ok(value) = HeaderValue::from_str(&id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

/// 只接受 1–64 位的 `[A-Za-z0-9_-]`，防止日志注入与超长头。
fn is_valid_request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn generate_request_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // 系统随机源不可用时退化为时间戳，请求 ID 只用于追踪，不承担安全职责。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        return format!("t{nanos:x}");
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 整个请求处理超过 `limit` 时返回 503 Problem（tower-http 的超时层只返回空响应体）。
pub fn timeout(
    limit: Duration,
) -> impl Fn(Request, Next) -> std::pin::Pin<Box<dyn Future<Output = Response> + Send>> + Clone {
    move |request: Request, next: Next| {
        Box::pin(async move {
            match tokio::time::timeout(limit, next.run(request)).await {
                Ok(response) => response,
                Err(_) => {
                    tracing::warn!(?limit, "请求处理超时");
                    Problem::service_unavailable().into_response()
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_id_validation() {
        assert!(is_valid_request_id("abc-123_XYZ"));
        assert!(!is_valid_request_id(""));
        assert!(!is_valid_request_id("has space"));
        assert!(!is_valid_request_id("line\nbreak"));
        assert!(!is_valid_request_id(&"a".repeat(65)));
    }

    #[test]
    fn generated_ids_are_valid_and_distinct() {
        let a = generate_request_id();
        let b = generate_request_id();
        assert!(is_valid_request_id(&a));
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
    }
}
