//! 帖子业务逻辑。业务规则：`docs/business-rules/posts.md`。

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::service::Author;
use ik_kernel::bus::{Audience, Effects, Event};
use ik_kernel::cursor::{self, Page};
use ik_kernel::{FieldError, Id, Problem};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::dto::{
    AuthorView, CategoryRef, CategoryView, FavoriteResult, FeedPage, FeedQuery, GiftRequest,
    GiftResult, HomeFeedPage, LikeResult, MarkReadRequest, MarkReadResult, MyPostItem,
    MyPostsQuery, MyPostsState, PinCandidate, PinCandidatesQuery, PostCard, PostDetail,
    PostEditView, PostInput, PostStatus, SavedPost, SavedQuery, TagRef, TripleResult, ViewResult,
};
use crate::repo::{self, CardRow, Content, DraftRow, FeedFilter, PostRow};
use crate::video::{self, ExternalVideo, MAX_EXTERNAL_VIDEOS};
use crate::{ContentState, anonymous, tags};
use ik_economy::benefits;
use ik_economy::service::{Dedup, ExpAction};
use ik_notify::interop::{Kind, Notice, Target};

const TITLE_MAX_CHARS: usize = 50;
const EDITOR_STATE_MAX_BYTES: usize = 256 * 1024;
const MAX_RAW_TAGS: usize = 100;
const EXCERPT_CHARS: usize = 120;
const DEFAULT_PAGE: u32 = 20;
const MAX_PAGE: u32 = 50;
const MAX_PINNED: i64 = 10;
const TAG_SUGGEST_DEFAULT: u32 = 8;
const TAG_SUGGEST_MAX: u32 = 10;
/// 匿名帖缺失种子时的固定占位，保证展示名确定且不崩溃（POST-091）。
const ANONYMOUS_FALLBACK_SEED: &str = "anonymous-fallback";
const GONE_USER_NAME: &str = "已注销用户";

// ---------- 错误 ----------

fn post_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "帖子不存在或已被删除")
}

fn anonymous_locked() -> Problem {
    Problem::new(
        StatusCode::FORBIDDEN,
        "ANONYMOUS_POST_LOCKED",
        "匿名委托发布后不能再修改、下线或删除",
    )
}

fn not_published() -> Problem {
    Problem::new(StatusCode::CONFLICT, "NOT_PUBLISHED", "这篇帖子还没有发布")
}

fn revision_conflict() -> Problem {
    Problem::new(
        StatusCode::PRECONDITION_FAILED,
        "REVISION_CONFLICT",
        "内容已在别处被修改，请刷新后重试",
    )
}

fn category_admin_only() -> Problem {
    Problem::new(
        StatusCode::FORBIDDEN,
        "CATEGORY_ADMIN_ONLY",
        "只有管理员可以在该分区发帖",
    )
}

fn field(field: &str, code: &str, detail: impl Into<String>) -> FieldError {
    FieldError {
        field: field.to_owned(),
        code: code.to_owned(),
        detail: detail.into(),
    }
}

// ---------- 校验 ----------

/// 不访问数据库就能完成的校验结果。
struct Prepared {
    title: String,
    body: String,
    editor_state: Option<serde_json::Value>,
    external_videos: serde_json::Value,
    category_id: Option<i64>,
    /// (归一化名称, slug)，已按 slug 去重、保序。
    tags: Vec<(String, String)>,
    is_anonymous: bool,
    /// 去重、保序后的封面。
    cover_ids: Vec<i64>,
}

/// 等级允许的图片数（封面、正文插图各自不超过它，POST-012/014）。
fn image_limit(principal: &Principal) -> usize {
    usize::from(benefits::for_level(principal.benefit_level().max(1)).post_max_images)
}

/// Markdown 正文里的图片数（`![...](...)`）。
fn body_image_count(body: &str) -> usize {
    static IMAGE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    IMAGE
        .get_or_init(|| regex::Regex::new(r"!\[[^\]]*\]\([^)]*\)").ok())
        .as_ref()
        .map_or(0, |re| re.find_iter(body).count())
}

/// 封面数与正文插图数（发布时按当前等级再校验一次，POST-013/057）。
fn check_images(
    principal: &Principal,
    cover_ids: &[i64],
    body: &str,
    errors: &mut Vec<FieldError>,
) {
    let limit = image_limit(principal);
    if cover_ids.len() > limit {
        errors.push(field(
            "coverIds",
            "TOO_MANY",
            format!("当前等级图片最多只能上传 {limit} 张"),
        ));
    }
    if body_image_count(body) > limit {
        errors.push(field(
            "body",
            "TOO_MANY_IMAGES",
            format!("当前等级正文图片最多只能上传 {limit} 张"),
        ));
    }
}

fn body_limit(principal: &Principal) -> usize {
    // 能走到写接口的人已经通过考试（或是工作人员）；等级数据缺失时至少按 Lv1 计。
    benefits::max_post_body_chars(principal.benefit_level().max(1))
}

fn check_body(principal: &Principal, body: &str, errors: &mut Vec<FieldError>) {
    let limit = body_limit(principal);
    if body.chars().count() > limit {
        errors.push(field(
            "body",
            "TOO_LONG",
            format!("当前等级正文最多只能输入 {limit} 字"),
        ));
    }
}

fn prepare(principal: &Principal, input: PostInput) -> Result<Prepared, Problem> {
    let mut errors = Vec::new();

    let title = input.title.trim().to_owned();
    if title.is_empty() {
        errors.push(field("title", "REQUIRED", "标题不能为空"));
    } else if title.chars().count() > TITLE_MAX_CHARS {
        errors.push(field(
            "title",
            "TOO_LONG",
            format!("标题最多 {TITLE_MAX_CHARS} 个字"),
        ));
    }

    check_body(principal, &input.body, &mut errors);

    let mut tag_list: Vec<(String, String)> = Vec::new();
    if input.tags.len() > MAX_RAW_TAGS {
        errors.push(field("tags", "TOO_MANY", "标签输入过多"));
    } else {
        for raw in &input.tags {
            if raw.len() > tags::MAX_RAW_TAG_LEN {
                errors.push(field("tags", "TOO_LONG", "单个标签过长"));
                break;
            }
            match tags::normalize_name(raw).and_then(|name| tags::slug(&name).map(|s| (name, s))) {
                Some((name, slug)) => {
                    if !tag_list.iter().any(|(_, existing)| *existing == slug) {
                        tag_list.push((name, slug));
                    }
                }
                None => {
                    errors.push(field(
                        "tags",
                        "INVALID",
                        format!("标签「{}」必须包含字母或数字", raw.trim()),
                    ));
                    break;
                }
            }
        }
        if tag_list.len() > tags::MAX_TAGS_PER_POST {
            errors.push(field(
                "tags",
                "TOO_MANY",
                format!("最多 {} 个标签", tags::MAX_TAGS_PER_POST),
            ));
        }
    }

    let mut videos: Vec<ExternalVideo> = Vec::new();
    if input.external_videos.len() > MAX_EXTERNAL_VIDEOS {
        errors.push(field(
            "externalVideos",
            "TOO_MANY",
            format!("最多只能嵌入 {MAX_EXTERNAL_VIDEOS} 个外部视频"),
        ));
    } else {
        for raw in &input.external_videos {
            match video::parse(raw) {
                Some(v) => videos.push(v),
                None => {
                    errors.push(field(
                        "externalVideos",
                        "INVALID",
                        "无法识别的 B 站视频链接",
                    ));
                    break;
                }
            }
        }
    }

    if let Some(state) = &input.editor_state {
        let size = serde_json::to_vec(state)
            .map(|v| v.len())
            .unwrap_or(usize::MAX);
        if size > EDITOR_STATE_MAX_BYTES {
            errors.push(field("editorState", "TOO_LARGE", "编辑器状态过大"));
        }
    }

    // 按原始条数拦截（不查库，POST-019）；通过后再去重。
    let raw_covers: Vec<i64> = input.cover_ids.iter().map(|id| id.get()).collect();
    check_images(principal, &raw_covers, &input.body, &mut errors);
    let mut cover_ids: Vec<i64> = Vec::with_capacity(raw_covers.len());
    for id in raw_covers {
        if !cover_ids.contains(&id) {
            cover_ids.push(id);
        }
    }

    if !errors.is_empty() {
        return Err(Problem::validation(errors));
    }
    let external_videos = serde_json::to_value(&videos).map_err(|e| Problem::internal_from(&e))?;
    Ok(Prepared {
        cover_ids,
        title,
        body: input.body,
        editor_state: input.editor_state,
        external_videos,
        category_id: input.category_id.map(Id::get),
        tags: tag_list,
        is_anonymous: input.is_anonymous,
    })
}

