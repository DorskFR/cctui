-- FUNCTION: session_usage_totals_apply()
CREATE FUNCTION public.session_usage_totals_apply() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        UPDATE session_usage_totals SET
            input_tokens          = input_tokens          - OLD.input_tokens,
            output_tokens         = output_tokens         - OLD.output_tokens,
            cache_read_tokens     = cache_read_tokens     - OLD.cache_read_tokens,
            cache_creation_tokens = cache_creation_tokens - OLD.cache_creation_tokens
        WHERE session_id = OLD.session_id AND model = COALESCE(OLD.model, '');
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        INSERT INTO session_usage_totals AS t
            (session_id, model, input_tokens, output_tokens, cache_read_tokens,
             cache_creation_tokens)
        VALUES (NEW.session_id, COALESCE(NEW.model, ''), NEW.input_tokens, NEW.output_tokens,
                NEW.cache_read_tokens, NEW.cache_creation_tokens)
        ON CONFLICT (session_id, model) DO UPDATE SET
            input_tokens          = t.input_tokens          + EXCLUDED.input_tokens,
            output_tokens         = t.output_tokens         + EXCLUDED.output_tokens,
            cache_read_tokens     = t.cache_read_tokens     + EXCLUDED.cache_read_tokens,
            cache_creation_tokens = t.cache_creation_tokens + EXCLUDED.cache_creation_tokens;
    END IF;
    RETURN NULL;
END;
$$;


SET default_tablespace = '';

SET default_table_access_method = heap;

-- TABLE: account_pool_members
CREATE TABLE public.account_pool_members (
    pool_id uuid NOT NULL,
    account_id uuid NOT NULL,
    "position" integer DEFAULT 0 NOT NULL
);

-- TABLE: account_pools
CREATE TABLE public.account_pools (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    strategy text DEFAULT 'headroom'::text NOT NULL,
    failover boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT account_pools_strategy CHECK ((strategy = ANY (ARRAY['headroom'::text, 'ordered'::text])))
);

-- TABLE: account_providers
CREATE TABLE public.account_providers (
    id uuid DEFAULT gen_random_uuid() CONSTRAINT oauth_accounts_id_not_null NOT NULL,
    user_id uuid CONSTRAINT oauth_accounts_user_id_not_null NOT NULL,
    provider text CONSTRAINT oauth_accounts_provider_not_null NOT NULL,
    encrypted_access_token text,
    encrypted_refresh_token text,
    expires_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() CONSTRAINT oauth_accounts_created_at_not_null NOT NULL,
    last_used_at timestamp with time zone,
    request_count bigint DEFAULT 0 CONSTRAINT oauth_accounts_request_count_not_null NOT NULL,
    bytes_transferred bigint DEFAULT 0 CONSTRAINT oauth_accounts_bytes_transferred_not_null NOT NULL,
    provider_account_id text,
    base_url text,
    models jsonb,
    auth_scheme text DEFAULT 'oauth'::text CONSTRAINT oauth_accounts_auth_scheme_not_null NOT NULL,
    managed boolean DEFAULT false CONSTRAINT oauth_accounts_managed_not_null NOT NULL,
    model_aliases jsonb,
    soft_limit_5h_pct integer,
    soft_limit_7d_pct integer,
    needs_reauth boolean DEFAULT false CONSTRAINT oauth_accounts_needs_reauth_not_null NOT NULL,
    last_auth_error text,
    last_auth_error_at timestamp with time zone,
    settings_json jsonb,
    account_id uuid NOT NULL,
    soft_limit_bypass_5h_minutes integer,
    soft_limit_bypass_7d_minutes integer,
    soft_limits_json jsonb,
    provider_settings jsonb,
    rate_limits_json jsonb,
    organization_uuid text,
    usage_notices jsonb,
    header_pin boolean DEFAULT true NOT NULL,
    usage_probe text,
    family text GENERATED ALWAYS AS (
CASE
    WHEN (provider = 'fireworks'::text) THEN 'fireworks'::text
    WHEN (provider = 'google'::text) THEN 'google'::text
    WHEN (provider ~~ '%openai%'::text) THEN 'openai'::text
    ELSE 'anthropic'::text
END) STORED
);

-- TABLE: account_redirects
CREATE TABLE public.account_redirects (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    from_account uuid NOT NULL,
    to_account uuid,
    family text NOT NULL,
    match_model text,
    to_model text,
    expires_at timestamp with time zone,
    reason text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT account_redirects_match_needs_model CHECK (((match_model IS NULL) OR (to_model IS NOT NULL))),
    CONSTRAINT account_redirects_not_identity CHECK ((to_account IS DISTINCT FROM from_account)),
    CONSTRAINT account_redirects_one_target CHECK (((to_account IS NULL) <> (to_model IS NULL)))
);

-- TABLE: account_tool_policies
CREATE TABLE public.account_tool_policies (
    account_id uuid NOT NULL,
    terms text[] DEFAULT '{}'::text[] NOT NULL,
    patterns text[] DEFAULT '{}'::text[] NOT NULL,
    protected_owners text[] DEFAULT '{}'::text[] NOT NULL,
    exempt_roots text[] DEFAULT '{}'::text[] NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: accounts
CREATE TABLE public.accounts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    env_json text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    pool_eligible boolean DEFAULT true NOT NULL,
    emoji text,
    pool_weight real DEFAULT 1.0 NOT NULL,
    CONSTRAINT accounts_pool_weight_check CHECK ((pool_weight > (0)::double precision))
);

