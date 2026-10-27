-- Increase the default QQ reply budget for newly created settings rows.
-- Keep the existing singleton value unchanged; administrators can opt in explicitly.
ALTER TABLE qq_settings
    ALTER COLUMN max_reply_chars SET DEFAULT 4000;
