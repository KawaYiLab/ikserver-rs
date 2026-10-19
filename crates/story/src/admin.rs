//! 剧情库的管理后台接口（只有管理员）：上传语料包、查看版本、回滚到旧版本。
//!
//! 上传：校验并构建（事务外，阻塞线程池）→ 一个事务里插入、设为生效、清理旧版本、记审计
//! → 提交后换上新语料。其他实例由 [`StoryState::spawn_watcher`] 在一分钟内跟上。

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, FromRef, State};
use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_auth::admin::{self, Audit};
use ik_auth::{AdminOperator, AuthState, Operator};
use ik_kernel::error::ProblemBody;
use ik_kernel::extract::{Json, Path};
use ik_kernel::{Id, Problem};
use serde::Serialize;
use sha2::{Digest, Sha256};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::corpus::{Bundle, build_from_bundle};
use crate::repo::{self, CorpusRow, NewCorpus};
use crate::{MAX_BUNDLE_BYTES, StoryState};

/// 上传正文上限（gzip 压缩后；当前全量数据约 6 MB）。
pub const MAX_UPLOAD_BYTES: usize = 64 * 1024 * 1024;
/// 保留的版本数（生效版本总是保留）。
const KEEP_VERSIONS: i64 = 5;

/// 一个语料版本（不含语料包本身）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CorpusView {
    #[schema(value_type = String)]
    pub id: Id,
    /// 数据版本（manifest.json 的 source_version，如 3.2.0）。
    pub source_version: String,
    pub language: String,
    pub docs: i32,
    pub lines: i32,
    /// 主线台词里带说话人的比例（0–1）。
    pub speaker_coverage: f32,
    /// 语料包大小（gzip 后，字节）。
    pub bundle_bytes: i64,
    pub bundle_sha256: String,
    /// 是否是生效版本。
    pub active: bool,
    #[schema(value_type = String)]
    pub uploaded_by: Id,
    pub created_at: DateTime<Utc>,
}

