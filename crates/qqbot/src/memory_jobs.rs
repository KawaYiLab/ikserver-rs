//! 以完整、已确认的终态 turn 提炼记忆。processed 标记与记忆变更同事务提交；时间只负责排序。
use crate::{QqState, authorization, identity::Snapshot, repo, text};
use chrono::{DateTime, Duration, Utc};
use ik_ai::memory_jobs::TranscriptLine;
use ik_kernel::{
    Problem,
    jobs::{IntoTaskHandlerResult, JobSpec, TaskHandler, WorkerContext},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};

const BATCH_TURNS: i64 = 12;
const GROUP_DELAY: Duration = Duration::minutes(10);
const MAX_ATTEMPTS: i16 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractMemories {
    pub user_id: i64,
    pub ai_user_id: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractGroupMemories {
    pub group_id: i64,
    pub ai_user_id: i64,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecoverMemoryBacklog {}

impl TaskHandler for ExtractMemories {
    const IDENTIFIER: &'static str = "qq.extract_memories";
    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(s) = ctx.get_ext::<QqState>() else {
            return Err("执行器没有注册 QqState".to_owned());
        };
        extract_personal(s, self.user_id, self.ai_user_id).await
    }
}
impl TaskHandler for ExtractGroupMemories {
    const IDENTIFIER: &'static str = "qq.extract_group_memories";
    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(s) = ctx.get_ext::<QqState>() else {
            return Err("执行器没有注册 QqState".to_owned());
        };
        extract_group(s, self.group_id, self.ai_user_id).await
    }
}
impl TaskHandler for RecoverMemoryBacklog {
    const IDENTIFIER: &'static str = "qq.recover_memories";
    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(s) = ctx.get_ext::<QqState>() else {
            return Err("执行器没有注册 QqState".to_owned());
        };
        recover(s).await.map_err(|e| format!("{e:?}"))
    }
}

#[derive(Clone, Copy)]
enum Stream {
    Personal(i64, i64),
    Group(i64, i64),
}
impl Stream {
    fn target(self) -> i64 {
        match self {
            Self::Personal(id, _) | Self::Group(id, _) => id,
        }
    }
    fn ai(self) -> i64 {
        match self {
            Self::Personal(_, ai) | Self::Group(_, ai) => ai,
        }
    }
    fn column(self) -> &'static str {
        match self {
            Self::Personal(..) => "personal_memory_processed_at",
            Self::Group(..) => "group_memory_processed_at",
        }
    }
    fn filter(self) -> &'static str {
        match self {
            Self::Personal(..) => "t.payer_user_id=$1",
            Self::Group(..) => "t.group_id=$1",
        }
    }
    async fn schedule(
        self,
        s: &QqState,
        tx: &mut PgConnection,
        at: DateTime<Utc>,
    ) -> Result<(), Problem> {
        let key = match self {
            Self::Personal(id, ai) => format!("qq-memory:{id}:{ai}"),
            Self::Group(id, ai) => format!("qq-group-memory:{id}:{ai}"),
        };
        let spec = JobSpec {
            max_attempts: Some(MAX_ATTEMPTS),
            queue_name: Some(key.clone()),
            job_key: Some(key),
            run_at: Some(at),
            ..JobSpec::default()
        };
        match self {
            Self::Personal(user_id, ai_user_id) => {
                s.jobs
                    .enqueue(
                        tx,
                        ExtractMemories {
                            user_id,
                            ai_user_id,
                        },
                        spec,
                    )
                    .await
            }
            Self::Group(group_id, ai_user_id) => {
                s.jobs
                    .enqueue(
                        tx,
                        ExtractGroupMemories {
                            group_id,
                            ai_user_id,
                        },
                        spec,
                    )
                    .await
            }
        }
    }
}

/// 兼容已有调用；新回复只应在 finish_turn 中、出站记录完整后入队。
pub(crate) async fn enqueue(
    s: &QqState,
    tx: &mut PgConnection,
    user_id: i64,
    ai_user_id: i64,
    group_id: Option<i64>,
    run_at: DateTime<Utc>,
) -> Result<(), Problem> {
    Stream::Personal(user_id, ai_user_id)
        .schedule(s, tx, run_at)
        .await?;
    if let Some(group_id) = group_id {
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        Stream::Group(group_id, ai_user_id)
            .schedule(s, tx, run_at.max(now + GROUP_DELAY))
            .await?;
    }
    Ok(())
}

