-- AI 生成链路的调优（ai-assistant.md LLM-016～017、GEN-011、OBS-001～002）。

-- 每个模型的生成参数（LLM-016）：temperature、topP、maxOutputTokens、contextTokens、cachedWeight、extraBody。
-- 没填的项用上游默认值或内置默认值，所以默认是空对象。
ALTER TABLE ai_models ADD COLUMN params JSONB NOT NULL DEFAULT '{}'
    CONSTRAINT ai_models_params_object CHECK (jsonb_typeof(params) = 'object');

-- 角色卡覆盖模型的采样参数（temperature、topP）。
ALTER TABLE ai_role_cards ADD COLUMN params JSONB NOT NULL DEFAULT '{}'
    CONSTRAINT ai_role_cards_params_object CHECK (jsonb_typeof(params) = 'object');

-- 任务的运行指标（OBS-001）：命中缓存的输入 token、首字延迟、工具轮数与调用次数、轮内重试次数、生成耗时。
-- 逐次尝试累加的与 prompt_tokens 一致；延迟与耗时记最后一次尝试的。
ALTER TABLE ai_jobs
    ADD COLUMN cached_tokens    BIGINT   NOT NULL DEFAULT 0,
    ADD COLUMN first_delta_ms   INTEGER,
    ADD COLUMN elapsed_ms       INTEGER,
    ADD COLUMN tool_rounds      SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN tool_calls       SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN retries          SMALLINT NOT NULL DEFAULT 0;

-- 调用记录（OBS-002）：后台打开开关后，每次尝试把发给模型的最后一份请求（含工具结果）和回复存下来，
-- 只有管理员能看，保留 7 天。私信内容敏感，默认关。
ALTER TABLE ai_settings ADD COLUMN trace_enabled BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE ai_job_traces (
    job_id      BIGINT      NOT NULL REFERENCES ai_jobs (id) ON DELETE CASCADE,
    attempt     SMALLINT    NOT NULL,
    model_key   TEXT        NOT NULL,
    -- 最后一轮发给上游的请求体（OpenAI 兼容格式）。
    request     JSONB       NOT NULL,
    -- 回复全文（清洗前）与结束方式。
    reply       TEXT        NOT NULL DEFAULT '',
    outcome     TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),

    PRIMARY KEY (job_id, attempt)
);
CREATE INDEX ai_job_traces_created_idx ON ai_job_traces (created_at);

-- 停止信号推送（GEN-011）：stop_requested_at 第一次写入时通知所有实例，生成方不必高频轮询。
-- NOTIFY 在事务提交时才发出，回滚则不发。
CREATE FUNCTION ai_jobs_notify_stop() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('ai_stop', NEW.id::text);
    RETURN NEW;
END;
$$;

CREATE TRIGGER ai_jobs_stop_notify
    AFTER UPDATE OF stop_requested_at ON ai_jobs
    FOR EACH ROW
    WHEN (OLD.stop_requested_at IS NULL AND NEW.stop_requested_at IS NOT NULL)
    EXECUTE FUNCTION ai_jobs_notify_stop();
