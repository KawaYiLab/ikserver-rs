//! 「大家都在问」：统计用户在 AI 会话里的开场提问，给敲敲展示每个角色的热门问题（AI-028）。
//!
//! 只记每段对话（含重置之后）的第一句；同一个人对同一个角色的同一个问题只算一次。
//! 疑似带个人信息的（链接、邮箱或 @、长串数字、多行）和太短太长的不记；展示前再过一遍屏蔽词，
//! 至少 [`MIN_ASKERS`] 个人问过才展示。记录保留 [`WINDOW_DAYS`] 天，撤回消息时一起删除。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use ik_kernel::Problem;
use ik_kernel::jobs::{IntoTaskHandlerResult, TaskHandler, WorkerContext};
use ik_kernel::site::{Screened, SiteValues};
use moka::future::Cache;
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};

use crate::AiState;

/// 统计最近多少天的提问；更早的由 [`PrunePopularQuestions`] 删除。
pub const WINDOW_DAYS: i64 = 30;
/// 至少多少个不同的人问过才展示。
pub const MIN_ASKERS: i64 = 3;
/// 每个角色最多展示几条。
pub const MAX_SHOWN: usize = 6;
/// 每个角色从数据库取的候选数（留出被屏蔽词过滤掉的余量）。
const CANDIDATES: i64 = 20;
/// 问题长度（字）。
const MIN_CHARS: usize = 4;
const MAX_CHARS: usize = 60;
/// 连续这么多位数字就当成手机号、QQ 号、UID 之类，不记。
const MAX_DIGIT_RUN: usize = 5;
const CACHE_TTL: Duration = Duration::from_secs(300);

/// 记下来的一条提问：归一化的键与展示用的原话。
#[derive(Debug, PartialEq, Eq)]
pub struct Question {
    pub key: String,
    pub text: String,
}

/// 能不能作为热门问题记下来；能的话给出归一化的键与整理过空白的原话。
pub fn question(raw: &str) -> Option<Question> {
    let raw = raw.trim();
    if raw.contains(['\n', '\r', '@']) || ik_kernel::site::has_link(raw) || raw.contains(":ik-") {
        return None;
    }
    let text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars = text.chars().count();
    if !(MIN_CHARS..=MAX_CHARS).contains(&chars) || longest_digit_run(&text) >= MAX_DIGIT_RUN {
        return None;
    }
    let key: String = text
        .trim_end_matches(|c: char| "?？!！。.~～，,、…".contains(c))
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if key.chars().count() < MIN_CHARS {
        return None;
    }
    Some(Question { key, text })
}