/// 分区必须存在且启用；管理员专属分区只有管理员能用（POST-032/033）。
async fn check_category(
    conn: &mut PgConnection,
    principal: &Principal,
    category_id: Option<i64>,
) -> Result<(), Problem> {
    let Some(id) = category_id else {
        return Ok(());
    };
    match repo::category_by_id(conn, id).await? {
        Some(category) if category.is_active => {
            if category.admin_only && !principal.role.is_admin() {
                return Err(category_admin_only());
            }
            Ok(())
        }
        _ => Err(Problem::field(
            "categoryId",
            "NOT_FOUND",
            "分区不存在或已停用",
        )),
    }
}

/// 解析标签：已停用的标签整单拒绝（先查后建，不部分创建，POST-025），缺的就建。
async fn resolve_tags(
    state: &ContentState,
    conn: &mut PgConnection,
    wanted: &[(String, String)],
) -> Result<Vec<i64>, Problem> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let slugs: Vec<String> = wanted.iter().map(|(_, slug)| slug.clone()).collect();
    let existing = repo::tags_by_slugs(conn, &slugs).await?;
    if let Some(blocked) = existing.iter().find(|t| t.is_blocked) {
        return Err(Problem::field(
            "tags",
            "BLOCKED",
            format!("标签「{}」已被停用", blocked.name),
        ));
    }
    let missing: Vec<&(String, String)> = wanted
        .iter()
        .filter(|(_, slug)| !existing.iter().any(|t| &t.slug == slug))
        .collect();
    let mut rows = existing;
    if !missing.is_empty() {
        let ids: Vec<i64> = missing.iter().map(|_| state.ids.next_raw()).collect();
        let names: Vec<String> = missing.iter().map(|(name, _)| name.clone()).collect();
        let new_slugs: Vec<String> = missing.iter().map(|(_, slug)| slug.clone()).collect();
        repo::insert_tags(conn, &ids, &names, &new_slugs).await?;
        rows = repo::tags_by_slugs(conn, &slugs).await?;
        // 并发请求可能抢先建了同名标签并（在我们重查之前）被停用。
        if let Some(blocked) = rows.iter().find(|t| t.is_blocked) {
            return Err(Problem::field(
                "tags",
                "BLOCKED",
                format!("标签「{}」已被停用", blocked.name),
            ));
        }
    }
    slugs
        .iter()
        .map(|slug| {
            rows.iter()
                .find(|t| &t.slug == slug)
                .map(|t| t.id)
                .ok_or_else(|| {
                    tracing::error!(%slug, "标签插入后重查不到");
                    Problem::internal()
                })
        })
        .collect()
}

/// 新引用的封面必须是自己图库里的图片（D-012 方案 A）。帖子里原本就有的不再校验
/// （作者之后从图库移除了也不影响已经用上的封面）。
async fn check_cover_owner(
    conn: &mut PgConnection,
    principal: &Principal,
    cover_ids: &[i64],
    existing: &[i64],
) -> Result<(), Problem> {
    let new: Vec<i64> = cover_ids
        .iter()
        .copied()
        .filter(|id| !existing.contains(id))
        .collect();
    if new.is_empty() {
        return Ok(());
    }
    let owned = ik_media::interop::owned_media(conn, principal.user_id.get(), &new).await?;
    if new.iter().all(|id| owned.contains(id)) {
        Ok(())
    } else {
        Err(Problem::field(
            "coverIds",
            "NOT_FOUND",
            "图片不存在或不在你的图库里",
        ))
    }
}

async fn resolve(
    state: &ContentState,
    conn: &mut PgConnection,
    principal: &Principal,
    prepared: Prepared,
    existing_covers: &[i64],
) -> Result<Content, Problem> {
    check_category(conn, principal, prepared.category_id).await?;
    check_cover_owner(conn, principal, &prepared.cover_ids, existing_covers).await?;
    let tag_ids = resolve_tags(state, conn, &prepared.tags).await?;
    Ok(Content {
        cover_ids: prepared.cover_ids,
        title: prepared.title,
        body: prepared.body,
        editor_state: prepared.editor_state,
        external_videos: prepared.external_videos,
        category_id: prepared.category_id,
        tag_ids,
        is_anonymous: prepared.is_anonymous,
    })
}

fn ensure_owner(principal: &Principal, row: &PostRow) -> Result<(), Problem> {
    if row.author_id == principal.user_id.get() {
        Ok(())
    } else {
        Err(Problem::forbidden())
    }
}

/// 已发布的匿名帖不可修改、下线、删除（POST-043）。
fn ensure_editable(row: &PostRow) -> Result<(), Problem> {
    if row.is_anonymous && row.status == "published" {
        Err(anonymous_locked())
    } else {
        Ok(())
    }
}

fn ensure_revision(row: &PostRow, expected: Option<i32>) -> Result<(), Problem> {
    match expected {
        Some(rev) if rev != row.revision => Err(revision_conflict()),
        _ => Ok(()),
    }
}

/// 锁住帖子并完成作者校验。
async fn lock_own_post(
    conn: &mut PgConnection,
    principal: &Principal,
    post_id: i64,
) -> Result<PostRow, Problem> {
    let row = repo::lock_post(conn, post_id)
        .await?
        .ok_or_else(post_not_found)?;
    ensure_owner(principal, &row)?;
    ensure_editable(&row)?;
    Ok(row)
}

/// 发布过一次就不能再切换匿名，下线回到草稿也一样：新帖通知与看过的人都能把匿名名对上真人（隐私红线）。
fn ensure_anonymity_unchanged(row: &PostRow, is_anonymous: bool) -> Result<(), Problem> {
    if row.first_published_at.is_some() && is_anonymous != row.is_anonymous {
        return Err(Problem::field(
            "isAnonymous",
            "LOCKED",
            "发布过的委托不能切换匿名",
        ));
    }
    Ok(())
}

/// 匿名切换：开启时复用已有种子（POST-053），关闭时保留种子以便再次开启时名字不变（种子永不下发）。
fn seed_for(is_anonymous: bool, existing: Option<&str>) -> Result<Option<String>, Problem> {
    match (is_anonymous, existing) {
        (_, Some(seed)) => Ok(Some(seed.to_owned())),
        (true, None) => Ok(Some(anonymous::new_seed()?)),
        (false, None) => Ok(None),
    }
}

pub(crate) fn published_event(
    post_id: i64,
    category_id: Option<i64>,
    first: bool,
) -> Result<Event, Problem> {
    Event::new(
        "post.published",
        Audience::Broadcast,
        &serde_json::json!({
            "postId": post_id.to_string(),
            "categoryId": category_id.map(|id| id.to_string()),
            "firstPublish": first,
        }),
    )
}

pub(crate) fn removed_event(post_id: i64) -> Result<Event, Problem> {
    Event::new(
        "post.removed",
        Audience::Broadcast,
        &serde_json::json!({ "postId": post_id.to_string() }),
    )
}

/// 首次发布时安排通知作者的粉丝（FOLLOW-009），由后台任务分批完成。匿名帖不要调用（FOLLOW-016）。
/// 大多数作者没有粉丝，这时不建任务。
async fn fanout_first_publish(
    state: &ContentState,
    conn: &mut PgConnection,
    author_id: i64,
    post_id: i64,
) -> Result<(), Problem> {
    if ik_social::interop::has_followers(conn, author_id).await? {
        crate::jobs::FanoutNewPost::schedule(state, conn, post_id).await?;
    }
    Ok(())
}

/// 首次发布的发帖经验：+4，每个业务日只有第一篇（POST-073）。
/// 「是否首次发布」看帖子自己的 `first_published_at`，不看经验流水（D-025 方案 A）。
async fn grant_first_publish_exp(
    state: &ContentState,
    conn: &mut PgConnection,
    author_id: i64,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    ik_economy::service::grant_exp(
        conn,
        &state.ids,
        author_id,
        ExpAction::CreatePost,
        4,
        Dedup::Daily,
        now,
    )
    .await?;
    Ok(())
}

// ---------- 写入 ----------

/// 新建帖子。`publish` 为真时直接发布，否则存为草稿。
pub async fn create(
    state: &ContentState,
    principal: &Principal,
    input: PostInput,
    publish: bool,
) -> Result<PostEditView, Problem> {
    principal.require_write()?;
    let prepared = prepare(principal, input)?;
    let post_id = state.ids.next_raw();
    let author_id = principal.user_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();

    let mut tx = state.db.begin().await?;
    let content = resolve(state, &mut tx, principal, prepared, &[]).await?;
    let seed = seed_for(content.is_anonymous, None)?;
    repo::insert_post(&mut tx, post_id, author_id, &content, seed.as_deref(), now).await?;
    repo::set_post_tags(&mut tx, post_id, &content.tag_ids).await?;
    if publish {
        repo::mark_published(&mut tx, post_id, author_id, now).await?;
        grant_first_publish_exp(state, &mut tx, author_id, now).await?;
        if !content.is_anonymous {
            fanout_first_publish(state, &mut tx, author_id, post_id).await?;
        }
        effects.publish(published_event(post_id, content.category_id, true)?);
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());

    edit_view_by_id(state, post_id).await
}

