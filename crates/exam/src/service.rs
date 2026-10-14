//! 入站考试业务逻辑。业务规则：`docs/business-rules/moderation-exam.md` 第 3 节。
//!
//! 开考与交卷都先锁用户行（auth interop），同一用户的考试操作因此串行：
//! 并发开考只会产生一场，并发交卷只会判一次分、发一次奖励。

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Duration, Utc};
use http::StatusCode;
use ik_auth::Principal;
use ik_auth::interop;
use ik_auth::service::RequestMeta;
use ik_economy::service::{Dedup, DennyAction, ExpAction};
use ik_kernel::{FieldError, Id, Problem};
use sqlx::PgConnection;

use crate::ExamState;
use crate::dto::{
    ActiveAttemptView, ExamConfigView, ExamStatusView, PaperQuestion, QuestionInput, ReviewQuery,
    ReviewQuestion, ReviewView, RewardView, StartView, SubmitRequest, SubmitResult,
};
use crate::paper::{self, Answers, ExamOption, QuestionKind, SnapshotQuestion};
use crate::repo::{self, AttemptRow, Finish, NewAttempt, NewQuestion};

const MAX_ANSWER_KEY_LEN: usize = 16;

// ---------- 错误 ----------

fn already_passed() -> Problem {
    Problem::conflict("EXAM_ALREADY_PASSED", "你已通过入站考试")
}

fn cooldown(secs: i64) -> Problem {
    Problem::new(
        StatusCode::TOO_MANY_REQUESTS,
        "EXAM_COOLDOWN",
        format!("考试失败次数过多，请 {} 分钟后再试", (secs + 59) / 60),
    )
    .with_retry_after(u64::try_from(secs).unwrap_or(1))
}

fn not_available() -> Problem {
    Problem::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "EXAM_NOT_AVAILABLE",
        "题库暂未配置，请稍后再试",
    )
}

fn attempt_not_found() -> Problem {
    Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "考试场次不存在")
}

fn already_submitted() -> Problem {
    Problem::conflict("EXAM_ATTEMPT_ALREADY_SUBMITTED", "本场考试已提交")
}

fn expired() -> Problem {
    Problem::conflict("EXAM_ATTEMPT_EXPIRED", "考试已超时，请重新开始")
}

// ---------- 公共 ----------

fn config_view(state: &ExamState) -> ExamConfigView {
    let s = &state.settings;
    ExamConfigView {
        question_count: s.question_count,
        pass_score_percent: s.pass_score_percent,
        time_limit_secs: s.time_limit_secs,
        max_fails_before_cooldown: s.max_fails_before_cooldown,
        fail_cooldown_secs: s.fail_cooldown_secs,
        reward_denny: s.reward_denny,
        reward_exp: s.reward_exp,
    }
}

/// 冷却剩余秒数（EXAM-030）：最近 N 次全部未通过时，从最近一次失败起冷却；向上取整。
/// `results` 为新的在前的 (是否通过, 时间)。
pub fn cooldown_remaining(
    results: &[(bool, DateTime<Utc>)],
    max_fails: u32,
    cooldown_secs: i64,
    now: DateTime<Utc>,
) -> i64 {
    let n = usize::try_from(max_fails).unwrap_or(usize::MAX);
    if n == 0 || results.len() < n || results.iter().take(n).any(|(passed, _)| *passed) {
        return 0;
    }
    let Some((_, latest)) = results.first() else {
        return 0;
    };
    let until = *latest + Duration::seconds(cooldown_secs);
    let millis = (until - now).num_milliseconds();
    if millis <= 0 {
        0
    } else {
        (millis + 999) / 1000
    }
}

async fn cooldown_for(
    state: &ExamState,
    conn: &mut PgConnection,
    user_id: i64,
    now: DateTime<Utc>,
) -> Result<i64, Problem> {
    let limit = i64::from(state.settings.max_fails_before_cooldown);
    let results: Vec<(bool, DateTime<Utc>)> = repo::recent_results(conn, user_id, now, limit)
        .await?
        .into_iter()
        .map(|r| (r.passed, r.at))
        .collect();
    Ok(cooldown_remaining(
        &results,
        state.settings.max_fails_before_cooldown,
        state.settings.fail_cooldown_secs,
        now,
    ))
}