/// 发送终态事务中调用：至少一段确认送达才有可提炼对话；未知/未送达的内容不进入记忆。
pub async fn finish_turn(
    s: &QqState,
    tx: &mut PgConnection,
    turn_id: i64,
    now: DateTime<Utc>,
) -> Result<(), Problem> {
    #[derive(FromRow)]
    struct Ready {
        identity_id: i64,
        owner_user_id: Option<i64>,
        binding_version: Option<i64>,
        payer_user_id: Option<i64>,
        ai_user_id: Option<i64>,
        group_id: Option<i64>,
        segments_sent: i32,
        finished_at: Option<DateTime<Utc>>,
        personal_memory_processed_at: Option<DateTime<Utc>>,
        group_memory_processed_at: Option<DateTime<Utc>>,
    }
    let row = sqlx::query_as::<_,Ready>("SELECT t.identity_id,m.owner_user_id,m.binding_version,t.payer_user_id,t.ai_user_id,t.group_id,t.segments_sent,t.finished_at,t.personal_memory_processed_at,t.group_memory_processed_at FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id WHERE t.id=$1")
        .bind(turn_id).fetch_optional(&mut *tx).await?;
    let Some(row) = row else {
        return Ok(());
    };
    if row.finished_at.is_none()
        || (row.personal_memory_processed_at.is_some() && row.group_memory_processed_at.is_some())
    {
        return Ok(());
    }
    let catalog = s.ai.catalog.get().await?;
    let eligible = row.segments_sent > 0
        && row.owner_user_id.is_some()
        && row.owner_user_id == row.payer_user_id
        && row.binding_version.is_some()
        && row.ai_user_id.is_some()
        && catalog.memory.enabled;
    if !eligible {
        sqlx::query("UPDATE qq_turns SET personal_memory_processed_at=COALESCE(personal_memory_processed_at,$2),group_memory_processed_at=COALESCE(group_memory_processed_at,$2) WHERE id=$1")
            .bind(turn_id).bind(now).execute(tx).await?;
        return Ok(());
    }
    // 身份是否还有效由提炼任务在读取前和应用事务内检查；此处不反向获取身份锁。
    let _ = row.identity_id;
    if let (Some(owner), Some(ai)) = (row.owner_user_id, row.ai_user_id) {
        enqueue(
            s,
            tx,
            owner,
            ai,
            row.group_id,
            now + Duration::seconds(catalog.memory.extract_delay_secs),
        )
        .await?;
    }
    if row.group_id.is_none() {
        sqlx::query("UPDATE qq_turns SET group_memory_processed_at=COALESCE(group_memory_processed_at,$2) WHERE id=$1")
            .bind(turn_id).bind(now).execute(tx).await?;
    }
    Ok(())
}

#[derive(Clone, FromRow)]
struct Candidate {
    id: i64,
    identity_id: i64,
    owner_user_id: i64,
    binding_version: i64,
}
impl Candidate {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            identity_id: self.identity_id,
            owner_user_id: Some(self.owner_user_id),
            binding_version: self.binding_version,
        }
    }
}

async fn candidates(s: &QqState, stream: Stream) -> Result<Vec<Candidate>, Problem> {
    // 不加 last_cursor 下限：迟到/迟提交的终态 turn 仍必须被选中。
    let sql=format!("SELECT t.id,t.identity_id,m.owner_user_id,m.binding_version FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id
        WHERE {} AND t.ai_user_id=$2 AND t.{} IS NULL AND t.finished_at IS NOT NULL AND t.segments_sent>0
        AND m.owner_user_id=t.payer_user_id AND m.binding_version IS NOT NULL
        ORDER BY t.finished_at,t.id LIMIT $3",stream.filter(),stream.column());
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(stream.target())
        .bind(stream.ai())
        .bind(BATCH_TURNS)
        .fetch_all(&s.db)
        .await?)
}
async fn mark(
    s: &QqState,
    tx: &mut PgConnection,
    stream: Stream,
    ids: &[i64],
) -> Result<(), Problem> {
    let _ = s;
    let sql = format!(
        "UPDATE qq_turns SET {}=COALESCE({},clock_timestamp()) WHERE id=ANY($1)",
        stream.column(),
        stream.column()
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(ids)
        .execute(tx)
        .await?;
    Ok(())
}
async fn continue_pending(
    s: &QqState,
    tx: &mut PgConnection,
    stream: Stream,
) -> Result<(), Problem> {
    let sql = format!(
        "SELECT EXISTS(SELECT 1 FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id WHERE {} AND t.ai_user_id=$2 AND t.{} IS NULL AND t.finished_at IS NOT NULL AND t.segments_sent>0 AND m.owner_user_id=t.payer_user_id AND m.binding_version IS NOT NULL)",
        stream.filter(),
        stream.column()
    );
    if sqlx::query_scalar::<_, bool>(sqlx::AssertSqlSafe(sql))
        .bind(stream.target())
        .bind(stream.ai())
        .fetch_one(&mut *tx)
        .await?
    {
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        stream.schedule(s, tx, now).await?;
    }
    Ok(())
}

#[derive(FromRow)]
struct Line {
    group_id: Option<i64>,
    direction: String,
    content: String,
    created_at: DateTime<Utc>,
    nickname: String,
}
async fn lines(s: &QqState, rows: &[Candidate]) -> Result<Vec<Line>, Problem> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    Ok(sqlx::query_as("SELECT t.group_id,m.direction,m.content,m.created_at,i.nickname FROM qq_turns t JOIN qq_messages source ON source.id=t.message_id JOIN qq_messages m ON m.id=source.id OR m.reply_to_id=source.id JOIN qq_identities i ON i.id=t.identity_id WHERE t.id=ANY($1) ORDER BY m.created_at,m.id")
        .bind(ids).fetch_all(&s.db).await?)
}