/// 保存工作副本。草稿直接改帖子；已发布的帖子存为“待发布修改”，线上内容不变。
pub async fn save_draft(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
    input: PostInput,
    expected_revision: Option<i32>,
) -> Result<PostEditView, Problem> {
    principal.require_write()?;
    let prepared = prepare(principal, input)?;
    let now = Utc::now();

    let mut tx = state.db.begin().await?;
    let row = lock_own_post(&mut tx, principal, post_id).await?;
    ensure_revision(&row, expected_revision)?;
    ensure_anonymity_unchanged(&row, prepared.is_anonymous)?;
    let existing = existing_covers(&mut tx, &row).await?;
    let content = resolve(state, &mut tx, principal, prepared, &existing).await?;
    if row.status == "published" {
        repo::upsert_draft(&mut tx, post_id, &content, now).await?;
        repo::bump_revision(&mut tx, post_id, now).await?;
    } else {
        let seed = seed_for(content.is_anonymous, row.anonymous_seed.as_deref())?;
        repo::update_post_content(&mut tx, post_id, &content, seed.as_deref(), now).await?;
        repo::set_post_tags(&mut tx, post_id, &content.tag_ids).await?;
    }
    tx.commit().await?;

    edit_view_by_id(state, post_id).await
}

/// 把草稿 / 待发布修改的内容写回帖子行，并清掉待发布修改。
async fn apply_content(
    conn: &mut PgConnection,
    row: &PostRow,
    content: &Content,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    let seed = seed_for(content.is_anonymous, row.anonymous_seed.as_deref())?;
    repo::update_post_content(conn, row.id, content, seed.as_deref(), now).await?;
    repo::set_post_tags(conn, row.id, &content.tag_ids).await?;
    repo::delete_draft(conn, row.id).await?;
    Ok(())
}

fn draft_content(row: &PostRow, draft: DraftRow) -> Content {
    Content {
        title: draft.title,
        body: draft.body,
        editor_state: draft.editor_state,
        external_videos: draft.external_videos,
        cover_ids: draft.cover_ids,
        category_id: draft.category_id,
        tag_ids: draft.tag_ids,
        is_anonymous: row.is_anonymous,
    }
}

/// 帖子里已经用上的封面（线上版本 + 待发布修改）。
async fn existing_covers(conn: &mut PgConnection, row: &PostRow) -> Result<Vec<i64>, Problem> {
    let mut ids = row.cover_ids.clone();
    if let Some(draft) = repo::pending_draft(conn, row.id).await? {
        ids.extend(draft.cover_ids);
    }
    Ok(ids)
}

/// 发布。可同时提交最新的工作副本（省一次保存请求）。
///
/// - 草稿 → 发布：刷新信息流排序；从没发布过时算首次发布（POST-054/057）。
/// - 已发布 + 有待发布修改：写回修改，只记编辑时间，不顶帖（POST-100）。
pub async fn publish(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
    input: Option<PostInput>,
    expected_revision: Option<i32>,
) -> Result<PostEditView, Problem> {
    principal.require_write()?;
    let prepared = input.map(|i| prepare(principal, i)).transpose()?;
    let now = Utc::now();
    let author_id = principal.user_id.get();
    let mut effects = Effects::new();

    let mut tx = state.db.begin().await?;
    let row = lock_own_post(&mut tx, principal, post_id).await?;
    ensure_revision(&row, expected_revision)?;
    let was_published = row.status == "published";
    if let Some(prepared) = &prepared {
        ensure_anonymity_unchanged(&row, prepared.is_anonymous)?;
    }

    let mut changed = false;
    let mut category_id = row.category_id;
    let mut title = row.title.clone();
    if let Some(prepared) = prepared {
        let existing = existing_covers(&mut tx, &row).await?;
        let content = resolve(state, &mut tx, principal, prepared, &existing).await?;
        category_id = content.category_id;
        title.clone_from(&content.title);
        apply_content(&mut tx, &row, &content, now).await?;
        changed = true;
    } else if let Some(draft) = repo::pending_draft(&mut tx, post_id).await? {
        // 草稿期间等级可能下降、分区可能被停用：发布前按当前状态再校验一次（POST-057）。
        let content = draft_content(&row, draft);
        let mut errors = Vec::new();
        check_body(principal, &content.body, &mut errors);
        check_images(principal, &content.cover_ids, &content.body, &mut errors);
        if !errors.is_empty() {
            return Err(Problem::validation(errors));
        }
        check_category(&mut tx, principal, content.category_id).await?;
        category_id = content.category_id;
        title.clone_from(&content.title);
        apply_content(&mut tx, &row, &content, now).await?;
        changed = true;
    } else if !was_published {
        let mut errors = Vec::new();
        check_body(principal, &row.body, &mut errors);
        check_images(principal, &row.cover_ids, &row.body, &mut errors);
        if !errors.is_empty() {
            return Err(Problem::validation(errors));
        }
        check_category(&mut tx, principal, row.category_id).await?;
    }

    if !was_published {
        let first = row.first_published_at.is_none();
        repo::mark_published(&mut tx, post_id, author_id, now).await?;
        if first {
            grant_first_publish_exp(state, &mut tx, author_id, now).await?;
            if !row.is_anonymous {
                fanout_first_publish(state, &mut tx, author_id, post_id).await?;
            }
        }
        effects.publish(published_event(post_id, category_id, first)?);
    } else if changed {
        repo::mark_edited(&mut tx, post_id, now).await?;
        effects.publish(published_event(post_id, category_id, false)?);
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());

    edit_view_by_id(state, post_id).await
}

/// 丢弃已发布帖子的待发布修改，回到线上版本（POST-058）。
pub async fn discard_pending(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
) -> Result<PostEditView, Problem> {
    principal.require_write()?;
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let row = lock_own_post(&mut tx, principal, post_id).await?;
    if row.status != "published" {
        return Err(not_published());
    }
    if repo::delete_draft(&mut tx, post_id).await? {
        repo::bump_revision(&mut tx, post_id, now).await?;
    }
    tx.commit().await?;
    edit_view_by_id(state, post_id).await
}

/// 下线：帖子回到草稿。待发布修改默认并入草稿保留（POST-059）。
pub async fn unpublish(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
    discard_pending: bool,
) -> Result<PostEditView, Problem> {
    principal.require_write()?;
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let row = lock_own_post(&mut tx, principal, post_id).await?;
    if row.status != "published" {
        return Err(not_published());
    }
    if let Some(draft) = repo::pending_draft(&mut tx, post_id).await? {
        if discard_pending {
            repo::delete_draft(&mut tx, post_id).await?;
        } else {
            let content = draft_content(&row, draft);
            apply_content(&mut tx, &row, &content, now).await?;
        }
    }
    repo::mark_unpublished(&mut tx, post_id, now).await?;
    effects.publish(removed_event(post_id)?);
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    edit_view_by_id(state, post_id).await
}

/// 删除自己的帖子。不需要通过考试（用户总能删除自己的内容，D-015）；不扣丁尼（D-007）。
pub async fn delete(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
) -> Result<(), Problem> {
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let row = lock_own_post(&mut tx, principal, post_id).await?;
    repo::delete_post(&mut tx, post_id).await?;
    if row.status == "published" {
        effects.publish(removed_event(post_id)?);
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(())
}

/// 管理员置顶（幂等）。只能置顶公开帖子。
pub async fn pin(state: &ContentState, principal: &Principal, post_id: i64) -> Result<(), Problem> {
    if !principal.role.is_admin() {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "只有管理员可以置顶帖子",
        ));
    }
    let mut conn = state.db.acquire().await?;
    if repo::pin(&mut conn, post_id, principal.user_id.get(), Utc::now()).await? {
        Ok(())
    } else {
        Err(post_not_found())
    }
}

pub async fn unpin(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
) -> Result<(), Problem> {
    if !principal.role.is_admin() {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "只有管理员可以取消置顶",
        ));
    }
    let mut conn = state.db.acquire().await?;
    if repo::unpin(&mut conn, post_id).await? {
        Ok(())
    } else {
        Err(post_not_found())
    }
}