fn snapshot_of(row: &AttemptRow) -> Result<Vec<SnapshotQuestion>, Problem> {
    serde_json::from_value(row.questions.clone()).map_err(|e| {
        tracing::error!(attempt_id = row.id, error = %e, "考试快照无法解析");
        Problem::internal()
    })
}

fn paper_of(snapshot: &[SnapshotQuestion]) -> Vec<PaperQuestion> {
    snapshot
        .iter()
        .map(|q| PaperQuestion {
            id: Id::new(q.id),
            question: q.question.clone(),
            kind: q.kind,
            options: q.options.clone(),
            weight: q.weight,
        })
        .collect()
}

/// 密码学随机源上的 `[0, n)` 均匀整数（拒绝采样避免取模偏差）。
struct Rng {
    failed: bool,
}

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        let Ok(n64) = u64::try_from(n) else {
            return 0;
        };
        if n64 <= 1 {
            return 0;
        }
        let zone = u64::MAX - (u64::MAX % n64);
        loop {
            let mut buf = [0u8; 8];
            if getrandom::fill(&mut buf).is_err() {
                self.failed = true;
                return 0;
            }
            let value = u64::from_le_bytes(buf);
            if value < zone {
                return usize::try_from(value % n64).unwrap_or(0);
            }
        }
    }
}

/// 抽题（EXAM-022）：只用能通过归一化的启用题，洗牌后取前 N 题，每题选项再各自洗牌。
async fn draw(
    state: &ExamState,
    conn: &mut PgConnection,
) -> Result<Vec<SnapshotQuestion>, Problem> {
    let mut pool: Vec<SnapshotQuestion> = repo::enabled_questions(conn)
        .await?
        .into_iter()
        .filter_map(|row| {
            let kind = QuestionKind::parse(&row.kind)?;
            let raw: Vec<ExamOption> = serde_json::from_value(row.options).ok()?;
            let options = paper::normalize_options(&raw)?;
            let correct_keys = paper::normalize_correct_keys(&row.correct_keys, &options)?;
            Some(SnapshotQuestion {
                id: row.id,
                question: row.question,
                kind,
                options,
                correct_keys,
                weight: row.weight.max(1),
            })
        })
        .collect();
    if pool.is_empty() {
        return Err(not_available());
    }
    let mut rng = Rng { failed: false };
    paper::shuffle(&mut pool, &mut |n| rng.below(n));
    pool.truncate(usize::try_from(state.settings.question_count).unwrap_or(usize::MAX));
    for question in &mut pool {
        paper::shuffle(&mut question.options, &mut |n| rng.below(n));
    }
    if rng.failed {
        tracing::error!("系统随机源不可用");
        return Err(Problem::internal());
    }
    Ok(pool)
}

// ---------- 状态 ----------

pub async fn status(state: &ExamState, principal: &Principal) -> Result<ExamStatusView, Problem> {
    let user_id = principal.user_id.get();
    let now = Utc::now();
    let mut conn = state.db.acquire().await?;
    let passed_at = interop::exam_passed_at(&mut conn, user_id)
        .await?
        .ok_or_else(Problem::unauthorized)?;
    if passed_at.is_some() {
        return Ok(ExamStatusView {
            passed: true,
            passed_at,
            cooldown_remaining_secs: 0,
            active_attempt: None,
            config: config_view(state),
        });
    }
    let cooldown_remaining_secs = cooldown_for(state, &mut conn, user_id, now).await?;
    let active_attempt = match repo::open_attempt(&mut conn, user_id, now).await? {
        Some(row) => Some(ActiveAttemptView {
            id: Id::new(row.id),
            started_at: row.started_at,
            expires_at: row.expires_at,
            question_count: snapshot_of(&row)?.len(),
        }),
        None => None,
    };
    Ok(ExamStatusView {
        passed: false,
        passed_at: None,
        cooldown_remaining_secs,
        active_attempt,
        config: config_view(state),
    })
}

// ---------- 开考 ----------

