//! 统一错误类型：RFC 9457 Problem Details。
//!
//! handler 返回 `Result<T, Problem>`。错误码（`code`）是给客户端分支用的稳定英文标识，
//! `detail` 是给用户看的中文提示。新增错误码必须登记到 `docs/api-guidelines.md`。

use axum::Json;
use axum::response::{IntoResponse, Response};
use http::{HeaderValue, StatusCode, header};
use serde::Serialize;
use utoipa::ToSchema;

/// 表单字段级错误。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct FieldError {
    pub field: String,
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct Problem {
    status: StatusCode,
    code: &'static str,
    detail: String,
    errors: Vec<FieldError>,
    retry_after_secs: Option<u64>,
}

impl Problem {
    pub fn new(status: StatusCode, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status,
            code,
            detail: detail.into(),
            errors: Vec::new(),
            retry_after_secs: None,
        }
    }

    /// 响应附带 `Retry-After: <秒>` 头（429 / 503 时让客户端知道何时重试）。
    pub fn with_retry_after(mut self, secs: u64) -> Self {
        self.retry_after_secs = Some(secs.max(1));
        self
    }

    pub fn retry_after_secs(&self) -> Option<u64> {
        self.retry_after_secs
    }

    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn field_errors(&self) -> &[FieldError] {
        &self.errors
    }

    pub fn with_field_errors(mut self, errors: Vec<FieldError>) -> Self {
        self.errors = errors;
        self
    }

    pub fn validation(errors: Vec<FieldError>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_FAILED",
            "请求参数不合法",
        )
        .with_field_errors(errors)
    }

    pub fn field(field: &str, code: &str, detail: impl Into<String>) -> Self {
        Self::validation(vec![FieldError {
            field: field.to_owned(),
            code: code.to_owned(),
            detail: detail.into(),
        }])
    }

    pub fn cursor_invalid() -> Self {
        Self::new(StatusCode::BAD_REQUEST, "CURSOR_INVALID", "分页游标无效")
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "UNAUTHORIZED", "请先登录")
    }

    pub fn forbidden() -> Self {
        Self::new(StatusCode::FORBIDDEN, "FORBIDDEN", "没有权限执行该操作")
    }

    pub fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", "内容不存在或不可见")
    }

    pub fn conflict(code: &'static str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, detail)
    }

    pub fn rate_limited() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "RATE_LIMITED",
            "操作过于频繁，请稍后再试",
        )
    }

    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL",
            "服务器开小差了，请稍后再试",
        )
    }

    pub fn service_unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "SERVICE_UNAVAILABLE",
            "服务暂时不可用，请稍后再试",
        )
    }

    /// 记录内部错误并返回脱敏的 500。原始错误只进日志，不进响应体。
    pub fn internal_from(error: &(dyn std::error::Error + 'static)) -> Self {
        tracing::error!(error = %error, request_id = ?crate::http::current_request_id(), "internal error");
        Self::internal()
    }
}

/// 序列化到响应体的形状（也用于 OpenAPI 文档）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProblemBody {
    #[serde(rename = "type")]
    pub type_uri: String,
    pub title: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub code: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<FieldError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl Problem {
    pub fn to_body(&self) -> ProblemBody {
        let is_server_error = self.status.is_server_error();
        ProblemBody {
            type_uri: format!("https://api.interk.net/problems/{}", kebab(self.code)),
            title: self.status.canonical_reason().unwrap_or("Error").to_owned(),
            status: self.status.as_u16(),
            // 5xx 永远使用固定文案，内部细节只进日志。
            detail: Some(if is_server_error {
                match self.status {
                    StatusCode::SERVICE_UNAVAILABLE => "服务暂时不可用，请稍后再试".to_owned(),
                    _ => "服务器开小差了，请稍后再试".to_owned(),
                }
            } else {
                self.detail.clone()
            }),
            code: self.code.to_owned(),
            errors: if is_server_error {
                Vec::new()
            } else {
                self.errors.clone()
            },
            request_id: crate::http::current_request_id(),
        }
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let mut response = (self.status, Json(self.to_body())).into_response();
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/problem+json"),
        );
        if let Some(secs) = self.retry_after_secs {
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        response
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status.as_u16(), self.code, self.detail)
    }
}

impl std::error::Error for Problem {}

impl From<sqlx::Error> for Problem {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::RowNotFound => Self::not_found(),
            sqlx::Error::PoolTimedOut => {
                tracing::error!(request_id = ?crate::http::current_request_id(), "database pool timed out");
                Self::service_unavailable()
            }
            other => Self::internal_from(&other),
        }
    }
}

impl From<validator::ValidationErrors> for Problem {
    fn from(errors: validator::ValidationErrors) -> Self {
        let mut list: Vec<FieldError> = errors
            .field_errors()
            .into_iter()
            .flat_map(|(field, items)| {
                items.iter().map(move |item| FieldError {
                    field: field.to_string(),
                    code: item.code.to_string(),
                    detail: item
                        .message
                        .as_ref()
                        .map_or_else(|| "取值不合法".to_owned(), ToString::to_string),
                })
            })
            .collect();
        list.sort_by(|a, b| a.field.cmp(&b.field).then_with(|| a.code.cmp(&b.code)));
        Self::validation(list)
    }
}

/// `EXAM_REQUIRED` → `exam-required`
fn kebab(code: &str) -> String {
    code.chars()
        .map(|c| {
            if c == '_' {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_error_keeps_detail_and_fields() {
        let body = Problem::field("title", "TOO_LONG", "标题最多 50 个字").to_body();
        assert_eq!(body.status, 400);
        assert_eq!(body.code, "VALIDATION_FAILED");
        assert_eq!(
            body.type_uri,
            "https://api.interk.net/problems/validation-failed"
        );
        assert_eq!(body.errors.len(), 1);
        assert_eq!(body.errors[0].field, "title");
    }

    #[test]
    fn server_error_hides_internal_detail() {
        let body = Problem::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL",
            "pg: secret leak",
        )
        .with_field_errors(vec![FieldError {
            field: "x".into(),
            code: "y".into(),
            detail: "z".into(),
        }])
        .to_body();
        assert_eq!(body.detail.as_deref(), Some("服务器开小差了，请稍后再试"));
        assert!(body.errors.is_empty());
    }

    #[test]
    fn response_uses_problem_json_content_type() {
        let response = Problem::not_found().into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
    }

    #[test]
    fn retry_after_header_is_set_when_requested() {
        let response = Problem::rate_limited().with_retry_after(7).into_response();
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "7");
        let response = Problem::rate_limited().into_response();
        assert!(response.headers().get(header::RETRY_AFTER).is_none());
    }

    #[test]
    fn row_not_found_maps_to_404() {
        assert_eq!(
            Problem::from(sqlx::Error::RowNotFound).status(),
            StatusCode::NOT_FOUND
        );
    }
}