/// 给帖子投 1 丁尼（DENNY-008）。帖子必须公开（D-048 方案 A：被隐藏的也不能投）、
/// 不能是匿名帖、不能是自己的帖子、双方不能有拉黑关系（BLOCK-016）；每人每帖终身一次。
pub async fn gift_denny(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
    request: GiftRequest,
) -> Result<GiftResult, Problem> {
    principal.require_write()?;
    let giver_id = principal.user_id.get();
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let post = repo::lock_post_ref(&mut tx, post_id)
        .await?
        .filter(|p| p.is_public)
        .ok_or_else(post_not_found)?;
    if post.is_anonymous {
        return Err(Problem::new(
            StatusCode::FORBIDDEN,
            "DENNY_ANONYMOUS_POST",
            "不能给匿名委托投丁尼",
        ));
    }
    if post.author_id == giver_id {
        return Err(ik_economy::service::self_gift());
    }
    ik_social::interop::ensure_no_block(&mut tx, giver_id, post.author_id).await?;
    let outcome = ik_economy::service::gift_post(
        &mut tx,
        &state.ids,
        giver_id,
        post.author_id,
        post.id,
        &request.message,
        now,
    )
    .await?;
    let denny_count = repo::increment_denny_count(&mut tx, post.id).await?;
    // 投币通知与扣款同一事务写入，推送在提交后（NOTIF-040）。
    let mut effects = Effects::new();
    notify_author(
        state,
        &mut tx,
        &post,
        giver_id,
        Kind::Denny,
        now,
        &mut effects,
    )
    .await?;
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(GiftResult {
        balance: outcome.balance,
        denny_count,
    })
}

/// 浏览量计数（POST-080）。`viewer` 是 `u:<用户 id>` 或 `ip:<地址>`。
/// 限流或冷却命中时照常 200，只是不计数。帖子不存在时冷却键保留，挡住反复上报。
pub async fn record_view(
    state: &ContentState,
    viewer: String,
    post_id: i64,
) -> Result<ViewResult, Problem> {
    let not_counted = ViewResult {
        counted: false,
        view_count: None,
    };
    if state.views.limiter.check_key(&viewer).is_err() {
        return Ok(not_counted);
    }
    let fresh = state
        .views
        .cooldown
        .entry(format!("{viewer}|{post_id}"))
        .or_insert(())
        .await
        .is_fresh();
    if !fresh {
        return Ok(not_counted);
    }
    let mut conn = state.db.acquire().await?;
    let views = repo::increment_views(&mut conn, post_id)
        .await?
        .ok_or_else(post_not_found)?;
    Ok(ViewResult {
        counted: true,
        view_count: Some(views),
    })
}

// ---------- 读取 ----------

/// 作者展示：匿名内容只给派生的假名；已注销账号显示「已注销用户」。评论等其他域共用。
/// 作者资料与头像缩略图地址（按用户 id）。
#[derive(Debug, Default)]
pub struct Authors {
    map: HashMap<i64, Author>,
    avatars: HashMap<i64, String>,
}

/// 批量读取作者资料与头像。匿名内容的作者不要传进来。
pub async fn load_authors(
    db: &sqlx::PgPool,
    media: &ik_media::MediaState,
    ids: &[i64],
) -> Result<Authors, Problem> {
    let map = ik_auth::service::authors(db, ids).await?;
    let avatars = media.avatar_urls(map.values()).await?;
    Ok(Authors { map, avatars })
}

pub fn author_view(
    author_id: i64,
    is_anonymous: bool,
    seed: Option<&str>,
    authors: &Authors,
) -> AuthorView {
    if is_anonymous {
        return AuthorView {
            id: None,
            name: anonymous::display_name(seed.unwrap_or(ANONYMOUS_FALLBACK_SEED)),
            level: None,
            anonymous: true,
            avatar_url: None,
        };
    }
    match authors.map.get(&author_id) {
        Some(author) if !author.gone => AuthorView {
            id: Some(author.id),
            name: author.username.clone(),
            level: Some(author.level),
            anonymous: false,
            avatar_url: authors.avatars.get(&author_id).cloned(),
        },
        _ => AuthorView {
            id: None,
            name: GONE_USER_NAME.to_owned(),
            level: None,
            anonymous: false,
            avatar_url: None,
        },
    }
}

pub(crate) fn excerpt(body: &str) -> String {
    body.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(EXCERPT_CHARS)
        .collect()
}

async fn tag_map(
    conn: &mut PgConnection,
    post_ids: &[i64],
) -> Result<HashMap<i64, Vec<TagRef>>, Problem> {
    let mut map: HashMap<i64, Vec<TagRef>> = HashMap::new();
    for row in repo::tags_for_posts(conn, post_ids).await? {
        map.entry(row.post_id).or_default().push(TagRef {
            name: row.name,
            slug: row.slug,
        });
    }
    Ok(map)
}

/// 只查非匿名帖的作者：匿名帖的真实作者不进入任何后续流程。
async fn author_map(state: &ContentState, rows: &[&CardRow]) -> Result<Authors, Problem> {
    let mut ids: Vec<i64> = rows
        .iter()
        .filter(|r| !r.is_anonymous)
        .map(|r| r.author_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    load_authors(&state.db, &state.media, &ids).await
}

async fn cards(
    state: &ContentState,
    conn: &mut PgConnection,
    viewer: Option<&Principal>,
    rows: Vec<CardRow>,
) -> Result<Vec<PostCard>, Problem> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut tags = tag_map(conn, &ids).await?;
    let authors = author_map(state, &rows.iter().collect::<Vec<_>>()).await?;
    let cover_ids: Vec<i64> = rows.iter().filter_map(|r| r.cover_id).collect();
    let mut covers = state.media.views(&cover_ids).await?;
    let viewer_id = viewer.map(|p| p.user_id.get());
    // 游客不查阅读记录（CACHE-008）。
    let read: std::collections::HashSet<i64> = match viewer_id {
        Some(viewer_id) if !ids.is_empty() => repo::read_among(conn, viewer_id, &ids)
            .await?
            .into_iter()
            .collect(),
        _ => std::collections::HashSet::new(),
    };
    Ok(rows
        .into_iter()
        .map(|row| PostCard {
            id: Id::new(row.id),
            author: author_view(
                row.author_id,
                row.is_anonymous,
                row.anonymous_seed.as_deref(),
                &authors,
            ),
            excerpt: excerpt(&row.excerpt_source),
            cover: row.cover_id.and_then(|id| covers.remove(&id)),
            title: row.title,
            category: match (row.category_id, row.category_slug, row.category_name) {
                (Some(id), Some(slug), Some(name)) => Some(CategoryRef {
                    id: Id::new(id),
                    slug,
                    name,
                }),
                _ => None,
            },
            tags: tags.remove(&row.id).unwrap_or_default(),
            is_anonymous: row.is_anonymous,
            is_pinned: row.pinned_at.is_some(),
            is_solved: row.solved_at.is_some(),
            view_count: row.view_count,
            like_count: row.like_count,
            comment_count: row.comment_count,
            published_at: row.published_at,
            bumped_at: row.bumped_at,
            edited_at: row.edited_at,
            bumped_by_self: viewer_id.is_some() && row.last_bumped_by == viewer_id,
            is_read: read.contains(&row.id),
        })
        .collect())
}

#[derive(Serialize, Deserialize)]
struct FeedCursor {
    b: DateTime<Utc>,
    i: i64,
}

fn page_limit(limit: Option<u32>) -> i64 {
    i64::from(limit.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE))
}

/// 解析后的信息流参数。
struct FeedScope {
    category_id: Option<i64>,
    tag_id: Option<i64>,
    after: Option<(DateTime<Utc>, i64)>,
    limit: i64,
}

async fn feed_scope(conn: &mut PgConnection, query: &FeedQuery) -> Result<FeedScope, Problem> {
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<FeedCursor>)
        .transpose()?
        .map(|c| (c.b, c.i));
    let category_id = match query.category.as_deref() {
        Some(slug) => match repo::category_by_slug(conn, slug).await? {
            Some(c) if c.is_active => Some(c.id),
            _ => {
                return Err(Problem::new(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    "分区不存在",
                ));
            }
        },
        None => None,
    };
    let tag_id = match query.tag.as_deref() {
        Some(slug) => match repo::tag_by_slug(conn, slug).await? {
            Some(t) if !t.is_blocked => Some(t.id),
            _ => {
                return Err(Problem::new(
                    StatusCode::NOT_FOUND,
                    "NOT_FOUND",
                    "标签不存在",
                ));
            }
        },
        None => None,
    };
    Ok(FeedScope {
        category_id,
        tag_id,
        after,
        limit: page_limit(query.limit),
    })
}

fn feed_page(items: Vec<PostCard>, limit: i64) -> Result<Page<PostCard>, Problem> {
    Page::from_rows(items, usize::try_from(limit).unwrap_or(20), |card| {
        FeedCursor {
            b: card.bumped_at,
            i: card.id.get(),
        }
    })
}

