-- 论坛工具按角色职责分组；旧角色默认保留全部论坛能力。
ALTER TABLE ai_role_cards
    ADD COLUMN forum_content_tools BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN forum_user_tools    BOOLEAN NOT NULL DEFAULT TRUE,
    ADD COLUMN forum_browse_tools  BOOLEAN NOT NULL DEFAULT TRUE;
