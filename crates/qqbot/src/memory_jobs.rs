//! QQ 对话的记忆提炼（ADR 0008 §8）：绑定用户的个人记忆（与站内同一份），以及群共享记忆。
//! 回复定稿时入队，同一个任务键每次往后推（MEM-020 的去抖），对话停下来以后才提炼一次。
use chrono::{DateTime, Duration, Utc};
use ik_ai::memory_jobs::TranscriptLine;
use ik_kernel::{
    Problem,
    jobs::{IntoTaskHandlerResult, JobSpec, TaskHandler, WorkerContext},
};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

use crate::{QqState, repo, text};

/// 一次最多提炼多少条消息（剩下的下次接着来）。
const BATCH: i64 = 60;
/// 群共享记忆至少等这么久再提炼（群里的话题比单聊散，攒一攒再看）。
const GROUP_DELAY: Duration = Duration::minutes(10);
/// 第一次提炼时往回看多久（更早的游客时期对话不追溯）。
const FIRST_LOOKBACK: Duration = Duration::days(1);
const MAX_ATTEMPTS: i16 = 3;

/// 后台任务：从一个绑定用户与一个角色的 QQ 对话里提炼个人记忆。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractMemories {
    pub user_id: i64,
    pub ai_user_id: i64,
}

/// 后台任务：从一个群与一个角色的对话里提炼群共享记忆。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractGroupMemories {
    pub group_id: i64,
    pub ai_user_id: i64,
}

impl TaskHandler for ExtractMemories {
    const IDENTIFIER: &'static str = "qq.extract_memories";
    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<QqState>() else {
            return Err("执行器没有注册 QqState".to_owned());
        };
        extract_personal(state, self.user_id, self.ai_user_id).await
    }
}

impl TaskHandler for ExtractGroupMemories {
    const IDENTIFIER: &'static str = "qq.extract_group_memories";
    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<QqState>() else {
            return Err("执行器没有注册 QqState".to_owned());
        };
        extract_group(state, self.group_id, self.ai_user_id).await
    }
}

/// 入队（在回复定稿的事务里）。群里的对话另排群共享记忆的提炼。
pub(crate) async fn enqueue(
    s: &QqState,
    tx: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
    group_id: Option<i64>,
    run_at: DateTime<Utc>,
) -> Result<(), Problem> {
    s.jobs
        .enqueue(
            tx,
            ExtractMemories {
                user_id,
                ai_user_id,
            },
            JobSpec {
                max_attempts: Some(MAX_ATTEMPTS),
                job_key: Some(format!("qq-memory:{user_id}:{ai_user_id}")),
                run_at: Some(run_at),
                ..JobSpec::default()
            },
        )
        .await?;
    if let Some(group_id) = group_id {
        s.jobs
            .enqueue(
                tx,
                ExtractGroupMemories {
                    group_id,
                    ai_user_id,
                },
                JobSpec {
                    max_attempts: Some(MAX_ATTEMPTS),
                    job_key: Some(format!("qq-group-memory:{group_id}:{ai_user_id}")),
                    run_at: Some(run_at.max(Utc::now() + GROUP_DELAY)),
                    ..JobSpec::default()
                },
            )
            .await?;
    }
    Ok(())
}