/// 置顶位 + 热门位 + 普通流。
struct Composed {
    pinned: Vec<PostCard>,
    hot: Vec<PostCard>,
    page: Page<PostCard>,
}

/// 组合信息流（FEED-140～149）：
/// - 置顶：没有按标签筛选时出现在第一页（分区页也显示，D-010 方案 A）；
/// - 热门：`hot_ids` 非空且既没按分区也没按标签筛选时出现在第一页，去掉已置顶的，被过滤的位置不补位（D-045）；
/// - 普通流：始终排除置顶与**原始**热门 id（翻页时也排除，避免第一页展示过的帖子再出现，FEED-143）。
async fn compose(
    state: &ContentState,
    viewer: Option<&Principal>,
    query: &FeedQuery,
    with_pinned: bool,
    hot_ids: &[i64],
) -> Result<Composed, Problem> {
    let mut conn = state.db.acquire().await?;
    let scope = feed_scope(&mut conn, query).await?;
    // 与读者有拉黑关系的作者的非匿名帖不出现（BLOCK-013），置顶、热门也一样。
    let excluded_authors =
        ik_social::interop::excluded_authors(&mut conn, viewer.map(|p| p.user_id.get())).await?;
    let show_pinned = with_pinned && scope.tag_id.is_none();
    let show_hot = !hot_ids.is_empty() && scope.category_id.is_none() && scope.tag_id.is_none();
    let first_page = scope.after.is_none();

    let pinned_rows = if show_pinned && first_page {
        repo::pinned_cards(&mut conn, &excluded_authors, MAX_PINNED).await?
    } else {
        Vec::new()
    };
    let mut hot_rows = if show_hot && first_page {
        repo::public_cards_by_ids(&mut conn, hot_ids, &excluded_authors).await?
    } else {
        Vec::new()
    };
    if show_pinned {
        hot_rows.retain(|row| row.pinned_at.is_none());
    }
    hot_rows.sort_by_key(|row| hot_ids.iter().position(|id| *id == row.id));

    let rows = repo::feed(
        &mut conn,
        &FeedFilter {
            category_id: scope.category_id,
            tag_id: scope.tag_id,
            exclude_pinned: show_pinned,
            after: scope.after,
            limit: scope.limit + 1,
            excluded_authors,
            exclude_ids: if show_hot {
                hot_ids.to_vec()
            } else {
                Vec::new()
            },
            author_ids: None,
            unread_for: None,
        },
    )
    .await?;

    let pinned = cards(state, &mut conn, viewer, pinned_rows).await?;
    let hot = cards(state, &mut conn, viewer, hot_rows).await?;
    let items = cards(state, &mut conn, viewer, rows).await?;
    Ok(Composed {
        pinned,
        hot,
        page: feed_page(items, scope.limit)?,
    })
}

/// 最新信息流（`GET /v1/posts`）：按最近活跃倒序，第一页带置顶，不注入热门。
pub async fn feed(
    state: &ContentState,
    viewer: Option<&Principal>,
    query: FeedQuery,
) -> Result<FeedPage, Problem> {
    let composed = compose(state, viewer, &query, true, &[]).await?;
    Ok(FeedPage {
        pinned: composed.pinned,
        items: composed.page.items,
        next_cursor: composed.page.next_cursor,
    })
}

/// 首页信息流：最新流 + 置顶 + 热门位（`hot_ids` 由 feed 域的热榜给出，已按展示位规则限流）。
pub async fn home_feed(
    state: &ContentState,
    viewer: Option<&Principal>,
    query: FeedQuery,
    hot_ids: &[i64],
) -> Result<HomeFeedPage, Problem> {
    let composed = compose(state, viewer, &query, true, hot_ids).await?;
    Ok(HomeFeedPage {
        pinned: composed.pinned,
        hot: composed.hot,
        items: composed.page.items,
        next_cursor: composed.page.next_cursor,
    })
}

/// 关注流：关注的人的**非匿名**帖子，按最近活跃倒序，不注入置顶与热门（FEED-130）。
/// 关注人数不设上限（D-041 方案 A）。
pub async fn following_feed(
    state: &ContentState,
    principal: &Principal,
    query: FeedQuery,
) -> Result<Page<PostCard>, Problem> {
    let me = principal.user_id.get();
    let mut conn = state.db.acquire().await?;
    let scope = feed_scope(&mut conn, &query).await?;
    let followees = ik_social::interop::following_ids(&mut conn, me).await?;
    if followees.is_empty() {
        return Ok(Page {
            items: Vec::new(),
            next_cursor: None,
        });
    }
    let excluded_authors = ik_social::interop::excluded_authors(&mut conn, Some(me)).await?;
    let rows = repo::feed(
        &mut conn,
        &FeedFilter {
            category_id: scope.category_id,
            tag_id: scope.tag_id,
            exclude_pinned: false,
            after: scope.after,
            limit: scope.limit + 1,
            excluded_authors,
            exclude_ids: Vec::new(),
            author_ids: Some(followees),
            unread_for: None,
        },
    )
    .await?;
    let items = cards(state, &mut conn, Some(principal), rows).await?;
    feed_page(items, scope.limit)
}

/// 未看流：最新流去掉自己读过的帖子，不注入置顶与热门（UNRD-001～006）。
pub async fn unread_feed(
    state: &ContentState,
    principal: &Principal,
    query: FeedQuery,
) -> Result<Page<PostCard>, Problem> {
    let me = principal.user_id.get();
    let mut conn = state.db.acquire().await?;
    let scope = feed_scope(&mut conn, &query).await?;
    let excluded_authors = ik_social::interop::excluded_authors(&mut conn, Some(me)).await?;
    let rows = repo::feed(
        &mut conn,
        &FeedFilter {
            category_id: scope.category_id,
            tag_id: scope.tag_id,
            exclude_pinned: false,
            after: scope.after,
            limit: scope.limit + 1,
            excluded_authors,
            exclude_ids: Vec::new(),
            author_ids: None,
            unread_for: Some(me),
        },
    )
    .await?;
    let items = cards(state, &mut conn, Some(principal), rows).await?;
    feed_page(items, scope.limit)
}

/// 按给定顺序取公开、且不被拉黑过滤的帖子卡片；不可见的跳过（热门榜：先过滤再分页，EDGE-006）。
pub async fn public_cards_in_order(
    state: &ContentState,
    viewer: Option<&Principal>,
    ids: &[i64],
) -> Result<Vec<PostCard>, Problem> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut conn = state.db.acquire().await?;
    let excluded_authors =
        ik_social::interop::excluded_authors(&mut conn, viewer.map(|p| p.user_id.get())).await?;
    let mut rows = repo::public_cards_by_ids(&mut conn, ids, &excluded_authors).await?;
    rows.sort_by_key(|row| ids.iter().position(|id| *id == row.id));
    cards(state, &mut conn, viewer, rows).await
}

/// 最多一次标记 100 篇（READ-002）。
const MAX_READ_BATCH: usize = 100;

