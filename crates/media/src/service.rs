//! 直传上传与图库。业务规则：`docs/business-rules/media-ops.md` 第 1、2、4 节。
//!
//! 流程：签名（校验声明、发预签名 PUT 与完成令牌）→ 客户端直传到暂存 key →
//! 完成（在并发闸门里 HEAD、带 If-Match 读回全部字节、算 SHA-256、嗅探类型，
//! 把校验过的字节写到服务端生成的新 key，再落库）。对象存储操作都在数据库事务之外。

use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Utc};
use governor::clock::{Clock, DefaultClock};
use http::StatusCode;
use ik_auth::Principal;
use ik_kernel::cursor::{self, Page};
use ik_kernel::{Id, Problem};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::MediaState;
use crate::cdn::Urls;
use crate::dto::{CompleteRequest, GalleryQuery, MediaView, SignRequest, SignResult};
use crate::gate::GateError;
use crate::repo::{self, MediaRow, NewMedia};
use crate::sniff::{self, MAX_UPLOAD_BYTES};
use crate::state::Backend;
use crate::storage::StorageError;
use crate::token::{self, UploadClaims};

const FILENAME_MAX_CHARS: usize = 255;
const DEFAULT_GALLERY: u32 = 24;
const MAX_GALLERY: u32 = 100;
/// 客户端上报的宽高上限（超出视为没给）。
const MAX_DIMENSION: u32 = 100_000;

pub(crate) fn view(urls: &Urls, row: MediaRow) -> MediaView {
    let size = u64::try_from(row.size_bytes).unwrap_or(0);
    MediaView {
        id: Id::new(row.id),
        url: urls.public_url(&row.object_key),
        thumb_url: urls.thumb_url(&row.object_key, size),
        mime: row.mime,
        size: row.size_bytes,
        width: row.width,
        height: row.height,
        name: row.name,
        thumbhash: row.thumbhash,
        created_at: row.created_at,
    }
}

fn not_configured() -> Problem {
    tracing::warn!("没有配置对象存储（IK_STORAGE__BUCKET），上传不可用");
    Problem::service_unavailable()
}

fn backend(state: &MediaState) -> Result<&Backend, Problem> {
    state.backend.as_deref().ok_or_else(not_configured)
}

fn token_invalid() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "UPLOAD_TOKEN_INVALID",
        "上传凭证无效",
    )
}

fn size_mismatch() -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "UPLOAD_SIZE_MISMATCH",
        "上传的文件大小与声明不符",
    )
}

fn storage_problem(error: StorageError) -> Problem {
    match error {
        StorageError::NotFound => Problem::new(
            StatusCode::BAD_REQUEST,
            "UPLOAD_NOT_FOUND",
            "没有找到已上传的文件，请重新上传",
        ),
        StorageError::Mismatch(reason) => {
            tracing::info!(reason, "上传的对象与签名不符");
            size_mismatch()
        }
        StorageError::Unavailable(detail) => {
            tracing::warn!(detail, "对象存储不可用");
            Problem::service_unavailable()
        }
    }
}

fn gate_problem(error: GateError) -> Problem {
    match error {
        GateError::Busy => Problem::new(
            StatusCode::TOO_MANY_REQUESTS,
            "UPLOAD_VERIFICATION_BUSY",
            "上传校验繁忙，请稍后重试",
        )
        .with_retry_after(5),
        GateError::QueueTimeout | GateError::TaskTimeout => Problem::new(
            StatusCode::TOO_MANY_REQUESTS,
            "UPLOAD_VERIFICATION_TIMEOUT",
            "上传校验超时，请稍后重试",
        )
        .with_retry_after(10),
        GateError::Failed => Problem::internal(),
    }
}

fn random_hex() -> Result<String, Problem> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| Problem::internal_from(&e))?;
    Ok(token::hex(&bytes))
}

/// `{root}/{sub}/{YYYY}/{MM}/{random}{ext}`，年月按北京时间（UPL-006/007/008）。
fn object_key(
    root: &str,
    sub: Option<&str>,
    now: DateTime<Utc>,
    ext: &str,
) -> Result<String, Problem> {
    let date = ik_kernel::time::beijing_date(now);
    let mut parts: Vec<String> = Vec::new();
    if !root.is_empty() {
        parts.push(root.to_owned());
    }
    if let Some(sub) = sub {
        parts.push(sub.to_owned());
    }
    parts.push(format!("{:04}", date.year()));
    parts.push(format!("{:02}", date.month()));
    parts.push(format!("{}{ext}", random_hex()?));
    Ok(parts.join("/"))
}

