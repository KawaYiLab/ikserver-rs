//! 试卷：题目归一化、抽题洗牌、判分。纯函数（随机源由调用方传入）。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Single,
    Multiple,
    Boolean,
}

impl QuestionKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Multiple => "multiple",
            Self::Boolean => "boolean",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "single" => Some(Self::Single),
            "multiple" => Some(Self::Multiple),
            "boolean" => Some(Self::Boolean),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExamOption {
    pub key: String,
    pub text: String,
}

/// 快照里的一道题（含正确答案，只存在服务端）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotQuestion {
    pub id: i64,
    pub question: String,
    pub kind: QuestionKind,
    pub options: Vec<ExamOption>,
    pub correct_keys: Vec<String>,
    pub weight: i32,
}

/// 选项归一化（EXAM-005）：至少 2 项，key/text trim 后非空，key 不重复。不合格返回 None。
pub fn normalize_options(raw: &[ExamOption]) -> Option<Vec<ExamOption>> {
    if raw.len() < 2 {
        return None;
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for option in raw {
        let key = option.key.trim();
        let text = option.text.trim();
        if key.is_empty() || text.is_empty() || !seen.insert(key.to_owned()) {
            return None;
        }
        out.push(ExamOption {
            key: key.to_owned(),
            text: text.to_owned(),
        });
    }
    Some(out)
}

/// 正确答案归一化（EXAM-006）：trim、去重、排序，非空且都是选项里的 key。
pub fn normalize_correct_keys(raw: &[String], options: &[ExamOption]) -> Option<Vec<String>> {
    let keys: BTreeSet<String> = raw.iter().map(|k| k.trim().to_owned()).collect();
    if keys.is_empty()
        || keys
            .iter()
            .any(|k| !options.iter().any(|option| &option.key == k))
    {
        return None;
    }
    Some(keys.into_iter().collect())
}

/// 题型与选项数、答案数是否匹配（EXAMADM-004）。返回中文错误。
pub fn check_kind_shape(
    kind: QuestionKind,
    options: &[ExamOption],
    correct: &[String],
) -> Result<(), &'static str> {
    match kind {
        QuestionKind::Single if options.len() < 2 => Err("单选题至少需要 2 个选项"),
        QuestionKind::Multiple if options.len() < 3 => Err("多选题至少需要 3 个选项"),
        QuestionKind::Boolean if options.len() != 2 => Err("判断题需要 2 个选项"),
        QuestionKind::Multiple if correct.len() < 2 => Err("多选题正确答案至少需要 2 项"),
        QuestionKind::Single | QuestionKind::Boolean if correct.len() != 1 => {
            Err("单选/判断题只能有 1 个正确答案")
        }
        _ => Ok(()),
    }
}

/// Fisher–Yates 洗牌。`rand_below(n)` 返回 `[0, n)` 内的均匀随机数。
pub fn shuffle<T>(items: &mut [T], rand_below: &mut impl FnMut(usize) -> usize) {
    for i in (1..items.len()).rev() {
        let j = rand_below(i + 1).min(i);
        items.swap(i, j);
    }
}

/// 考生的作答：题目 id → 选中的 key（已去重排序）。
pub type Answers = BTreeMap<i64, BTreeSet<String>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grade {
    pub score: i32,
    pub total_score: i32,
    pub score_percent: i16,
    pub correct_count: i32,
    pub question_count: i32,
    pub passed: bool,
}

/// 某道题是否答对：选中的 key 集合与正确答案完全一致；正确答案为空的题永远算错（EXAM-026）。
pub fn is_correct(question: &SnapshotQuestion, answer: Option<&BTreeSet<String>>) -> bool {
    match answer {
        Some(chosen) if !question.correct_keys.is_empty() => {
            chosen.len() == question.correct_keys.len()
                && question.correct_keys.iter().all(|k| chosen.contains(k))
        }
        _ => false,
    }
}