/// 开考（EXAM-021）。顺序：账号可用 → 未通过 → 不在冷却中 → 有进行中的场次就原样恢复 → 抽题新建。
pub async fn start(
    state: &ExamState,
    principal: &Principal,
    meta: &RequestMeta,
) -> Result<StartView, Problem> {
    let user_id = principal.user_id.get();
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let user = interop::lock_active_user(&mut tx, user_id)
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
    if user.exam_passed_at.is_some() {
        return Err(already_passed());
    }
    repo::close_expired(&mut tx, user_id, now).await?;
    let remaining = cooldown_for(state, &mut tx, user_id, now).await?;
    if remaining > 0 {
        // 收尾过期场次的写入照常提交，下次查询不必重复计算。
        tx.commit().await?;
        return Err(cooldown(remaining));
    }

    if let Some(open) = repo::open_attempt(&mut tx, user_id, now).await? {
        let snapshot = snapshot_of(&open)?;
        tx.commit().await?;
        return Ok(StartView {
            attempt_id: Id::new(open.id),
            resumed: true,
            started_at: open.started_at,
            expires_at: open.expires_at,
            questions: paper_of(&snapshot),
            config: config_view(state),
        });
    }

    let snapshot = draw(state, &mut tx).await?;
    let id = state.ids.next_raw();
    let expires_at = now + Duration::seconds(state.settings.time_limit_secs);
    repo::insert_attempt(
        &mut tx,
        &NewAttempt {
            id,
            user_id,
            questions: serde_json::to_value(&snapshot).map_err(|e| Problem::internal_from(&e))?,
            started_at: now,
            expires_at,
            request_ip: &meta.ip.to_string(),
        },
    )
    .await?;
    tx.commit().await?;
    Ok(StartView {
        attempt_id: Id::new(id),
        resumed: false,
        started_at: now,
        expires_at,
        questions: paper_of(&snapshot),
        config: config_view(state),
    })
}

// ---------- 交卷 ----------

/// 作答只接受快照里的题目与选项（D-060 方案 A，同时限制了写入大小）。
fn parse_answers(
    snapshot: &[SnapshotQuestion],
    raw: &std::collections::BTreeMap<String, Vec<String>>,
) -> Result<Answers, Problem> {
    let by_id: HashMap<i64, &SnapshotQuestion> = snapshot.iter().map(|q| (q.id, q)).collect();
    let invalid = |detail: String| {
        Problem::validation(vec![FieldError {
            field: "answers".to_owned(),
            code: "INVALID".to_owned(),
            detail,
        }])
    };
    let mut answers = Answers::new();
    for (key, keys) in raw {
        let question = key
            .parse::<i64>()
            .ok()
            .and_then(|id| by_id.get(&id))
            .ok_or_else(|| invalid(format!("题目 {key} 不在本场考试中")))?;
        if keys.len() > question.options.len() {
            return Err(invalid(format!("题目 {key} 的选项过多")));
        }
        let mut chosen = BTreeSet::new();
        for k in keys {
            let k = k.trim();
            if k.len() > MAX_ANSWER_KEY_LEN || !question.options.iter().any(|o| o.key == k) {
                return Err(invalid(format!("题目 {key} 没有选项 {k}")));
            }
            chosen.insert(k.to_owned());
        }
        if !chosen.is_empty() {
            answers.insert(question.id, chosen);
        }
    }
    Ok(answers)
}

fn answers_json(answers: &Answers) -> serde_json::Value {
    serde_json::Value::Object(
        answers
            .iter()
            .map(|(id, keys)| {
                (
                    id.to_string(),
                    serde_json::Value::Array(
                        keys.iter()
                            .map(|k| serde_json::Value::String(k.clone()))
                            .collect(),
                    ),
                )
            })
            .collect(),
    )
}