/// 个人记忆：只取这个用户自己的发言与机器人回他的话（群里别人的话不进他的记忆），按场合分批：
/// 群里说的记作在那个群说的（只在那个群用），单聊说的没有场合限制。
pub async fn extract_personal(s: &QqState, user_id: i64, ai_user_id: i64) -> Result<(), String> {
    let mut conn = s.db.acquire().await.map_err(|e| e.to_string())?;
    let since = sqlx::query_scalar!(
        "SELECT until FROM qq_memory_progress WHERE user_id=$1 AND ai_user_id=$2",
        user_id,
        ai_user_id
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or_else(|| Utc::now() - FIRST_LOOKBACK);
    // 机器人的回复以提问者的身份存（identity_id 是提问者），所以按身份取到的正是「他的话 + 回他的话」。
    let rows = sqlx::query!(
        "SELECT m.group_id, m.direction, m.content, m.created_at
         FROM qq_messages m JOIN qq_identities i ON i.id = m.identity_id
         WHERE i.user_id = $1 AND m.ai_user_id = $2 AND m.created_at > $3
         ORDER BY m.created_at, m.id LIMIT $4",
        user_id,
        ai_user_id,
        since,
        BATCH
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| e.to_string())?;
    drop(conn);
    let Some(until) = rows.iter().map(|r| r.created_at).max() else {
        return Ok(());
    };
    let mut scopes: Vec<(Option<i64>, Vec<TranscriptLine>)> = Vec::new();
    for row in rows {
        let line = TranscriptLine {
            from_ai: row.direction == "out",
            speaker: None,
            at: row.created_at,
            text: row.content,
        };
        match scopes.iter_mut().find(|(group, _)| *group == row.group_id) {
            Some((_, lines)) => lines.push(line),
            None => scopes.push((row.group_id, vec![line])),
        }
    }
    for (group_id, lines) in scopes {
        let scope = group_id.map(repo::group_scope);
        ik_ai::memory_jobs::extract_external(&s.ai, user_id, ai_user_id, scope.as_deref(), &lines)
            .await?;
    }
    sqlx::query!(
        "INSERT INTO qq_memory_progress(user_id,ai_user_id,until) VALUES($1,$2,$3)
         ON CONFLICT(user_id,ai_user_id) DO UPDATE SET until=GREATEST(qq_memory_progress.until,EXCLUDED.until)",
        user_id,
        ai_user_id,
        until
    )
    .execute(&s.db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 群共享记忆：只用绑定用户的问答（游客的话不沉淀进群的共同记忆），说话人用清洗过的群昵称。
pub async fn extract_group(s: &QqState, group_id: i64, ai_user_id: i64) -> Result<(), String> {
    let mut conn = s.db.acquire().await.map_err(|e| e.to_string())?;
    let since = sqlx::query_scalar!(
        "SELECT until FROM qq_group_memory_progress WHERE group_id=$1 AND ai_user_id=$2",
        group_id,
        ai_user_id
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| e.to_string())?
    .unwrap_or_else(|| Utc::now() - FIRST_LOOKBACK);
    let rows = sqlx::query!(
        "SELECT m.direction, m.content, m.created_at, i.nickname
         FROM qq_messages m JOIN qq_identities i ON i.id = m.identity_id
         WHERE m.group_id = $1 AND m.ai_user_id = $2 AND m.created_at > $3 AND i.user_id IS NOT NULL
         ORDER BY m.created_at, m.id LIMIT $4",
        group_id,
        ai_user_id,
        since,
        BATCH
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| e.to_string())?;
    drop(conn);
    let Some(until) = rows.iter().map(|r| r.created_at).max() else {
        return Ok(());
    };
    let lines: Vec<TranscriptLine> = rows
        .into_iter()
        .map(|row| {
            let nickname = text::nickname(&row.nickname);
            TranscriptLine {
                from_ai: row.direction == "out",
                speaker: Some(if nickname.is_empty() {
                    "群友".to_owned()
                } else {
                    nickname
                }),
                at: row.created_at,
                text: row.content,
            }
        })
        .collect();
    ik_ai::shared_memory::extract(&s.ai, &repo::group_scope(group_id), ai_user_id, &lines).await?;
    sqlx::query!(
        "INSERT INTO qq_group_memory_progress(group_id,ai_user_id,until) VALUES($1,$2,$3)
         ON CONFLICT(group_id,ai_user_id) DO UPDATE SET until=GREATEST(qq_group_memory_progress.until,EXCLUDED.until)",
        group_id,
        ai_user_id,
        until
    )
    .execute(&s.db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}