/// 文件名：取最后一段、去首尾空白，不超过 255 字，扩展名须为允许的图片扩展名。
fn validate_filename(raw: &str) -> Result<String, Problem> {
    let name = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if name.is_empty() {
        return Err(Problem::field("filename", "REQUIRED", "缺少文件名"));
    }
    if name.chars().count() > FILENAME_MAX_CHARS || name.chars().any(char::is_control) {
        return Err(Problem::field("filename", "INVALID", "文件名不合法"));
    }
    if !sniff::extension_allowed(&name) {
        return Err(Problem::field(
            "filename",
            "TYPE_NOT_ALLOWED",
            "只能上传 jpg、png、gif、webp、avif 图片",
        ));
    }
    Ok(name)
}

fn check_sign_rate(backend: &Backend, user_id: i64) -> Result<(), Problem> {
    backend.sign_limit.check_key(&user_id).map_err(|denied| {
        let wait = denied.wait_time_from(DefaultClock::default().now());
        Problem::rate_limited().with_retry_after(wait.as_secs().saturating_add(1))
    })
}

/// 签名（UPL-030）。需要通过入站考试（UPL-020）。声明了内容哈希、且自己上传过相同内容时直接返回已有文件
/// （只查自己的，D-077 方案 A）。
pub async fn sign(
    state: &MediaState,
    principal: &Principal,
    request: SignRequest,
) -> Result<SignResult, Problem> {
    principal.require_write()?;
    let backend = backend(state)?;
    let me = principal.user_id.get();
    check_sign_rate(backend, me)?;
    let name = validate_filename(&request.filename)?;
    let mime = sniff::normalize_mime(&request.mime_type).ok_or_else(|| {
        Problem::field(
            "mimeType",
            "TYPE_NOT_ALLOWED",
            "只能上传 jpg、png、gif、webp、avif 图片",
        )
    })?;
    if request.size == 0 || request.size > MAX_UPLOAD_BYTES {
        return Err(Problem::field(
            "size",
            "OUT_OF_RANGE",
            "文件大小须在 1 字节到 30 MiB 之间",
        ));
    }
    let sha = token::normalize_sha256(request.content_hash.as_deref());
    if let Some(sha) = &sha {
        let mut conn = state.db.acquire().await?;
        if let Some(row) = repo::by_sha256(&mut conn, sha, Some(me)).await? {
            return Ok(SignResult {
                existing: true,
                media: Some(view(&state.urls, row)),
                upload_id: None,
                upload_url: None,
                method: None,
                headers: None,
                expires_at: None,
            });
        }
    }

    let now = Utc::now();
    let key = object_key(
        &backend.root_prefix,
        Some("staging"),
        now,
        sniff::canonical_ext(mime),
    )?;
    let presigned = backend
        .storage
        .presign_put(&key, mime, request.size, backend.sign_expires)
        .await
        .map_err(storage_problem)?;
    let expires_at = now
        + chrono::Duration::from_std(backend.sign_expires).unwrap_or(chrono::Duration::minutes(10));
    let upload_id = backend
        .key
        .sign(&UploadClaims {
            exp: expires_at.timestamp(),
            uid: me,
            key,
            size: request.size,
            sha,
            name,
        })
        .ok_or_else(Problem::internal)?;
    Ok(SignResult {
        existing: false,
        media: None,
        upload_id: Some(upload_id),
        upload_url: Some(presigned.url),
        method: Some("PUT".to_owned()),
        headers: Some(presigned.headers.into_iter().collect::<BTreeMap<_, _>>()),
        expires_at: Some(expires_at),
    })
}

fn dimension(value: Option<u32>) -> Option<i32> {
    value
        .filter(|v| (1..=MAX_DIMENSION).contains(v))
        .and_then(|v| i32::try_from(v).ok())
}

/// 暂存对象尽力删除：失败只记日志（存储生命周期规则兜底，UPL-036/060）。
async fn discard(backend: &Backend, key: &str) {
    if let Err(error) = backend.storage.delete(key).await {
        tracing::warn!(%error, key, "删除对象失败");
    }
}