/// 交卷（EXAM-024–027、040）。超时交卷照样记一次失败（先提交再返回 409）。
/// 首次通过时在同一事务内：写通过时间 → 发丁尼与经验（考试经验不占每日额度）→ 重算等级（至少 1 级）。
/// 奖励因为之前发过而跳过时视为成功（D-064 方案 A）。
pub async fn submit(
    state: &ExamState,
    principal: &Principal,
    attempt_id: i64,
    request: SubmitRequest,
) -> Result<SubmitResult, Problem> {
    let user_id = principal.user_id.get();
    let now = Utc::now();
    let mut tx = state.db.begin().await?;
    let user = interop::lock_active_user(&mut tx, user_id)
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
    let attempt = repo::lock_attempt(&mut tx, attempt_id, user_id)
        .await?
        .ok_or_else(attempt_not_found)?;
    if attempt.submitted_at.is_some() {
        return Err(already_submitted());
    }
    let snapshot = snapshot_of(&attempt)?;
    let answers = parse_answers(&snapshot, &request.answers)?;
    let pass_percent = state.settings.pass_score_percent;

    if attempt.expires_at <= now {
        let total = paper::grade(&snapshot, &Answers::new(), pass_percent).total_score;
        repo::finish_attempt(
            &mut tx,
            attempt.id,
            &Finish {
                answers: answers_json(&answers),
                score: 0,
                total_score: total,
                score_percent: 0,
                passed: false,
                submitted_at: now,
            },
        )
        .await?;
        tx.commit().await?;
        return Err(expired());
    }

    let grade = paper::grade(&snapshot, &answers, pass_percent);
    repo::finish_attempt(
        &mut tx,
        attempt.id,
        &Finish {
            answers: answers_json(&answers),
            score: grade.score,
            total_score: grade.total_score,
            score_percent: grade.score_percent,
            passed: grade.passed,
            submitted_at: now,
        },
    )
    .await?;

    let mut reward = None;
    if grade.passed && user.exam_passed_at.is_none() {
        interop::mark_exam_passed(&mut tx, user_id, now).await?;
        let reward_key = format!("user:{user_id}");
        let denny = ik_economy::service::add_denny(
            &mut tx,
            &state.ids,
            user_id,
            DennyAction::ExamPassed,
            state.settings.reward_denny,
            Some(&reward_key),
            "通过入站考试奖励",
            now,
        )
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
        let exp = ik_economy::service::grant_exp(
            &mut tx,
            &state.ids,
            user_id,
            ExpAction::ExamPassed,
            state.settings.reward_exp,
            Dedup::Ref(reward_key),
            now,
        )
        .await?
        .ok_or_else(ik_auth::service::account_blocked)?;
        ik_economy::service::refresh_level(&mut tx, user_id).await?;
        reward = Some(RewardView {
            denny: denny.delta,
            exp: exp.added,
        });
    }
    let cooldown_remaining_secs = if grade.passed {
        0
    } else {
        cooldown_for(state, &mut tx, user_id, now).await?
    };
    tx.commit().await?;

    Ok(SubmitResult {
        passed: grade.passed,
        score: grade.score,
        total_score: grade.total_score,
        score_percent: grade.score_percent,
        correct_count: grade.correct_count,
        question_count: grade.question_count,
        pass_score_percent: pass_percent,
        cooldown_remaining_secs,
        reward,
    })
}

// ---------- 回顾 ----------

/// 回顾已交卷的场次（EXAM-028/029）：按快照重新判分；不下发正确答案；解析读题库当前内容。
pub async fn review(
    state: &ExamState,
    principal: &Principal,
    query: ReviewQuery,
) -> Result<ReviewView, Problem> {
    let user_id = principal.user_id.get();
    let mut conn = state.db.acquire().await?;
    let attempt = repo::submitted_attempt(&mut conn, user_id, query.attempt_id.map(Id::get))
        .await?
        .ok_or_else(|| {
            Problem::new(StatusCode::NOT_FOUND, "NOT_FOUND", "未找到已提交的考试记录")
        })?;
    let submitted_at = attempt.submitted_at.ok_or_else(Problem::internal)?;
    let snapshot = snapshot_of(&attempt)?;
    let answers: Answers = attempt
        .answers
        .clone()
        .and_then(|v| {
            serde_json::from_value::<std::collections::BTreeMap<String, Vec<String>>>(v).ok()
        })
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(k, v)| Some((k.parse().ok()?, v.into_iter().collect())))
        .collect();
    let ids: Vec<i64> = snapshot.iter().map(|q| q.id).collect();
    let explanations: HashMap<i64, Option<String>> = repo::explanations(&mut conn, &ids)
        .await?
        .into_iter()
        .map(|r| (r.id, r.explanation))
        .collect();
    let grade = paper::grade(&snapshot, &answers, state.settings.pass_score_percent);
    let questions = snapshot
        .iter()
        .map(|q| {
            let chosen = answers.get(&q.id);
            let is_correct = paper::is_correct(q, chosen);
            ReviewQuestion {
                id: Id::new(q.id),
                question: q.question.clone(),
                kind: q.kind,
                options: q.options.clone(),
                user_answer: chosen
                    .map(|c| c.iter().cloned().collect())
                    .unwrap_or_default(),
                is_correct,
                weight: q.weight,
                score: if is_correct { q.weight } else { 0 },
                explanation: explanations.get(&q.id).cloned().flatten(),
            }
        })
        .collect();
    // 总分以交卷时存下的为准（超时交卷记 0 分、未通过），逐题对错按快照重新判。
    Ok(ReviewView {
        attempt_id: Id::new(attempt.id),
        passed: attempt.passed.unwrap_or(false),
        score: attempt.score.unwrap_or(0),
        total_score: attempt.total_score.unwrap_or(grade.total_score),
        score_percent: attempt.score_percent.unwrap_or(0),
        correct_count: grade.correct_count,
        question_count: grade.question_count,
        submitted_at,
        questions,
    })
}

