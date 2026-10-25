-- ADR 0008, phase 1. QQ secrets stay in deployment configuration.
ALTER TABLE ai_role_cards ADD COLUMN qq_enabled BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE qq_bots (
    app_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL DEFAULT '',
    default_card_slug TEXT REFERENCES ai_role_cards(slug) ON DELETE SET NULL,
    enabled BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE TABLE qq_groups (
    id BIGINT PRIMARY KEY,
    app_id TEXT NOT NULL REFERENCES qq_bots(app_id),
    group_openid TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    card_slug TEXT REFERENCES ai_role_cards(slug) ON DELETE SET NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    blocked BOOLEAN NOT NULL DEFAULT FALSE,
    context_reset_at TIMESTAMPTZ,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    left_at TIMESTAMPTZ,
    last_active_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (app_id, group_openid)
);
CREATE TABLE qq_identities (
    id BIGINT PRIMARY KEY,
    app_id TEXT NOT NULL REFERENCES qq_bots(app_id),
    scene TEXT NOT NULL CHECK (scene IN ('group','c2c')),
    openid TEXT NOT NULL,
    group_id BIGINT REFERENCES qq_groups(id) ON DELETE CASCADE,
    nickname TEXT NOT NULL DEFAULT '' CHECK (char_length(nickname) <= 32),
    user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    bound_at TIMESTAMPTZ,
    card_slug TEXT REFERENCES ai_role_cards(slug) ON DELETE SET NULL,
    blocked BOOLEAN NOT NULL DEFAULT FALSE,
    available BOOLEAN NOT NULL DEFAULT TRUE,
    context_reset_at TIMESTAMPTZ,
    last_active_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((scene = 'group') = (group_id IS NOT NULL)),
    UNIQUE NULLS NOT DISTINCT (app_id, scene, openid, group_id)
);
CREATE INDEX qq_identities_user ON qq_identities(user_id) WHERE user_id IS NOT NULL;
CREATE TABLE qq_messages (
    id BIGINT PRIMARY KEY,
    app_id TEXT NOT NULL REFERENCES qq_bots(app_id),
    identity_id BIGINT NOT NULL REFERENCES qq_identities(id) ON DELETE CASCADE,
    group_id BIGINT REFERENCES qq_groups(id) ON DELETE CASCADE,
    ai_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    direction TEXT NOT NULL CHECK (direction IN ('in','out')),
    mentioned BOOLEAN NOT NULL DEFAULT FALSE,
    content TEXT NOT NULL CHECK (char_length(content) <= 4000),
    qq_msg_id TEXT,
    reply_to_id BIGINT REFERENCES qq_messages(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX qq_messages_dedupe ON qq_messages(app_id, qq_msg_id) WHERE direction = 'in';
CREATE INDEX qq_messages_group_context ON qq_messages(group_id, created_at DESC, id DESC);
CREATE INDEX qq_messages_identity_context ON qq_messages(identity_id, ai_user_id, created_at DESC, id DESC);
CREATE TABLE qq_turns (
    id BIGINT PRIMARY KEY,
    execution_key TEXT NOT NULL UNIQUE,
    message_id BIGINT NOT NULL UNIQUE REFERENCES qq_messages(id) ON DELETE CASCADE,
    identity_id BIGINT NOT NULL REFERENCES qq_identities(id) ON DELETE CASCADE,
    group_id BIGINT REFERENCES qq_groups(id) ON DELETE CASCADE,
    ai_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    payer_user_id BIGINT REFERENCES users(id) ON DELETE SET NULL,
    status TEXT NOT NULL DEFAULT 'queued' CHECK (status IN ('queued','processing','completed','failed','skipped')),
    reason TEXT,
    hold_id BIGINT,
    quota_day DATE,
    hold_amount BIGINT NOT NULL DEFAULT 0,
    guest_reserved BOOLEAN NOT NULL DEFAULT FALSE,
    guest_token_reserved BIGINT NOT NULL DEFAULT 0,
    model_key TEXT,
    prompt_tokens BIGINT NOT NULL DEFAULT 0,
    completion_tokens BIGINT NOT NULL DEFAULT 0,
    output TEXT[],
    segments_sent INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX qq_turns_recent ON qq_turns(created_at DESC);
CREATE TABLE qq_guest_usage (
    day DATE NOT NULL,
    identity_id BIGINT NOT NULL REFERENCES qq_identities(id) ON DELETE CASCADE,
    turns INTEGER NOT NULL DEFAULT 0 CHECK (turns >= 0),
    notified BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY(day, identity_id)
);
CREATE TABLE qq_group_usage (
    day DATE NOT NULL,
    group_id BIGINT NOT NULL REFERENCES qq_groups(id) ON DELETE CASCADE,
    guest_turns INTEGER NOT NULL DEFAULT 0 CHECK (guest_turns >= 0),
    tokens BIGINT NOT NULL DEFAULT 0 CHECK (tokens >= 0),
    PRIMARY KEY(day, group_id)
);
CREATE TABLE qq_guest_tokens (
    day DATE PRIMARY KEY,
    used BIGINT NOT NULL DEFAULT 0 CHECK (used >= 0)
);
CREATE TABLE qq_settings (
    singleton BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton),
    enabled BOOLEAN NOT NULL DEFAULT FALSE,
    guest_daily_turns INTEGER NOT NULL DEFAULT 10 CHECK (guest_daily_turns BETWEEN 0 AND 1000),
    guest_group_daily_turns INTEGER NOT NULL DEFAULT 100 CHECK (guest_group_daily_turns BETWEEN 0 AND 100000),
    guest_daily_tokens BIGINT NOT NULL DEFAULT 2000000 CHECK (guest_daily_tokens >= 0),
    guest_model_key TEXT,
    group_context_turns INTEGER NOT NULL DEFAULT 12 CHECK (group_context_turns BETWEEN 1 AND 30),
    group_context_hours INTEGER NOT NULL DEFAULT 2 CHECK (group_context_hours BETWEEN 1 AND 24),
    c2c_context_turns INTEGER NOT NULL DEFAULT 20 CHECK (c2c_context_turns BETWEEN 1 AND 60),
    reply_timeout_secs INTEGER NOT NULL DEFAULT 90 CHECK (reply_timeout_secs BETWEEN 10 AND 180),
    max_reply_chars INTEGER NOT NULL DEFAULT 600 CHECK (max_reply_chars BETWEEN 1 AND 4000),
    link_mode TEXT NOT NULL DEFAULT 'strip' CHECK (link_mode IN ('strip','domain')),
    blocked_words TEXT[] NOT NULL DEFAULT '{}',
    guest_reads_group_memory BOOLEAN NOT NULL DEFAULT FALSE,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO qq_settings(singleton) VALUES (TRUE);
