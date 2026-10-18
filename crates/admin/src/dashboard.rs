//! 仪表盘与搜索索引运维（media-ops.md ADMINCRUD-007：重建索引、比对索引数与表行数）。

use std::sync::atomic::Ordering;

use chrono::{NaiveDate, Utc};
use ik_auth::Operator;
use ik_auth::admin::{self, Audit, UserCounts};
use ik_kernel::Problem;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

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
    /// 其中我领取了、还没结案的。
    pub mine: i64,
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

pub async fn dashboard(state: &AdminState, op: &Operator) -> Result<Dashboard, Problem> {
    let (start, _) = ik_kernel::time::biz_day_range(Utc::now());
    let mut conn = state.db.acquire().await?;
    let users = admin::user_counts(&mut conn, start).await?;
    let (posts, posts_today) = ik_content::interop::post_counts(&mut conn, start).await?;
    let (comments, comments_today) = ik_comments::interop::comment_counts(&mut conn, start).await?;
    drop(conn);
    let (pending, reviewing) = ik_moderation::admin::open_counts(&state.moderation).await?;
    let mine = ik_moderation::admin::claimed_by(&state.moderation, op.user_id()).await?;
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
        reports: ReportCounts {
            pending,
            reviewing,
            mine,
        },
        exam: ik_exam::admin::stats(&state.exam).await?,
        ai: ik_ai::admin::overview(&state.ai).await?,
    })
}

#[derive(Debug, Default, Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
#[into_params(parameter_in = Query)]
pub struct TrendQuery {
    /// 7（默认）或 30；其他值按 7。
    pub days: Option<u32>,
}

/// 按业务日（北京时间 04:00 切日）的趋势，旧的在前；最后一天是今天（还没过完）。
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Trends {
    pub days: Vec<NaiveDate>,
    /// 新注册的账号。
    pub users: Vec<i64>,
    /// 首次发布的帖子。
    pub posts: Vec<i64>,
    pub comments: Vec<i64>,
    /// 收到的举报条数。
    pub reports: Vec<i64>,
}

pub async fn trends(state: &AdminState, query: TrendQuery) -> Result<Trends, Problem> {
    let days = if query.days == Some(30) { 30 } else { 7 };
    let today = ik_kernel::time::biz_day(Utc::now());
    let bounds = ik_kernel::time::biz_day_bounds(today, days);
    let first = today - chrono::Duration::days(i64::from(days - 1));
    let mut conn = state.db.acquire().await?;
    Ok(Trends {
        days: (0..i64::from(days))
            .map(|i| first + chrono::Duration::days(i))
            .collect(),
        users: admin::daily_signups(&mut conn, &bounds).await?,
        posts: ik_content::interop::daily_posts(&mut conn, &bounds).await?,
        comments: ik_comments::interop::daily_comments(&mut conn, &bounds).await?,
        reports: ik_moderation::admin::daily_reports(&mut conn, &bounds).await?,
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
