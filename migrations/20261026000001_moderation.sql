-- 举报与处罚（RPT、RPTADM、SANCTION）。归 ik-moderation。业务规则见 docs/business-rules/moderation-exam.md。

CREATE TABLE reports (
    id                      BIGINT      PRIMARY KEY,
    reporter_id             BIGINT      NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    -- 目标：帖子、评论、用户。id 稳定（内容删除后仍能按目标聚合，替代旧的 targetKey，RPT-004）。
    target_type             TEXT        NOT NULL,
    target_id               BIGINT      NOT NULL,
    -- 内容作者或被举报的用户（警告、封禁的对象）。匿名内容也记录，但从不对外展示（AGENTS.md 规则 10）。
    owner_user_id           BIGINT,
    reason                  TEXT        NOT NULL,
    detail                  TEXT,
    status                  TEXT        NOT NULL DEFAULT 'pending',
    resolution              TEXT,
    resolution_note         TEXT,
    target_resolution_note  TEXT,
    assigned_to             BIGINT      REFERENCES users (id) ON DELETE SET NULL,
    assigned_at             TIMESTAMPTZ,
    handled_by              BIGINT      REFERENCES users (id) ON DELETE SET NULL,
    handled_at              TIMESTAMPTZ,
    -- 举报时的目标快照（标题、摘要、图片、匿名与否），内容被删后后台仍能看到（RPT-013）。
    snapshot                JSONB       NOT NULL,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT reports_target_type_check CHECK (target_type IN ('post', 'comment', 'user')),
    CONSTRAINT reports_reason_check CHECK (
        reason IN ('spam', 'abuse', 'porn', 'illegal', 'privacy', 'misinfo', 'plagiarism', 'other')
    ),
    CONSTRAINT reports_status_check CHECK (status IN ('pending', 'reviewing', 'accepted', 'rejected')),
    CONSTRAINT reports_resolution_check CHECK (
        resolution IS NULL
        OR resolution IN ('none', 'content_hidden', 'content_deleted', 'user_warned', 'user_blocked')
    ),
    CONSTRAINT reports_closed_check CHECK (
        (status IN ('accepted', 'rejected')) = (resolution IS NOT NULL AND handled_at IS NOT NULL)
    ),
    CONSTRAINT reports_assigned_pair CHECK ((assigned_to IS NULL) = (assigned_at IS NULL) OR assigned_to IS NULL),
    CONSTRAINT reports_detail_len CHECK (detail IS NULL OR char_length(detail) <= 500),
    CONSTRAINT reports_other_needs_detail CHECK (reason <> 'other' OR detail IS NOT NULL),
    CONSTRAINT reports_notes_len CHECK (
        (resolution_note IS NULL OR char_length(resolution_note) <= 500)
        AND (target_resolution_note IS NULL OR char_length(target_resolution_note) <= 500)
    )
);
-- 同一人对同一目标同时只能有一条未结案的举报（RPT-011）；结案后可以再举报。
CREATE UNIQUE INDEX reports_open_uq ON reports (reporter_id, target_type, target_id)
    WHERE status IN ('pending', 'reviewing');
-- 按目标聚合、组锁。
CREATE INDEX reports_target_idx ON reports (target_type, target_id, status);
-- 后台列表按状态与时间。
CREATE INDEX reports_status_created_idx ON reports (status, created_at DESC);
-- 我的举报。
CREATE INDEX reports_reporter_idx ON reports (reporter_id, id DESC);
CREATE INDEX reports_assigned_idx ON reports (assigned_to) WHERE assigned_to IS NOT NULL;
CREATE INDEX reports_handled_by_idx ON reports (handled_by) WHERE handled_by IS NOT NULL;

-- 审核日志：每次结案、自动隐藏、解封一行，便于追溯谁在什么时候做了什么。
CREATE TABLE moderation_log (
    id              BIGINT      PRIMARY KEY,
    -- 自动隐藏为 NULL。
    actor_id        BIGINT      REFERENCES users (id) ON DELETE SET NULL,
    action          TEXT        NOT NULL,
    target_type     TEXT        NOT NULL,
    target_id       BIGINT      NOT NULL,
    report_count    INTEGER     NOT NULL DEFAULT 0,
    note            TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT moderation_log_action_check CHECK (
        action IN ('auto_hide', 'reject', 'hide_content', 'delete_content', 'warn_user', 'block_user', 'unblock_user')
    ),
    CONSTRAINT moderation_log_target_type_check CHECK (target_type IN ('post', 'comment', 'user'))
);
CREATE INDEX moderation_log_target_idx ON moderation_log (target_type, target_id, created_at DESC);
CREATE INDEX moderation_log_actor_idx ON moderation_log (actor_id, created_at DESC) WHERE actor_id IS NOT NULL;