pub async fn extract_personal(s: &QqState, user_id: i64, ai_user_id: i64) -> Result<(), String> {
    extract_batch(s, Stream::Personal(user_id, ai_user_id))
        .await
        .map_err(|e| format!("{e:?}"))
}
pub async fn extract_group(s: &QqState, group_id: i64, ai_user_id: i64) -> Result<(), String> {
    extract_batch(s, Stream::Group(group_id, ai_user_id))
        .await
        .map_err(|e| format!("{e:?}"))
}

async fn extract_batch(s: &QqState, stream: Stream) -> Result<(), Problem> {
    let rows = candidates(s, stream).await?;
    if rows.is_empty() {
        return Ok(());
    }
    let catalog = s.ai.catalog.get().await?;
    let mut valid = Vec::new();
    let mut skipped = Vec::new();
    {
        let mut conn = s.db.acquire().await?;
        for row in rows {
            if s.config.enabled
                && catalog.memory.enabled
                && authorization::validate(&mut conn, &row.snapshot()).await?
            {
                valid.push(row);
            } else {
                skipped.push(row.id);
            }
        }
    }
    if !skipped.is_empty() {
        let mut tx = s.db.begin().await?;
        mark(s, &mut tx, stream, &skipped).await?;
        tx.commit().await?;
    }
    if valid.is_empty() {
        let mut tx = s.db.begin().await?;
        continue_pending(s, &mut tx, stream).await?;
        tx.commit().await?;
        return Ok(());
    }
    let transcript = lines(s, &valid).await?;
    // 读正文期间可能发生注销/解绑；先丢弃这批已撤销的正文，再进入外部模型调用。
    if discard_revoked(s, stream, &valid).await? {
        return Ok(());
    }

    let mut personal = Vec::new();
    let mut shared = None;
    match stream {
        Stream::Personal(user, ai) => {
            let mut scopes = std::collections::BTreeMap::<Option<i64>, Vec<TranscriptLine>>::new();
            for row in transcript {
                scopes
                    .entry(row.group_id)
                    .or_default()
                    .push(TranscriptLine {
                        from_ai: row.direction == "out",
                        speaker: None,
                        at: row.created_at,
                        text: row.content,
                    });
            }
            for (group, lines) in scopes {
                if discard_revoked(s, stream, &valid).await? {
                    return Ok(());
                }

                let scope = group.map(repo::group_scope);
                if let Some(plan) =
                    ik_ai::memory_jobs::prepare_external(&s.ai, user, ai, scope.as_deref(), &lines)
                        .await
                        .map_err(|e| {
                            tracing::warn!(error=%e,"QQ 个人记忆提炼失败");
                            Problem::service_unavailable()
                        })?
                {
                    personal.push(plan);
                }
            }
        }
        Stream::Group(group, ai) => {
            let lines: Vec<_> = transcript
                .into_iter()
                .map(|row| TranscriptLine {
                    from_ai: row.direction == "out",
                    speaker: Some(text::nickname(&row.nickname)),
                    at: row.created_at,
                    text: row.content,
                })
                .collect();
            shared =
                ik_ai::shared_memory::prepare_extract(&s.ai, &repo::group_scope(group), ai, &lines)
                    .await
                    .map_err(|e| {
                        tracing::warn!(error=%e,"QQ 群记忆提炼失败");
                        Problem::service_unavailable()
                    })?;
        }
    }
    let mut tx = s.db.begin().await?;
    let memory_on = ik_ai::memory_jobs::enabled_in(&mut tx).await?;
    let mut revoked = Vec::new();
    valid.sort_by_key(|r| (r.owner_user_id, r.identity_id));
    for row in &valid {
        if !memory_on || !authorization::validate(&mut tx, &row.snapshot()).await? {
            revoked.push(row.id);
        }
    }
    if !revoked.is_empty() {
        // 方案可能混用了后来撤销的发言，整份丢弃；其余未处理的完整 turn 重新提炼。
        mark(s, &mut tx, stream, &revoked).await?;
        continue_pending(s, &mut tx, stream).await?;
        tx.commit().await?;
        return Ok(());
    }
    let ids: Vec<i64> = valid.iter().map(|r| r.id).collect();
    let sql = format!(
        "SELECT id FROM qq_turns WHERE id=ANY($1) AND {} IS NULL ORDER BY id FOR UPDATE",
        stream.column()
    );
    let pending: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(&ids)
        .fetch_all(&mut *tx)
        .await?;
    if pending.len() != ids.len() {
        continue_pending(s, &mut tx, stream).await?;
        tx.commit().await?;
        return Ok(());
    }
    for plan in personal {
        ik_ai::memory_jobs::apply_external(&mut tx, &s.ai, plan).await?;
    }
    if let Some(plan) = shared {
        ik_ai::shared_memory::apply_extract(&mut tx, &s.ai, plan).await?;
    }
    mark(s, &mut tx, stream, &ids).await?;
    continue_pending(s, &mut tx, stream).await?;
    tx.commit().await?;
    if let Stream::Personal(user, ai) = stream {
        ik_ai::memory_jobs::refresh_external_embeddings(&s.ai, user, ai).await;
    }
    Ok(())
}