-- TABLE: adapters_enabled
CREATE TABLE public.adapters_enabled (
    machine_id uuid NOT NULL,
    adapter_id text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: auth_keys
CREATE TABLE public.auth_keys (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    key_hash text NOT NULL,
    key_preview text,
    label text,
    kind text DEFAULT 'user'::text NOT NULL,
    machine_id uuid,
    dispatcher_id uuid,
    expires_at timestamp with time zone,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    session_id text,
    last_used_at timestamp with time zone,
    passkey_id uuid
);

-- TABLE: dispatch_handles
CREATE TABLE public.dispatch_handles (
    session_id text NOT NULL,
    dispatcher_name text NOT NULL,
    handle text NOT NULL,
    namespace text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: dispatch_policies
CREATE TABLE public.dispatch_policies (
    user_id uuid NOT NULL,
    dispatchers text[] DEFAULT '{}'::text[] NOT NULL,
    profiles text[] DEFAULT '{}'::text[] NOT NULL,
    adapters text[] DEFAULT '{}'::text[] NOT NULL,
    models text[] DEFAULT '{}'::text[] NOT NULL,
    context_packs jsonb DEFAULT '[]'::jsonb NOT NULL,
    max_timeout_minutes integer,
    max_concurrent integer,
    daily_cap integer,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT dispatch_policies_daily_cap_check CHECK (((daily_cap IS NULL) OR (daily_cap > 0))),
    CONSTRAINT dispatch_policies_max_concurrent_check CHECK (((max_concurrent IS NULL) OR (max_concurrent > 0))),
    CONSTRAINT dispatch_policies_max_timeout_minutes_check CHECK (((max_timeout_minutes IS NULL) OR (max_timeout_minutes > 0)))
);

-- TABLE: dispatch_policy_uses
CREATE TABLE public.dispatch_policy_uses (
    session_id text NOT NULL,
    user_id uuid NOT NULL,
    dispatched_at timestamp with time zone DEFAULT now() NOT NULL,
    id bigint NOT NULL
);

-- SEQUENCE: dispatch_policy_uses_id_seq
CREATE SEQUENCE public.dispatch_policy_uses_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: dispatch_policy_uses_id_seq
ALTER SEQUENCE public.dispatch_policy_uses_id_seq OWNED BY public.dispatch_policy_uses.id;

-- TABLE: dispatch_record_dismissals
CREATE TABLE public.dispatch_record_dismissals (
    user_id uuid NOT NULL,
    session_id text NOT NULL,
    dismissed_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: dispatch_records
CREATE TABLE public.dispatch_records (
    session_id text NOT NULL,
    user_id uuid,
    dispatcher_name text NOT NULL,
    name text,
    profile text,
    repo text,
    model text,
    adapter text,
    prompt_head text,
    prompt_file text,
    prompt_truncated boolean DEFAULT false NOT NULL,
    env_keys text[] DEFAULT '{}'::text[] NOT NULL,
    replay jsonb,
    outcome text DEFAULT 'pending'::text NOT NULL,
    end_reason text,
    detail text,
    redispatched_as text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    worker_exit jsonb,
    queued boolean DEFAULT false NOT NULL,
    started_at timestamp with time zone,
    CONSTRAINT dispatch_records_outcome_check CHECK ((outcome = ANY (ARRAY['pending'::text, 'forwarded'::text, 'registered'::text, 'refused'::text, 'failed'::text])))
);

-- TABLE: dispatchers
CREATE TABLE public.dispatchers (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    kind text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    deleted_at timestamp with time zone,
    key_hash text,
    key_preview text,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    default_account_id uuid,
    default_account_provider text,
    default_pool_id uuid
);

-- TABLE: events
CREATE TABLE public.events (
    id bigint NOT NULL,
    occurred_at timestamp with time zone DEFAULT now() NOT NULL,
    kind text NOT NULL,
    severity text DEFAULT 'info'::text NOT NULL,
    session_id text,
    machine_id uuid,
    user_id uuid,
    actor text NOT NULL,
    summary text NOT NULL,
    detail jsonb DEFAULT '{}'::jsonb NOT NULL
);

-- SEQUENCE: events_id_seq
CREATE SEQUENCE public.events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: events_id_seq
ALTER SEQUENCE public.events_id_seq OWNED BY public.events.id;

-- TABLE: gateway_blocks
CREATE TABLE public.gateway_blocks (
    id bigint NOT NULL,
    session_id text,
    account_id uuid,
    tool_name text NOT NULL,
    rule text NOT NULL,
    input_sha256 text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- SEQUENCE: gateway_blocks_id_seq
CREATE SEQUENCE public.gateway_blocks_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: gateway_blocks_id_seq
ALTER SEQUENCE public.gateway_blocks_id_seq OWNED BY public.gateway_blocks.id;

-- TABLE: instance_settings
CREATE TABLE public.instance_settings (
    key text NOT NULL,
    value jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: key_acls
CREATE TABLE public.key_acls (
    key_id uuid NOT NULL,
    scope text NOT NULL,
    granted_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: machines
CREATE TABLE public.machines (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    key_hash text NOT NULL,
    first_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    deleted_at timestamp with time zone,
    display_name text,
    kind text DEFAULT 'persistent'::text NOT NULL,
    dispatch_key text,
    hue smallint,
    key_preview text,
    update_hook boolean DEFAULT false NOT NULL,
    harness_autoupdate jsonb,
    harness_report jsonb,
    harness_report_at timestamp with time zone,
    CONSTRAINT machines_hue_check CHECK (((hue >= 0) AND (hue < 360)))
);

-- TABLE: resource_shares
CREATE TABLE public.resource_shares (
    resource_type text NOT NULL,
    resource_id uuid NOT NULL,
    grantee_id uuid NOT NULL,
    action text DEFAULT 'use'::text NOT NULL,
    granted_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone
);

-- TABLE: session_account_rebinds
CREATE TABLE public.session_account_rebinds (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    session_id text NOT NULL,
    pool_id uuid,
    from_account text NOT NULL,
    to_account text NOT NULL,
    reason text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_spawn_capabilities
CREATE TABLE public.session_spawn_capabilities (
    session_id text NOT NULL,
    capability jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_spawner_intents
CREATE TABLE public.session_spawner_intents (
    spawn_key text NOT NULL,
    user_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    session_name text
);

-- TABLE: session_token_usage
CREATE TABLE public.session_token_usage (
    session_id text NOT NULL,
    message_id text NOT NULL,
    input_tokens bigint DEFAULT 0 NOT NULL,
    output_tokens bigint DEFAULT 0 NOT NULL,
    cache_read_tokens bigint DEFAULT 0 NOT NULL,
    cache_creation_tokens bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    model text,
    gateway_rewrote_body boolean DEFAULT false NOT NULL
);

-- TABLE: session_tokens
CREATE TABLE public.session_tokens (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    token_hash text NOT NULL,
    session_id text NOT NULL,
    account_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    encrypted_token text,
    expires_at timestamp with time zone,
    last_used_at timestamp with time zone,
    pool_id uuid,
    user_id uuid,
    kind text DEFAULT 'session'::text NOT NULL,
    label text
);

-- TABLE: session_usage_totals
CREATE TABLE public.session_usage_totals (
    session_id text NOT NULL,
    model text DEFAULT ''::text NOT NULL,
    input_tokens bigint DEFAULT 0 NOT NULL,
    output_tokens bigint DEFAULT 0 NOT NULL,
    cache_read_tokens bigint DEFAULT 0 NOT NULL,
    cache_creation_tokens bigint DEFAULT 0 NOT NULL
);

-- TABLE: session_webhooks
CREATE TABLE public.session_webhooks (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    session_id text NOT NULL,
    user_id uuid,
    notify_url text NOT NULL,
    secret text,
    task_id text NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    payload jsonb,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    sent_at timestamp with time zone
);

-- TABLE: sessions
CREATE TABLE public.sessions (
    id text NOT NULL,
    parent_id text,
    account_id text,
    machine_id text NOT NULL,
    working_dir text NOT NULL,
    status text DEFAULT 'new'::text NOT NULL,
    registered_at timestamp with time zone DEFAULT now() NOT NULL,
    last_heartbeat timestamp with time zone DEFAULT now() NOT NULL,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    user_id uuid,
    machine_uuid uuid,
    adapter_id text DEFAULT 'claude-code'::text NOT NULL,
    origin text,
    dispatch_handle text,
    tempo text,
    agent_state text,
    activity text,
    session_name text,
    model text,
    effort text,
    pinned boolean DEFAULT false NOT NULL,
    pinned_at timestamp with time zone,
    soft_limit_reason text,
    last_tool_at timestamp with time zone,
    last_tool_name text,
    tool_use_count integer DEFAULT 0 NOT NULL,
    intent text,
    children jsonb DEFAULT '[]'::jsonb NOT NULL,
    transcript_offset bigint DEFAULT 0 NOT NULL,
    ended_at timestamp with time zone,
    end_reason text,
    end_detail text,
    soft_limit_key text,
    todos jsonb,
    todo_updated_at timestamp with time zone,
    permission_mode text,
    keepalive_json jsonb,
    last_keepalive_at timestamp with time zone,
    archived_by text,
    room_id uuid,
    spawned_by uuid,
    CONSTRAINT sessions_status_check CHECK ((status = ANY (ARRAY['new'::text, 'active'::text, 'inactive'::text, 'archived'::text, 'draft'::text, 'ended'::text, 'failed'::text])))
);

-- TABLE: spawn_tree_grants
CREATE TABLE public.spawn_tree_grants (
    root_id text NOT NULL,
    parent_id text NOT NULL,
    child_id text NOT NULL,
    budget_usd double precision NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: user_acls
CREATE TABLE public.user_acls (
    user_id uuid NOT NULL,
    scope text NOT NULL,
    granted_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: user_settings
CREATE TABLE public.user_settings (
    user_id uuid NOT NULL,
    version integer DEFAULT 1 NOT NULL,
    data jsonb DEFAULT '{}'::jsonb NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: user_tokens
CREATE TABLE public.user_tokens (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    token_hash text NOT NULL,
    label text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone,
    revoked_at timestamp with time zone,
    token_preview text
);

-- TABLE: users
CREATE TABLE public.users (
    id uuid NOT NULL,
    name text NOT NULL,
    key_hash text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    revoked_at timestamp with time zone,
    can_dispatch boolean DEFAULT true NOT NULL,
    disabled_at timestamp with time zone
);

-- DEFAULT: dispatch_policy_uses id
ALTER TABLE ONLY public.dispatch_policy_uses ALTER COLUMN id SET DEFAULT nextval('public.dispatch_policy_uses_id_seq'::regclass);

-- DEFAULT: events id
ALTER TABLE ONLY public.events ALTER COLUMN id SET DEFAULT nextval('public.events_id_seq'::regclass);

-- DEFAULT: gateway_blocks id
ALTER TABLE ONLY public.gateway_blocks ALTER COLUMN id SET DEFAULT nextval('public.gateway_blocks_id_seq'::regclass);

-- CONSTRAINT: account_pool_members account_pool_members_pkey
ALTER TABLE ONLY public.account_pool_members
    ADD CONSTRAINT account_pool_members_pkey PRIMARY KEY (pool_id, account_id);

-- CONSTRAINT: account_pools account_pools_pkey
ALTER TABLE ONLY public.account_pools
    ADD CONSTRAINT account_pools_pkey PRIMARY KEY (id);

-- CONSTRAINT: account_redirects account_redirects_pkey
ALTER TABLE ONLY public.account_redirects
    ADD CONSTRAINT account_redirects_pkey PRIMARY KEY (id);

-- CONSTRAINT: account_tool_policies account_tool_policies_pkey
ALTER TABLE ONLY public.account_tool_policies
    ADD CONSTRAINT account_tool_policies_pkey PRIMARY KEY (account_id);

-- CONSTRAINT: accounts accounts_pkey
ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_pkey PRIMARY KEY (id);

-- CONSTRAINT: adapters_enabled adapters_enabled_pkey
ALTER TABLE ONLY public.adapters_enabled
    ADD CONSTRAINT adapters_enabled_pkey PRIMARY KEY (machine_id, adapter_id);

-- CONSTRAINT: auth_keys auth_keys_key_hash_key
ALTER TABLE ONLY public.auth_keys
    ADD CONSTRAINT auth_keys_key_hash_key UNIQUE (key_hash);

-- CONSTRAINT: auth_keys auth_keys_pkey
ALTER TABLE ONLY public.auth_keys
    ADD CONSTRAINT auth_keys_pkey PRIMARY KEY (id);

-- CONSTRAINT: dispatch_handles dispatch_handles_pkey
ALTER TABLE ONLY public.dispatch_handles
    ADD CONSTRAINT dispatch_handles_pkey PRIMARY KEY (session_id);

-- CONSTRAINT: dispatch_policies dispatch_policies_pkey
ALTER TABLE ONLY public.dispatch_policies
    ADD CONSTRAINT dispatch_policies_pkey PRIMARY KEY (user_id);

-- CONSTRAINT: dispatch_policy_uses dispatch_policy_uses_pkey
ALTER TABLE ONLY public.dispatch_policy_uses
    ADD CONSTRAINT dispatch_policy_uses_pkey PRIMARY KEY (id);

-- CONSTRAINT: dispatch_record_dismissals dispatch_record_dismissals_pkey
ALTER TABLE ONLY public.dispatch_record_dismissals
    ADD CONSTRAINT dispatch_record_dismissals_pkey PRIMARY KEY (user_id, session_id);

-- CONSTRAINT: dispatch_records dispatch_records_pkey
ALTER TABLE ONLY public.dispatch_records
    ADD CONSTRAINT dispatch_records_pkey PRIMARY KEY (session_id);

-- CONSTRAINT: dispatchers dispatchers_pkey
ALTER TABLE ONLY public.dispatchers
    ADD CONSTRAINT dispatchers_pkey PRIMARY KEY (id);

-- CONSTRAINT: events events_pkey
ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_pkey PRIMARY KEY (id);

-- CONSTRAINT: gateway_blocks gateway_blocks_pkey
ALTER TABLE ONLY public.gateway_blocks
    ADD CONSTRAINT gateway_blocks_pkey PRIMARY KEY (id);

-- CONSTRAINT: instance_settings instance_settings_pkey
ALTER TABLE ONLY public.instance_settings
    ADD CONSTRAINT instance_settings_pkey PRIMARY KEY (key);

-- CONSTRAINT: key_acls key_acls_pkey
ALTER TABLE ONLY public.key_acls
    ADD CONSTRAINT key_acls_pkey PRIMARY KEY (key_id, scope);

-- CONSTRAINT: machines machines_key_hash_key
ALTER TABLE ONLY public.machines
    ADD CONSTRAINT machines_key_hash_key UNIQUE (key_hash);

-- CONSTRAINT: machines machines_pkey
ALTER TABLE ONLY public.machines
    ADD CONSTRAINT machines_pkey PRIMARY KEY (id);

-- CONSTRAINT: account_providers oauth_accounts_pkey
ALTER TABLE ONLY public.account_providers
    ADD CONSTRAINT oauth_accounts_pkey PRIMARY KEY (id);

-- CONSTRAINT: resource_shares resource_shares_pkey
ALTER TABLE ONLY public.resource_shares
    ADD CONSTRAINT resource_shares_pkey PRIMARY KEY (resource_type, resource_id, grantee_id, action);

-- CONSTRAINT: session_account_rebinds session_account_rebinds_pkey
ALTER TABLE ONLY public.session_account_rebinds
    ADD CONSTRAINT session_account_rebinds_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_spawn_capabilities session_spawn_capabilities_pkey
ALTER TABLE ONLY public.session_spawn_capabilities
    ADD CONSTRAINT session_spawn_capabilities_pkey PRIMARY KEY (session_id);

-- CONSTRAINT: session_spawner_intents session_spawner_intents_pkey
ALTER TABLE ONLY public.session_spawner_intents
    ADD CONSTRAINT session_spawner_intents_pkey PRIMARY KEY (spawn_key);

-- CONSTRAINT: session_token_usage session_token_usage_pkey
ALTER TABLE ONLY public.session_token_usage
    ADD CONSTRAINT session_token_usage_pkey PRIMARY KEY (session_id, message_id);

-- CONSTRAINT: session_tokens session_tokens_pkey
ALTER TABLE ONLY public.session_tokens
    ADD CONSTRAINT session_tokens_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_tokens session_tokens_token_hash_key
ALTER TABLE ONLY public.session_tokens
    ADD CONSTRAINT session_tokens_token_hash_key UNIQUE (token_hash);

-- CONSTRAINT: session_usage_totals session_usage_totals_pkey
ALTER TABLE ONLY public.session_usage_totals
    ADD CONSTRAINT session_usage_totals_pkey PRIMARY KEY (session_id, model);

-- CONSTRAINT: session_webhooks session_webhooks_pkey
ALTER TABLE ONLY public.session_webhooks
    ADD CONSTRAINT session_webhooks_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_webhooks session_webhooks_session_id_key
ALTER TABLE ONLY public.session_webhooks
    ADD CONSTRAINT session_webhooks_session_id_key UNIQUE (session_id);

-- CONSTRAINT: sessions sessions_pkey
ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_pkey PRIMARY KEY (id);

-- CONSTRAINT: spawn_tree_grants spawn_tree_grants_pkey
ALTER TABLE ONLY public.spawn_tree_grants
    ADD CONSTRAINT spawn_tree_grants_pkey PRIMARY KEY (child_id);

-- CONSTRAINT: user_acls user_acls_pkey
ALTER TABLE ONLY public.user_acls
    ADD CONSTRAINT user_acls_pkey PRIMARY KEY (user_id, scope);

-- CONSTRAINT: user_settings user_settings_pkey
ALTER TABLE ONLY public.user_settings
    ADD CONSTRAINT user_settings_pkey PRIMARY KEY (user_id);

-- CONSTRAINT: user_tokens user_tokens_pkey
ALTER TABLE ONLY public.user_tokens
    ADD CONSTRAINT user_tokens_pkey PRIMARY KEY (id);

-- CONSTRAINT: user_tokens user_tokens_token_hash_key
ALTER TABLE ONLY public.user_tokens
    ADD CONSTRAINT user_tokens_token_hash_key UNIQUE (token_hash);

-- CONSTRAINT: users users_key_hash_key
ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_key_hash_key UNIQUE (key_hash);

-- CONSTRAINT: users users_pkey
ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);

-- INDEX: account_pool_members_account
CREATE INDEX account_pool_members_account ON public.account_pool_members USING btree (account_id);

-- INDEX: account_pools_user_name
CREATE UNIQUE INDEX account_pools_user_name ON public.account_pools USING btree (user_id, lower(name));

-- INDEX: account_providers_account_family
CREATE UNIQUE INDEX account_providers_account_family ON public.account_providers USING btree (account_id, family);

-- INDEX: account_redirects_by_user
CREATE INDEX account_redirects_by_user ON public.account_redirects USING btree (user_id);

-- INDEX: account_redirects_uniq
CREATE UNIQUE INDEX account_redirects_uniq ON public.account_redirects USING btree (user_id, from_account, family, COALESCE(match_model, ''::text));

-- INDEX: accounts_user_name
CREATE UNIQUE INDEX accounts_user_name ON public.accounts USING btree (user_id, name);

-- INDEX: dispatch_policy_uses_by_session
CREATE INDEX dispatch_policy_uses_by_session ON public.dispatch_policy_uses USING btree (session_id);

-- INDEX: dispatch_policy_uses_by_user_day
CREATE INDEX dispatch_policy_uses_by_user_day ON public.dispatch_policy_uses USING btree (user_id, dispatched_at);

-- INDEX: dispatch_records_by_user_name
CREATE INDEX dispatch_records_by_user_name ON public.dispatch_records USING btree (user_id, name, created_at);

-- INDEX: dispatch_records_by_user_time
CREATE INDEX dispatch_records_by_user_time ON public.dispatch_records USING btree (user_id, created_at);

-- INDEX: dispatchers_key_hash_uniq
CREATE UNIQUE INDEX dispatchers_key_hash_uniq ON public.dispatchers USING btree (key_hash) WHERE (key_hash IS NOT NULL);

-- INDEX: dispatchers_name_live
CREATE UNIQUE INDEX dispatchers_name_live ON public.dispatchers USING btree (name) WHERE (deleted_at IS NULL);

-- INDEX: events_kind_idx
CREATE INDEX events_kind_idx ON public.events USING btree (kind, occurred_at DESC, id DESC);

-- INDEX: events_machine_idx
CREATE INDEX events_machine_idx ON public.events USING btree (machine_id, occurred_at DESC, id DESC);

-- INDEX: events_occurred_at_idx
CREATE INDEX events_occurred_at_idx ON public.events USING btree (occurred_at DESC, id DESC);

-- INDEX: events_session_idx
CREATE INDEX events_session_idx ON public.events USING btree (session_id, occurred_at DESC, id DESC);

-- INDEX: gateway_blocks_account
CREATE INDEX gateway_blocks_account ON public.gateway_blocks USING btree (account_id, created_at DESC);

-- INDEX: gateway_blocks_session
CREATE INDEX gateway_blocks_session ON public.gateway_blocks USING btree (session_id, created_at DESC);

-- INDEX: idx_auth_keys_dispatcher
CREATE INDEX idx_auth_keys_dispatcher ON public.auth_keys USING btree (dispatcher_id) WHERE (dispatcher_id IS NOT NULL);

-- INDEX: idx_auth_keys_ephemeral_expiry
CREATE INDEX idx_auth_keys_ephemeral_expiry ON public.auth_keys USING btree (expires_at) WHERE (kind = 'ephemeral'::text);

-- INDEX: idx_auth_keys_machine
CREATE INDEX idx_auth_keys_machine ON public.auth_keys USING btree (machine_id) WHERE (machine_id IS NOT NULL);

-- INDEX: idx_auth_keys_passkey
CREATE INDEX idx_auth_keys_passkey ON public.auth_keys USING btree (passkey_id) WHERE (passkey_id IS NOT NULL);

-- INDEX: idx_auth_keys_session
CREATE INDEX idx_auth_keys_session ON public.auth_keys USING btree (session_id) WHERE (session_id IS NOT NULL);

-- INDEX: idx_auth_keys_user
CREATE INDEX idx_auth_keys_user ON public.auth_keys USING btree (user_id);

-- INDEX: idx_machines_deleted_at
CREATE INDEX idx_machines_deleted_at ON public.machines USING btree (deleted_at);

-- INDEX: idx_machines_kind
CREATE INDEX idx_machines_kind ON public.machines USING btree (kind) WHERE (kind <> 'persistent'::text);

-- INDEX: idx_machines_user
CREATE INDEX idx_machines_user ON public.machines USING btree (user_id);

-- INDEX: idx_resource_shares_grantee
CREATE INDEX idx_resource_shares_grantee ON public.resource_shares USING btree (grantee_id) WHERE (revoked_at IS NULL);

-- INDEX: idx_resource_shares_resource
CREATE INDEX idx_resource_shares_resource ON public.resource_shares USING btree (resource_type, resource_id) WHERE (revoked_at IS NULL);

-- INDEX: idx_session_token_usage_created_covering
CREATE INDEX idx_session_token_usage_created_covering ON public.session_token_usage USING btree (created_at) INCLUDE (session_id, input_tokens, output_tokens, cache_read_tokens);

-- INDEX: idx_session_token_usage_session_created
CREATE INDEX idx_session_token_usage_session_created ON public.session_token_usage USING btree (session_id, created_at) INCLUDE (model, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens);

-- INDEX: idx_session_tokens_account_id
CREATE INDEX idx_session_tokens_account_id ON public.session_tokens USING btree (account_id, session_id);

-- INDEX: idx_session_webhooks_pending
CREATE INDEX idx_session_webhooks_pending ON public.session_webhooks USING btree (state, next_attempt_at) WHERE (state = 'pending'::text);

-- INDEX: idx_sessions_adapter
CREATE INDEX idx_sessions_adapter ON public.sessions USING btree (adapter_id);

-- INDEX: idx_sessions_keepalive_due
CREATE INDEX idx_sessions_keepalive_due ON public.sessions USING btree (last_keepalive_at) WHERE (keepalive_json IS NOT NULL);

-- INDEX: idx_sessions_live
CREATE INDEX idx_sessions_live ON public.sessions USING btree (status, registered_at DESC) WHERE (status <> 'archived'::text);

-- INDEX: idx_sessions_machine
CREATE INDEX idx_sessions_machine ON public.sessions USING btree (machine_id);

-- INDEX: idx_sessions_machine_uuid
CREATE INDEX idx_sessions_machine_uuid ON public.sessions USING btree (machine_uuid);

-- INDEX: idx_sessions_origin
CREATE INDEX idx_sessions_origin ON public.sessions USING btree (origin) WHERE (origin IS NOT NULL);

-- INDEX: idx_sessions_parent
CREATE INDEX idx_sessions_parent ON public.sessions USING btree (parent_id);

-- INDEX: idx_sessions_registered_at_desc
CREATE INDEX idx_sessions_registered_at_desc ON public.sessions USING btree (registered_at DESC);

-- INDEX: idx_sessions_status
CREATE INDEX idx_sessions_status ON public.sessions USING btree (status);

-- INDEX: idx_sessions_status_registered_at
CREATE INDEX idx_sessions_status_registered_at ON public.sessions USING btree (status, registered_at DESC);

-- INDEX: idx_sessions_user
CREATE INDEX idx_sessions_user ON public.sessions USING btree (user_id);

-- INDEX: idx_user_tokens_user_active
CREATE INDEX idx_user_tokens_user_active ON public.user_tokens USING btree (user_id) WHERE (revoked_at IS NULL);

-- INDEX: oauth_accounts_managed_user_provider
CREATE UNIQUE INDEX oauth_accounts_managed_user_provider ON public.account_providers USING btree (user_id, provider) WHERE managed;

-- INDEX: session_account_rebinds_session
CREATE INDEX session_account_rebinds_session ON public.session_account_rebinds USING btree (session_id, created_at DESC);

-- INDEX: session_token_usage_created_idx
CREATE INDEX session_token_usage_created_idx ON public.session_token_usage USING btree (created_at);

-- INDEX: session_token_usage_session_idx
CREATE INDEX session_token_usage_session_idx ON public.session_token_usage USING btree (session_id);

-- INDEX: session_tokens_expires_at
CREATE INDEX session_tokens_expires_at ON public.session_tokens USING btree (expires_at) WHERE (revoked_at IS NULL);

-- INDEX: session_tokens_service_by_account
CREATE INDEX session_tokens_service_by_account ON public.session_tokens USING btree (account_id, created_at) WHERE (kind = 'service'::text);

-- INDEX: session_tokens_session
CREATE INDEX session_tokens_session ON public.session_tokens USING btree (session_id);

-- INDEX: session_tokens_session_live
CREATE INDEX session_tokens_session_live ON public.session_tokens USING btree (session_id) WHERE (revoked_at IS NULL);

-- INDEX: session_tokens_user_live
CREATE INDEX session_tokens_user_live ON public.session_tokens USING btree (user_id) WHERE (revoked_at IS NULL);

-- INDEX: sessions_room
CREATE INDEX sessions_room ON public.sessions USING btree (room_id) WHERE (room_id IS NOT NULL);

-- INDEX: spawn_tree_grants_parent_idx
CREATE INDEX spawn_tree_grants_parent_idx ON public.spawn_tree_grants USING btree (parent_id);

-- INDEX: spawn_tree_grants_root_idx
CREATE INDEX spawn_tree_grants_root_idx ON public.spawn_tree_grants USING btree (root_id);

-- INDEX: uniq_machines_dispatch_per_user
CREATE UNIQUE INDEX uniq_machines_dispatch_per_user ON public.machines USING btree (user_id) WHERE ((kind = 'dispatch'::text) AND (deleted_at IS NULL));

-- TRIGGER: session_token_usage session_usage_totals_apply
CREATE TRIGGER session_usage_totals_apply AFTER INSERT OR DELETE OR UPDATE ON public.session_token_usage FOR EACH ROW EXECUTE FUNCTION public.session_usage_totals_apply();

-- FK CONSTRAINT: account_pool_members account_pool_members_account_id_fkey
ALTER TABLE ONLY public.account_pool_members
    ADD CONSTRAINT account_pool_members_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_pool_members account_pool_members_pool_id_fkey
ALTER TABLE ONLY public.account_pool_members
    ADD CONSTRAINT account_pool_members_pool_id_fkey FOREIGN KEY (pool_id) REFERENCES public.account_pools(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_pools account_pools_user_id_fkey
ALTER TABLE ONLY public.account_pools
    ADD CONSTRAINT account_pools_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_providers account_providers_account_id_fkey
ALTER TABLE ONLY public.account_providers
    ADD CONSTRAINT account_providers_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_redirects account_redirects_from_account_fkey
ALTER TABLE ONLY public.account_redirects
    ADD CONSTRAINT account_redirects_from_account_fkey FOREIGN KEY (from_account) REFERENCES public.accounts(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_redirects account_redirects_to_account_fkey
ALTER TABLE ONLY public.account_redirects
    ADD CONSTRAINT account_redirects_to_account_fkey FOREIGN KEY (to_account) REFERENCES public.accounts(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_redirects account_redirects_user_id_fkey
ALTER TABLE ONLY public.account_redirects
    ADD CONSTRAINT account_redirects_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_tool_policies account_tool_policies_account_id_fkey
ALTER TABLE ONLY public.account_tool_policies
    ADD CONSTRAINT account_tool_policies_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;

-- FK CONSTRAINT: accounts accounts_user_id_fkey
ALTER TABLE ONLY public.accounts
    ADD CONSTRAINT accounts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: adapters_enabled adapters_enabled_machine_id_fkey
ALTER TABLE ONLY public.adapters_enabled
    ADD CONSTRAINT adapters_enabled_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES public.machines(id) ON DELETE CASCADE;

-- FK CONSTRAINT: auth_keys auth_keys_dispatcher_id_fkey
ALTER TABLE ONLY public.auth_keys
    ADD CONSTRAINT auth_keys_dispatcher_id_fkey FOREIGN KEY (dispatcher_id) REFERENCES public.dispatchers(id) ON DELETE CASCADE;

-- FK CONSTRAINT: auth_keys auth_keys_machine_id_fkey
ALTER TABLE ONLY public.auth_keys
    ADD CONSTRAINT auth_keys_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES public.machines(id) ON DELETE CASCADE;

-- FK CONSTRAINT: auth_keys auth_keys_user_id_fkey
ALTER TABLE ONLY public.auth_keys
    ADD CONSTRAINT auth_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: dispatch_policies dispatch_policies_user_id_fkey
ALTER TABLE ONLY public.dispatch_policies
    ADD CONSTRAINT dispatch_policies_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: dispatch_policy_uses dispatch_policy_uses_user_id_fkey
ALTER TABLE ONLY public.dispatch_policy_uses
    ADD CONSTRAINT dispatch_policy_uses_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: dispatch_record_dismissals dispatch_record_dismissals_session_id_fkey
ALTER TABLE ONLY public.dispatch_record_dismissals
    ADD CONSTRAINT dispatch_record_dismissals_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.dispatch_records(session_id) ON DELETE CASCADE;

-- FK CONSTRAINT: dispatch_record_dismissals dispatch_record_dismissals_user_id_fkey
ALTER TABLE ONLY public.dispatch_record_dismissals
    ADD CONSTRAINT dispatch_record_dismissals_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: dispatch_records dispatch_records_user_id_fkey
ALTER TABLE ONLY public.dispatch_records
    ADD CONSTRAINT dispatch_records_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: dispatchers dispatchers_default_account_id_fkey
ALTER TABLE ONLY public.dispatchers
    ADD CONSTRAINT dispatchers_default_account_id_fkey FOREIGN KEY (default_account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;

-- FK CONSTRAINT: dispatchers dispatchers_default_pool_id_fkey
ALTER TABLE ONLY public.dispatchers
    ADD CONSTRAINT dispatchers_default_pool_id_fkey FOREIGN KEY (default_pool_id) REFERENCES public.account_pools(id) ON DELETE SET NULL;

-- FK CONSTRAINT: dispatchers dispatchers_user_id_fkey
ALTER TABLE ONLY public.dispatchers
    ADD CONSTRAINT dispatchers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: events events_machine_id_fkey
ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES public.machines(id) ON DELETE SET NULL;

-- FK CONSTRAINT: events events_session_id_fkey
ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE SET NULL;

-- FK CONSTRAINT: events events_user_id_fkey
ALTER TABLE ONLY public.events
    ADD CONSTRAINT events_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

-- FK CONSTRAINT: gateway_blocks gateway_blocks_account_id_fkey
ALTER TABLE ONLY public.gateway_blocks
    ADD CONSTRAINT gateway_blocks_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE CASCADE;

-- FK CONSTRAINT: key_acls key_acls_key_id_fkey
ALTER TABLE ONLY public.key_acls
    ADD CONSTRAINT key_acls_key_id_fkey FOREIGN KEY (key_id) REFERENCES public.auth_keys(id) ON DELETE CASCADE;

-- FK CONSTRAINT: machines machines_user_id_fkey
ALTER TABLE ONLY public.machines
    ADD CONSTRAINT machines_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_providers oauth_accounts_user_id_fkey
ALTER TABLE ONLY public.account_providers
    ADD CONSTRAINT oauth_accounts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: resource_shares resource_shares_grantee_id_fkey
ALTER TABLE ONLY public.resource_shares
    ADD CONSTRAINT resource_shares_grantee_id_fkey FOREIGN KEY (grantee_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_account_rebinds session_account_rebinds_pool_id_fkey
ALTER TABLE ONLY public.session_account_rebinds
    ADD CONSTRAINT session_account_rebinds_pool_id_fkey FOREIGN KEY (pool_id) REFERENCES public.account_pools(id) ON DELETE SET NULL;

-- FK CONSTRAINT: session_spawner_intents session_spawner_intents_user_id_fkey
ALTER TABLE ONLY public.session_spawner_intents
    ADD CONSTRAINT session_spawner_intents_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_token_usage session_token_usage_session_id_fkey
ALTER TABLE ONLY public.session_token_usage
    ADD CONSTRAINT session_token_usage_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_tokens session_tokens_account_id_fkey
ALTER TABLE ONLY public.session_tokens
    ADD CONSTRAINT session_tokens_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.account_providers(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_tokens session_tokens_pool_id_fkey
ALTER TABLE ONLY public.session_tokens
    ADD CONSTRAINT session_tokens_pool_id_fkey FOREIGN KEY (pool_id) REFERENCES public.account_pools(id) ON DELETE SET NULL;

-- FK CONSTRAINT: session_tokens session_tokens_user_id_fkey
ALTER TABLE ONLY public.session_tokens
    ADD CONSTRAINT session_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_usage_totals session_usage_totals_session_id_fkey
ALTER TABLE ONLY public.session_usage_totals
    ADD CONSTRAINT session_usage_totals_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_webhooks session_webhooks_user_id_fkey
ALTER TABLE ONLY public.session_webhooks
    ADD CONSTRAINT session_webhooks_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: sessions sessions_machine_uuid_fkey
ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_machine_uuid_fkey FOREIGN KEY (machine_uuid) REFERENCES public.machines(id);

-- FK CONSTRAINT: sessions sessions_parent_id_fkey
ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_parent_id_fkey FOREIGN KEY (parent_id) REFERENCES public.sessions(id) ON DELETE SET NULL;

-- FK CONSTRAINT: sessions sessions_spawned_by_fkey
ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_spawned_by_fkey FOREIGN KEY (spawned_by) REFERENCES public.users(id) ON DELETE SET NULL;

-- FK CONSTRAINT: sessions sessions_user_id_fkey
ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id);

-- FK CONSTRAINT: user_acls user_acls_user_id_fkey
ALTER TABLE ONLY public.user_acls
    ADD CONSTRAINT user_acls_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: user_settings user_settings_user_id_fkey
ALTER TABLE ONLY public.user_settings
    ADD CONSTRAINT user_settings_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: user_tokens user_tokens_user_id_fkey
ALTER TABLE ONLY public.user_tokens
    ADD CONSTRAINT user_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
