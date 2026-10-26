-- QQ delivery is an application state machine; Graphile jobs only wake it.
ALTER TABLE qq_settings ADD COLUMN dispatch_paused BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE qq_conversations (
    id BIGINT PRIMARY KEY,
    app_id TEXT NOT NULL REFERENCES qq_bots(app_id),
    scene TEXT NOT NULL CHECK (scene IN ('group','c2c')),
    target_id BIGINT NOT NULL,
    next_seq BIGINT NOT NULL DEFAULT 0,
    active_turn_id BIGINT,
    lease_epoch BIGINT NOT NULL DEFAULT 0,
    lease_until TIMESTAMPTZ,
    wake_version BIGINT NOT NULL DEFAULT 0,
    wake_at TIMESTAMPTZ,
    UNIQUE(app_id,scene,target_id)
);
CREATE INDEX qq_conversations_due ON qq_conversations(wake_at) WHERE wake_at IS NOT NULL;

ALTER TABLE qq_turns
    ADD COLUMN conversation_id BIGINT REFERENCES qq_conversations(id),
    ADD COLUMN conversation_seq BIGINT,
    ADD COLUMN protocol_version SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN phase TEXT NOT NULL DEFAULT 'queued' CHECK (phase IN ('queued','preparing','generating','delivering','terminal')),
    ADD COLUMN platform_deadline_at TIMESTAMPTZ,
    ADD COLUMN generation_deadline_at TIMESTAMPTZ,
    ADD COLUMN delivery_deadline_at TIMESTAMPTZ,
    ADD COLUMN cost_finalized_at TIMESTAMPTZ,
    ADD COLUMN quota_finalized_at TIMESTAMPTZ,
    ADD COLUMN follow_up_ids BIGINT[] NOT NULL DEFAULT '{}';
CREATE UNIQUE INDEX qq_turns_conversation_sequence ON qq_turns(conversation_id,conversation_seq);
CREATE INDEX qq_turns_pending_conversation ON qq_turns(conversation_id,conversation_seq) WHERE status IN ('queued','processing');

CREATE TABLE qq_reply_segments (
    turn_id BIGINT NOT NULL REFERENCES qq_turns(id) ON DELETE CASCADE,
    msg_seq INTEGER NOT NULL CHECK(msg_seq BETWEEN 1 AND 5),
    content TEXT NOT NULL CHECK(char_length(content) <= 4000),
    payload_hash BYTEA NOT NULL,
    payload_version INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','sending','unknown','pending_review','accepted','rejected','cancelled')),
    attempt_count INTEGER NOT NULL DEFAULT 0,
    attempt_epoch BIGINT,
    retry_at TIMESTAMPTZ,
    auth_corrected BOOLEAN NOT NULL DEFAULT FALSE,
    url_corrected BOOLEAN NOT NULL DEFAULT FALSE,
    qq_message_id TEXT,
    last_http_status INTEGER,
    last_platform_code BIGINT,
    trace_id TEXT,
    accepted_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY(turn_id,msg_seq)
);
CREATE TABLE qq_send_attempts (
    id BIGINT PRIMARY KEY,
    turn_id BIGINT NOT NULL,
    msg_seq INTEGER NOT NULL,
    attempt_no INTEGER NOT NULL,
    lease_epoch BIGINT NOT NULL,
    payload_hash BYTEA NOT NULL,
    payload_version INTEGER NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    result TEXT NOT NULL DEFAULT 'unknown',
    endpoint TEXT,
    http_status INTEGER,
    platform_code BIGINT,
    code BIGINT,
    err_code BIGINT,
    malformed BOOLEAN NOT NULL DEFAULT FALSE,
    audit_id TEXT,
    trace_id TEXT,
    FOREIGN KEY(turn_id,msg_seq) REFERENCES qq_reply_segments(turn_id,msg_seq) ON DELETE CASCADE,
    UNIQUE(turn_id,msg_seq,attempt_no)
);
CREATE INDEX qq_send_attempts_recent ON qq_send_attempts(started_at DESC);

CREATE TABLE qq_app_delivery_limits (
    app_id TEXT PRIMARY KEY REFERENCES qq_bots(app_id) ON DELETE CASCADE,
    next_send_at TIMESTAMPTZ NOT NULL DEFAULT '-infinity',
    blocked_until TIMESTAMPTZ NOT NULL DEFAULT '-infinity'
);

-- Conversion of old live turns is deliberately done by qq.reconcile, using the
-- ordinary quota ledger and terminal transitions. SQL must not guess model cost.
UPDATE qq_turns SET phase='terminal',cost_finalized_at=COALESCE(finished_at,now()),quota_finalized_at=COALESCE(finished_at,now()) WHERE status IN ('completed','failed','skipped');
