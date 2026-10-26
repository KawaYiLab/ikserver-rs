-- Pause dispatch and drain in-flight workers before applying this migration.
ALTER TABLE qq_settings
    ADD COLUMN reply_format TEXT NOT NULL DEFAULT 'markdown' CHECK (reply_format IN ('plain','markdown')),
    DROP CONSTRAINT qq_settings_link_mode_check,
    ALTER COLUMN link_mode SET DEFAULT 'allow',
    ADD CONSTRAINT qq_settings_link_mode_check CHECK (link_mode IN ('strip','domain','allow'));
UPDATE qq_settings SET link_mode='allow', updated_at=now();

-- Existing payloads and attempts were plain text. Do not reinterpret or rehash them.
ALTER TABLE qq_reply_segments
    ADD COLUMN format TEXT NOT NULL DEFAULT 'plain' CHECK (format IN ('plain','markdown'));
ALTER TABLE qq_send_attempts
    ADD COLUMN format TEXT NOT NULL DEFAULT 'plain' CHECK (format IN ('plain','markdown'));