/// 完成上传（UPL-032～036）。返回（是否新建，文件）。同一凭证重试返回同一个文件（收据幂等）。
pub async fn complete(
    state: &MediaState,
    principal: &Principal,
    upload_id: &str,
    request: CompleteRequest,
) -> Result<(bool, MediaView), Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    let backend_arc = state.backend.clone().ok_or_else(not_configured)?;
    let upload_id = upload_id.trim();
    let claims = backend_arc
        .key
        .verify(upload_id)
        .filter(|c| c.uid == me)
        .ok_or_else(token_invalid)?;
    let receipt = token::receipt(upload_id);

    let mut conn = state.db.acquire().await?;
    if let Some(row) = repo::by_receipt(&mut conn, &receipt).await? {
        repo::add_uploader(&mut conn, row.id, me, Utc::now()).await?;
        return Ok((false, view(&state.urls, row)));
    }
    if Utc::now().timestamp() >= claims.exp {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "UPLOAD_TOKEN_EXPIRED",
            "上传凭证已过期，请重新上传",
        ));
    }
    // 自己已经有相同内容的文件（含这次凭证之前的重试）：直接用。
    if let Some(sha) = &claims.sha
        && let Some(row) = repo::by_sha256(&mut conn, sha, Some(me)).await?
    {
        drop(conn);
        discard(&backend_arc, &claims.key).await;
        return Ok((false, view(&state.urls, row)));
    }
    drop(conn);

    let task_state = state.clone();
    // 校验任务的状态机很大（S3 请求、30 MiB 缓冲的所有权），放到堆上。
    let task = Box::pin(async move {
        verify_and_publish(task_state, backend_arc, me, claims, receipt, request).await
    });
    let backend = backend(state)?;
    let (created, row) = backend.gate.run(task).await.map_err(gate_problem)??;
    Ok((created, view(&state.urls, row)))
}

async fn verify_and_publish(
    state: MediaState,
    backend: std::sync::Arc<Backend>,
    me: i64,
    claims: UploadClaims,
    receipt: String,
    request: CompleteRequest,
) -> Result<(bool, MediaRow), Problem> {
    let storage = &backend.storage;
    let (size, etag) = storage.head(&claims.key).await.map_err(storage_problem)?;
    if size != claims.size || size > MAX_UPLOAD_BYTES {
        return Err(size_mismatch());
    }
    let bytes = storage
        .read_exact(&claims.key, &etag, size)
        .await
        .map_err(storage_problem)?;
    let sha = token::hex(&Sha256::digest(&bytes));
    if claims
        .sha
        .as_deref()
        .is_some_and(|declared| declared != sha)
    {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "UPLOAD_CHECKSUM_MISMATCH",
            "上传的文件内容与声明的哈希不符",
        ));
    }
    let Some(mime) = sniff::sniff(&bytes) else {
        return Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "UPLOAD_NOT_IMAGE",
            "上传的文件不是支持的图片格式",
        ));
    };
    let now = Utc::now();

    // 已经有人上传过相同内容：字节已经校验过（持有证明成立），复用并把自己加为上传者（D-077 方案 A）。
    let mut tx = state.db.begin().await?;
    if let Some(row) = repo::by_sha256(&mut tx, &sha, None).await? {
        repo::add_uploader(&mut tx, row.id, me, now).await?;
        tx.commit().await?;
        // 没有声明哈希时保留暂存对象：同一凭证重试还要能重新校验（生命周期规则会清理它）。
        if claims.sha.is_some() {
            discard(&backend, &claims.key).await;
        }
        return Ok((false, row));
    }
    drop(tx);

    // ThumbHash 是 CPU 密集的解码，放到阻塞线程；失败只是没有占位。
    let (bytes, thumbhash) = tokio::task::spawn_blocking(move || {
        let hash = crate::thumb::compute(&bytes, mime);
        (bytes, hash)
    })
    .await
    .map_err(|e| Problem::internal_from(&e))?;

    // 发布到客户端不可写的新 key（UPL-052），写的是校验过的那份字节。
    let final_key = object_key(&backend.root_prefix, None, now, sniff::canonical_ext(mime))?;
    let size_bytes = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
    storage
        .put(&final_key, bytes, mime)
        .await
        .map_err(storage_problem)?;

    let mut tx = state.db.begin().await?;
    let id = state.ids.next_raw();
    let inserted = repo::insert(
        &mut tx,
        &NewMedia {
            id,
            object_key: &final_key,
            mime,
            size_bytes,
            width: dimension(request.width),
            height: dimension(request.height),
            sha256: &sha,
            receipt: &receipt,
            name: &claims.name,
            thumbhash: thumbhash.as_deref(),
            now,
        },
    )
    .await?;
    if !inserted {
        // 同一凭证的并发完成：对方先落库了。用对方的，删掉自己刚发布的对象（UPL-035）。
        let winner = repo::by_receipt(&mut tx, &receipt)
            .await?
            .ok_or_else(Problem::internal)?;
        repo::add_uploader(&mut tx, winner.id, me, now).await?;
        tx.commit().await?;
        discard(&backend, &final_key).await;
        return Ok((false, winner));
    }
    repo::add_uploader(&mut tx, id, me, now).await?;
    let row = repo::by_receipt(&mut tx, &receipt)
        .await?
        .ok_or_else(Problem::internal)?;
    tx.commit().await?;
    discard(&backend, &claims.key).await;
    Ok((true, row))
}