/// 批量标为已读（READ-002）：去空白、去重，最多 100 篇；有不存在的帖子时整批拒绝（404），不写入。
pub async fn mark_read(
    state: &ContentState,
    principal: &Principal,
    request: MarkReadRequest,
) -> Result<MarkReadResult, Problem> {
    let mut ids: Vec<i64> = Vec::new();
    for raw in request
        .post_ids
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
    {
        let id = raw
            .parse::<Id<()>>()
            .map_err(|_| Problem::field("postIds", "INVALID", "帖子 id 格式不正确"))?
            .get();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    if ids.len() > MAX_READ_BATCH {
        return Err(Problem::field(
            "postIds",
            "TOO_MANY",
            format!("一次最多标记 {MAX_READ_BATCH} 篇"),
        ));
    }
    if ids.is_empty() {
        return Ok(MarkReadResult { marked: 0 });
    }
    let mut conn = state.db.acquire().await?;
    if repo::existing_post_ids(&mut conn, &ids).await?.len() != ids.len() {
        return Err(post_not_found());
    }
    let marked = repo::mark_read(&mut conn, principal.user_id.get(), &ids, Utc::now()).await?;
    Ok(MarkReadResult { marked })
}
fn blocked_post() -> Problem {
    Problem::new(StatusCode::FORBIDDEN, "USER_BLOCKED", "无法查看该委托")
}

/// 公开详情。草稿、不存在一律 404；被隐藏的帖子只有作者本人能看。
/// 读者与作者之间存在拉黑时，非匿名帖返回 403 `USER_BLOCKED`（BLOCK-012）；匿名帖照常可看。
pub async fn detail(
    state: &ContentState,
    viewer: Option<&Principal>,
    post_id: i64,
) -> Result<PostDetail, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::get_post(&mut conn, post_id)
        .await?
        .ok_or_else(post_not_found)?;
    let is_owner = viewer.is_some_and(|p| p.user_id.get() == row.author_id);
    if row.status != "published" || (row.hidden_at.is_some() && !is_owner) {
        return Err(post_not_found());
    }
    if let Some(viewer) = viewer
        && !row.is_anonymous
        && ik_social::interop::any_block(&mut conn, viewer.user_id.get(), row.author_id).await?
    {
        return Err(blocked_post());
    }
    let (Some(published_at), Some(first_published_at), Some(bumped_at)) =
        (row.published_at, row.first_published_at, row.bumped_at)
    else {
        tracing::error!(post_id, "已发布帖子缺少发布时间字段");
        return Err(Problem::internal());
    };

    let category = match row.category_id {
        Some(id) => repo::category_by_id(&mut conn, id).await?,
        None => None,
    };
    let tags = tag_map(&mut conn, &[row.id])
        .await?
        .remove(&row.id)
        .unwrap_or_default();
    let authors = if row.is_anonymous {
        Authors::default()
    } else {
        load_authors(&state.db, &state.media, &[row.author_id]).await?
    };
    let covers = media_in_order(state, &row.cover_ids).await?;
    let external_videos: Vec<ExternalVideo> =
        serde_json::from_value(row.external_videos.clone()).unwrap_or_default();
    let denny_given = match viewer {
        Some(p) => !ik_economy::service::gifted_posts(&mut conn, p.user_id.get(), &[row.id])
            .await?
            .is_empty(),
        None => false,
    };
    let marks = match viewer {
        Some(p) => Some(repo::viewer_marks(&mut conn, row.id, p.user_id.get()).await?),
        None => None,
    };

    Ok(PostDetail {
        id: Id::new(row.id),
        author: author_view(
            row.author_id,
            row.is_anonymous,
            row.anonymous_seed.as_deref(),
            &authors,
        ),
        title: row.title,
        body: row.body,
        covers,
        allow_comment_accept: category.as_ref().is_some_and(|c| c.allow_comment_accept),
        category: category.map(|c| CategoryRef {
            id: Id::new(c.id),
            slug: c.slug,
            name: c.name,
        }),
        tags,
        external_videos,
        is_anonymous: row.is_anonymous,
        is_pinned: row.pinned_at.is_some(),
        pinned_at: row.pinned_at,
        solved_at: row.solved_at,
        view_count: row.view_count,
        like_count: row.like_count,
        comment_count: row.comment_count,
        favorite_count: row.favorite_count,
        denny_count: row.denny_count,
        published_at,
        first_published_at,
        edited_at: row.edited_at,
        bumped_at,
        is_owner,
        is_hidden: row.hidden_at.is_some(),
        denny_given,
        liked: marks.as_ref().is_some_and(|m| m.liked),
        favorited: marks.as_ref().is_some_and(|m| m.favorited),
    })
}

/// 作者编辑视图。
pub async fn edit_view(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
) -> Result<PostEditView, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::get_post(&mut conn, post_id)
        .await?
        .ok_or_else(post_not_found)?;
    ensure_owner(principal, &row)?;
    ensure_editable(&row)?;
    build_edit_view(state, &mut conn, row).await
}

async fn edit_view_by_id(state: &ContentState, post_id: i64) -> Result<PostEditView, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::get_post(&mut conn, post_id)
        .await?
        .ok_or_else(post_not_found)?;
    build_edit_view(state, &mut conn, row).await
}

/// 按给定顺序取媒体视图（不存在的跳过）。
async fn media_in_order(
    state: &ContentState,
    ids: &[i64],
) -> Result<Vec<ik_media::dto::MediaView>, Problem> {
    let mut views = state.media.views(ids).await?;
    Ok(ids.iter().filter_map(|id| views.remove(id)).collect())
}

async fn build_edit_view(
    state: &ContentState,
    conn: &mut PgConnection,
    row: PostRow,
) -> Result<PostEditView, Problem> {
    let draft = repo::pending_draft(conn, row.id).await?;
    let has_pending_changes = draft.is_some();
    let status = if row.status == "published" {
        PostStatus::Published
    } else {
        PostStatus::Draft
    };
    let (title, body, editor_state, videos, category_id, tag_ids, cover_ids, updated_at) =
        match draft {
            Some(d) => (
                d.title,
                d.body,
                d.editor_state,
                d.external_videos,
                d.category_id,
                d.tag_ids,
                d.cover_ids,
                d.updated_at.max(row.updated_at),
            ),
            None => {
                let ids = repo::post_tag_ids(conn, row.id).await?;
                (
                    row.title,
                    row.body,
                    row.editor_state,
                    row.external_videos,
                    row.category_id,
                    ids,
                    row.cover_ids,
                    row.updated_at,
                )
            }
        };
    let covers = media_in_order(state, &cover_ids).await?;
    let tag_rows = repo::tags_by_ids(conn, &tag_ids).await?;
    let tags = tag_ids
        .iter()
        .filter_map(|id| tag_rows.iter().find(|t| t.id == *id && !t.is_blocked))
        .map(|t| TagRef {
            name: t.name.clone(),
            slug: t.slug.clone(),
        })
        .collect();
    let external_videos = serde_json::from_value::<Vec<ExternalVideo>>(videos)
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.source)
        .collect();
    Ok(PostEditView {
        id: Id::new(row.id),
        status,
        title,
        body,
        category_id: category_id.map(Id::new),
        tags,
        is_anonymous: row.is_anonymous,
        external_videos,
        covers,
        editor_state,
        revision: row.revision,
        has_pending_changes,
        published_at: row.published_at,
        first_published_at: row.first_published_at,
        is_hidden: row.hidden_at.is_some(),
        updated_at,
    })
}

#[derive(Serialize, Deserialize)]
struct MyCursor {
    u: DateTime<Utc>,
    i: i64,
}

pub async fn my_posts(
    state: &ContentState,
    principal: &Principal,
    query: MyPostsQuery,
) -> Result<Page<MyPostItem>, Problem> {
    let limit = page_limit(query.limit);
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<MyCursor>)
        .transpose()?
        .map(|c| (c.u, c.i));
    let status = match query.state {
        MyPostsState::All => None,
        MyPostsState::Draft => Some("draft"),
        MyPostsState::Published => Some("published"),
    };
    let mut conn = state.db.acquire().await?;
    let rows = repo::my_posts(&mut conn, principal.user_id.get(), status, after, limit + 1).await?;
    let items = rows
        .into_iter()
        .map(|row| MyPostItem {
            id: Id::new(row.id),
            status: if row.status == "published" {
                PostStatus::Published
            } else {
                PostStatus::Draft
            },
            title: row.title,
            is_anonymous: row.is_anonymous,
            has_pending_changes: row.has_pending_changes,
            is_hidden: row.hidden_at.is_some(),
            view_count: row.view_count,
            like_count: row.like_count,
            comment_count: row.comment_count,
            published_at: row.published_at,
            updated_at: row.updated_at,
        })
        .collect();
    Page::from_rows(
        items,
        usize::try_from(limit).unwrap_or(20),
        |item: &MyPostItem| MyCursor {
            u: item.updated_at,
            i: item.id.get(),
        },
    )
}

// ---------- 分区与标签 ----------

pub async fn categories(state: &ContentState) -> Result<Vec<CategoryView>, Problem> {
    let mut conn = state.db.acquire().await?;
    Ok(repo::active_categories(&mut conn)
        .await?
        .into_iter()
        .map(|c| CategoryView {
            id: Id::new(c.id),
            slug: c.slug,
            name: c.name,
            description: c.description,
            admin_only: c.admin_only,
            allow_comment_accept: c.allow_comment_accept,
        })
        .collect())
}

fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 标签联想（POST-028）：slug 子串匹配，排除停用标签。
pub async fn suggest_tags(
    state: &ContentState,
    q: &str,
    limit: Option<u32>,
) -> Result<Vec<TagRef>, Problem> {
    if q.len() > tags::MAX_RAW_TAG_LEN {
        return Err(Problem::field("q", "TOO_LONG", "关键词过长"));
    }
    let limit = i64::from(
        limit
            .unwrap_or(TAG_SUGGEST_DEFAULT)
            .clamp(1, TAG_SUGGEST_MAX),
    );
    let pattern = format!("%{}%", like_escape(&tags::query_slug(q)));
    let mut conn = state.db.acquire().await?;
    Ok(repo::suggest_tags(&mut conn, &pattern, limit)
        .await?
        .into_iter()
        .map(|t| TagRef {
            name: t.name,
            slug: t.slug,
        })
        .collect())
}