impl From<CorpusRow> for CorpusView {
    fn from(r: CorpusRow) -> Self {
        Self {
            id: Id::new(r.id),
            source_version: r.source_version,
            language: r.language,
            docs: r.docs,
            lines: r.lines,
            speaker_coverage: r.speaker_coverage,
            bundle_bytes: r.bundle_bytes,
            bundle_sha256: r.bundle_sha256,
            active: r.active,
            uploaded_by: Id::new(r.uploaded_by),
            created_at: r.created_at,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CorpusList {
    /// 本实例正在使用的版本；用本地语料（`IK_STORY__SOURCE`）或未启用时为空。
    #[schema(value_type = Option<String>)]
    pub serving: Option<Id>,
    /// 本实例是否在用本地语料（开发环境）。此时上传仍会入库并生效，但重启后又回到本地语料。
    pub local_source: bool,
    /// 新的在前。
    pub corpora: Vec<CorpusView>,
}

fn db_of(state: &StoryState) -> Result<(&sqlx::PgPool, &ik_kernel::Snowflake), Problem> {
    match (state.db(), state.ids()) {
        (Some(db), Some(ids)) => Ok((db, ids)),
        _ => Err(Problem::service_unavailable()),
    }
}

fn bundle_invalid(error: &anyhow::Error) -> Problem {
    let detail: String = format!("语料包不合格：{error:#}")
        .chars()
        .take(300)
        .collect();
    Problem::new(StatusCode::BAD_REQUEST, "STORY_BUNDLE_INVALID", detail)
}

/// 上传语料包并设为生效版本。与已有版本内容相同时不重复保存，只把那个版本设为生效。
pub async fn upload(state: &StoryState, op: &Operator, body: Bytes) -> Result<CorpusView, Problem> {
    let (db, ids) = db_of(state)?;
    if body.is_empty() {
        return Err(bundle_invalid(&anyhow::anyhow!("正文为空")));
    }
    // 事务外、阻塞线程池里：解压、校验、构建（约 1 秒），顺便得到要入库的 gzip 包与哈希。
    let (corpus, stored, sha256) = tokio::task::spawn_blocking(move || {
        let bundle = Bundle::decode(&body, MAX_BUNDLE_BYTES)?;
        let corpus = build_from_bundle(&bundle)?;
        let gz = if body.starts_with(&[0x1f, 0x8b]) {
            body.to_vec()
        } else {
            bundle.encode_gzip()?
        };
        let sha = hex(&Sha256::digest(&gz));
        anyhow::Ok((corpus, gz, sha))
    })
    .await
    .map_err(|_| Problem::internal())?
    .map_err(|e| bundle_invalid(&e))?;

    let now = Utc::now();
    let mut tx = db.begin().await?;
    repo::lock(&mut tx).await?;
    let (id, reused) = match repo::find_by_sha(&mut tx, &sha256).await? {
        Some(existing) => (existing, true),
        None => {
            let id = ids.next_raw();
            repo::insert(
                &mut tx,
                &NewCorpus {
                    id,
                    source_version: &corpus.meta.source_version,
                    language: &corpus.meta.language,
                    docs: i32::try_from(corpus.docs.len()).unwrap_or(i32::MAX),
                    lines: i32::try_from(corpus.lines.len()).unwrap_or(i32::MAX),
                    speaker_coverage: corpus.meta.speaker_coverage.clamp(0.0, 1.0),
                    bundle: &stored,
                    bundle_sha256: &sha256,
                    uploaded_by: op.user_id(),
                    now,
                },
            )
            .await?;
            (id, false)
        }
    };
    repo::activate(&mut tx, id).await?;
    let pruned = repo::prune(&mut tx, KEEP_VERSIONS).await?;
    admin::record(
        &mut tx,
        ids,
        op,
        Audit {
            action: "story.corpus.upload",
            target_type: "story_corpus",
            target_id: Some(id.to_string()),
            details: serde_json::json!({
                "sourceVersion": corpus.meta.source_version,
                "docs": corpus.docs.len(),
                "lines": corpus.lines.len(),
                "bundleBytes": stored.len(),
                "sha256": sha256,
                "reused": reused,
                "pruned": pruned,
            }),
        },
        now,
    )
    .await?;
    let row = repo::get(&mut *tx, id)
        .await?
        .ok_or_else(Problem::internal)?;
    tx.commit().await?;

    state.install_uploaded(corpus, id).await;
    Ok(row.into())
}

pub async fn list(state: &StoryState) -> Result<CorpusList, Problem> {
    let (db, _) = db_of(state)?;
    let corpora = repo::list(db).await?.into_iter().map(Into::into).collect();
    Ok(CorpusList {
        serving: state.version().map(Id::new),
        local_source: state.uses_local_source(),
        corpora,
    })
}

/// 把一个旧版本设为生效（回滚）。
pub async fn activate(state: &StoryState, op: &Operator, id: i64) -> Result<CorpusView, Problem> {
    let (db, ids) = db_of(state)?;
    let now = Utc::now();
    let mut tx = db.begin().await?;
    repo::lock(&mut tx).await?;
    if !repo::activate(&mut tx, id).await? {
        return Err(Problem::not_found());
    }
    admin::record(
        &mut tx,
        ids,
        op,
        Audit {
            action: "story.corpus.activate",
            target_type: "story_corpus",
            target_id: Some(id.to_string()),
            details: serde_json::json!({}),
        },
        now,
    )
    .await?;
    let row = repo::get(&mut *tx, id)
        .await?
        .ok_or_else(Problem::internal)?;
    tx.commit().await?;
    // 提交后再构建换上；失败时数据库已经切过去，其他实例与下次启动会用新版本，这里如实报错。
    state.load_active().await.map_err(|e| {
        tracing::error!(error = %format!("{e:#}"), "剧情库回滚后加载失败");
        Problem::internal()
    })?;
    Ok(row.into())
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

pub fn router<S>() -> OpenApiRouter<S>
where
    S: Clone + Send + Sync + 'static,
    AuthState: FromRef<S>,
    StoryState: FromRef<S>,
{
    OpenApiRouter::new()
        .routes(routes!(upload_corpus))
        // 语料包远大于全站默认的正文上限，只对这个接口放宽。
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES))
        .merge(
            OpenApiRouter::new()
                .routes(routes!(list_corpora))
                .routes(routes!(activate_corpus)),
        )
}

/// 上传剧情库语料
///
/// 正文是 zzz-story-extractor 的语料包（gzip 压缩的 JSON，见 `python -m zzz_story upload`）。
/// 校验并建好索引后设为生效版本，本实例立即切换，其他实例一分钟内跟上；与已有版本内容相同时
/// 不重复保存。只保留最近 5 个版本。
#[utoipa::path(
    put,
    path = "/v1/admin/story/corpus",
    tag = "admin",
    request_body(content = Vec<u8>, content_type = "application/gzip", description = "gzip 压缩的语料包"),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 201, body = CorpusView),
        (status = 400, description = "STORY_BUNDLE_INVALID", body = ProblemBody, content_type = "application/problem+json"),
        (status = 403, description = "FORBIDDEN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn upload_corpus(
    State(state): State<StoryState>,
    AdminOperator(op): AdminOperator,
    body: Bytes,
) -> Result<(StatusCode, Json<CorpusView>), Problem> {
    Ok((StatusCode::CREATED, Json(upload(&state, &op, body).await?)))
}

/// 剧情库版本列表
#[utoipa::path(
    get,
    path = "/v1/admin/story/corpora",
    tag = "admin",
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = CorpusList),
        (status = 403, description = "FORBIDDEN", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn list_corpora(
    State(state): State<StoryState>,
    AdminOperator(_op): AdminOperator,
) -> Result<Json<CorpusList>, Problem> {
    Ok(Json(list(&state).await?))
}

/// 回滚到某个剧情库版本
#[utoipa::path(
    post,
    path = "/v1/admin/story/corpora/{id}/activate",
    tag = "admin",
    params(("id" = String, Path)),
    security(("session_cookie" = []), ("bearer" = [])),
    responses(
        (status = 200, body = CorpusView),
        (status = 403, description = "FORBIDDEN", body = ProblemBody, content_type = "application/problem+json"),
        (status = 404, description = "NOT_FOUND", body = ProblemBody, content_type = "application/problem+json"),
    )
)]
async fn activate_corpus(
    State(state): State<StoryState>,
    AdminOperator(op): AdminOperator,
    Path(id): Path<Id>,
) -> Result<Json<CorpusView>, Problem> {
    Ok(Json(activate(&state, &op, id.get()).await?))
}