// ---------- 我的图库 ----------

#[derive(Serialize, Deserialize)]
struct GalleryCursor {
    a: DateTime<Utc>,
    i: i64,
}

/// 我上传过的图片（按加入图库的时间倒序，MEUP-001）。
pub async fn gallery(
    state: &MediaState,
    principal: &Principal,
    query: GalleryQuery,
) -> Result<Page<MediaView>, Problem> {
    let limit = i64::from(query.limit.unwrap_or(DEFAULT_GALLERY).clamp(1, MAX_GALLERY));
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<GalleryCursor>)
        .transpose()?
        .map(|c| (c.a, c.i));
    let mut conn = state.db.acquire().await?;
    let rows = repo::gallery(&mut conn, principal.user_id.get(), after, limit + 1).await?;
    let take = usize::try_from(limit).unwrap_or(24);
    let next_cursor = match (rows.len() > take, rows.get(take.saturating_sub(1))) {
        (true, Some(last)) => Some(cursor::encode(&GalleryCursor {
            a: last.added_at,
            i: last.id,
        })?),
        _ => None,
    };
    let items = rows
        .into_iter()
        .take(take)
        .map(|r| {
            view(
                &state.urls,
                MediaRow {
                    id: r.id,
                    object_key: r.object_key,
                    mime: r.mime,
                    size_bytes: r.size_bytes,
                    width: r.width,
                    height: r.height,
                    name: r.name,
                    thumbhash: r.thumbhash,
                    created_at: r.created_at,
                },
            )
        })
        .collect();
    Ok(Page { items, next_cursor })
}

/// 从图库移除：只断开自己与文件的关联，从不物理删除（MEUP-004、D-078 方案 A）。
/// 已经被引用（头像、帖子封面等）的地方不受影响。不是自己的文件一律 404。
pub async fn remove(state: &MediaState, principal: &Principal, id: i64) -> Result<(), Problem> {
    let mut conn = state.db.acquire().await?;
    if repo::remove_uploader(&mut conn, id, principal.user_id.get()).await? {
        Ok(())
    } else {
        Err(Problem::not_found())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames() {
        assert_eq!(validate_filename(r"C:\pics\ 猫.JPG ").unwrap(), "猫.JPG");
        assert_eq!(validate_filename("a/b/c.webp").unwrap(), "c.webp");
        for bad in [
            "",
            "  ",
            "x.svg",
            "noext",
            "a\nb.png",
            &format!("{}.png", "a".repeat(300)),
        ] {
            assert!(validate_filename(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn keys() {
        let now = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 1, 31, 17, 0, 0).unwrap();
        let key = object_key("uploads", Some("staging"), now, ".png").unwrap();
        // 北京时间已经是 2 月 1 日。
        assert!(key.starts_with("uploads/staging/2026/02/"), "{key}");
        assert!(key.ends_with(".png"));
        assert_eq!(key.len(), "uploads/staging/2026/02/".len() + 32 + 4);
        assert!(
            object_key("", None, now, ".jpg")
                .unwrap()
                .starts_with("2026/02/")
        );
    }

    #[test]
    fn dimensions() {
        assert_eq!(dimension(Some(1920)), Some(1920));
        assert_eq!(dimension(Some(0)), None);
        assert_eq!(dimension(Some(1_000_000)), None);
        assert_eq!(dimension(None), None);
    }
}