fn longest_digit_run(text: &str) -> usize {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        // 全角数字也算。
        if c.is_ascii_digit() || ('０'..='９').contains(&c) {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    longest
}

/// 用户在 AI 会话里发出一条消息时调用（与消息同一事务）。不是这段对话的第一句、
/// 带图或不像问题的不记。
pub(crate) async fn record(
    conn: &mut PgConnection,
    ai_user_id: i64,
    context_from: Option<DateTime<Utc>>,
    message: &ik_messaging::SentMessage,
) -> Result<(), Problem> {
    if message.image_count > 0 {
        return Ok(());
    }
    let Some(question) = question(&message.content) else {
        return Ok(());
    };
    let earlier = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM messages
               WHERE conversation_id = $1 AND sender_id = $2 AND kind = 'text' AND id <> $3
                 AND ($4::timestamptz IS NULL OR created_at >= $4)
           ) AS "earlier!""#,
        message.conversation_id,
        message.sender_id,
        message.id,
        context_from,
    )
    .fetch_one(&mut *conn)
    .await?;
    if earlier {
        return Ok(());
    }
    sqlx::query!(
        "INSERT INTO ai_question_asks (ai_user_id, question_key, user_id, text, message_id, asked_at)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (ai_user_id, question_key, user_id) DO UPDATE
         SET text = EXCLUDED.text, message_id = EXCLUDED.message_id, asked_at = EXCLUDED.asked_at",
        ai_user_id,
        question.key,
        message.sender_id,
        question.text,
        message.id,
        message.created_at,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 撤回消息时删掉它带来的记录。
pub(crate) async fn forget(conn: &mut PgConnection, message_id: i64) -> Result<(), Problem> {
    sqlx::query!(
        "DELETE FROM ai_question_asks WHERE message_id = $1",
        message_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// 各角色（按 AI 账号）的热门问题候选，问的人多的在前。
async fn load(db: &PgPool, now: DateTime<Utc>) -> Result<HashMap<i64, Vec<String>>, Problem> {
    let since = now - chrono::Duration::days(WINDOW_DAYS);
    let rows = sqlx::query!(
        r#"SELECT ai_user_id AS "ai_user_id!", text AS "text!" FROM (
               SELECT ai_user_id,
                      mode() WITHIN GROUP (ORDER BY text) AS text,
                      row_number() OVER (
                          PARTITION BY ai_user_id ORDER BY count(*) DESC, max(asked_at) DESC
                      ) AS rank
               FROM ai_question_asks
               WHERE asked_at >= $1
               GROUP BY ai_user_id, question_key
               HAVING count(*) >= $2
           ) ranked
           WHERE rank <= $3
           ORDER BY ai_user_id, rank"#,
        since,
        MIN_ASKERS,
        CANDIDATES,
    )
    .fetch_all(db)
    .await?;
    let mut out: HashMap<i64, Vec<String>> = HashMap::new();
    for row in rows {
        out.entry(row.ai_user_id).or_default().push(row.text);
    }
    Ok(out)
}

/// 热门问题的进程内缓存（5 分钟）：角色列表匿名可读，不每次都去聚合。
#[derive(Clone)]
pub struct PopularCache {
    db: PgPool,
    cache: Cache<(), Arc<HashMap<i64, Vec<String>>>>,
}

impl PopularCache {
    pub fn new(db: PgPool) -> Self {
        Self {
            db,
            cache: Cache::builder()
                .max_capacity(1)
                .time_to_live(CACHE_TTL)
                .build(),
        }
    }

    pub async fn get(&self) -> Result<Arc<HashMap<i64, Vec<String>>>, Problem> {
        let db = self.db.clone();
        self.cache
            .try_get_with((), async move { load(&db, Utc::now()).await.map(Arc::new) })
            .await
            .map_err(|e| (*e).clone())
    }

    pub async fn invalidate(&self) {
        self.cache.invalidate(&()).await;
    }
}

/// 过一遍屏蔽词（命中拦截词或替换词的都不展示），去掉和角色卡示例问题重复的，最多 [`MAX_SHOWN`] 条。
pub fn shown(candidates: &[String], site: Option<&SiteValues>, preset: &[String]) -> Vec<String> {
    let preset: Vec<String> = preset
        .iter()
        .filter_map(|q| question(q).map(|q| q.key))
        .collect();
    candidates
        .iter()
        .filter(|text| {
            site.is_none_or(
                |site| matches!(site.screen(text), Screened::Clean(ref c) if c == *text),
            )
        })
        .filter(|text| question(text).is_none_or(|q| !preset.contains(&q.key)))
        .take(MAX_SHOWN)
        .cloned()
        .collect()
}

/// 删除统计窗口之外的提问记录。由执行器每天安排一次；失败不重试，第二天再来。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrunePopularQuestions {}

impl TaskHandler for PrunePopularQuestions {
    const IDENTIFIER: &'static str = "ai.prune_popular_questions";

    async fn run(self, ctx: WorkerContext) -> impl IntoTaskHandlerResult {
        let Some(state) = ctx.get_ext::<AiState>() else {
            return Err("执行器没有注册 AiState".to_owned());
        };
        prune(&state.db, Utc::now())
            .await
            .map(|_| ())
            .map_err(|p| format!("{p:?}"))
    }
}

/// 删除 `now` 往前 [`WINDOW_DAYS`] 天之外的记录，返回删了几行。
pub async fn prune(db: &PgPool, now: DateTime<Utc>) -> Result<u64, Problem> {
    let since = now - chrono::Duration::days(WINDOW_DAYS);
    let done = sqlx::query!("DELETE FROM ai_question_asks WHERE asked_at < $1", since)
        .execute(db)
        .await?;
    Ok(done.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(raw: &str) -> Option<String> {
        question(raw).map(|q| q.key)
    }

    #[test]
    fn variants_of_one_question_share_a_key() {
        assert_eq!(key("最近有什么热帖？"), key(" 最近有 什么热帖 ?"));
        assert_eq!(key("What is ZZZ?"), key("what is zzz"));
        assert_eq!(
            question("  最近有  什么热帖？ ").map(|q| q.text).as_deref(),
            Some("最近有 什么热帖？")
        );
    }

    #[test]
    fn personal_looking_messages_are_not_recorded() {
        for raw in [
            "我的 QQ 是 12345678 帮我看看",
            "我的手机号１３８１２３４５６７８",
            "联系 me@example.com 可以吗",
            "@某人 你好呀你好",
            "看看 https://example.com 这个",
            "第一行\n第二行内容",
            ":ik-smile: 你好你好",
            "你好",
            "？？？？？",
            &"字".repeat(61),
        ] {
            assert_eq!(question(raw), None, "{raw}");
        }
        assert!(question("2.5 版本什么时候更新").is_some());
    }

    #[test]
    fn shown_skips_screened_and_preset_questions() {
        let mut site = SiteValues::from_config(&ik_kernel::config::Config::default());
        site.filter.words = vec![
            ik_kernel::site::WatchedWord {
                word: "坏词".into(),
                action: ik_kernel::site::WordAction::Censor,
            },
            ik_kernel::site::WatchedWord {
                word: "禁词".into(),
                action: ik_kernel::site::WordAction::Block,
            },
        ];
        let candidates: Vec<String> = [
            "带坏词的问题",
            "带禁词的问题",
            "最近有什么热帖",
            "新角色是谁",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            shown(&candidates, Some(&site), &["最近有什么热帖？".into()]),
            ["新角色是谁"]
        );
        assert_eq!(shown(&candidates, None, &[]).len(), 4);
    }
}