/// 新建分区的参数（运维命令与将来的管理后台共用）。
#[derive(Debug, Clone, Default)]
pub struct NewCategoryInput {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub sort_order: i32,
    pub admin_only: bool,
    pub allow_comment_accept: bool,
}

pub async fn create_category(
    state: &ContentState,
    input: NewCategoryInput,
) -> Result<CategoryView, Problem> {
    let slug = input.slug.trim().to_lowercase();
    let name = input.name.trim();
    let slug_ok = !slug.is_empty()
        && slug.len() <= 48
        && slug.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && slug
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !slug_ok {
        return Err(Problem::field(
            "slug",
            "INVALID",
            "slug 只能包含小写字母、数字和连字符",
        ));
    }
    if name.is_empty() || name.chars().count() > 30 {
        return Err(Problem::field("name", "INVALID", "名称需为 1–30 个字"));
    }
    let id = state.ids.next_raw();
    let mut conn = state.db.acquire().await?;
    let result = repo::insert_category(
        &mut conn,
        &repo::NewCategory {
            id,
            slug: &slug,
            name,
            description: input.description.trim(),
            sort_order: input.sort_order,
            admin_only: input.admin_only,
            allow_comment_accept: input.allow_comment_accept,
        },
    )
    .await;
    match result {
        Ok(()) => {}
        Err(error) if ik_kernel::db::is_unique_violation(&error, "categories_slug_uq") => {
            return Err(Problem::conflict("ALREADY_EXISTS", "该 slug 已被使用"));
        }
        Err(error) => return Err(error.into()),
    }
    Ok(CategoryView {
        id: Id::new(id),
        slug,
        name: name.to_owned(),
        description: input.description.trim().to_owned(),
        admin_only: input.admin_only,
        allow_comment_accept: input.allow_comment_accept,
    })
}

// ---------- 点赞、收藏、三连 ----------

/// 可以新增点赞 / 收藏的帖子：必须公开（LIKE-009、FAV-007）；非匿名帖还要求双方没有拉黑
/// （LIKE-007、FAV-004），匿名帖不做拉黑检查。
async fn reactable_post(
    conn: &mut PgConnection,
    user_id: i64,
    post_id: i64,
) -> Result<repo::PostRefRow, Problem> {
    let post = repo::post_ref(conn, post_id)
        .await?
        .filter(|p| p.is_public)
        .ok_or_else(post_not_found)?;
    if !post.is_anonymous {
        ik_social::interop::ensure_no_block(conn, user_id, post.author_id).await?;
    }
    Ok(post)
}

/// 点赞者每个业务日第一次点赞（帖子与评论合计）+1 经验；赞自己的帖子不给，赞匿名帖照常给。
/// 被点赞者没有经验（LIKE-015、D-049）。
async fn grant_like_exp(
    state: &ContentState,
    conn: &mut PgConnection,
    liker_id: i64,
    author_id: i64,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    if liker_id != author_id {
        ik_economy::service::grant_exp(
            conn,
            &state.ids,
            liker_id,
            ExpAction::LikeGive,
            1,
            Dedup::Daily,
            now,
        )
        .await?;
    }
    Ok(())
}

/// 帖子被赞、被收藏、被投币时通知作者。匿名帖不发（LIKE-013、FAV-009）；
/// 自己给自己、拉黑、收件人不可用等由 notify 统一抑制。
async fn notify_author(
    state: &ContentState,
    conn: &mut PgConnection,
    post: &repo::PostRefRow,
    sender_id: i64,
    kind: Kind,
    now: DateTime<Utc>,
    effects: &mut Effects,
) -> Result<(), Problem> {
    if post.is_anonymous {
        return Ok(());
    }
    ik_notify::interop::send(
        conn,
        &state.ids,
        Notice {
            recipient_id: post.author_id,
            sender_id,
            kind,
            post_id: Some(post.id),
            comment_id: None,
            post_title: Some(&post.title),
            excerpt: "",
        },
        now,
        effects,
    )
    .await?;
    Ok(())
}

/// 点赞（幂等）。需要通过入站考试。重复点赞不重复通知。
pub async fn like(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
) -> Result<LikeResult, Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let post = reactable_post(&mut tx, me, post_id).await?;
    let added = repo::insert_like(&mut tx, post.id, me, now).await?;
    let counts = repo::add_reaction_counts(&mut tx, post.id, i32::from(added), 0)
        .await?
        .ok_or_else(post_not_found)?;
    if added {
        grant_like_exp(state, &mut tx, me, post.author_id, now).await?;
        notify_author(state, &mut tx, &post, me, Kind::Like, now, &mut effects).await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(LikeResult {
        liked: true,
        like_count: counts.like_count,
    })
}

/// 取消点赞（幂等）。不要求通过考试；帖子已经不可见（隐藏、下线、拉黑）时也能取消（LIKE-018）。
/// 当天的点赞经验不收回；发出的点赞通知撤回（D-011 方案 A）。
pub async fn unlike(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
) -> Result<LikeResult, Problem> {
    let me = principal.user_id.get();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let removed = repo::delete_like(&mut tx, post_id, me).await?;
    let counts = repo::add_reaction_counts(&mut tx, post_id, -i32::from(removed), 0)
        .await?
        .ok_or_else(post_not_found)?;
    if removed {
        ik_notify::interop::retract(&mut tx, me, Kind::Like, Target::Post(post_id), &mut effects)
            .await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(LikeResult {
        liked: false,
        like_count: counts.like_count,
    })
}

/// 收藏（幂等）。需要通过入站考试；收藏没有经验（FAV-010）。
pub async fn favorite(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
) -> Result<FavoriteResult, Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let post = reactable_post(&mut tx, me, post_id).await?;
    let added = repo::insert_favorite(&mut tx, post.id, me, now).await?;
    let counts = repo::add_reaction_counts(&mut tx, post.id, 0, i32::from(added))
        .await?
        .ok_or_else(post_not_found)?;
    if added {
        notify_author(state, &mut tx, &post, me, Kind::Favorite, now, &mut effects).await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(FavoriteResult {
        favorited: true,
        favorite_count: counts.favorite_count,
    })
}

/// 取消收藏（幂等）。不要求通过考试，帖子不可见时也能取消。收藏通知一并撤回（D-011 方案 A）。
pub async fn unfavorite(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
) -> Result<FavoriteResult, Problem> {
    let me = principal.user_id.get();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let removed = repo::delete_favorite(&mut tx, post_id, me).await?;
    let counts = repo::add_reaction_counts(&mut tx, post_id, 0, -i32::from(removed))
        .await?
        .ok_or_else(post_not_found)?;
    if removed {
        ik_notify::interop::retract(
            &mut tx,
            me,
            Kind::Favorite,
            Target::Post(post_id),
            &mut effects,
        )
        .await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());
    Ok(FavoriteResult {
        favorited: false,
        favorite_count: counts.favorite_count,
    })
}

/// 一键三连：点赞 + 收藏 + 投 1 丁尼（TRIPLE-001）。
///
/// 点赞、收藏是「确保存在」，在同一个事务里完成；任一步被拒（不可见、拉黑）整个三连失败，
/// 不会只成功一半（旧系统并行执行，可能留下一半，TRIPLE-004）。投币随后单独执行，
/// 失败只通过 `dennyError` 返回原因，不回滚点赞和收藏（TRIPLE-003）。
pub async fn triple(
    state: &ContentState,
    principal: &Principal,
    post_id: i64,
) -> Result<TripleResult, Problem> {
    principal.require_write()?;
    let me = principal.user_id.get();
    let now = Utc::now();
    let mut effects = Effects::new();
    let mut tx = state.db.begin().await?;
    let post = reactable_post(&mut tx, me, post_id).await?;
    let liked = repo::insert_like(&mut tx, post.id, me, now).await?;
    let favorited = repo::insert_favorite(&mut tx, post.id, me, now).await?;
    let counts =
        repo::add_reaction_counts(&mut tx, post.id, i32::from(liked), i32::from(favorited))
            .await?
            .ok_or_else(post_not_found)?;
    if liked {
        grant_like_exp(state, &mut tx, me, post.author_id, now).await?;
        notify_author(state, &mut tx, &post, me, Kind::Like, now, &mut effects).await?;
    }
    if favorited {
        notify_author(state, &mut tx, &post, me, Kind::Favorite, now, &mut effects).await?;
    }
    tx.commit().await?;
    effects.dispatch(state.bus.as_ref());

    let (denny_given, denny_error, denny_count, balance) =
        match gift_denny(state, principal, post.id, GiftRequest::default()).await {
            Ok(gift) => (true, None, gift.denny_count, Some(gift.balance)),
            Err(error) => {
                if error.status().is_server_error() {
                    tracing::error!(post_id = post.id, code = error.code(), "三连投币失败");
                }
                let mut conn = state.db.acquire().await?;
                let count = repo::denny_count(&mut conn, post.id).await?.unwrap_or(0);
                (false, Some(error.code().to_owned()), count, None)
            }
        };
    Ok(TripleResult {
        like_count: counts.like_count,
        favorite_count: counts.favorite_count,
        denny_given,
        denny_error,
        denny_count,
        balance,
    })
}

#[derive(Serialize, Deserialize)]
struct SavedCursor {
    s: DateTime<Utc>,
    i: i64,
}

#[derive(Clone, Copy)]
enum Saved {
    Likes,
    Favorites,
}

async fn saved_posts(
    state: &ContentState,
    principal: &Principal,
    query: SavedQuery,
    kind: Saved,
) -> Result<Page<SavedPost>, Problem> {
    let limit = page_limit(query.limit);
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<SavedCursor>)
        .transpose()?
        .map(|c| (c.s, c.i));
    let me = principal.user_id.get();
    let mut conn = state.db.acquire().await?;
    let excluded = ik_social::interop::excluded_authors(&mut conn, Some(me)).await?;
    let rows = match kind {
        Saved::Likes => repo::liked_cards(&mut conn, me, &excluded, after, limit + 1).await?,
        Saved::Favorites => {
            repo::favorite_cards(&mut conn, me, &excluded, after, limit + 1).await?
        }
    };
    let (times, rows): (Vec<DateTime<Utc>>, Vec<CardRow>) =
        rows.into_iter().map(repo::SavedCardRow::split).unzip();
    let cards = cards(state, &mut conn, Some(principal), rows).await?;
    let items = times
        .into_iter()
        .zip(cards)
        .map(|(saved_at, post)| SavedPost { post, saved_at })
        .collect();
    Page::from_rows(
        items,
        usize::try_from(limit).unwrap_or(20),
        |item: &SavedPost| SavedCursor {
            s: item.saved_at,
            i: item.post.id.get(),
        },
    )
}

