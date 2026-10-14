-- 入站考试：题库与考试场次。业务规则见 docs/business-rules/moderation-exam.md 第 3–4 节，
-- 设计说明见 docs/schema.md。通过状态存在 users.exam_passed_at（auth 域），由 exam 通过 auth 的接口写入。

CREATE TABLE exam_questions (
    id            BIGINT      PRIMARY KEY,
    question      TEXT        NOT NULL,
    kind          TEXT        NOT NULL DEFAULT 'single',
    -- [{ "key": "A", "text": "..." }, ...]，key 非空且不重复（EXAM-005）。
    options       JSONB       NOT NULL,
    -- 已归一化（去重、排序），且都是选项里的 key（EXAM-006）。永不下发给考生。
    correct_keys  TEXT[]      NOT NULL,
    -- 回顾时展示的解析（EXAM-029 读当前题库的内容）。
    explanation   TEXT,
    weight        INTEGER     NOT NULL DEFAULT 1,
    enabled       BOOLEAN     NOT NULL DEFAULT TRUE,
    tag           TEXT,
    legacy_id     TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT exam_questions_legacy_id_uq UNIQUE (legacy_id),
    CONSTRAINT exam_questions_question_len CHECK (char_length(question) BETWEEN 1 AND 2000),
    CONSTRAINT exam_questions_kind_check CHECK (kind IN ('single', 'multiple', 'boolean')),
    CONSTRAINT exam_questions_options_shape CHECK (
        jsonb_typeof(options) = 'array' AND jsonb_array_length(options) BETWEEN 2 AND 26
    ),
    CONSTRAINT exam_questions_correct_nonempty CHECK (cardinality(correct_keys) >= 1),
    CONSTRAINT exam_questions_weight_pos CHECK (weight >= 1)
);

-- 抽题只读启用中的题。
CREATE INDEX exam_questions_enabled_idx ON exam_questions (id) WHERE enabled;

CREATE TABLE exam_attempts (
    id             BIGINT      PRIMARY KEY,
    user_id        BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 开考时的题目快照，含正确答案与权重；判分与回顾只看快照，题库后来的改动不影响已开考的场次。
    questions      JSONB       NOT NULL,
    -- { "<题目 id>": ["A", "C"], ... }，只接受快照里的题目与选项（D-060 方案 A）。
    answers        JSONB,
    score          INTEGER,
    total_score    INTEGER,
    score_percent  SMALLINT,
    passed         BOOLEAN,
    started_at     TIMESTAMPTZ NOT NULL,
    expires_at     TIMESTAMPTZ NOT NULL,
    -- 交卷时间。过期未交的场次在下次开考时按「失败」收尾，交卷时间记为过期时间（D-060 方案 A）。
    submitted_at   TIMESTAMPTZ,
    request_ip     TEXT,

    CONSTRAINT exam_attempts_expiry CHECK (expires_at > started_at),
    CONSTRAINT exam_attempts_result_shape CHECK (
        (submitted_at IS NULL) = (passed IS NULL)
        AND (submitted_at IS NULL) = (score IS NULL)
        AND (submitted_at IS NULL) = (score_percent IS NULL)
    ),
    CONSTRAINT exam_attempts_score_range CHECK (
        score_percent IS NULL OR score_percent BETWEEN 0 AND 100
    )
);

-- 每人最多一场未交卷的考试（开考恢复同一场，EXAM-021）。
CREATE UNIQUE INDEX exam_attempts_one_open_uq ON exam_attempts (user_id) WHERE submitted_at IS NULL;
-- 冷却判断取最近几次已交卷的场次 + user_id 外键。
CREATE INDEX exam_attempts_user_submitted_idx ON exam_attempts (user_id, submitted_at DESC);
