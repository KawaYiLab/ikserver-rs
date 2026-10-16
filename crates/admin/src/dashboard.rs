//! 仪表盘与搜索索引运维（media-ops.md ADMINCRUD-007：重建索引、比对索引数与表行数）。

use std::sync::atomic::Ordering;

use chrono::Utc;
use ik_auth::Operator;
use ik_auth::admin::{self, Audit, UserCounts};
use ik_kernel::Problem;
use serde::Serialize;
use utoipa::ToSchema;

use crate::AdminState;

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CountPair {
    pub total: i64,
    /// 今天（北京时间业务日）新增。
    pub today: i64,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReportCounts {
    /// 待处理的举报目标数。
    pub pending: i64,
    /// 复核中（已领取）的举报目标数。
    pub reviewing: i64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Dashboard {
    pub users: UserCounts,
    pub posts: CountPair,
    pub comments: CountPair,
    pub reports: ReportCounts,
    pub exam: ik_exam::admin::QuestionStats,
    pub ai: ik_ai::admin::AiOverview,
}

pub async fn dashboard(state: &AdminState) -> Result<Dashboard, Problem> {
    let (start, _) = ik_kernel::time::biz_day_range(Utc::now());
    let mut conn = state.db.acquire().await?;
    let users = admin::user_counts(&mut conn, start).await?;
    let (posts, posts_today) = ik_content::interop::post_counts(&mut conn, start).await?;
    let (comments, comments_today) = ik_comments::interop::comment_counts(&mut conn, start).await?;
    drop(conn);
    let (pending, reviewing) = ik_moderation::admin::open_counts(&state.moderation).await?;
    Ok(Dashboard {
        users,
        posts: CountPair {
            total: posts,
            today: posts_today,
        },
        comments: CountPair {
            total: comments,
            today: comments_today,
        },
        reports: ReportCounts { pending, reviewing },
        exam: ik_exam::admin::stats(&state.exam).await?,
        ai: ik_ai::admin::overview(&state.ai).await?,
    })
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchStatus {
    /// 没有配置搜索引擎时全部查询走数据库兜底。
    pub engine_configured: bool,
    /// 索引里的文档数；读不到（未配置或搜索引擎不可用）时为空。
    pub indexed_posts: Option<i64>,
    pub indexed_comments: Option<i64>,
    /// 读取索引统计失败的原因（只给管理员排障）。
    pub engine_error: Option<String>,
    /// 数据库里已发布的帖子数、评论数（索引只收公开的，略少是正常的）。
    pub db_posts: i64,
    pub db_comments: i64,
    /// 本实例正在重建索引。
    pub reindexing: bool,
}

pub async fn search_status(state: &AdminState) -> Result<SearchStatus, Problem> {
    let (start, _) = ik_kernel::time::biz_day_range(Utc::now());
    let mut conn = state.db.acquire().await?;
    let (db_posts, _) = ik_content::interop::post_counts(&mut conn, start).await?;
    let (db_comments, _) = ik_comments::interop::comment_counts(&mut conn, start).await?;
    drop(conn);
    let (indexed, engine_error) = match state.search.document_counts().await {
        Ok(counts) => (counts, None),
        Err(error) => (None, Some(error.to_string())),
    };
    let as_i64 = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    Ok(SearchStatus {
        engine_configured: state.search.engine_configured(),
        indexed_posts: indexed.map(|(p, _)| as_i64(p)),
        indexed_comments: indexed.map(|(_, c)| as_i64(c)),
        engine_error,
        db_posts,
        db_comments,
        reindexing: state.reindexing.load(Ordering::Acquire),
    })
}

/// 在后台重建索引（全量对账：补齐缺的、删掉残留的）。同一实例同时只跑一个。
pub async fn start_reindex(state: &AdminState, op: &Operator) -> Result<(), Problem> {
    if !state.search.engine_configured() {
        return Err(Problem::conflict(
            "SEARCH_ENGINE_NOT_CONFIGURED",
            "没有配置搜索引擎",
        ));
    }
    if state
        .reindexing
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(Problem::conflict(
            "REINDEX_RUNNING",
            "索引正在重建，请稍后再试",
        ));
    }
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let recorded = admin::record(
        &mut tx,
        &state.ids,
        op,
        Audit {
            action: "search.reindex",
            target_type: "search",
            target_id: None,
            details: serde_json::json!({}),
        },
        now,
    )
    .await;
    if let Err(problem) = recorded {
        state.reindexing.store(false, Ordering::Release);
        return Err(problem);
    }
    if let Err(error) = tx.commit().await {
        state.reindexing.store(false, Ordering::Release);
        return Err(error.into());
    }
    let search = state.search.clone();
    let flag = state.reindexing.clone();
    tokio::spawn(async move {
        match search.reindex().await {
            Ok(()) => tracing::info!("管理后台触发的索引重建完成"),
            Err(error) => tracing::error!(?error, "管理后台触发的索引重建失败"),
        }
        flag.store(false, Ordering::Release);
    });
    Ok(())
}
