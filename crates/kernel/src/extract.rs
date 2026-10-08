//! 替代 axum 默认提取器：请求体/参数解析失败时同样返回 Problem JSON，而不是纯文本。

use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts};
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use serde::Serialize;

use crate::Problem;

#[derive(Debug, Clone, Copy, Default, FromRequest)]
#[from_request(via(axum::Json), rejection(Problem))]
pub struct Json<T>(pub T);

/// 查询参数。解析失败 → 400 `VALIDATION_FAILED`。
#[derive(Debug, Clone, Copy, Default, FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(Problem))]
pub struct Query<T>(pub T);

/// 路径参数。解析失败（例如 id 不是数字）→ 404：不存在的资源与格式错误的 id 不必区分。
#[derive(Debug, Clone, Copy, FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(Problem))]
pub struct Path<T>(pub T);

impl From<QueryRejection> for Problem {
    fn from(rejection: QueryRejection) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "VALIDATION_FAILED",
            "请求参数不合法",
        )
        .with_field_errors(vec![crate::FieldError {
            field: String::new(),
            code: "INVALID_QUERY".to_owned(),
            detail: rejection.body_text(),
        }])
    }
}

impl From<PathRejection> for Problem {
    fn from(_: PathRejection) -> Self {
        Self::not_found()
    }
}

impl<T: Serialize> IntoResponse for Json<T> {
    fn into_response(self) -> Response {
        axum::Json(self.0).into_response()
    }
}

impl From<JsonRejection> for Problem {
    fn from(rejection: JsonRejection) -> Self {
        match rejection {
            JsonRejection::MissingJsonContentType(_) => Self::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "UNSUPPORTED_MEDIA_TYPE",
                "请求体必须是 application/json",
            ),
            JsonRejection::JsonSyntaxError(_) => Self::new(
                StatusCode::BAD_REQUEST,
                "INVALID_JSON",
                "请求体不是合法的 JSON",
            ),
            JsonRejection::JsonDataError(error) => Self::new(
                StatusCode::BAD_REQUEST,
                "VALIDATION_FAILED",
                "请求参数不合法",
            )
            .with_field_errors(vec![crate::FieldError {
                field: String::new(),
                code: "INVALID_FORMAT".to_owned(),
                detail: error.body_text(),
            }]),
            JsonRejection::BytesRejection(_) => Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "PAYLOAD_TOO_LARGE",
                "请求体过大或读取失败",
            ),
            _ => Self::new(
                StatusCode::BAD_REQUEST,
                "VALIDATION_FAILED",
                "请求参数不合法",
            ),
        }
    }
}
