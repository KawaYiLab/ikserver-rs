-- 旧 Strapi 后端（KawaYiLab/ikserver@8fbdc34，Strapi 5.49.0）的表结构，schema 改名为 legacy。
-- 生成方式见 crates/legacy/README.md：在空库上启动旧后端让它自建表，再 pg_dump --section=pre-data。
-- 只用于编译期检查与测试；生产环境的 legacy schema 来自真实数据的导出，不跑这个文件。
CREATE SCHEMA legacy;
CREATE TABLE legacy.admin_permissions (
    id integer NOT NULL,
    document_id character varying(255),
    action character varying(255),
    action_parameters jsonb,
    subject character varying(255),
    properties jsonb,
    conditions jsonb,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.admin_permissions_api_token_lnk (
    id integer NOT NULL,
    permission_id integer,
    api_token_id integer,
    permission_ord double precision
);
CREATE SEQUENCE legacy.admin_permissions_api_token_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.admin_permissions_api_token_lnk_id_seq OWNED BY legacy.admin_permissions_api_token_lnk.id;
CREATE SEQUENCE legacy.admin_permissions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.admin_permissions_id_seq OWNED BY legacy.admin_permissions.id;
CREATE TABLE legacy.admin_permissions_role_lnk (
    id integer NOT NULL,
    permission_id integer,
    role_id integer,
    permission_ord double precision
);
CREATE SEQUENCE legacy.admin_permissions_role_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.admin_permissions_role_lnk_id_seq OWNED BY legacy.admin_permissions_role_lnk.id;
CREATE TABLE legacy.admin_roles (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    code character varying(255),
    description character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.admin_roles_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.admin_roles_id_seq OWNED BY legacy.admin_roles.id;
CREATE TABLE legacy.admin_users (
    id integer NOT NULL,
    document_id character varying(255),
    firstname character varying(255),
    lastname character varying(255),
    username character varying(255),
    email character varying(255),
    password character varying(255),
    reset_password_token character varying(255),
    registration_token character varying(255),
    is_active boolean,
    blocked boolean,
    prefered_language character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.admin_users_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.admin_users_id_seq OWNED BY legacy.admin_users.id;
CREATE TABLE legacy.admin_users_roles_lnk (
    id integer NOT NULL,
    user_id integer,
    role_id integer,
    role_ord double precision,
    user_ord double precision
);
CREATE SEQUENCE legacy.admin_users_roles_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.admin_users_roles_lnk_id_seq OWNED BY legacy.admin_users_roles_lnk.id;
CREATE TABLE legacy.ai_agent_configs (
    id integer NOT NULL,
    document_id character varying(255),
    global_enabled boolean,
    daily_tokens_per_user integer,
    daily_tokens integer,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.ai_agent_configs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.ai_agent_configs_id_seq OWNED BY legacy.ai_agent_configs.id;
CREATE TABLE legacy.ai_agent_jobs (
    id integer NOT NULL,
    document_id character varying(255),
    execution_key character varying(255),
    job_id character varying(255),
    quota_user_id integer,
    reservation_id character varying(255),
    payload jsonb,
    job_type character varying(255),
    status character varying(255),
    trigger_id character varying(255),
    error_message text,
    prompt_tokens integer,
    completion_tokens integer,
    model_key character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.ai_agent_jobs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.ai_agent_jobs_id_seq OWNED BY legacy.ai_agent_jobs.id;
CREATE TABLE legacy.ai_agent_jobs_role_card_lnk (
    id integer NOT NULL,
    ai_agent_job_id integer,
    ai_role_card_id integer
);
CREATE SEQUENCE legacy.ai_agent_jobs_role_card_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.ai_agent_jobs_role_card_lnk_id_seq OWNED BY legacy.ai_agent_jobs_role_card_lnk.id;
CREATE TABLE legacy.ai_models (
    id integer NOT NULL,
    document_id character varying(255),
    key character varying(255),
    display_name character varying(255),
    description text,
    badge character varying(255),
    astrbot_provider_id character varying(255),
    enabled boolean,
    sort_order integer,
    cost_weight numeric(10,2),
    input_price_per_m_tokens numeric(10,2),
    output_price_per_m_tokens numeric(10,2),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.ai_models_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.ai_models_id_seq OWNED BY legacy.ai_models.id;
CREATE TABLE legacy.ai_role_cards (
    id integer NOT NULL,
    document_id character varying(255),
    display_name character varying(255),
    slug character varying(255),
    bio text,
    enabled boolean,
    knock_visible boolean,
    dm_enabled boolean,
    comment_enabled boolean,
    sort_order integer,
    output_mode character varying(255),
    astrbot_ref character varying(255),
    suggested_questions jsonb,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.ai_role_cards_allowed_models_lnk (
    id integer NOT NULL,
    ai_role_card_id integer,
    ai_model_id integer,
    ai_model_ord double precision
);
CREATE SEQUENCE legacy.ai_role_cards_allowed_models_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.ai_role_cards_allowed_models_lnk_id_seq OWNED BY legacy.ai_role_cards_allowed_models_lnk.id;
CREATE TABLE legacy.ai_role_cards_bound_user_lnk (
    id integer NOT NULL,
    ai_role_card_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.ai_role_cards_bound_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.ai_role_cards_bound_user_lnk_id_seq OWNED BY legacy.ai_role_cards_bound_user_lnk.id;
CREATE TABLE legacy.ai_role_cards_default_model_lnk (
    id integer NOT NULL,
    ai_role_card_id integer,
    ai_model_id integer
);
CREATE SEQUENCE legacy.ai_role_cards_default_model_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.ai_role_cards_default_model_lnk_id_seq OWNED BY legacy.ai_role_cards_default_model_lnk.id;
CREATE SEQUENCE legacy.ai_role_cards_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.ai_role_cards_id_seq OWNED BY legacy.ai_role_cards.id;
CREATE TABLE legacy.article_hot_views (
    id integer NOT NULL,
    document_id character varying(255),
    article_document_id character varying(255),
    viewer_hash character varying(255),
    dedupe_key character varying(255),
    last_viewed_at timestamp(6) without time zone,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.article_hot_views_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.article_hot_views_id_seq OWNED BY legacy.article_hot_views.id;
CREATE TABLE legacy.article_reads (
    id integer NOT NULL,
    document_id character varying(255),
    dedupe_key character varying(255),
    is_read boolean,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.article_reads_article_lnk (
    id integer NOT NULL,
    article_read_id integer,
    article_id integer,
    article_read_ord double precision
);
CREATE SEQUENCE legacy.article_reads_article_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.article_reads_article_lnk_id_seq OWNED BY legacy.article_reads_article_lnk.id;
CREATE SEQUENCE legacy.article_reads_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.article_reads_id_seq OWNED BY legacy.article_reads.id;
CREATE TABLE legacy.article_reads_user_lnk (
    id integer NOT NULL,
    article_read_id integer,
    user_id integer,
    article_read_ord double precision
);
CREATE SEQUENCE legacy.article_reads_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.article_reads_user_lnk_id_seq OWNED BY legacy.article_reads_user_lnk.id;
CREATE TABLE legacy.articles (
    id integer NOT NULL,
    document_id character varying(255),
    title character varying(255),
    text text,
    editor_state jsonb,
    external_videos jsonb,
    commentscount integer,
    likescount integer,
    views integer,
    is_anonymous boolean,
    is_hidden boolean,
    hidden_reason character varying(255),
    anonymous_seed character varying(255),
    bumped_at timestamp(6) without time zone,
    last_bumped_by_user_id integer,
    edited_at timestamp(6) without time zone,
    denny_count integer,
    triple_count integer,
    is_pinned boolean,
    pinned_at timestamp(6) without time zone,
    solved_at timestamp(6) without time zone,
    accepted_comment_document_id character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    first_published_at timestamp(6) without time zone,
    locale character varying(255)
);
CREATE TABLE legacy.articles_author_lnk (
    id integer NOT NULL,
    article_id integer,
    author_id integer,
    article_ord double precision
);
CREATE SEQUENCE legacy.articles_author_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.articles_author_lnk_id_seq OWNED BY legacy.articles_author_lnk.id;
CREATE TABLE legacy.articles_category_lnk (
    id integer NOT NULL,
    article_id integer,
    category_id integer,
    article_ord double precision
);
CREATE SEQUENCE legacy.articles_category_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.articles_category_lnk_id_seq OWNED BY legacy.articles_category_lnk.id;
CREATE TABLE legacy.articles_cmps (
    id integer NOT NULL,
    entity_id integer,
    cmp_id integer,
    component_type character varying(255),
    field character varying(255),
    "order" double precision
);
CREATE SEQUENCE legacy.articles_cmps_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.articles_cmps_id_seq OWNED BY legacy.articles_cmps.id;
CREATE SEQUENCE legacy.articles_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.articles_id_seq OWNED BY legacy.articles.id;
CREATE TABLE legacy.articles_pinned_by_lnk (
    id integer NOT NULL,
    article_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.articles_pinned_by_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.articles_pinned_by_lnk_id_seq OWNED BY legacy.articles_pinned_by_lnk.id;
CREATE TABLE legacy.articles_tags_lnk (
    id integer NOT NULL,
    article_id integer,
    tag_id integer,
    tag_ord double precision,
    article_ord double precision
);
CREATE SEQUENCE legacy.articles_tags_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.articles_tags_lnk_id_seq OWNED BY legacy.articles_tags_lnk.id;
CREATE TABLE legacy.auth_login_attempts (
    id integer NOT NULL,
    document_id character varying(255),
    provider character varying(255),
    mode character varying(255),
    opaque_state_hash character varying(255),
    poll_secret_hash character varying(255),
    status character varying(255),
    expires_at timestamp(6) without time zone,
    error_code character varying(255),
    completed_at timestamp(6) without time zone,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.auth_login_attempts_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.auth_login_attempts_id_seq OWNED BY legacy.auth_login_attempts.id;
CREATE TABLE legacy.auth_login_attempts_user_lnk (
    id integer NOT NULL,
    auth_login_attempt_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.auth_login_attempts_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.auth_login_attempts_user_lnk_id_seq OWNED BY legacy.auth_login_attempts_user_lnk.id;
CREATE TABLE legacy.auth_sessions (
    id integer NOT NULL,
    document_id character varying(255),
    user_id integer,
    device_id character varying(255),
    refresh_token_hash character varying(255),
    token_family character varying(255),
    user_agent character varying(255),
    ip character varying(255),
    location character varying(255),
    login_method character varying(255),
    login_at timestamp(6) without time zone,
    ip_hash character varying(255),
    expires_at timestamp(6) without time zone,
    last_seen_at timestamp(6) without time zone,
    revoked_at timestamp(6) without time zone,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.auth_sessions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.auth_sessions_id_seq OWNED BY legacy.auth_sessions.id;
CREATE TABLE legacy.authors (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    bio jsonb,
    profile_pinned_articles jsonb,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.authors_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.authors_id_seq OWNED BY legacy.authors.id;
CREATE TABLE legacy.avatars (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    type character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.avatars_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.avatars_id_seq OWNED BY legacy.avatars.id;
CREATE TABLE legacy.business_cards (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    description text,
    story jsonb,
    type character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.business_cards_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.business_cards_id_seq OWNED BY legacy.business_cards.id;
CREATE TABLE legacy.categories (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    slug character varying(255),
    "order" integer,
    is_active boolean,
    admin_only boolean,
    allow_comment_accept boolean,
    description text,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.categories_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.categories_id_seq OWNED BY legacy.categories.id;
CREATE TABLE legacy.check_in_logs (
    id integer NOT NULL,
    document_id character varying(255),
    check_in_day date,
    rank integer,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.check_in_logs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.check_in_logs_id_seq OWNED BY legacy.check_in_logs.id;
CREATE TABLE legacy.check_in_logs_user_lnk (
    id integer NOT NULL,
    check_in_log_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.check_in_logs_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.check_in_logs_user_lnk_id_seq OWNED BY legacy.check_in_logs_user_lnk.id;
CREATE TABLE legacy.comments (
    id integer NOT NULL,
    document_id character varying(255),
    agent_job_id character varying(255),
    quota_receipt jsonb,
    content text,
    likescount integer,
    is_anonymous boolean,
    is_hidden boolean,
    hidden_reason character varying(255),
    anonymous_seed character varying(255),
    pinned_at timestamp(6) without time zone,
    accepted_at timestamp(6) without time zone,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.comments_accepted_by_lnk (
    id integer NOT NULL,
    comment_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.comments_accepted_by_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.comments_accepted_by_lnk_id_seq OWNED BY legacy.comments_accepted_by_lnk.id;
CREATE TABLE legacy.comments_article_lnk (
    id integer NOT NULL,
    comment_id integer,
    article_id integer,
    comment_ord double precision
);
CREATE SEQUENCE legacy.comments_article_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.comments_article_lnk_id_seq OWNED BY legacy.comments_article_lnk.id;
CREATE TABLE legacy.comments_author_lnk (
    id integer NOT NULL,
    comment_id integer,
    author_id integer,
    comment_ord double precision
);
CREATE SEQUENCE legacy.comments_author_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.comments_author_lnk_id_seq OWNED BY legacy.comments_author_lnk.id;
CREATE SEQUENCE legacy.comments_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.comments_id_seq OWNED BY legacy.comments.id;
CREATE TABLE legacy.comments_parent_lnk (
    id integer NOT NULL,
    comment_id integer,
    inv_comment_id integer,
    comment_ord double precision
);
CREATE SEQUENCE legacy.comments_parent_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.comments_parent_lnk_id_seq OWNED BY legacy.comments_parent_lnk.id;
CREATE TABLE legacy.comments_pinned_by_lnk (
    id integer NOT NULL,
    comment_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.comments_pinned_by_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.comments_pinned_by_lnk_id_seq OWNED BY legacy.comments_pinned_by_lnk.id;
CREATE TABLE legacy.components_shared_media (
    id integer NOT NULL
);
CREATE SEQUENCE legacy.components_shared_media_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.components_shared_media_id_seq OWNED BY legacy.components_shared_media.id;
CREATE TABLE legacy.components_shared_quotes (
    id integer NOT NULL,
    title character varying(255),
    body text
);
CREATE SEQUENCE legacy.components_shared_quotes_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.components_shared_quotes_id_seq OWNED BY legacy.components_shared_quotes.id;
CREATE TABLE legacy.components_shared_rich_texts (
    id integer NOT NULL,
    body text
);
CREATE SEQUENCE legacy.components_shared_rich_texts_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.components_shared_rich_texts_id_seq OWNED BY legacy.components_shared_rich_texts.id;
CREATE TABLE legacy.components_shared_seos (
    id integer NOT NULL,
    meta_title character varying(255),
    meta_description text
);
CREATE SEQUENCE legacy.components_shared_seos_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.components_shared_seos_id_seq OWNED BY legacy.components_shared_seos.id;
CREATE TABLE legacy.components_shared_sliders (
    id integer NOT NULL
);
CREATE SEQUENCE legacy.components_shared_sliders_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.components_shared_sliders_id_seq OWNED BY legacy.components_shared_sliders.id;
CREATE TABLE legacy.conversation_members (
    id integer NOT NULL,
    document_id character varying(255),
    role character varying(255),
    last_read_at timestamp(6) without time zone,
    muted boolean,
    pinned boolean,
    left_at timestamp(6) without time zone,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.conversation_members_conversation_lnk (
    id integer NOT NULL,
    conversation_member_id integer,
    conversation_id integer,
    conversation_member_ord double precision
);
CREATE SEQUENCE legacy.conversation_members_conversation_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.conversation_members_conversation_lnk_id_seq OWNED BY legacy.conversation_members_conversation_lnk.id;
CREATE SEQUENCE legacy.conversation_members_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.conversation_members_id_seq OWNED BY legacy.conversation_members.id;
CREATE TABLE legacy.conversation_members_user_lnk (
    id integer NOT NULL,
    conversation_member_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.conversation_members_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.conversation_members_user_lnk_id_seq OWNED BY legacy.conversation_members_user_lnk.id;
CREATE TABLE legacy.conversations (
    id integer NOT NULL,
    document_id character varying(255),
    context_epoch integer,
    direct_pair_key character varying(255),
    kind character varying(255),
    title character varying(255),
    last_message_at timestamp(6) without time zone,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.conversations_ai_model_lnk (
    id integer NOT NULL,
    conversation_id integer,
    ai_model_id integer
);
CREATE SEQUENCE legacy.conversations_ai_model_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.conversations_ai_model_lnk_id_seq OWNED BY legacy.conversations_ai_model_lnk.id;
CREATE SEQUENCE legacy.conversations_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.conversations_id_seq OWNED BY legacy.conversations.id;
CREATE TABLE legacy.conversations_last_message_lnk (
    id integer NOT NULL,
    conversation_id integer,
    message_id integer
);
CREATE SEQUENCE legacy.conversations_last_message_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.conversations_last_message_lnk_id_seq OWNED BY legacy.conversations_last_message_lnk.id;
CREATE TABLE legacy.conversations_owner_lnk (
    id integer NOT NULL,
    conversation_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.conversations_owner_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.conversations_owner_lnk_id_seq OWNED BY legacy.conversations_owner_lnk.id;
CREATE TABLE legacy.denny_givens (
    id integer NOT NULL,
    document_id character varying(255),
    amount integer,
    message character varying(255),
    dedupe_key character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.denny_givens_article_lnk (
    id integer NOT NULL,
    denny_given_id integer,
    article_id integer,
    denny_given_ord double precision
);
CREATE SEQUENCE legacy.denny_givens_article_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.denny_givens_article_lnk_id_seq OWNED BY legacy.denny_givens_article_lnk.id;
CREATE SEQUENCE legacy.denny_givens_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.denny_givens_id_seq OWNED BY legacy.denny_givens.id;
CREATE TABLE legacy.denny_givens_user_lnk (
    id integer NOT NULL,
    denny_given_id integer,
    user_id integer,
    denny_given_ord double precision
);
CREATE SEQUENCE legacy.denny_givens_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.denny_givens_user_lnk_id_seq OWNED BY legacy.denny_givens_user_lnk.id;
CREATE TABLE legacy.denny_logs (
    id integer NOT NULL,
    document_id character varying(255),
    action character varying(255),
    amount integer,
    balance integer,
    related_type character varying(255),
    related_id character varying(255),
    description character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.denny_logs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.denny_logs_id_seq OWNED BY legacy.denny_logs.id;
CREATE TABLE legacy.denny_logs_user_lnk (
    id integer NOT NULL,
    denny_log_id integer,
    user_id integer,
    denny_log_ord double precision
);
CREATE SEQUENCE legacy.denny_logs_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.denny_logs_user_lnk_id_seq OWNED BY legacy.denny_logs_user_lnk.id;
CREATE TABLE legacy.emote_groups (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    "order" integer,
    is_active boolean,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.emote_groups_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.emote_groups_id_seq OWNED BY legacy.emote_groups.id;
CREATE TABLE legacy.emotes (
    id integer NOT NULL,
    document_id character varying(255),
    code character varying(255),
    name character varying(255),
    "order" integer,
    is_active boolean,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.emotes_group_lnk (
    id integer NOT NULL,
    emote_id integer,
    emote_group_id integer,
    emote_ord double precision
);
CREATE SEQUENCE legacy.emotes_group_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.emotes_group_lnk_id_seq OWNED BY legacy.emotes_group_lnk.id;
CREATE SEQUENCE legacy.emotes_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.emotes_id_seq OWNED BY legacy.emotes.id;
CREATE TABLE legacy.exam_attempts (
    id integer NOT NULL,
    document_id character varying(255),
    questions_snapshot jsonb,
    answers jsonb,
    score integer,
    total_score integer,
    score_percent integer,
    passed boolean,
    started_at timestamp(6) without time zone,
    submitted_at timestamp(6) without time zone,
    expires_at timestamp(6) without time zone,
    request_ip character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.exam_attempts_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.exam_attempts_id_seq OWNED BY legacy.exam_attempts.id;
CREATE TABLE legacy.exam_attempts_user_lnk (
    id integer NOT NULL,
    exam_attempt_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.exam_attempts_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.exam_attempts_user_lnk_id_seq OWNED BY legacy.exam_attempts_user_lnk.id;
CREATE TABLE legacy.exam_questions (
    id integer NOT NULL,
    document_id character varying(255),
    question text,
    type character varying(255),
    options jsonb,
    correct_keys jsonb,
    explanation text,
    weight integer,
    enabled boolean,
    tag character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.exam_questions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.exam_questions_id_seq OWNED BY legacy.exam_questions.id;
CREATE TABLE legacy.experience_logs (
    id integer NOT NULL,
    document_id character varying(255),
    action character varying(255),
    exp integer,
    related_type character varying(255),
    related_id character varying(255),
    description character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.experience_logs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.experience_logs_id_seq OWNED BY legacy.experience_logs.id;
CREATE TABLE legacy.experience_logs_user_lnk (
    id integer NOT NULL,
    experience_log_id integer,
    user_id integer,
    experience_log_ord double precision
);
CREATE SEQUENCE legacy.experience_logs_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.experience_logs_user_lnk_id_seq OWNED BY legacy.experience_logs_user_lnk.id;
CREATE TABLE legacy.favorites (
    id integer NOT NULL,
    document_id character varying(255),
    dedupe_key character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.favorites_article_lnk (
    id integer NOT NULL,
    favorite_id integer,
    article_id integer,
    favorite_ord double precision
);
CREATE SEQUENCE legacy.favorites_article_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.favorites_article_lnk_id_seq OWNED BY legacy.favorites_article_lnk.id;
CREATE SEQUENCE legacy.favorites_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.favorites_id_seq OWNED BY legacy.favorites.id;
CREATE TABLE legacy.favorites_user_lnk (
    id integer NOT NULL,
    favorite_id integer,
    user_id integer,
    favorite_ord double precision
);
CREATE SEQUENCE legacy.favorites_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.favorites_user_lnk_id_seq OWNED BY legacy.favorites_user_lnk.id;
CREATE TABLE legacy.files (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    alternative_text text,
    caption text,
    focal_point jsonb,
    width integer,
    height integer,
    formats jsonb,
    hash character varying(255),
    ext character varying(255),
    mime character varying(255),
    size numeric(10,2),
    url text,
    preview_url text,
    provider character varying(255),
    provider_metadata jsonb,
    folder_path character varying(255),
    verified_content_hash character varying(255),
    direct_upload_id character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.files_folder_lnk (
    id integer NOT NULL,
    file_id integer,
    folder_id integer,
    file_ord double precision
);
CREATE SEQUENCE legacy.files_folder_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.files_folder_lnk_id_seq OWNED BY legacy.files_folder_lnk.id;
CREATE SEQUENCE legacy.files_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.files_id_seq OWNED BY legacy.files.id;
CREATE TABLE legacy.files_related_mph (
    id integer NOT NULL,
    file_id integer,
    related_id integer,
    related_type character varying(255),
    field character varying(255),
    "order" double precision
);
CREATE SEQUENCE legacy.files_related_mph_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.files_related_mph_id_seq OWNED BY legacy.files_related_mph.id;
CREATE TABLE legacy.files_uploaders_lnk (
    id integer NOT NULL,
    file_id integer,
    user_id integer,
    user_ord double precision
);
CREATE SEQUENCE legacy.files_uploaders_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.files_uploaders_lnk_id_seq OWNED BY legacy.files_uploaders_lnk.id;
CREATE TABLE legacy.follows (
    id integer NOT NULL,
    document_id character varying(255),
    dedupe_key character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.follows_follower_lnk (
    id integer NOT NULL,
    follow_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.follows_follower_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.follows_follower_lnk_id_seq OWNED BY legacy.follows_follower_lnk.id;
CREATE TABLE legacy.follows_following_lnk (
    id integer NOT NULL,
    follow_id integer,
    author_id integer
);
CREATE SEQUENCE legacy.follows_following_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.follows_following_lnk_id_seq OWNED BY legacy.follows_following_lnk.id;
CREATE SEQUENCE legacy.follows_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.follows_id_seq OWNED BY legacy.follows.id;
CREATE TABLE legacy.i18n_locale (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    code character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.i18n_locale_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.i18n_locale_id_seq OWNED BY legacy.i18n_locale.id;
CREATE TABLE legacy.identity_conflicts (
    id integer NOT NULL,
    provider character varying(32) NOT NULL,
    provider_subject character varying(255),
    provider_account_id character varying(255),
    provider_alias character varying(255),
    reason character varying(64) NOT NULL,
    details text,
    resolved_by integer,
    resolved_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL,
    updated_at timestamp with time zone DEFAULT CURRENT_TIMESTAMP NOT NULL
);
CREATE SEQUENCE legacy.identity_conflicts_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.identity_conflicts_id_seq OWNED BY legacy.identity_conflicts.id;
CREATE TABLE legacy.likes (
    id integer NOT NULL,
    document_id character varying(255),
    dedupe_key character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.likes_article_lnk (
    id integer NOT NULL,
    like_id integer,
    article_id integer,
    like_ord double precision
);
CREATE SEQUENCE legacy.likes_article_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.likes_article_lnk_id_seq OWNED BY legacy.likes_article_lnk.id;
CREATE TABLE legacy.likes_comment_lnk (
    id integer NOT NULL,
    like_id integer,
    comment_id integer,
    like_ord double precision
);
CREATE SEQUENCE legacy.likes_comment_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.likes_comment_lnk_id_seq OWNED BY legacy.likes_comment_lnk.id;
CREATE SEQUENCE legacy.likes_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.likes_id_seq OWNED BY legacy.likes.id;
CREATE TABLE legacy.likes_user_lnk (
    id integer NOT NULL,
    like_id integer,
    user_id integer,
    like_ord double precision
);
CREATE SEQUENCE legacy.likes_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.likes_user_lnk_id_seq OWNED BY legacy.likes_user_lnk.id;
CREATE TABLE legacy.messages (
    id integer NOT NULL,
    document_id character varying(255),
    agent_job_id character varying(255),
    generation_epoch integer,
    stream_finished_at timestamp(6) without time zone,
    quota_receipt jsonb,
    content text,
    kind character varying(255),
    edited_at timestamp(6) without time zone,
    deleted_at timestamp(6) without time zone,
    workflow jsonb,
    ai_model_key character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.messages_conversation_lnk (
    id integer NOT NULL,
    message_id integer,
    conversation_id integer,
    message_ord double precision
);
CREATE SEQUENCE legacy.messages_conversation_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.messages_conversation_lnk_id_seq OWNED BY legacy.messages_conversation_lnk.id;
CREATE SEQUENCE legacy.messages_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.messages_id_seq OWNED BY legacy.messages.id;
CREATE TABLE legacy.messages_reply_to_lnk (
    id integer NOT NULL,
    message_id integer,
    inv_message_id integer
);
CREATE SEQUENCE legacy.messages_reply_to_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.messages_reply_to_lnk_id_seq OWNED BY legacy.messages_reply_to_lnk.id;
CREATE TABLE legacy.messages_sender_lnk (
    id integer NOT NULL,
    message_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.messages_sender_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.messages_sender_lnk_id_seq OWNED BY legacy.messages_sender_lnk.id;
CREATE TABLE legacy.messages_trigger_message_lnk (
    id integer NOT NULL,
    message_id integer,
    inv_message_id integer
);
CREATE SEQUENCE legacy.messages_trigger_message_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.messages_trigger_message_lnk_id_seq OWNED BY legacy.messages_trigger_message_lnk.id;
CREATE TABLE legacy.mihoyo_bindings (
    id integer NOT NULL,
    document_id character varying(255),
    aid character varying(255),
    mid character varying(255),
    zzz_uid character varying(255),
    zzz_nickname character varying(255),
    zzz_level integer,
    zzz_region character varying(255),
    zzz_region_name character varying(255),
    last_synced_at timestamp(6) without time zone,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.mihoyo_bindings_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.mihoyo_bindings_id_seq OWNED BY legacy.mihoyo_bindings.id;
CREATE TABLE legacy.mihoyo_bindings_user_lnk (
    id integer NOT NULL,
    mihoyo_binding_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.mihoyo_bindings_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.mihoyo_bindings_user_lnk_id_seq OWNED BY legacy.mihoyo_bindings_user_lnk.id;
CREATE TABLE legacy.notification_buckets (
    id integer NOT NULL,
    document_id character varying(255),
    recipient_user_id integer,
    bucket_key character varying(255),
    sender_key character varying(255),
    category character varying(255),
    last_at timestamp(6) without time zone,
    preview character varying(255),
    last_notification_id integer,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.notification_buckets_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.notification_buckets_id_seq OWNED BY legacy.notification_buckets.id;
CREATE TABLE legacy.notifications (
    id integer NOT NULL,
    document_id character varying(255),
    type character varying(255),
    message text,
    is_read boolean,
    bucket_key character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.notifications_article_lnk (
    id integer NOT NULL,
    notification_id integer,
    article_id integer
);
CREATE SEQUENCE legacy.notifications_article_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.notifications_article_lnk_id_seq OWNED BY legacy.notifications_article_lnk.id;
CREATE TABLE legacy.notifications_comment_lnk (
    id integer NOT NULL,
    notification_id integer,
    comment_id integer
);
CREATE SEQUENCE legacy.notifications_comment_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.notifications_comment_lnk_id_seq OWNED BY legacy.notifications_comment_lnk.id;
CREATE SEQUENCE legacy.notifications_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.notifications_id_seq OWNED BY legacy.notifications.id;
CREATE TABLE legacy.notifications_recipient_lnk (
    id integer NOT NULL,
    notification_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.notifications_recipient_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.notifications_recipient_lnk_id_seq OWNED BY legacy.notifications_recipient_lnk.id;
CREATE TABLE legacy.notifications_sender_lnk (
    id integer NOT NULL,
    notification_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.notifications_sender_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.notifications_sender_lnk_id_seq OWNED BY legacy.notifications_sender_lnk.id;
CREATE TABLE legacy.register_verifications (
    id integer NOT NULL,
    document_id character varying(255),
    email character varying(255),
    code_hash character varying(255),
    purpose character varying(255),
    expires_at timestamp(6) without time zone,
    sent_at timestamp(6) without time zone,
    consumed_at timestamp(6) without time zone,
    attempts integer,
    request_ip character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.register_verifications_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.register_verifications_id_seq OWNED BY legacy.register_verifications.id;
CREATE TABLE legacy.report_actions (
    id integer NOT NULL,
    document_id character varying(255),
    operation_key character varying(255),
    target_key character varying(255),
    payload jsonb,
    state character varying(255),
    claim_token character varying(255),
    lease_until timestamp(6) without time zone,
    next_attempt_at timestamp(6) without time zone,
    attempts integer,
    last_error text,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.report_actions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.report_actions_id_seq OWNED BY legacy.report_actions.id;
CREATE TABLE legacy.reports (
    id integer NOT NULL,
    document_id character varying(255),
    target_key character varying(255),
    action_pending boolean,
    target_type character varying(255),
    reason character varying(255),
    detail text,
    report_status character varying(255),
    resolution character varying(255),
    resolution_note text,
    target_resolution_note text,
    handled_by character varying(255),
    handled_at timestamp(6) without time zone,
    assigned_at timestamp(6) without time zone,
    snapshot jsonb,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.reports_article_lnk (
    id integer NOT NULL,
    report_id integer,
    article_id integer
);
CREATE SEQUENCE legacy.reports_article_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.reports_article_lnk_id_seq OWNED BY legacy.reports_article_lnk.id;
CREATE TABLE legacy.reports_assigned_to_lnk (
    id integer NOT NULL,
    report_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.reports_assigned_to_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.reports_assigned_to_lnk_id_seq OWNED BY legacy.reports_assigned_to_lnk.id;
CREATE TABLE legacy.reports_comment_lnk (
    id integer NOT NULL,
    report_id integer,
    comment_id integer
);
CREATE SEQUENCE legacy.reports_comment_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.reports_comment_lnk_id_seq OWNED BY legacy.reports_comment_lnk.id;
CREATE SEQUENCE legacy.reports_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.reports_id_seq OWNED BY legacy.reports.id;
CREATE TABLE legacy.reports_reporter_lnk (
    id integer NOT NULL,
    report_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.reports_reporter_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.reports_reporter_lnk_id_seq OWNED BY legacy.reports_reporter_lnk.id;
CREATE TABLE legacy.reports_target_user_lnk (
    id integer NOT NULL,
    report_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.reports_target_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.reports_target_user_lnk_id_seq OWNED BY legacy.reports_target_user_lnk.id;
CREATE TABLE legacy.strapi_ai_localization_jobs (
    id integer NOT NULL,
    content_type character varying(255) NOT NULL,
    related_document_id character varying(255) NOT NULL,
    source_locale character varying(255) NOT NULL,
    target_locales jsonb NOT NULL,
    status character varying(255) NOT NULL,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone
);
CREATE SEQUENCE legacy.strapi_ai_localization_jobs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_ai_localization_jobs_id_seq OWNED BY legacy.strapi_ai_localization_jobs.id;
CREATE TABLE legacy.strapi_ai_metadata_jobs (
    id integer NOT NULL,
    status character varying(255) NOT NULL,
    created_at timestamp(6) without time zone,
    completed_at timestamp(6) without time zone
);
CREATE SEQUENCE legacy.strapi_ai_metadata_jobs_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_ai_metadata_jobs_id_seq OWNED BY legacy.strapi_ai_metadata_jobs.id;
CREATE TABLE legacy.strapi_api_token_permissions (
    id integer NOT NULL,
    document_id character varying(255),
    action character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.strapi_api_token_permissions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_api_token_permissions_id_seq OWNED BY legacy.strapi_api_token_permissions.id;
CREATE TABLE legacy.strapi_api_token_permissions_token_lnk (
    id integer NOT NULL,
    api_token_permission_id integer,
    api_token_id integer,
    api_token_permission_ord double precision
);
CREATE SEQUENCE legacy.strapi_api_token_permissions_token_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_api_token_permissions_token_lnk_id_seq OWNED BY legacy.strapi_api_token_permissions_token_lnk.id;
CREATE TABLE legacy.strapi_api_tokens (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    description character varying(255),
    kind character varying(255),
    type character varying(255),
    access_key character varying(255),
    encrypted_key text,
    last_used_at timestamp(6) without time zone,
    expires_at timestamp(6) without time zone,
    lifespan bigint,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.strapi_api_tokens_admin_user_owner_lnk (
    id integer NOT NULL,
    api_token_id integer,
    user_id integer,
    api_token_ord double precision
);
CREATE SEQUENCE legacy.strapi_api_tokens_admin_user_owner_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_api_tokens_admin_user_owner_lnk_id_seq OWNED BY legacy.strapi_api_tokens_admin_user_owner_lnk.id;
CREATE SEQUENCE legacy.strapi_api_tokens_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_api_tokens_id_seq OWNED BY legacy.strapi_api_tokens.id;
CREATE TABLE legacy.strapi_core_store_settings (
    id integer NOT NULL,
    key character varying(255),
    value text,
    type character varying(255),
    environment character varying(255),
    tag character varying(255)
);
CREATE SEQUENCE legacy.strapi_core_store_settings_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_core_store_settings_id_seq OWNED BY legacy.strapi_core_store_settings.id;
CREATE TABLE legacy.strapi_database_schema (
    id integer NOT NULL,
    schema json,
    "time" timestamp without time zone,
    hash character varying(255)
);
CREATE SEQUENCE legacy.strapi_database_schema_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_database_schema_id_seq OWNED BY legacy.strapi_database_schema.id;
CREATE TABLE legacy.strapi_history_versions (
    id integer NOT NULL,
    content_type character varying(255) NOT NULL,
    related_document_id character varying(255),
    locale character varying(255),
    status character varying(255),
    data jsonb,
    schema jsonb,
    created_at timestamp(6) without time zone,
    created_by_id integer
);
CREATE SEQUENCE legacy.strapi_history_versions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_history_versions_id_seq OWNED BY legacy.strapi_history_versions.id;
CREATE TABLE legacy.strapi_migrations (
    id integer NOT NULL,
    name character varying(255),
    "time" timestamp without time zone
);
CREATE SEQUENCE legacy.strapi_migrations_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_migrations_id_seq OWNED BY legacy.strapi_migrations.id;
CREATE TABLE legacy.strapi_migrations_internal (
    id integer NOT NULL,
    name character varying(255),
    "time" timestamp without time zone
);
CREATE SEQUENCE legacy.strapi_migrations_internal_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_migrations_internal_id_seq OWNED BY legacy.strapi_migrations_internal.id;
CREATE TABLE legacy.strapi_release_actions (
    id integer NOT NULL,
    document_id character varying(255),
    type character varying(255),
    content_type character varying(255),
    entry_document_id character varying(255),
    locale character varying(255),
    is_entry_valid boolean,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer
);
CREATE SEQUENCE legacy.strapi_release_actions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_release_actions_id_seq OWNED BY legacy.strapi_release_actions.id;
CREATE TABLE legacy.strapi_release_actions_release_lnk (
    id integer NOT NULL,
    release_action_id integer,
    release_id integer,
    release_action_ord double precision
);
CREATE SEQUENCE legacy.strapi_release_actions_release_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_release_actions_release_lnk_id_seq OWNED BY legacy.strapi_release_actions_release_lnk.id;
CREATE TABLE legacy.strapi_releases (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    released_at timestamp(6) without time zone,
    scheduled_at timestamp(6) without time zone,
    timezone character varying(255),
    status character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.strapi_releases_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_releases_id_seq OWNED BY legacy.strapi_releases.id;
CREATE TABLE legacy.strapi_sessions (
    id integer NOT NULL,
    document_id character varying(255),
    user_id character varying(255),
    session_id character varying(255),
    child_id character varying(255),
    device_id character varying(255),
    origin character varying(255),
    expires_at timestamp(6) without time zone,
    absolute_expires_at timestamp(6) without time zone,
    status character varying(255),
    type character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.strapi_sessions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_sessions_id_seq OWNED BY legacy.strapi_sessions.id;
CREATE TABLE legacy.strapi_transfer_token_permissions (
    id integer NOT NULL,
    document_id character varying(255),
    action character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.strapi_transfer_token_permissions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_transfer_token_permissions_id_seq OWNED BY legacy.strapi_transfer_token_permissions.id;
CREATE TABLE legacy.strapi_transfer_token_permissions_token_lnk (
    id integer NOT NULL,
    transfer_token_permission_id integer,
    transfer_token_id integer,
    transfer_token_permission_ord double precision
);
CREATE SEQUENCE legacy.strapi_transfer_token_permissions_token_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_transfer_token_permissions_token_lnk_id_seq OWNED BY legacy.strapi_transfer_token_permissions_token_lnk.id;
CREATE TABLE legacy.strapi_transfer_tokens (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    description character varying(255),
    access_key character varying(255),
    last_used_at timestamp(6) without time zone,
    expires_at timestamp(6) without time zone,
    lifespan bigint,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.strapi_transfer_tokens_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_transfer_tokens_id_seq OWNED BY legacy.strapi_transfer_tokens.id;
CREATE TABLE legacy.strapi_webhooks (
    id integer NOT NULL,
    name character varying(255),
    url text,
    headers jsonb,
    events jsonb,
    enabled boolean
);
CREATE SEQUENCE legacy.strapi_webhooks_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_webhooks_id_seq OWNED BY legacy.strapi_webhooks.id;
CREATE TABLE legacy.strapi_workflows (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    content_types jsonb,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.strapi_workflows_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_workflows_id_seq OWNED BY legacy.strapi_workflows.id;
CREATE TABLE legacy.strapi_workflows_stage_required_to_publish_lnk (
    id integer NOT NULL,
    workflow_id integer,
    workflow_stage_id integer
);
CREATE SEQUENCE legacy.strapi_workflows_stage_required_to_publish_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_workflows_stage_required_to_publish_lnk_id_seq OWNED BY legacy.strapi_workflows_stage_required_to_publish_lnk.id;
CREATE TABLE legacy.strapi_workflows_stages (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    color character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.strapi_workflows_stages_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_workflows_stages_id_seq OWNED BY legacy.strapi_workflows_stages.id;
CREATE TABLE legacy.strapi_workflows_stages_permissions_lnk (
    id integer NOT NULL,
    workflow_stage_id integer,
    permission_id integer,
    permission_ord double precision
);
CREATE SEQUENCE legacy.strapi_workflows_stages_permissions_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_workflows_stages_permissions_lnk_id_seq OWNED BY legacy.strapi_workflows_stages_permissions_lnk.id;
CREATE TABLE legacy.strapi_workflows_stages_workflow_lnk (
    id integer NOT NULL,
    workflow_stage_id integer,
    workflow_id integer,
    workflow_stage_ord double precision
);
CREATE SEQUENCE legacy.strapi_workflows_stages_workflow_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.strapi_workflows_stages_workflow_lnk_id_seq OWNED BY legacy.strapi_workflows_stages_workflow_lnk.id;
CREATE TABLE legacy.tags (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    slug character varying(255),
    is_blocked boolean,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.tags_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.tags_id_seq OWNED BY legacy.tags.id;
CREATE TABLE legacy.up_permissions (
    id integer NOT NULL,
    document_id character varying(255),
    action character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.up_permissions_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.up_permissions_id_seq OWNED BY legacy.up_permissions.id;
CREATE TABLE legacy.up_permissions_role_lnk (
    id integer NOT NULL,
    permission_id integer,
    role_id integer,
    permission_ord double precision
);
CREATE SEQUENCE legacy.up_permissions_role_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.up_permissions_role_lnk_id_seq OWNED BY legacy.up_permissions_role_lnk.id;
CREATE TABLE legacy.up_roles (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    description character varying(255),
    type character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.up_roles_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.up_roles_id_seq OWNED BY legacy.up_roles.id;
CREATE TABLE legacy.up_users (
    id integer NOT NULL,
    document_id character varying(255),
    username character varying(255),
    email character varying(255),
    provider character varying(255),
    account_status character varying(255),
    account_cleanup_pending boolean,
    password character varying(255),
    reset_password_token character varying(255),
    confirmation_token character varying(255),
    confirmed boolean,
    blocked boolean,
    exp integer,
    level integer,
    last_check_in_date date,
    consecutive_check_in_days integer,
    profile_hidden boolean,
    denny integer,
    denny_given integer,
    is_ai_agent boolean,
    is_admin boolean,
    exam_passed boolean,
    exam_passed_at timestamp(6) without time zone,
    read_watermark_at timestamp(6) without time zone,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.up_users_author_lnk (
    id integer NOT NULL,
    user_id integer,
    author_id integer
);
CREATE SEQUENCE legacy.up_users_author_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.up_users_author_lnk_id_seq OWNED BY legacy.up_users_author_lnk.id;
CREATE TABLE legacy.up_users_equipped_avatar_lnk (
    id integer NOT NULL,
    user_id integer,
    avatar_id integer
);
CREATE SEQUENCE legacy.up_users_equipped_avatar_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.up_users_equipped_avatar_lnk_id_seq OWNED BY legacy.up_users_equipped_avatar_lnk.id;
CREATE TABLE legacy.up_users_equipped_card_lnk (
    id integer NOT NULL,
    user_id integer,
    business_card_id integer
);
CREATE SEQUENCE legacy.up_users_equipped_card_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.up_users_equipped_card_lnk_id_seq OWNED BY legacy.up_users_equipped_card_lnk.id;
CREATE SEQUENCE legacy.up_users_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.up_users_id_seq OWNED BY legacy.up_users.id;
CREATE TABLE legacy.up_users_role_lnk (
    id integer NOT NULL,
    user_id integer,
    role_id integer,
    user_ord double precision
);
CREATE SEQUENCE legacy.up_users_role_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.up_users_role_lnk_id_seq OWNED BY legacy.up_users_role_lnk.id;
CREATE TABLE legacy.upload_folders (
    id integer NOT NULL,
    document_id character varying(255),
    name character varying(255),
    path_id integer,
    path character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.upload_folders_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.upload_folders_id_seq OWNED BY legacy.upload_folders.id;
CREATE TABLE legacy.upload_folders_parent_lnk (
    id integer NOT NULL,
    folder_id integer,
    inv_folder_id integer,
    folder_ord double precision
);
CREATE SEQUENCE legacy.upload_folders_parent_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.upload_folders_parent_lnk_id_seq OWNED BY legacy.upload_folders_parent_lnk.id;
CREATE TABLE legacy.user_blocks (
    id integer NOT NULL,
    document_id character varying(255),
    dedupe_key character varying(255),
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE TABLE legacy.user_blocks_blocked_user_lnk (
    id integer NOT NULL,
    user_block_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.user_blocks_blocked_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.user_blocks_blocked_user_lnk_id_seq OWNED BY legacy.user_blocks_blocked_user_lnk.id;
CREATE TABLE legacy.user_blocks_blocker_lnk (
    id integer NOT NULL,
    user_block_id integer,
    user_id integer
);
CREATE SEQUENCE legacy.user_blocks_blocker_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.user_blocks_blocker_lnk_id_seq OWNED BY legacy.user_blocks_blocker_lnk.id;
CREATE SEQUENCE legacy.user_blocks_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.user_blocks_id_seq OWNED BY legacy.user_blocks.id;
CREATE TABLE legacy.user_identities (
    id integer NOT NULL,
    document_id character varying(255),
    provider character varying(255),
    provider_subject character varying(255),
    provider_account_id character varying(255),
    provider_alias character varying(255),
    verified_at timestamp(6) without time zone,
    last_login_at timestamp(6) without time zone,
    metadata jsonb,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    published_at timestamp(6) without time zone,
    created_by_id integer,
    updated_by_id integer,
    locale character varying(255)
);
CREATE SEQUENCE legacy.user_identities_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.user_identities_id_seq OWNED BY legacy.user_identities.id;
CREATE TABLE legacy.user_identities_user_lnk (
    id integer NOT NULL,
    user_identity_id integer,
    user_id integer,
    user_identity_ord double precision
);
CREATE SEQUENCE legacy.user_identities_user_lnk_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;
ALTER SEQUENCE legacy.user_identities_user_lnk_id_seq OWNED BY legacy.user_identities_user_lnk.id;
ALTER TABLE ONLY legacy.admin_permissions ALTER COLUMN id SET DEFAULT nextval('legacy.admin_permissions_id_seq'::regclass);
ALTER TABLE ONLY legacy.admin_permissions_api_token_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.admin_permissions_api_token_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.admin_permissions_role_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.admin_permissions_role_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.admin_roles ALTER COLUMN id SET DEFAULT nextval('legacy.admin_roles_id_seq'::regclass);
ALTER TABLE ONLY legacy.admin_users ALTER COLUMN id SET DEFAULT nextval('legacy.admin_users_id_seq'::regclass);
ALTER TABLE ONLY legacy.admin_users_roles_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.admin_users_roles_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.ai_agent_configs ALTER COLUMN id SET DEFAULT nextval('legacy.ai_agent_configs_id_seq'::regclass);
ALTER TABLE ONLY legacy.ai_agent_jobs ALTER COLUMN id SET DEFAULT nextval('legacy.ai_agent_jobs_id_seq'::regclass);
ALTER TABLE ONLY legacy.ai_agent_jobs_role_card_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.ai_agent_jobs_role_card_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.ai_models ALTER COLUMN id SET DEFAULT nextval('legacy.ai_models_id_seq'::regclass);
ALTER TABLE ONLY legacy.ai_role_cards ALTER COLUMN id SET DEFAULT nextval('legacy.ai_role_cards_id_seq'::regclass);
ALTER TABLE ONLY legacy.ai_role_cards_allowed_models_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.ai_role_cards_allowed_models_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.ai_role_cards_bound_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.ai_role_cards_bound_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.ai_role_cards_default_model_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.ai_role_cards_default_model_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.article_hot_views ALTER COLUMN id SET DEFAULT nextval('legacy.article_hot_views_id_seq'::regclass);
ALTER TABLE ONLY legacy.article_reads ALTER COLUMN id SET DEFAULT nextval('legacy.article_reads_id_seq'::regclass);
ALTER TABLE ONLY legacy.article_reads_article_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.article_reads_article_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.article_reads_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.article_reads_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.articles ALTER COLUMN id SET DEFAULT nextval('legacy.articles_id_seq'::regclass);
ALTER TABLE ONLY legacy.articles_author_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.articles_author_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.articles_category_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.articles_category_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.articles_cmps ALTER COLUMN id SET DEFAULT nextval('legacy.articles_cmps_id_seq'::regclass);
ALTER TABLE ONLY legacy.articles_pinned_by_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.articles_pinned_by_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.articles_tags_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.articles_tags_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.auth_login_attempts ALTER COLUMN id SET DEFAULT nextval('legacy.auth_login_attempts_id_seq'::regclass);
ALTER TABLE ONLY legacy.auth_login_attempts_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.auth_login_attempts_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.auth_sessions ALTER COLUMN id SET DEFAULT nextval('legacy.auth_sessions_id_seq'::regclass);
ALTER TABLE ONLY legacy.authors ALTER COLUMN id SET DEFAULT nextval('legacy.authors_id_seq'::regclass);
ALTER TABLE ONLY legacy.avatars ALTER COLUMN id SET DEFAULT nextval('legacy.avatars_id_seq'::regclass);
ALTER TABLE ONLY legacy.business_cards ALTER COLUMN id SET DEFAULT nextval('legacy.business_cards_id_seq'::regclass);
ALTER TABLE ONLY legacy.categories ALTER COLUMN id SET DEFAULT nextval('legacy.categories_id_seq'::regclass);
ALTER TABLE ONLY legacy.check_in_logs ALTER COLUMN id SET DEFAULT nextval('legacy.check_in_logs_id_seq'::regclass);
ALTER TABLE ONLY legacy.check_in_logs_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.check_in_logs_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.comments ALTER COLUMN id SET DEFAULT nextval('legacy.comments_id_seq'::regclass);
ALTER TABLE ONLY legacy.comments_accepted_by_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.comments_accepted_by_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.comments_article_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.comments_article_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.comments_author_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.comments_author_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.comments_parent_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.comments_parent_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.comments_pinned_by_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.comments_pinned_by_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.components_shared_media ALTER COLUMN id SET DEFAULT nextval('legacy.components_shared_media_id_seq'::regclass);
ALTER TABLE ONLY legacy.components_shared_quotes ALTER COLUMN id SET DEFAULT nextval('legacy.components_shared_quotes_id_seq'::regclass);
ALTER TABLE ONLY legacy.components_shared_rich_texts ALTER COLUMN id SET DEFAULT nextval('legacy.components_shared_rich_texts_id_seq'::regclass);
ALTER TABLE ONLY legacy.components_shared_seos ALTER COLUMN id SET DEFAULT nextval('legacy.components_shared_seos_id_seq'::regclass);
ALTER TABLE ONLY legacy.components_shared_sliders ALTER COLUMN id SET DEFAULT nextval('legacy.components_shared_sliders_id_seq'::regclass);
ALTER TABLE ONLY legacy.conversation_members ALTER COLUMN id SET DEFAULT nextval('legacy.conversation_members_id_seq'::regclass);
ALTER TABLE ONLY legacy.conversation_members_conversation_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.conversation_members_conversation_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.conversation_members_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.conversation_members_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.conversations ALTER COLUMN id SET DEFAULT nextval('legacy.conversations_id_seq'::regclass);
ALTER TABLE ONLY legacy.conversations_ai_model_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.conversations_ai_model_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.conversations_last_message_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.conversations_last_message_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.conversations_owner_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.conversations_owner_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.denny_givens ALTER COLUMN id SET DEFAULT nextval('legacy.denny_givens_id_seq'::regclass);
ALTER TABLE ONLY legacy.denny_givens_article_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.denny_givens_article_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.denny_givens_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.denny_givens_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.denny_logs ALTER COLUMN id SET DEFAULT nextval('legacy.denny_logs_id_seq'::regclass);
ALTER TABLE ONLY legacy.denny_logs_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.denny_logs_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.emote_groups ALTER COLUMN id SET DEFAULT nextval('legacy.emote_groups_id_seq'::regclass);
ALTER TABLE ONLY legacy.emotes ALTER COLUMN id SET DEFAULT nextval('legacy.emotes_id_seq'::regclass);
ALTER TABLE ONLY legacy.emotes_group_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.emotes_group_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.exam_attempts ALTER COLUMN id SET DEFAULT nextval('legacy.exam_attempts_id_seq'::regclass);
ALTER TABLE ONLY legacy.exam_attempts_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.exam_attempts_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.exam_questions ALTER COLUMN id SET DEFAULT nextval('legacy.exam_questions_id_seq'::regclass);
ALTER TABLE ONLY legacy.experience_logs ALTER COLUMN id SET DEFAULT nextval('legacy.experience_logs_id_seq'::regclass);
ALTER TABLE ONLY legacy.experience_logs_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.experience_logs_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.favorites ALTER COLUMN id SET DEFAULT nextval('legacy.favorites_id_seq'::regclass);
ALTER TABLE ONLY legacy.favorites_article_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.favorites_article_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.favorites_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.favorites_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.files ALTER COLUMN id SET DEFAULT nextval('legacy.files_id_seq'::regclass);
ALTER TABLE ONLY legacy.files_folder_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.files_folder_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.files_related_mph ALTER COLUMN id SET DEFAULT nextval('legacy.files_related_mph_id_seq'::regclass);
ALTER TABLE ONLY legacy.files_uploaders_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.files_uploaders_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.follows ALTER COLUMN id SET DEFAULT nextval('legacy.follows_id_seq'::regclass);
ALTER TABLE ONLY legacy.follows_follower_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.follows_follower_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.follows_following_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.follows_following_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.i18n_locale ALTER COLUMN id SET DEFAULT nextval('legacy.i18n_locale_id_seq'::regclass);
ALTER TABLE ONLY legacy.identity_conflicts ALTER COLUMN id SET DEFAULT nextval('legacy.identity_conflicts_id_seq'::regclass);
ALTER TABLE ONLY legacy.likes ALTER COLUMN id SET DEFAULT nextval('legacy.likes_id_seq'::regclass);
ALTER TABLE ONLY legacy.likes_article_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.likes_article_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.likes_comment_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.likes_comment_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.likes_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.likes_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.messages ALTER COLUMN id SET DEFAULT nextval('legacy.messages_id_seq'::regclass);
ALTER TABLE ONLY legacy.messages_conversation_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.messages_conversation_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.messages_reply_to_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.messages_reply_to_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.messages_sender_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.messages_sender_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.messages_trigger_message_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.messages_trigger_message_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.mihoyo_bindings ALTER COLUMN id SET DEFAULT nextval('legacy.mihoyo_bindings_id_seq'::regclass);
ALTER TABLE ONLY legacy.mihoyo_bindings_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.mihoyo_bindings_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.notification_buckets ALTER COLUMN id SET DEFAULT nextval('legacy.notification_buckets_id_seq'::regclass);
ALTER TABLE ONLY legacy.notifications ALTER COLUMN id SET DEFAULT nextval('legacy.notifications_id_seq'::regclass);
ALTER TABLE ONLY legacy.notifications_article_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.notifications_article_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.notifications_comment_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.notifications_comment_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.notifications_recipient_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.notifications_recipient_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.notifications_sender_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.notifications_sender_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.register_verifications ALTER COLUMN id SET DEFAULT nextval('legacy.register_verifications_id_seq'::regclass);
ALTER TABLE ONLY legacy.report_actions ALTER COLUMN id SET DEFAULT nextval('legacy.report_actions_id_seq'::regclass);
ALTER TABLE ONLY legacy.reports ALTER COLUMN id SET DEFAULT nextval('legacy.reports_id_seq'::regclass);
ALTER TABLE ONLY legacy.reports_article_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.reports_article_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.reports_assigned_to_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.reports_assigned_to_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.reports_comment_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.reports_comment_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.reports_reporter_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.reports_reporter_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.reports_target_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.reports_target_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_ai_localization_jobs ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_ai_localization_jobs_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_ai_metadata_jobs ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_ai_metadata_jobs_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_api_token_permissions ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_api_token_permissions_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_api_token_permissions_token_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_api_token_permissions_token_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_api_tokens ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_api_tokens_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_api_tokens_admin_user_owner_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_api_tokens_admin_user_owner_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_core_store_settings ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_core_store_settings_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_database_schema ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_database_schema_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_history_versions ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_history_versions_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_migrations ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_migrations_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_migrations_internal ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_migrations_internal_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_release_actions ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_release_actions_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_release_actions_release_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_release_actions_release_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_releases ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_releases_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_sessions ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_sessions_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_transfer_token_permissions ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_transfer_token_permissions_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_transfer_token_permissions_token_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_transfer_token_permissions_token_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_transfer_tokens ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_transfer_tokens_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_webhooks ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_webhooks_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_workflows ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_workflows_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_workflows_stage_required_to_publish_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_workflows_stage_required_to_publish_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_workflows_stages ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_workflows_stages_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_workflows_stages_permissions_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_workflows_stages_permissions_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.strapi_workflows_stages_workflow_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.strapi_workflows_stages_workflow_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.tags ALTER COLUMN id SET DEFAULT nextval('legacy.tags_id_seq'::regclass);
ALTER TABLE ONLY legacy.up_permissions ALTER COLUMN id SET DEFAULT nextval('legacy.up_permissions_id_seq'::regclass);
ALTER TABLE ONLY legacy.up_permissions_role_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.up_permissions_role_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.up_roles ALTER COLUMN id SET DEFAULT nextval('legacy.up_roles_id_seq'::regclass);
ALTER TABLE ONLY legacy.up_users ALTER COLUMN id SET DEFAULT nextval('legacy.up_users_id_seq'::regclass);
ALTER TABLE ONLY legacy.up_users_author_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.up_users_author_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.up_users_equipped_avatar_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.up_users_equipped_avatar_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.up_users_equipped_card_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.up_users_equipped_card_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.up_users_role_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.up_users_role_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.upload_folders ALTER COLUMN id SET DEFAULT nextval('legacy.upload_folders_id_seq'::regclass);
ALTER TABLE ONLY legacy.upload_folders_parent_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.upload_folders_parent_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.user_blocks ALTER COLUMN id SET DEFAULT nextval('legacy.user_blocks_id_seq'::regclass);
ALTER TABLE ONLY legacy.user_blocks_blocked_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.user_blocks_blocked_user_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.user_blocks_blocker_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.user_blocks_blocker_lnk_id_seq'::regclass);
ALTER TABLE ONLY legacy.user_identities ALTER COLUMN id SET DEFAULT nextval('legacy.user_identities_id_seq'::regclass);
ALTER TABLE ONLY legacy.user_identities_user_lnk ALTER COLUMN id SET DEFAULT nextval('legacy.user_identities_user_lnk_id_seq'::regclass);