/// 判分（EXAM-026）：答对得该题权重分，多选不给部分分；百分比向下取整；达到及格线即通过。
pub fn grade(questions: &[SnapshotQuestion], answers: &Answers, pass_percent: u32) -> Grade {
    let mut score: i64 = 0;
    let mut total: i64 = 0;
    let mut correct = 0i32;
    for question in questions {
        let weight = i64::from(question.weight.max(1));
        total += weight;
        if is_correct(question, answers.get(&question.id)) {
            score += weight;
            correct += 1;
        }
    }
    let percent = if total > 0 { score * 100 / total } else { 0 };
    Grade {
        score: i32::try_from(score).unwrap_or(i32::MAX),
        total_score: i32::try_from(total).unwrap_or(i32::MAX),
        score_percent: i16::try_from(percent).unwrap_or(0),
        correct_count: correct,
        question_count: i32::try_from(questions.len()).unwrap_or(i32::MAX),
        passed: percent >= i64::from(pass_percent),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn opts(keys: &[&str]) -> Vec<ExamOption> {
        keys.iter()
            .map(|k| ExamOption {
                key: (*k).to_owned(),
                text: format!("选项{k}"),
            })
            .collect()
    }

    fn q(id: i64, kind: QuestionKind, correct: &[&str], weight: i32) -> SnapshotQuestion {
        SnapshotQuestion {
            id,
            question: format!("题{id}"),
            kind,
            options: opts(&["A", "B", "C", "D"]),
            correct_keys: correct.iter().map(|k| (*k).to_owned()).collect(),
            weight,
        }
    }

    fn ans(pairs: &[(i64, &[&str])]) -> Answers {
        pairs
            .iter()
            .map(|(id, keys)| (*id, keys.iter().map(|k| (*k).to_owned()).collect()))
            .collect()
    }

    #[test]
    fn options_are_trimmed_and_unique() {
        let raw = vec![
            ExamOption {
                key: " A ".into(),
                text: " 是 ".into(),
            },
            ExamOption {
                key: "B".into(),
                text: "否".into(),
            },
        ];
        let norm = normalize_options(&raw).unwrap();
        assert_eq!(norm[0].key, "A");
        assert_eq!(norm[0].text, "是");
        assert!(normalize_options(&raw[..1]).is_none());
        let dup = vec![raw[1].clone(), raw[1].clone()];
        assert!(normalize_options(&dup).is_none());
    }

    #[test]
    fn correct_keys_must_be_subset() {
        let options = opts(&["A", "B", "C"]);
        assert_eq!(
            normalize_correct_keys(&["C".into(), " A".into(), "C".into()], &options),
            Some(vec!["A".to_owned(), "C".to_owned()])
        );
        assert!(normalize_correct_keys(&["Z".into()], &options).is_none());
        assert!(normalize_correct_keys(&[], &options).is_none());
    }

    #[test]
    fn kind_shape_rules() {
        let two = opts(&["A", "B"]);
        let three = opts(&["A", "B", "C"]);
        let one_key = vec!["A".to_owned()];
        let two_keys = vec!["A".to_owned(), "B".to_owned()];
        assert!(check_kind_shape(QuestionKind::Single, &two, &one_key).is_ok());
        assert!(check_kind_shape(QuestionKind::Single, &two, &two_keys).is_err());
        assert!(check_kind_shape(QuestionKind::Multiple, &two, &two_keys).is_err());
        assert!(check_kind_shape(QuestionKind::Multiple, &three, &one_key).is_err());
        assert!(check_kind_shape(QuestionKind::Multiple, &three, &two_keys).is_ok());
        assert!(check_kind_shape(QuestionKind::Boolean, &three, &one_key).is_err());
        assert!(check_kind_shape(QuestionKind::Boolean, &two, &one_key).is_ok());
    }

    #[test]
    fn grading_is_all_or_nothing_and_weighted() {
        let questions = vec![
            q(1, QuestionKind::Single, &["A"], 1),
            q(2, QuestionKind::Multiple, &["A", "C"], 2),
            q(3, QuestionKind::Single, &["B"], 2),
        ];
        // 第 2 题少选一个不给分。
        let g = grade(
            &questions,
            &ans(&[(1, &["A"]), (2, &["A"]), (3, &["B"])]),
            60,
        );
        assert_eq!((g.score, g.total_score, g.correct_count), (3, 5, 2));
        assert_eq!(g.score_percent, 60);
        assert!(g.passed, "及格线含边界");
        let g = grade(&questions, &ans(&[(1, &["A"]), (2, &["A", "C"])]), 60);
        assert_eq!(g.score_percent, 60);
        let g = grade(&questions, &ans(&[(2, &["A", "C"]), (3, &["C"])]), 60);
        assert_eq!((g.score, g.score_percent, g.passed), (2, 40, false));
        // 多选多选了也不对。
        let g = grade(&questions, &ans(&[(2, &["A", "B", "C"])]), 60);
        assert_eq!(g.score, 0);
    }

    #[test]
    fn empty_paper_scores_zero() {
        let g = grade(&[], &Answers::new(), 60);
        assert_eq!((g.score_percent, g.passed), (0, false));
    }

    #[test]
    fn percent_rounds_down() {
        let questions = vec![
            q(1, QuestionKind::Single, &["A"], 1),
            q(2, QuestionKind::Single, &["A"], 1),
            q(3, QuestionKind::Single, &["A"], 1),
        ];
        let g = grade(&questions, &ans(&[(1, &["A"]), (2, &["A"])]), 67);
        assert_eq!(g.score_percent, 66);
        assert!(!g.passed);
    }

    proptest! {
        #[test]
        fn shuffle_is_a_permutation(len in 0usize..60, seed in any::<u64>()) {
            let mut state = seed | 1;
            let mut rng = |n: usize| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                usize::try_from(state % (n as u64)).unwrap_or(0)
            };
            let mut items: Vec<usize> = (0..len).collect();
            shuffle(&mut items, &mut rng);
            let mut sorted = items.clone();
            sorted.sort_unstable();
            prop_assert_eq!(sorted, (0..len).collect::<Vec<_>>());
        }

        #[test]
        fn grade_is_bounded(answer_all in any::<bool>(), weights in proptest::collection::vec(1i32..5, 0..20)) {
            let questions: Vec<SnapshotQuestion> = weights
                .iter()
                .enumerate()
                .map(|(i, w)| q(i64::try_from(i).unwrap(), QuestionKind::Single, &["A"], *w))
                .collect();
            let answers: Answers = if answer_all {
                questions.iter().map(|q| (q.id, ["A".to_owned()].into())).collect()
            } else {
                Answers::new()
            };
            let g = grade(&questions, &answers, 60);
            prop_assert!(g.score <= g.total_score);
            prop_assert!((0..=100).contains(&g.score_percent));
            prop_assert_eq!(g.passed, answer_all && !questions.is_empty());
        }
    }
}