// ---------- 题库 ----------

/// 新建题目（EXAMADM-004/005）。返回题目 id。
pub async fn create_question(state: &ExamState, input: QuestionInput) -> Result<i64, Problem> {
    let invalid = |field: &str, detail: &str| Problem::field(field, "INVALID", detail);
    let question = input.question.trim();
    if question.is_empty() {
        return Err(invalid("question", "题干不能为空"));
    }
    if question.chars().count() > 2000 {
        return Err(invalid("question", "题干最多 2000 字"));
    }
    let options = paper::normalize_options(&input.options)
        .filter(|o| o.len() <= 26)
        .ok_or_else(|| invalid("options", "选项格式不正确，每项需要非空的 key 和 text"))?;
    let correct = paper::normalize_correct_keys(&input.correct_keys, &options)
        .ok_or_else(|| invalid("correctKeys", "正确答案必须是选项 key 的子集"))?;
    paper::check_kind_shape(input.kind, &options, &correct)
        .map_err(|detail| invalid("kind", detail))?;
    let weight = input.weight.filter(|w| *w >= 1).unwrap_or(1);
    let explanation = input
        .explanation
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let tag = input
        .tag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let id = state.ids.next_raw();
    let mut conn = state.db.acquire().await?;
    repo::insert_question(
        &mut conn,
        &NewQuestion {
            id,
            question,
            kind: input.kind.as_str(),
            options: serde_json::to_value(&options).map_err(|e| Problem::internal_from(&e))?,
            correct_keys: &correct,
            explanation,
            weight,
            enabled: input.enabled != Some(false),
            tag,
            now: Utc::now(),
        },
    )
    .await?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(min: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap() + Duration::minutes(min)
    }

    #[test]
    fn cooldown_needs_n_consecutive_failures() {
        let now = at(10);
        let fails = vec![(false, at(5)), (false, at(3)), (false, at(1))];
        assert_eq!(cooldown_remaining(&fails, 3, 1800, now), 25 * 60);
        assert_eq!(cooldown_remaining(&fails[..2], 3, 1800, now), 0);
        let mixed = vec![(false, at(5)), (true, at(3)), (false, at(1))];
        assert_eq!(cooldown_remaining(&mixed, 3, 1800, now), 0);
        assert_eq!(cooldown_remaining(&fails, 3, 1800, at(36)), 0);
    }

    #[test]
    fn cooldown_rounds_up() {
        let now = at(0) + Duration::milliseconds(1);
        let fails = vec![(false, at(0)); 3];
        assert_eq!(cooldown_remaining(&fails, 3, 60, now), 60);
    }

    fn snapshot() -> Vec<SnapshotQuestion> {
        vec![SnapshotQuestion {
            id: 7,
            question: "1+1?".into(),
            kind: QuestionKind::Single,
            options: vec![
                ExamOption {
                    key: "A".into(),
                    text: "2".into(),
                },
                ExamOption {
                    key: "B".into(),
                    text: "3".into(),
                },
            ],
            correct_keys: vec!["A".into()],
            weight: 1,
        }]
    }

    #[test]
    fn answers_must_reference_snapshot() {
        let snap = snapshot();
        let ok = parse_answers(&snap, &[("7".to_owned(), vec![" A".to_owned()])].into()).unwrap();
        assert_eq!(ok.get(&7).unwrap().iter().next().unwrap(), "A");
        for bad in [
            [("8".to_owned(), vec!["A".to_owned()])],
            [("7".to_owned(), vec!["C".to_owned()])],
            [("x".to_owned(), vec!["A".to_owned()])],
        ] {
            assert!(parse_answers(&snap, &bad.into()).is_err());
        }
        let too_many = [(
            "7".to_owned(),
            vec!["A".to_owned(), "B".to_owned(), "A".to_owned()],
        )];
        assert!(parse_answers(&snap, &too_many.into()).is_err());
        let empty = parse_answers(&snap, &[("7".to_owned(), Vec::new())].into()).unwrap();
        assert!(empty.is_empty());
    }
}