/// 返回 true 表示调用方必须丢弃整批已读正文/尚未应用的方案。
async fn discard_revoked(s: &QqState, stream: Stream, rows: &[Candidate]) -> Result<bool, Problem> {
    let mut conn = s.db.acquire().await?;
    let on = ik_ai::memory_jobs::enabled_in(&mut conn).await?;
    let mut revoked = Vec::new();
    for row in rows {
        if !on || !authorization::validate(&mut conn, &row.snapshot()).await? {
            revoked.push(row.id);
        }
    }
    drop(conn);
    if revoked.is_empty() {
        return Ok(false);
    }
    let mut tx = s.db.begin().await?;
    mark(s, &mut tx, stream, &revoked).await?;
    continue_pending(s, &mut tx, stream).await?;
    tx.commit().await?;
    Ok(true)
}

/// 恢复待提炼项时保留各流的防抖截止时间；群至少等待十分钟。
pub async fn recover(s: &QqState) -> Result<(), Problem> {
    let catalog = s.ai.catalog.get().await?;
    let personal_delay = Duration::seconds(catalog.memory.extract_delay_secs);
    let group_delay = personal_delay.max(GROUP_DELAY);
    let personal:Vec<(i64,i64,DateTime<Utc>)>=sqlx::query_as("SELECT t.payer_user_id,t.ai_user_id,max(t.finished_at) FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id WHERE t.finished_at IS NOT NULL AND t.segments_sent>0 AND m.owner_user_id=t.payer_user_id AND m.binding_version IS NOT NULL AND t.ai_user_id IS NOT NULL AND t.personal_memory_processed_at IS NULL GROUP BY t.payer_user_id,t.ai_user_id ORDER BY max(t.finished_at) LIMIT 100")
        .fetch_all(&s.db).await?;
    let groups:Vec<(i64,i64,DateTime<Utc>)>=sqlx::query_as("SELECT t.group_id,t.ai_user_id,max(t.finished_at) FROM qq_turns t JOIN qq_messages m ON m.id=t.message_id WHERE t.finished_at IS NOT NULL AND t.segments_sent>0 AND m.owner_user_id=t.payer_user_id AND m.binding_version IS NOT NULL AND t.group_id IS NOT NULL AND t.ai_user_id IS NOT NULL AND t.group_memory_processed_at IS NULL GROUP BY t.group_id,t.ai_user_id ORDER BY max(t.finished_at) LIMIT 100")
        .fetch_all(&s.db).await?;
    let mut tx = s.db.begin().await?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut *tx)
        .await?;
    for (user, ai, last) in personal {
        let due = last + personal_delay;
        if due <= now {
            Stream::Personal(user, ai).schedule(s, &mut tx, due).await?;
        }
    }
    for (group, ai, last) in groups {
        let due = last + group_delay;
        if due <= now {
            Stream::Group(group, ai).schedule(s, &mut tx, due).await?;
        }
    }
    tx.commit().await?;
    Ok(())
}