/// 我点赞过的帖子（新赞的在前）。只含当前仍公开、且不被拉黑过滤的帖子。
pub async fn my_likes(
    state: &ContentState,
    principal: &Principal,
    query: SavedQuery,
) -> Result<Page<SavedPost>, Problem> {
    saved_posts(state, principal, query, Saved::Likes).await
}

/// 我收藏的帖子（新收藏的在前）。只含当前仍公开、且不被拉黑过滤的帖子。
pub async fn my_favorites(
    state: &ContentState,
    principal: &Principal,
    query: SavedQuery,
) -> Result<Page<SavedPost>, Problem> {
    saved_posts(state, principal, query, Saved::Favorites).await
}

// ---------- 作者主页 ----------

/// 作者公开的非匿名帖子统计（PROFILE-015）。
#[derive(Debug, Clone, Copy, Default)]
pub struct AuthorPostStats {
    pub post_count: i64,
    pub total_views: i64,
    pub total_comments: i64,
    pub total_likes: i64,
}

pub async fn author_stats(
    state: &ContentState,
    author_id: i64,
) -> Result<AuthorPostStats, Problem> {
    let mut conn = state.db.acquire().await?;
    let row = repo::author_stats(&mut conn, author_id).await?;
    Ok(AuthorPostStats {
        post_count: row.post_count,
        total_views: row.total_views,
        total_comments: row.total_comments,
        total_likes: row.total_likes,
    })
}

#[derive(Serialize, Deserialize)]
struct AuthorCursor {
    p: DateTime<Utc>,
    i: i64,
}

/// 作者主页的帖子（新发布的在前）。匿名帖对任何人都不出现（PROFILE-018）；
/// `include_hidden` 只应在作者本人查看时为真。
pub async fn author_posts(
    state: &ContentState,
    viewer: Option<&Principal>,
    author_id: i64,
    include_hidden: bool,
    raw_cursor: Option<&str>,
    limit: Option<u32>,
) -> Result<Page<PostCard>, Problem> {
    let limit = page_limit(limit);
    let after = raw_cursor
        .map(cursor::decode::<AuthorCursor>)
        .transpose()?
        .map(|c| (c.p, c.i));
    let mut conn = state.db.acquire().await?;
    let rows = repo::author_cards(&mut conn, author_id, include_hidden, after, limit + 1).await?;
    let items = cards(state, &mut conn, viewer, rows).await?;
    Page::from_rows(items, usize::try_from(limit).unwrap_or(20), |card| {
        AuthorCursor {
            p: card.published_at,
            i: card.id.get(),
        }
    })
}

/// 主页精选：按给定顺序返回其中仍然有效的帖子（已发布、非匿名、属于作者），失效的跳过。
pub async fn author_posts_by_ids(
    state: &ContentState,
    viewer: Option<&Principal>,
    author_id: i64,
    ids: &[i64],
    include_hidden: bool,
) -> Result<Vec<PostCard>, Problem> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut conn = state.db.acquire().await?;
    let rows = repo::author_cards_by_ids(&mut conn, author_id, ids, include_hidden).await?;
    let mut cards = cards(state, &mut conn, viewer, rows).await?;
    cards.sort_by_key(|card| ids.iter().position(|id| *id == card.id.get()));
    Ok(cards)
}

/// `ids` 中可以设为主页精选的（自己的、已发布、非匿名；被隐藏的也可以，D-026 方案 A）。
pub async fn pinnable_post_ids(
    state: &ContentState,
    author_id: i64,
    ids: &[i64],
) -> Result<Vec<i64>, Problem> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut conn = state.db.acquire().await?;
    Ok(repo::pinnable_ids(&mut conn, author_id, ids).await?)
}

/// 主页精选的候选（自己的已发布非匿名帖，新的在前，可按标题搜索，PROFILE-013）。
pub async fn pin_candidates(
    state: &ContentState,
    principal: &Principal,
    query: PinCandidatesQuery,
) -> Result<Page<PinCandidate>, Problem> {
    let limit = page_limit(query.limit);
    let after = query
        .cursor
        .as_deref()
        .map(cursor::decode::<AuthorCursor>)
        .transpose()?
        .map(|c| (c.p, c.i));
    let pattern = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(|q| format!("%{}%", like_escape(&q.chars().take(50).collect::<String>())));
    let mut conn = state.db.acquire().await?;
    let rows = repo::pin_candidates(
        &mut conn,
        principal.user_id.get(),
        pattern.as_deref(),
        after,
        limit + 1,
    )
    .await?;
    let items = rows
        .into_iter()
        .map(|row| PinCandidate {
            id: Id::new(row.id),
            title: row.title,
            published_at: row.published_at,
            is_hidden: row.hidden_at.is_some(),
        })
        .collect();
    Page::from_rows(
        items,
        usize::try_from(limit).unwrap_or(20),
        |item: &PinCandidate| AuthorCursor {
            p: item.published_at,
            i: item.id.get(),
        },
    )
}

// ---------- 搜索（search 域的数据库兜底）----------

/// 数据库兜底搜索的窗口上限（SRCH-014、EDGE-008）。
pub const SEARCH_FALLBACK_CAP: i64 = 1000;

/// 数据库兜底搜索（search 域在搜索引擎不可用或命中全是残留时调用）：返回按排序排好的帖子 id，
/// 最多 1000 个。已做公开、分区与拉黑过滤（匿名帖不按拉黑过滤）。
pub async fn search_post_ids(
    state: &ContentState,
    viewer: Option<&Principal>,
    q: &str,
    category_slug: Option<&str>,
    title_only: bool,
    hot: bool,
) -> Result<Vec<i64>, Problem> {
    let mut conn = state.db.acquire().await?;
    let excluded_authors =
        ik_social::interop::excluded_authors(&mut conn, viewer.map(|p| p.user_id.get())).await?;
    let ids = repo::search_post_ids(
        &mut conn,
        q,
        category_slug,
        &excluded_authors,
        title_only,
        hot,
        SEARCH_FALLBACK_CAP,
    )
    .await?;
    if i64::try_from(ids.len()).unwrap_or(i64::MAX) >= SEARCH_FALLBACK_CAP {
        tracing::warn!(cap = SEARCH_FALLBACK_CAP, "帖子搜索兜底触顶，结果被截断");
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_collapses_whitespace_and_truncates() {
        assert_eq!(excerpt("  第一行\n\n第二行\t  结束 "), "第一行 第二行 结束");
        assert_eq!(excerpt(&"字".repeat(500)).chars().count(), EXCERPT_CHARS);
    }

    #[test]
    fn like_pattern_is_escaped() {
        assert_eq!(like_escape(r"a%b_c\d"), r"a\%b\_c\\d");
    }
}
