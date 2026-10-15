//! 对外的请求与响应类型。

use chrono::{DateTime, Utc};
use ik_kernel::Id;
use ik_kernel::id::marker::Media;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// 媒体文件。`url` 是原图，`thumbUrl` 是 360 宽 WebP 缩略图（没有 CDN 时与原图相同）。
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MediaView {
    #[schema(value_type = String)]
    pub id: Id<Media>,
    pub url: String,
    pub thumb_url: String,
    /// 服务端嗅探出的真实类型。
    pub mime: String,
    /// 字节数。
    pub size: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    /// 上传时的文件名（只用于展示）。
    pub name: String,
    /// ThumbHash（base64），图片加载前的模糊占位。AVIF 或解码失败时省略。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbhash: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SignRequest {
    /// 文件名，扩展名须为 jpg/jpeg/png/gif/webp/avif 之一。
    pub filename: String,
    /// `image/jpeg` | `image/png` | `image/gif` | `image/webp` | `image/avif`（`image/jpg` 视为 jpeg）。
    pub mime_type: String,
    /// 字节数，1～30 MiB。
    pub size: u64,
    /// 文件内容的 SHA-256（64 位十六进制，可选）。提供时如果自己上传过相同内容的文件会直接返回它；
    /// 完成时也会校验上传的字节与它一致。格式不对时视为未提供。
    #[serde(default)]
    pub content_hash: Option<String>,
}

/// 签名结果：要么直接复用已有文件（`existing = true`，带 `media`），
/// 要么按 `method`、`headers` 把原文件传到 `uploadUrl`，再调用完成接口。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignResult {
    pub existing: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaView>,
    /// 完成接口路径里的 `uploadId`（不透明，含签名）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_url: Option<String>,
    /// 固定为 `PUT`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// 上传时必须带上的请求头（`Content-Type` 等）。`Content-Length` 也被签入，必须与 `size` 一致。
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompleteRequest {
    /// 客户端测得的宽高（服务端不校验，UPL-016）。
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct GalleryQuery {
    pub cursor: Option<String>,
    /// 默认 24，最大 100。
    pub limit: Option<u32>,
}
