-- EXTENSION: btree_gin
CREATE EXTENSION IF NOT EXISTS btree_gin WITH SCHEMA public;

-- EXTENSION: pg_trgm
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;

-- EXTENSION: pgcrypto
CREATE EXTENSION IF NOT EXISTS pgcrypto WITH SCHEMA public;

-- FUNCTION: delete_session_attachments()
CREATE FUNCTION public.delete_session_attachments() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    DELETE FROM session_attachments WHERE session_id = OLD.id;
    RETURN OLD;
END;
$$;

-- TABLE: account_limit_resets
CREATE TABLE public.account_limit_resets (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    provider_id uuid NOT NULL,
    idempotency_key text NOT NULL,
    credit_id text,
    outcome text NOT NULL,
    requested_by uuid,
    at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: account_usage_samples
CREATE TABLE public.account_usage_samples (
    provider_id uuid NOT NULL,
    window_key text NOT NULL,
    utilization double precision NOT NULL,
    resets_at timestamp with time zone,
    sampled_at timestamp with time zone DEFAULT now() NOT NULL,
    amount_usd double precision,
    source text DEFAULT 'poll'::text NOT NULL
);

-- TABLE: account_window_closes
CREATE TABLE public.account_window_closes (
    provider_id uuid NOT NULL,
    window_key text NOT NULL,
    resets_at timestamp with time zone NOT NULL,
    final_utilization double precision NOT NULL,
    wasted_pct double precision NOT NULL,
    closed_at timestamp with time zone DEFAULT now() NOT NULL,
    source text DEFAULT 'sample'::text NOT NULL
);

-- TABLE: api_keys
CREATE TABLE public.api_keys (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    provider text DEFAULT 'anthropic'::text NOT NULL,
    encrypted_key text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    user_id uuid
);

-- TABLE: archive_index
CREATE TABLE public.archive_index (
    id bigint NOT NULL,
    machine_id uuid NOT NULL,
    project_dir text NOT NULL,
    session_id text NOT NULL,
    sha256 text NOT NULL,
    size_bytes bigint NOT NULL,
    line_count integer,
    uploaded_at timestamp with time zone DEFAULT now() NOT NULL,
    first_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    source text DEFAULT 'synced'::text NOT NULL
);

-- SEQUENCE: archive_index_id_seq
CREATE SEQUENCE public.archive_index_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: archive_index_id_seq
ALTER SEQUENCE public.archive_index_id_seq OWNED BY public.archive_index.id;

-- TABLE: archive_manifest
CREATE TABLE public.archive_manifest (
    machine_id uuid NOT NULL,
    project_dir text NOT NULL,
    session_id text NOT NULL,
    size_bytes bigint NOT NULL,
    mtime timestamp with time zone NOT NULL,
    reported_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: bookmarks
CREATE TABLE public.bookmarks (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    session_id text,
    seq bigint,
    message_id text,
    title text NOT NULL,
    body text NOT NULL,
    role text NOT NULL,
    session_name text,
    note text,
    message_ts timestamp with time zone NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: cctuiverse_links
CREATE TABLE public.cctuiverse_links (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    session_id text,
    room_id uuid,
    kind text NOT NULL,
    role text NOT NULL,
    state text NOT NULL,
    label text NOT NULL,
    peer_label text,
    peer_room_name text,
    public_key bytea NOT NULL,
    encrypted_private_key text,
    peer_public_key bytea,
    peer_link_id uuid,
    peer_url text,
    invite_token_hash bytea,
    invite_expires_at timestamp with time zone,
    envelope_nonce text NOT NULL,
    settings jsonb DEFAULT '{}'::jsonb NOT NULL,
    sent_count integer DEFAULT 0 NOT NULL,
    preamble_pending boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    activated_at timestamp with time zone,
    closed_at timestamp with time zone,
    peer_404_count integer DEFAULT 0 NOT NULL,
    peer_404_since timestamp with time zone,
    CONSTRAINT cctuiverse_links_check CHECK ((((kind = 'session'::text) AND (session_id IS NOT NULL)) OR ((kind = 'room'::text) AND ((room_id IS NOT NULL) OR (session_id IS NOT NULL))))),
    CONSTRAINT cctuiverse_links_kind_check CHECK ((kind = ANY (ARRAY['session'::text, 'room'::text]))),
    CONSTRAINT cctuiverse_links_role_check CHECK ((role = ANY (ARRAY['inviter'::text, 'joiner'::text]))),
    CONSTRAINT cctuiverse_links_state_check CHECK ((state = ANY (ARRAY['pending'::text, 'active'::text, 'closed'::text])))
);

-- TABLE: cctuiverse_messages
CREATE TABLE public.cctuiverse_messages (
    id bigint NOT NULL,
    link_id uuid NOT NULL,
    message_id uuid NOT NULL,
    direction text NOT NULL,
    kind text NOT NULL,
    body jsonb NOT NULL,
    status text NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone,
    first_queued_at timestamp with time zone,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    delivered_at timestamp with time zone,
    CONSTRAINT cctuiverse_messages_direction_check CHECK ((direction = ANY (ARRAY['in'::text, 'out'::text]))),
    CONSTRAINT cctuiverse_messages_kind_check CHECK ((kind = ANY (ARRAY['direct'::text, 'room_post'::text, 'close'::text]))),
    CONSTRAINT cctuiverse_messages_status_check CHECK ((status = ANY (ARRAY['review'::text, 'queued'::text, 'delivering'::text, 'delivered'::text, 'held'::text, 'released'::text, 'dropped'::text, 'failed'::text])))
);

-- SEQUENCE: cctuiverse_messages_id_seq
CREATE SEQUENCE public.cctuiverse_messages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: cctuiverse_messages_id_seq
ALTER SEQUENCE public.cctuiverse_messages_id_seq OWNED BY public.cctuiverse_messages.id;

-- TABLE: cctuiverse_nonces
CREATE TABLE public.cctuiverse_nonces (
    link_id uuid NOT NULL,
    nonce text NOT NULL,
    seen_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: cluster_secrets
CREATE TABLE public.cluster_secrets (
    name text NOT NULL,
    value text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: codex_account_model_catalogs
CREATE TABLE public.codex_account_model_catalogs (
    provider_id uuid NOT NULL,
    catalog jsonb NOT NULL,
    fetched_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: codex_model_catalogs
CREATE TABLE public.codex_model_catalogs (
    machine_id uuid NOT NULL,
    catalog jsonb NOT NULL,
    fetched_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: commands
CREATE TABLE public.commands (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    session_id text NOT NULL,
    kind text NOT NULL,
    payload jsonb DEFAULT '{}'::jsonb NOT NULL,
    status text DEFAULT 'pending'::text NOT NULL,
    issued_at timestamp with time zone DEFAULT now() NOT NULL,
    completed_at timestamp with time zone
);

-- TABLE: context_items
CREATE TABLE public.context_items (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    kind text NOT NULL,
    name text NOT NULL,
    title text NOT NULL,
    body text DEFAULT ''::text NOT NULL,
    scope text DEFAULT 'user'::text NOT NULL,
    scope_ref text,
    tags text[] DEFAULT '{}'::text[] NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    version integer DEFAULT 1 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    filters jsonb DEFAULT '{}'::jsonb NOT NULL,
    pending jsonb,
    created_by_session text,
    updated_by_session text,
    CONSTRAINT context_items_check CHECK (((scope = 'user'::text) = (scope_ref IS NULL))),
    CONSTRAINT context_items_kind_check CHECK ((kind = ANY (ARRAY['memory'::text, 'prompt'::text, 'rule'::text]))),
    CONSTRAINT context_items_scope_check CHECK ((scope = ANY (ARRAY['user'::text, 'machine'::text, 'path'::text, 'label'::text])))
);

-- TABLE: context_tool_saves
CREATE TABLE public.context_tool_saves (
    id bigint NOT NULL,
    session_id text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- SEQUENCE: context_tool_saves_id_seq
CREATE SEQUENCE public.context_tool_saves_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: context_tool_saves_id_seq
ALTER SEQUENCE public.context_tool_saves_id_seq OWNED BY public.context_tool_saves.id;

-- TABLE: daemon_blobs
CREATE TABLE public.daemon_blobs (
    hash text NOT NULL,
    media_type text,
    byte_len bigint NOT NULL,
    bytes bytea NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    file_stored boolean DEFAULT false NOT NULL
);

-- TABLE: device_auth_requests
CREATE TABLE public.device_auth_requests (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    device_code_hash text NOT NULL,
    user_code text NOT NULL,
    client_name text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    last_polled_at timestamp with time zone,
    approved_at timestamp with time zone,
    approved_by uuid,
    denied_at timestamp with time zone,
    claimed_at timestamp with time zone,
    client_ip text,
    user_agent text
);

-- TABLE: dispatched_turns
CREATE TABLE public.dispatched_turns (
    session_id text NOT NULL,
    turn_id uuid NOT NULL,
    command_id uuid,
    claimed_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: draft_launch_queue
CREATE TABLE public.draft_launch_queue (
    draft_id text NOT NULL,
    user_id uuid,
    state text DEFAULT 'scheduled'::text NOT NULL,
    launch_at timestamp with time zone NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone NOT NULL,
    last_error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    launched_at timestamp with time zone,
    CONSTRAINT draft_launch_queue_state_check CHECK ((state = ANY (ARRAY['scheduled'::text, 'launching'::text, 'launched'::text, 'dead'::text])))
);

-- TABLE: guard_steps
CREATE TABLE public.guard_steps (
    session_id text NOT NULL,
    state jsonb NOT NULL,
    reported_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: image_blob_backfill
CREATE TABLE public.image_blob_backfill (
    name text NOT NULL,
    last_event_id bigint NOT NULL,
    rows_rewritten bigint DEFAULT 0 NOT NULL,
    blob_bytes bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    images_valid bigint DEFAULT 0 NOT NULL,
    images_corrupt bigint DEFAULT 0 NOT NULL,
    images_recovered bigint DEFAULT 0 NOT NULL
);

-- TABLE: labels
CREATE TABLE public.labels (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    color text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    user_id uuid NOT NULL
);

-- TABLE: machine_bandwidth
CREATE TABLE public.machine_bandwidth (
    machine_id uuid NOT NULL,
    forward bigint DEFAULT 0 NOT NULL,
    retransmit bigint DEFAULT 0 NOT NULL,
    backfill bigint DEFAULT 0 NOT NULL,
    self_update bigint DEFAULT 0 NOT NULL,
    blob_put bigint DEFAULT 0 NOT NULL,
    heartbeat bigint DEFAULT 0 NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: machine_resources
CREATE TABLE public.machine_resources (
    machine_id uuid NOT NULL,
    cpu_pct real DEFAULT 0 NOT NULL,
    mem_pct real DEFAULT 0 NOT NULL,
    mem_used_bytes bigint DEFAULT 0 NOT NULL,
    mem_total_bytes bigint DEFAULT 0 NOT NULL,
    disk_pct real DEFAULT 0 NOT NULL,
    disk_used_bytes bigint DEFAULT 0 NOT NULL,
    disk_total_bytes bigint DEFAULT 0 NOT NULL,
    disk_path text DEFAULT ''::text NOT NULL,
    load1 real,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: plugin_settings
CREATE TABLE public.plugin_settings (
    plugin_id text NOT NULL,
    setting_values jsonb DEFAULT '{}'::jsonb NOT NULL,
    proxy_secret text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: plugins
CREATE TABLE public.plugins (
    id text NOT NULL,
    version text NOT NULL,
    manifest jsonb NOT NULL,
    archive_hash text NOT NULL,
    enabled boolean DEFAULT false NOT NULL,
    installed_by uuid,
    installed_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: pods
CREATE TABLE public.pods (
    pod text NOT NULL,
    pod_ip text NOT NULL,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    heartbeat_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: preview_tickets_used
CREATE TABLE public.preview_tickets_used (
    nonce text NOT NULL,
    expires_at timestamp with time zone NOT NULL
);

-- TABLE: previews
CREATE TABLE public.previews (
    id text NOT NULL,
    session_id text NOT NULL,
    user_id uuid NOT NULL,
    machine_id uuid NOT NULL,
    port integer NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    detached_at timestamp with time zone
);

-- TABLE: privacy_scan_jobs
CREATE TABLE public.privacy_scan_jobs (
    id uuid NOT NULL,
    user_id uuid NOT NULL,
    dry_run boolean NOT NULL,
    session_ids text[],
    since timestamp with time zone,
    status text NOT NULL,
    cancel_requested boolean DEFAULT false NOT NULL,
    rows_total bigint,
    rows_scanned bigint DEFAULT 0 NOT NULL,
    rows_changed bigint DEFAULT 0 NOT NULL,
    substitutions bigint DEFAULT 0 NOT NULL,
    by_category jsonb DEFAULT '{}'::jsonb NOT NULL,
    samples jsonb DEFAULT '[]'::jsonb NOT NULL,
    error text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    finished_at timestamp with time zone,
    CONSTRAINT privacy_scan_jobs_status_check CHECK ((status = ANY (ARRAY['running'::text, 'completed'::text, 'cancelled'::text, 'failed'::text])))
);

-- TABLE: prompts
CREATE TABLE public.prompts (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    name text NOT NULL,
    content text NOT NULL,
    description text,
    tags text[] DEFAULT '{}'::text[],
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    scope_owner text,
    scope_repo text,
    kind text DEFAULT 'general'::text NOT NULL,
    user_id uuid,
    CONSTRAINT prompts_repo_requires_owner CHECK (((scope_repo IS NULL) OR (scope_owner IS NOT NULL)))
);

-- TABLE: room_messages
CREATE TABLE public.room_messages (
    id bigint NOT NULL,
    room_id uuid NOT NULL,
    seq bigint NOT NULL,
    sender_session_id text,
    sender_label text NOT NULL,
    body text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- SEQUENCE: room_messages_id_seq
CREATE SEQUENCE public.room_messages_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: room_messages_id_seq
ALTER SEQUENCE public.room_messages_id_seq OWNED BY public.room_messages.id;

-- TABLE: rooms
CREATE TABLE public.rooms (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    next_seq bigint DEFAULT 0 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    archived_at timestamp with time zone
);

-- TABLE: search_chunks
CREATE TABLE public.search_chunks (
    event_id bigint NOT NULL,
    ord integer NOT NULL,
    session_id text NOT NULL,
    body text NOT NULL
);

-- TABLE: search_index_state
CREATE TABLE public.search_index_state (
    name text NOT NULL,
    last_event_id bigint NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: self_update_runs
CREATE TABLE public.self_update_runs (
    id uuid NOT NULL,
    machine_id uuid NOT NULL,
    version text NOT NULL,
    from_version text NOT NULL,
    phase text NOT NULL,
    exit_code integer,
    detail text DEFAULT ''::text NOT NULL,
    output_tail text,
    started_by uuid,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_attachments
CREATE TABLE public.session_attachments (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    session_id text NOT NULL,
    message_id text,
    name text NOT NULL,
    hash text NOT NULL,
    size bigint NOT NULL,
    content_type text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_auto_archive
CREATE TABLE public.session_auto_archive (
    spawn_key text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_auto_resume
CREATE TABLE public.session_auto_resume (
    session_id text NOT NULL,
    error_event_id bigint NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    state text DEFAULT 'pending'::text NOT NULL,
    next_attempt_at timestamp with time zone DEFAULT now() NOT NULL,
    last_error text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_blob_links
CREATE TABLE public.session_blob_links (
    session_id text NOT NULL,
    hash text NOT NULL,
    first_seen_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_context
CREATE TABLE public.session_context (
    spawn_key text NOT NULL,
    item_ids uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    scope jsonb,
    rules jsonb DEFAULT '[]'::jsonb NOT NULL,
    memories jsonb DEFAULT '[]'::jsonb NOT NULL
);

-- TABLE: session_file_links
CREATE TABLE public.session_file_links (
    session_id text NOT NULL,
    path text NOT NULL,
    first_seen_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_followup_intents
CREATE TABLE public.session_followup_intents (
    spawn_key text NOT NULL,
    parent_session_id text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_images
CREATE TABLE public.session_images (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    session_id text NOT NULL,
    sha256 text NOT NULL,
    media_type text NOT NULL,
    byte_len integer NOT NULL,
    bytes bytea NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_label_intents
CREATE TABLE public.session_label_intents (
    spawn_key text NOT NULL,
    label_ids uuid[] NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_labels
CREATE TABLE public.session_labels (
    session_id text NOT NULL,
    label_id uuid NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_message_pins
CREATE TABLE public.session_message_pins (
    id bigint NOT NULL,
    user_id uuid NOT NULL,
    session_id text NOT NULL,
    seq bigint NOT NULL,
    message_id text,
    note text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- SEQUENCE: session_message_pins_id_seq
CREATE SEQUENCE public.session_message_pins_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: session_message_pins_id_seq
ALTER SEQUENCE public.session_message_pins_id_seq OWNED BY public.session_message_pins.id;

-- TABLE: session_message_queue
CREATE TABLE public.session_message_queue (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    session_id text NOT NULL,
    user_id uuid,
    body text NOT NULL,
    ask_picks jsonb,
    state text DEFAULT 'scheduled'::text NOT NULL,
    deliver_at timestamp with time zone NOT NULL,
    attempts integer DEFAULT 0 NOT NULL,
    next_attempt_at timestamp with time zone NOT NULL,
    last_error text,
    turn_id uuid DEFAULT gen_random_uuid() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    sent_at timestamp with time zone,
    origin text DEFAULT 'human'::text NOT NULL,
    CONSTRAINT session_message_queue_origin_check CHECK ((origin = ANY (ARRAY['human'::text, 'macro'::text]))),
    CONSTRAINT session_message_queue_state_check CHECK ((state = ANY (ARRAY['scheduled'::text, 'sending'::text, 'sent'::text, 'cancelled'::text, 'dead'::text])))
);

-- TABLE: session_profiles
CREATE TABLE public.session_profiles (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    name text NOT NULL,
    harness text DEFAULT 'claude-code'::text NOT NULL,
    account_id uuid,
    pool_id uuid,
    no_account boolean DEFAULT false NOT NULL,
    model_alias text,
    effort text,
    permission_mode text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    sort_order integer DEFAULT 0 NOT NULL,
    service_tier text,
    context_items uuid[] DEFAULT '{}'::uuid[] NOT NULL,
    CONSTRAINT session_profiles_check CHECK (((num_nonnulls(account_id, pool_id) + (no_account)::integer) <= 1))
);

-- TABLE: session_reads
CREATE TABLE public.session_reads (
    user_id uuid NOT NULL,
    session_id text NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_transcript
CREATE TABLE public.session_transcript (
    id bigint NOT NULL,
    session_id text NOT NULL,
    raw_json text NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- SEQUENCE: session_transcript_id_seq
CREATE SEQUENCE public.session_transcript_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: session_transcript_id_seq
ALTER SEQUENCE public.session_transcript_id_seq OWNED BY public.session_transcript.id;

-- TABLE: session_user_actions
CREATE TABLE public.session_user_actions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    session_id text NOT NULL,
    title text NOT NULL,
    detail text,
    kind text DEFAULT 'action'::text NOT NULL,
    blocking boolean DEFAULT false NOT NULL,
    status text DEFAULT 'open'::text NOT NULL,
    note text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    resolved_at timestamp with time zone,
    resolved_by text
);

-- TABLE: session_viewers
CREATE TABLE public.session_viewers (
    session_id text NOT NULL,
    pod text NOT NULL,
    seen_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: session_voice_notes
CREATE TABLE public.session_voice_notes (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    session_id text NOT NULL,
    media_type text NOT NULL,
    byte_len integer NOT NULL,
    duration_ms integer,
    text text NOT NULL,
    bytes bytea NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: skill_registry
CREATE TABLE public.skill_registry (
    name text NOT NULL,
    version text NOT NULL,
    sha256 text NOT NULL,
    size_bytes bigint NOT NULL,
    uploaded_by_machine uuid,
    uploaded_by_user uuid NOT NULL,
    uploaded_at timestamp with time zone DEFAULT now() NOT NULL,
    content_type text DEFAULT 'application/zstd'::text NOT NULL
);

-- TABLE: stream_events
CREATE TABLE public.stream_events (
    id bigint NOT NULL,
    session_id text NOT NULL,
    event_type text NOT NULL,
    payload jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    content_hash bytea GENERATED ALWAYS AS (public.digest((payload)::text, 'sha256'::text)) STORED,
    search_text text GENERATED ALWAYS AS (((((((COALESCE((payload ->> 'text'::text), ''::text) || ' '::text) || COALESCE((payload ->> 'content'::text), ''::text)) || ' '::text) || COALESCE((payload ->> 'tool'::text), ''::text)) || ' '::text) || COALESCE(((payload -> 'input'::text))::text, ''::text))) STORED,
    turn_id uuid
)
WITH (autovacuum_analyze_scale_factor='0.02', autovacuum_vacuum_scale_factor='0.05');

-- SEQUENCE: stream_events_id_seq
CREATE SEQUENCE public.stream_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

-- SEQUENCE OWNED BY: stream_events_id_seq
ALTER SEQUENCE public.stream_events_id_seq OWNED BY public.stream_events.id;

-- TABLE: triggers
CREATE TABLE public.triggers (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    kind text NOT NULL,
    config jsonb DEFAULT '{}'::jsonb NOT NULL,
    secret_hash text,
    enabled boolean DEFAULT false NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: usage_notice_steps
CREATE TABLE public.usage_notice_steps (
    session_id text NOT NULL,
    window_key text NOT NULL,
    step integer NOT NULL,
    notified_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: user_drafts
CREATE TABLE public.user_drafts (
    user_id uuid NOT NULL,
    key text NOT NULL,
    text text NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

-- TABLE: webauthn_challenges
CREATE TABLE public.webauthn_challenges (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid,
    kind text NOT NULL,
    state jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    CONSTRAINT webauthn_challenges_kind CHECK ((kind = ANY (ARRAY['register'::text, 'authenticate'::text])))
);

-- TABLE: webauthn_credentials
CREATE TABLE public.webauthn_credentials (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    user_id uuid NOT NULL,
    credential_id bytea NOT NULL,
    passkey jsonb NOT NULL,
    label text NOT NULL,
    discoverable boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    last_used_at timestamp with time zone
);

-- TABLE: ws_presence
CREATE TABLE public.ws_presence (
    kind text NOT NULL,
    entity_id uuid NOT NULL,
    pod text NOT NULL,
    pod_ip text NOT NULL,
    connected_at timestamp with time zone DEFAULT now() NOT NULL,
    heartbeat_at timestamp with time zone DEFAULT now() NOT NULL
);

-- DEFAULT: archive_index id
ALTER TABLE ONLY public.archive_index ALTER COLUMN id SET DEFAULT nextval('public.archive_index_id_seq'::regclass);

-- DEFAULT: cctuiverse_messages id
ALTER TABLE ONLY public.cctuiverse_messages ALTER COLUMN id SET DEFAULT nextval('public.cctuiverse_messages_id_seq'::regclass);

-- DEFAULT: context_tool_saves id
ALTER TABLE ONLY public.context_tool_saves ALTER COLUMN id SET DEFAULT nextval('public.context_tool_saves_id_seq'::regclass);

-- DEFAULT: room_messages id
ALTER TABLE ONLY public.room_messages ALTER COLUMN id SET DEFAULT nextval('public.room_messages_id_seq'::regclass);

-- DEFAULT: session_message_pins id
ALTER TABLE ONLY public.session_message_pins ALTER COLUMN id SET DEFAULT nextval('public.session_message_pins_id_seq'::regclass);

-- DEFAULT: session_transcript id
ALTER TABLE ONLY public.session_transcript ALTER COLUMN id SET DEFAULT nextval('public.session_transcript_id_seq'::regclass);

-- DEFAULT: stream_events id
ALTER TABLE ONLY public.stream_events ALTER COLUMN id SET DEFAULT nextval('public.stream_events_id_seq'::regclass);

-- CONSTRAINT: account_limit_resets account_limit_resets_pkey
ALTER TABLE ONLY public.account_limit_resets
    ADD CONSTRAINT account_limit_resets_pkey PRIMARY KEY (id);

-- CONSTRAINT: account_usage_samples account_usage_samples_pkey
ALTER TABLE ONLY public.account_usage_samples
    ADD CONSTRAINT account_usage_samples_pkey PRIMARY KEY (provider_id, window_key, sampled_at);

-- CONSTRAINT: account_window_closes account_window_closes_pkey
ALTER TABLE ONLY public.account_window_closes
    ADD CONSTRAINT account_window_closes_pkey PRIMARY KEY (provider_id, window_key, resets_at);

-- CONSTRAINT: api_keys api_keys_pkey
ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_pkey PRIMARY KEY (id);

-- CONSTRAINT: archive_index archive_index_machine_id_session_id_key
ALTER TABLE ONLY public.archive_index
    ADD CONSTRAINT archive_index_machine_id_session_id_key UNIQUE (machine_id, session_id);

-- CONSTRAINT: archive_index archive_index_pkey
ALTER TABLE ONLY public.archive_index
    ADD CONSTRAINT archive_index_pkey PRIMARY KEY (id);

-- CONSTRAINT: archive_manifest archive_manifest_pkey
ALTER TABLE ONLY public.archive_manifest
    ADD CONSTRAINT archive_manifest_pkey PRIMARY KEY (machine_id, session_id);

-- CONSTRAINT: bookmarks bookmarks_pkey
ALTER TABLE ONLY public.bookmarks
    ADD CONSTRAINT bookmarks_pkey PRIMARY KEY (id);

-- CONSTRAINT: cctuiverse_links cctuiverse_links_pkey
ALTER TABLE ONLY public.cctuiverse_links
    ADD CONSTRAINT cctuiverse_links_pkey PRIMARY KEY (id);

-- CONSTRAINT: cctuiverse_messages cctuiverse_messages_link_id_direction_message_id_key
ALTER TABLE ONLY public.cctuiverse_messages
    ADD CONSTRAINT cctuiverse_messages_link_id_direction_message_id_key UNIQUE (link_id, direction, message_id);

-- CONSTRAINT: cctuiverse_messages cctuiverse_messages_pkey
ALTER TABLE ONLY public.cctuiverse_messages
    ADD CONSTRAINT cctuiverse_messages_pkey PRIMARY KEY (id);

-- CONSTRAINT: cctuiverse_nonces cctuiverse_nonces_pkey
ALTER TABLE ONLY public.cctuiverse_nonces
    ADD CONSTRAINT cctuiverse_nonces_pkey PRIMARY KEY (link_id, nonce);

-- CONSTRAINT: cluster_secrets cluster_secrets_pkey
ALTER TABLE ONLY public.cluster_secrets
    ADD CONSTRAINT cluster_secrets_pkey PRIMARY KEY (name);

-- CONSTRAINT: codex_account_model_catalogs codex_account_model_catalogs_pkey
ALTER TABLE ONLY public.codex_account_model_catalogs
    ADD CONSTRAINT codex_account_model_catalogs_pkey PRIMARY KEY (provider_id);

-- CONSTRAINT: codex_model_catalogs codex_model_catalogs_pkey
ALTER TABLE ONLY public.codex_model_catalogs
    ADD CONSTRAINT codex_model_catalogs_pkey PRIMARY KEY (machine_id);

-- CONSTRAINT: commands commands_pkey
ALTER TABLE ONLY public.commands
    ADD CONSTRAINT commands_pkey PRIMARY KEY (id);

-- CONSTRAINT: context_items context_items_pkey
ALTER TABLE ONLY public.context_items
    ADD CONSTRAINT context_items_pkey PRIMARY KEY (id);

-- CONSTRAINT: context_items context_items_user_id_kind_name_key
ALTER TABLE ONLY public.context_items
    ADD CONSTRAINT context_items_user_id_kind_name_key UNIQUE (user_id, kind, name);

-- CONSTRAINT: context_tool_saves context_tool_saves_pkey
ALTER TABLE ONLY public.context_tool_saves
    ADD CONSTRAINT context_tool_saves_pkey PRIMARY KEY (id);

-- CONSTRAINT: daemon_blobs daemon_blobs_pkey
ALTER TABLE ONLY public.daemon_blobs
    ADD CONSTRAINT daemon_blobs_pkey PRIMARY KEY (hash);

-- CONSTRAINT: device_auth_requests device_auth_requests_device_code_hash_key
ALTER TABLE ONLY public.device_auth_requests
    ADD CONSTRAINT device_auth_requests_device_code_hash_key UNIQUE (device_code_hash);

-- CONSTRAINT: device_auth_requests device_auth_requests_pkey
ALTER TABLE ONLY public.device_auth_requests
    ADD CONSTRAINT device_auth_requests_pkey PRIMARY KEY (id);

-- CONSTRAINT: device_auth_requests device_auth_requests_user_code_key
ALTER TABLE ONLY public.device_auth_requests
    ADD CONSTRAINT device_auth_requests_user_code_key UNIQUE (user_code);

-- CONSTRAINT: dispatched_turns dispatched_turns_pkey
ALTER TABLE ONLY public.dispatched_turns
    ADD CONSTRAINT dispatched_turns_pkey PRIMARY KEY (session_id, turn_id);

-- CONSTRAINT: draft_launch_queue draft_launch_queue_pkey
ALTER TABLE ONLY public.draft_launch_queue
    ADD CONSTRAINT draft_launch_queue_pkey PRIMARY KEY (draft_id);

-- CONSTRAINT: guard_steps guard_steps_pkey
ALTER TABLE ONLY public.guard_steps
    ADD CONSTRAINT guard_steps_pkey PRIMARY KEY (session_id);

-- CONSTRAINT: image_blob_backfill image_blob_backfill_pkey
ALTER TABLE ONLY public.image_blob_backfill
    ADD CONSTRAINT image_blob_backfill_pkey PRIMARY KEY (name);

-- CONSTRAINT: labels labels_pkey
ALTER TABLE ONLY public.labels
    ADD CONSTRAINT labels_pkey PRIMARY KEY (id);

-- CONSTRAINT: machine_bandwidth machine_bandwidth_pkey
ALTER TABLE ONLY public.machine_bandwidth
    ADD CONSTRAINT machine_bandwidth_pkey PRIMARY KEY (machine_id);

-- CONSTRAINT: machine_resources machine_resources_pkey
ALTER TABLE ONLY public.machine_resources
    ADD CONSTRAINT machine_resources_pkey PRIMARY KEY (machine_id);

-- CONSTRAINT: plugin_settings plugin_settings_pkey
ALTER TABLE ONLY public.plugin_settings
    ADD CONSTRAINT plugin_settings_pkey PRIMARY KEY (plugin_id);

-- CONSTRAINT: plugins plugins_pkey
ALTER TABLE ONLY public.plugins
    ADD CONSTRAINT plugins_pkey PRIMARY KEY (id);

-- CONSTRAINT: pods pods_pkey
ALTER TABLE ONLY public.pods
    ADD CONSTRAINT pods_pkey PRIMARY KEY (pod);

-- CONSTRAINT: preview_tickets_used preview_tickets_used_pkey
ALTER TABLE ONLY public.preview_tickets_used
    ADD CONSTRAINT preview_tickets_used_pkey PRIMARY KEY (nonce);

-- CONSTRAINT: previews previews_pkey
ALTER TABLE ONLY public.previews
    ADD CONSTRAINT previews_pkey PRIMARY KEY (id);

-- CONSTRAINT: privacy_scan_jobs privacy_scan_jobs_pkey
ALTER TABLE ONLY public.privacy_scan_jobs
    ADD CONSTRAINT privacy_scan_jobs_pkey PRIMARY KEY (id);

-- CONSTRAINT: prompts prompts_name_key
ALTER TABLE ONLY public.prompts
    ADD CONSTRAINT prompts_name_key UNIQUE (name);

-- CONSTRAINT: prompts prompts_pkey
ALTER TABLE ONLY public.prompts
    ADD CONSTRAINT prompts_pkey PRIMARY KEY (id);

-- CONSTRAINT: room_messages room_messages_pkey
ALTER TABLE ONLY public.room_messages
    ADD CONSTRAINT room_messages_pkey PRIMARY KEY (id);

-- CONSTRAINT: room_messages room_messages_room_id_seq_key
ALTER TABLE ONLY public.room_messages
    ADD CONSTRAINT room_messages_room_id_seq_key UNIQUE (room_id, seq);

-- CONSTRAINT: rooms rooms_pkey
ALTER TABLE ONLY public.rooms
    ADD CONSTRAINT rooms_pkey PRIMARY KEY (id);

-- CONSTRAINT: search_chunks search_chunks_pkey
ALTER TABLE ONLY public.search_chunks
    ADD CONSTRAINT search_chunks_pkey PRIMARY KEY (event_id, ord);

-- CONSTRAINT: search_index_state search_index_state_pkey
ALTER TABLE ONLY public.search_index_state
    ADD CONSTRAINT search_index_state_pkey PRIMARY KEY (name);

-- CONSTRAINT: self_update_runs self_update_runs_pkey
ALTER TABLE ONLY public.self_update_runs
    ADD CONSTRAINT self_update_runs_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_attachments session_attachments_pkey
ALTER TABLE ONLY public.session_attachments
    ADD CONSTRAINT session_attachments_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_auto_archive session_auto_archive_pkey
ALTER TABLE ONLY public.session_auto_archive
    ADD CONSTRAINT session_auto_archive_pkey PRIMARY KEY (spawn_key);

-- CONSTRAINT: session_auto_resume session_auto_resume_pkey
ALTER TABLE ONLY public.session_auto_resume
    ADD CONSTRAINT session_auto_resume_pkey PRIMARY KEY (session_id);

-- CONSTRAINT: session_blob_links session_blob_links_pkey
ALTER TABLE ONLY public.session_blob_links
    ADD CONSTRAINT session_blob_links_pkey PRIMARY KEY (session_id, hash);

-- CONSTRAINT: session_context session_context_pkey
ALTER TABLE ONLY public.session_context
    ADD CONSTRAINT session_context_pkey PRIMARY KEY (spawn_key);

-- CONSTRAINT: session_file_links session_file_links_pkey
ALTER TABLE ONLY public.session_file_links
    ADD CONSTRAINT session_file_links_pkey PRIMARY KEY (session_id, path);

-- CONSTRAINT: session_followup_intents session_followup_intents_pkey
ALTER TABLE ONLY public.session_followup_intents
    ADD CONSTRAINT session_followup_intents_pkey PRIMARY KEY (spawn_key);

-- CONSTRAINT: session_images session_images_pkey
ALTER TABLE ONLY public.session_images
    ADD CONSTRAINT session_images_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_images session_images_session_id_sha256_key
ALTER TABLE ONLY public.session_images
    ADD CONSTRAINT session_images_session_id_sha256_key UNIQUE (session_id, sha256);

-- CONSTRAINT: session_label_intents session_label_intents_pkey
ALTER TABLE ONLY public.session_label_intents
    ADD CONSTRAINT session_label_intents_pkey PRIMARY KEY (spawn_key);

-- CONSTRAINT: session_labels session_labels_pkey
ALTER TABLE ONLY public.session_labels
    ADD CONSTRAINT session_labels_pkey PRIMARY KEY (session_id, label_id);

-- CONSTRAINT: session_message_pins session_message_pins_pkey
ALTER TABLE ONLY public.session_message_pins
    ADD CONSTRAINT session_message_pins_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_message_pins session_message_pins_user_id_session_id_seq_key
ALTER TABLE ONLY public.session_message_pins
    ADD CONSTRAINT session_message_pins_user_id_session_id_seq_key UNIQUE (user_id, session_id, seq);

-- CONSTRAINT: session_message_queue session_message_queue_pkey
ALTER TABLE ONLY public.session_message_queue
    ADD CONSTRAINT session_message_queue_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_profiles session_profiles_pkey
ALTER TABLE ONLY public.session_profiles
    ADD CONSTRAINT session_profiles_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_profiles session_profiles_user_id_name_key
ALTER TABLE ONLY public.session_profiles
    ADD CONSTRAINT session_profiles_user_id_name_key UNIQUE (user_id, name);

-- CONSTRAINT: session_reads session_reads_pkey
ALTER TABLE ONLY public.session_reads
    ADD CONSTRAINT session_reads_pkey PRIMARY KEY (user_id, session_id);

-- CONSTRAINT: session_transcript session_transcript_pkey
ALTER TABLE ONLY public.session_transcript
    ADD CONSTRAINT session_transcript_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_user_actions session_user_actions_pkey
ALTER TABLE ONLY public.session_user_actions
    ADD CONSTRAINT session_user_actions_pkey PRIMARY KEY (id);

-- CONSTRAINT: session_viewers session_viewers_pkey
ALTER TABLE ONLY public.session_viewers
    ADD CONSTRAINT session_viewers_pkey PRIMARY KEY (session_id, pod);

-- CONSTRAINT: session_voice_notes session_voice_notes_pkey
ALTER TABLE ONLY public.session_voice_notes
    ADD CONSTRAINT session_voice_notes_pkey PRIMARY KEY (id);

-- CONSTRAINT: skill_registry skill_registry_pkey
ALTER TABLE ONLY public.skill_registry
    ADD CONSTRAINT skill_registry_pkey PRIMARY KEY (uploaded_by_user, name);

-- CONSTRAINT: stream_events stream_events_pkey
ALTER TABLE ONLY public.stream_events
    ADD CONSTRAINT stream_events_pkey PRIMARY KEY (id);

-- CONSTRAINT: triggers triggers_pkey
ALTER TABLE ONLY public.triggers
    ADD CONSTRAINT triggers_pkey PRIMARY KEY (id);

-- CONSTRAINT: usage_notice_steps usage_notice_steps_pkey
ALTER TABLE ONLY public.usage_notice_steps
    ADD CONSTRAINT usage_notice_steps_pkey PRIMARY KEY (session_id, window_key);

-- CONSTRAINT: user_drafts user_drafts_pkey
ALTER TABLE ONLY public.user_drafts
    ADD CONSTRAINT user_drafts_pkey PRIMARY KEY (user_id, key);

-- CONSTRAINT: webauthn_challenges webauthn_challenges_pkey
ALTER TABLE ONLY public.webauthn_challenges
    ADD CONSTRAINT webauthn_challenges_pkey PRIMARY KEY (id);

-- CONSTRAINT: webauthn_credentials webauthn_credentials_credential_id_key
ALTER TABLE ONLY public.webauthn_credentials
    ADD CONSTRAINT webauthn_credentials_credential_id_key UNIQUE (credential_id);

-- CONSTRAINT: webauthn_credentials webauthn_credentials_pkey
ALTER TABLE ONLY public.webauthn_credentials
    ADD CONSTRAINT webauthn_credentials_pkey PRIMARY KEY (id);

-- CONSTRAINT: ws_presence ws_presence_pkey
ALTER TABLE ONLY public.ws_presence
    ADD CONSTRAINT ws_presence_pkey PRIMARY KEY (kind, entity_id);

-- INDEX: account_limit_resets_provider_at
CREATE INDEX account_limit_resets_provider_at ON public.account_limit_resets USING btree (provider_id, at DESC);

-- INDEX: account_usage_samples_sampled_at_idx
CREATE INDEX account_usage_samples_sampled_at_idx ON public.account_usage_samples USING btree (sampled_at);

-- INDEX: account_window_closes_closed_at_idx
CREATE INDEX account_window_closes_closed_at_idx ON public.account_window_closes USING btree (closed_at);

-- INDEX: api_keys_user_id_idx
CREATE INDEX api_keys_user_id_idx ON public.api_keys USING btree (user_id);

-- INDEX: bookmarks_session_seq_idx
CREATE INDEX bookmarks_session_seq_idx ON public.bookmarks USING btree (session_id, seq);

-- INDEX: bookmarks_user_created_idx
CREATE INDEX bookmarks_user_created_idx ON public.bookmarks USING btree (user_id, created_at DESC);

-- INDEX: cctuiverse_links_room
CREATE INDEX cctuiverse_links_room ON public.cctuiverse_links USING btree (room_id) WHERE (state = 'active'::text);

-- INDEX: cctuiverse_links_session
CREATE INDEX cctuiverse_links_session ON public.cctuiverse_links USING btree (session_id) WHERE (state <> 'closed'::text);

-- INDEX: cctuiverse_links_user
CREATE INDEX cctuiverse_links_user ON public.cctuiverse_links USING btree (user_id);

-- INDEX: cctuiverse_messages_outbox
CREATE INDEX cctuiverse_messages_outbox ON public.cctuiverse_messages USING btree (status, next_attempt_at) WHERE ((direction = 'out'::text) AND (status = 'queued'::text));

-- INDEX: context_items_owner_idx
CREATE INDEX context_items_owner_idx ON public.context_items USING btree (user_id, kind) WHERE enabled;

-- INDEX: context_tool_saves_session_idx
CREATE INDEX context_tool_saves_session_idx ON public.context_tool_saves USING btree (session_id, created_at);

-- INDEX: device_auth_requests_expires_at_idx
CREATE INDEX device_auth_requests_expires_at_idx ON public.device_auth_requests USING btree (expires_at);

-- INDEX: dispatched_turns_claimed_at_idx
CREATE INDEX dispatched_turns_claimed_at_idx ON public.dispatched_turns USING btree (claimed_at);

-- INDEX: idx_archive_machine_project
CREATE INDEX idx_archive_machine_project ON public.archive_index USING btree (machine_id, project_dir);

-- INDEX: idx_archive_session
CREATE INDEX idx_archive_session ON public.archive_index USING btree (session_id);

-- INDEX: idx_commands_session_pending
CREATE INDEX idx_commands_session_pending ON public.commands USING btree (session_id) WHERE (status = 'pending'::text);

-- INDEX: idx_draft_launch_queue_due
CREATE INDEX idx_draft_launch_queue_due ON public.draft_launch_queue USING btree (state, launch_at) WHERE (state = 'scheduled'::text);

-- INDEX: idx_manifest_machine_project
CREATE INDEX idx_manifest_machine_project ON public.archive_manifest USING btree (machine_id, project_dir);

-- INDEX: idx_search_chunks_session_body_trgm
CREATE INDEX idx_search_chunks_session_body_trgm ON public.search_chunks USING gin (session_id, body public.gin_trgm_ops);

-- INDEX: idx_session_message_queue_due
CREATE INDEX idx_session_message_queue_due ON public.session_message_queue USING btree (state, deliver_at) WHERE (state = 'scheduled'::text);

-- INDEX: idx_session_message_queue_session
CREATE INDEX idx_session_message_queue_session ON public.session_message_queue USING btree (session_id, created_at);

-- INDEX: idx_session_transcript_session
CREATE INDEX idx_session_transcript_session ON public.session_transcript USING btree (session_id, id);

-- INDEX: idx_skill_registry_user
CREATE INDEX idx_skill_registry_user ON public.skill_registry USING btree (uploaded_by_user);

-- INDEX: idx_stream_events_api_error
CREATE INDEX idx_stream_events_api_error ON public.stream_events USING btree (created_at DESC) WHERE ((event_type = 'message'::text) AND ((payload ->> 'role'::text) = 'assistant'::text) AND ((payload ->> 'text'::text) ~~ 'API Error:%'::text));

-- INDEX: idx_stream_events_latest_message
CREATE INDEX idx_stream_events_latest_message ON public.stream_events USING btree (session_id, created_at DESC) WHERE (event_type = 'message'::text);

-- INDEX: idx_stream_events_login_error
CREATE INDEX idx_stream_events_login_error ON public.stream_events USING btree (created_at DESC) WHERE ((event_type = 'message'::text) AND ((payload ->> 'role'::text) = 'assistant'::text) AND ((payload ->> 'text'::text) ~~ 'Please run /login%'::text));

-- INDEX: idx_stream_events_session
CREATE INDEX idx_stream_events_session ON public.stream_events USING btree (session_id, created_at);

-- INDEX: idx_stream_events_session_id
CREATE INDEX idx_stream_events_session_id ON public.stream_events USING btree (session_id, id);

-- INDEX: idx_stream_events_session_search_trgm
CREATE INDEX idx_stream_events_session_search_trgm ON public.stream_events USING gin (session_id, "left"(search_text, 8192) public.gin_trgm_ops) WITH (fastupdate=off);

-- INDEX: idx_stream_events_turn_id
CREATE INDEX idx_stream_events_turn_id ON public.stream_events USING btree (session_id, turn_id) WHERE (turn_id IS NOT NULL);

-- INDEX: idx_triggers_user
CREATE INDEX idx_triggers_user ON public.triggers USING btree (user_id);

-- INDEX: labels_user_name_lower_key
CREATE UNIQUE INDEX labels_user_name_lower_key ON public.labels USING btree (user_id, lower(name));

-- INDEX: preview_tickets_used_expires_idx
CREATE INDEX preview_tickets_used_expires_idx ON public.preview_tickets_used USING btree (expires_at);

-- INDEX: previews_machine_idx
CREATE INDEX previews_machine_idx ON public.previews USING btree (machine_id);

-- INDEX: previews_session_idx
CREATE INDEX previews_session_idx ON public.previews USING btree (session_id);

-- INDEX: previews_session_port_idx
CREATE UNIQUE INDEX previews_session_port_idx ON public.previews USING btree (session_id, port);

-- INDEX: privacy_scan_jobs_one_running_idx
CREATE UNIQUE INDEX privacy_scan_jobs_one_running_idx ON public.privacy_scan_jobs USING btree (user_id) WHERE (status = 'running'::text);

-- INDEX: privacy_scan_jobs_user_created_idx
CREATE INDEX privacy_scan_jobs_user_created_idx ON public.privacy_scan_jobs USING btree (user_id, created_at DESC);

-- INDEX: prompts_kind_global_uniq
CREATE UNIQUE INDEX prompts_kind_global_uniq ON public.prompts USING btree (kind) WHERE ((scope_owner IS NULL) AND (scope_repo IS NULL));

-- INDEX: prompts_kind_owner_uniq
CREATE UNIQUE INDEX prompts_kind_owner_uniq ON public.prompts USING btree (kind, scope_owner) WHERE ((scope_owner IS NOT NULL) AND (scope_repo IS NULL));

-- INDEX: prompts_kind_repo_uniq
CREATE UNIQUE INDEX prompts_kind_repo_uniq ON public.prompts USING btree (kind, scope_owner, scope_repo) WHERE (scope_repo IS NOT NULL);

-- INDEX: prompts_user_id_idx
CREATE INDEX prompts_user_id_idx ON public.prompts USING btree (user_id);

-- INDEX: room_messages_room_seq
CREATE INDEX room_messages_room_seq ON public.room_messages USING btree (room_id, seq);

-- INDEX: rooms_owner_name
CREATE UNIQUE INDEX rooms_owner_name ON public.rooms USING btree (user_id, lower(name));

-- INDEX: rooms_user_live
CREATE INDEX rooms_user_live ON public.rooms USING btree (user_id) WHERE (archived_at IS NULL);

-- INDEX: self_update_runs_started_at_idx
CREATE INDEX self_update_runs_started_at_idx ON public.self_update_runs USING btree (started_at DESC);

-- INDEX: session_attachments_session_idx
CREATE INDEX session_attachments_session_idx ON public.session_attachments USING btree (session_id, created_at);

-- INDEX: session_images_session_idx
CREATE INDEX session_images_session_idx ON public.session_images USING btree (session_id);

-- INDEX: session_labels_label_idx
CREATE INDEX session_labels_label_idx ON public.session_labels USING btree (label_id);

-- INDEX: session_message_pins_session_idx
CREATE INDEX session_message_pins_session_idx ON public.session_message_pins USING btree (session_id, user_id, seq);

-- INDEX: session_profiles_user_order_idx
CREATE INDEX session_profiles_user_order_idx ON public.session_profiles USING btree (user_id, sort_order);

-- INDEX: session_reads_user_idx
CREATE INDEX session_reads_user_idx ON public.session_reads USING btree (user_id);

-- INDEX: session_user_actions_session_idx
CREATE INDEX session_user_actions_session_idx ON public.session_user_actions USING btree (session_id, status, created_at);

-- INDEX: session_voice_notes_session_idx
CREATE INDEX session_voice_notes_session_idx ON public.session_voice_notes USING btree (session_id);

-- INDEX: stream_events_dedup_line_idx
CREATE UNIQUE INDEX stream_events_dedup_line_idx ON public.stream_events USING btree (session_id, event_type, content_hash, ((payload ->> 'line_id'::text))) WHERE (payload ? 'line_id'::text);

-- INDEX: stream_events_dedup_turn_idx
CREATE UNIQUE INDEX stream_events_dedup_turn_idx ON public.stream_events USING btree (session_id, event_type, content_hash, COALESCE(turn_id, '00000000-0000-0000-0000-000000000000'::uuid));

-- INDEX: user_drafts_user_updated_idx
CREATE INDEX user_drafts_user_updated_idx ON public.user_drafts USING btree (user_id, updated_at DESC);

-- INDEX: webauthn_challenges_expiry
CREATE INDEX webauthn_challenges_expiry ON public.webauthn_challenges USING btree (expires_at);

-- INDEX: webauthn_credentials_by_user
CREATE INDEX webauthn_credentials_by_user ON public.webauthn_credentials USING btree (user_id);

-- INDEX: ws_presence_pod_idx
CREATE INDEX ws_presence_pod_idx ON public.ws_presence USING btree (pod);

-- TRIGGER: sessions sessions_delete_attachments
CREATE TRIGGER sessions_delete_attachments AFTER DELETE ON public.sessions FOR EACH ROW EXECUTE FUNCTION public.delete_session_attachments();

-- FK CONSTRAINT: account_limit_resets account_limit_resets_provider_id_fkey
ALTER TABLE ONLY public.account_limit_resets
    ADD CONSTRAINT account_limit_resets_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.account_providers(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_limit_resets account_limit_resets_requested_by_fkey
ALTER TABLE ONLY public.account_limit_resets
    ADD CONSTRAINT account_limit_resets_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.users(id) ON DELETE SET NULL;

-- FK CONSTRAINT: account_usage_samples account_usage_samples_provider_id_fkey
ALTER TABLE ONLY public.account_usage_samples
    ADD CONSTRAINT account_usage_samples_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.account_providers(id) ON DELETE CASCADE;

-- FK CONSTRAINT: account_window_closes account_window_closes_provider_id_fkey
ALTER TABLE ONLY public.account_window_closes
    ADD CONSTRAINT account_window_closes_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.account_providers(id) ON DELETE CASCADE;

-- FK CONSTRAINT: api_keys api_keys_user_id_fkey
ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

-- FK CONSTRAINT: archive_index archive_index_machine_id_fkey
ALTER TABLE ONLY public.archive_index
    ADD CONSTRAINT archive_index_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES public.machines(id) ON DELETE CASCADE;

-- FK CONSTRAINT: archive_manifest archive_manifest_machine_id_fkey
ALTER TABLE ONLY public.archive_manifest
    ADD CONSTRAINT archive_manifest_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES public.machines(id) ON DELETE CASCADE;

-- FK CONSTRAINT: bookmarks bookmarks_session_id_fkey
ALTER TABLE ONLY public.bookmarks
    ADD CONSTRAINT bookmarks_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE SET NULL;

-- FK CONSTRAINT: bookmarks bookmarks_user_id_fkey
ALTER TABLE ONLY public.bookmarks
    ADD CONSTRAINT bookmarks_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: cctuiverse_links cctuiverse_links_room_id_fkey
ALTER TABLE ONLY public.cctuiverse_links
    ADD CONSTRAINT cctuiverse_links_room_id_fkey FOREIGN KEY (room_id) REFERENCES public.rooms(id) ON DELETE CASCADE;

-- FK CONSTRAINT: cctuiverse_links cctuiverse_links_session_id_fkey
ALTER TABLE ONLY public.cctuiverse_links
    ADD CONSTRAINT cctuiverse_links_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: cctuiverse_links cctuiverse_links_user_id_fkey
ALTER TABLE ONLY public.cctuiverse_links
    ADD CONSTRAINT cctuiverse_links_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: cctuiverse_messages cctuiverse_messages_link_id_fkey
ALTER TABLE ONLY public.cctuiverse_messages
    ADD CONSTRAINT cctuiverse_messages_link_id_fkey FOREIGN KEY (link_id) REFERENCES public.cctuiverse_links(id) ON DELETE CASCADE;

-- FK CONSTRAINT: codex_account_model_catalogs codex_account_model_catalogs_provider_id_fkey
ALTER TABLE ONLY public.codex_account_model_catalogs
    ADD CONSTRAINT codex_account_model_catalogs_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.account_providers(id) ON DELETE CASCADE;

-- FK CONSTRAINT: codex_model_catalogs codex_model_catalogs_machine_id_fkey
ALTER TABLE ONLY public.codex_model_catalogs
    ADD CONSTRAINT codex_model_catalogs_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES public.machines(id) ON DELETE CASCADE;

-- FK CONSTRAINT: commands commands_session_id_fkey
ALTER TABLE ONLY public.commands
    ADD CONSTRAINT commands_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: context_items context_items_user_id_fkey
ALTER TABLE ONLY public.context_items
    ADD CONSTRAINT context_items_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: device_auth_requests device_auth_requests_approved_by_fkey
ALTER TABLE ONLY public.device_auth_requests
    ADD CONSTRAINT device_auth_requests_approved_by_fkey FOREIGN KEY (approved_by) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: dispatched_turns dispatched_turns_session_id_fkey
ALTER TABLE ONLY public.dispatched_turns
    ADD CONSTRAINT dispatched_turns_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: draft_launch_queue draft_launch_queue_draft_id_fkey
ALTER TABLE ONLY public.draft_launch_queue
    ADD CONSTRAINT draft_launch_queue_draft_id_fkey FOREIGN KEY (draft_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: draft_launch_queue draft_launch_queue_user_id_fkey
ALTER TABLE ONLY public.draft_launch_queue
    ADD CONSTRAINT draft_launch_queue_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: guard_steps guard_steps_session_id_fkey
ALTER TABLE ONLY public.guard_steps
    ADD CONSTRAINT guard_steps_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: labels labels_user_id_fkey
ALTER TABLE ONLY public.labels
    ADD CONSTRAINT labels_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: machine_bandwidth machine_bandwidth_machine_id_fkey
ALTER TABLE ONLY public.machine_bandwidth
    ADD CONSTRAINT machine_bandwidth_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES public.machines(id) ON DELETE CASCADE;

-- FK CONSTRAINT: machine_resources machine_resources_machine_id_fkey
ALTER TABLE ONLY public.machine_resources
    ADD CONSTRAINT machine_resources_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES public.machines(id) ON DELETE CASCADE;

-- FK CONSTRAINT: plugins plugins_installed_by_fkey
ALTER TABLE ONLY public.plugins
    ADD CONSTRAINT plugins_installed_by_fkey FOREIGN KEY (installed_by) REFERENCES public.users(id) ON DELETE SET NULL;

-- FK CONSTRAINT: previews previews_user_id_fkey
ALTER TABLE ONLY public.previews
    ADD CONSTRAINT previews_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: privacy_scan_jobs privacy_scan_jobs_user_id_fkey
ALTER TABLE ONLY public.privacy_scan_jobs
    ADD CONSTRAINT privacy_scan_jobs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: prompts prompts_user_id_fkey
ALTER TABLE ONLY public.prompts
    ADD CONSTRAINT prompts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;

-- FK CONSTRAINT: room_messages room_messages_room_id_fkey
ALTER TABLE ONLY public.room_messages
    ADD CONSTRAINT room_messages_room_id_fkey FOREIGN KEY (room_id) REFERENCES public.rooms(id) ON DELETE CASCADE;

-- FK CONSTRAINT: room_messages room_messages_sender_session_id_fkey
ALTER TABLE ONLY public.room_messages
    ADD CONSTRAINT room_messages_sender_session_id_fkey FOREIGN KEY (sender_session_id) REFERENCES public.sessions(id) ON DELETE SET NULL;

-- FK CONSTRAINT: rooms rooms_user_id_fkey
ALTER TABLE ONLY public.rooms
    ADD CONSTRAINT rooms_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: search_chunks search_chunks_event_id_fkey
ALTER TABLE ONLY public.search_chunks
    ADD CONSTRAINT search_chunks_event_id_fkey FOREIGN KEY (event_id) REFERENCES public.stream_events(id) ON DELETE CASCADE;

-- FK CONSTRAINT: search_chunks search_chunks_session_id_fkey
ALTER TABLE ONLY public.search_chunks
    ADD CONSTRAINT search_chunks_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: self_update_runs self_update_runs_machine_id_fkey
ALTER TABLE ONLY public.self_update_runs
    ADD CONSTRAINT self_update_runs_machine_id_fkey FOREIGN KEY (machine_id) REFERENCES public.machines(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_auto_resume session_auto_resume_session_id_fkey
ALTER TABLE ONLY public.session_auto_resume
    ADD CONSTRAINT session_auto_resume_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_blob_links session_blob_links_session_id_fkey
ALTER TABLE ONLY public.session_blob_links
    ADD CONSTRAINT session_blob_links_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_file_links session_file_links_session_id_fkey
ALTER TABLE ONLY public.session_file_links
    ADD CONSTRAINT session_file_links_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_images session_images_session_id_fkey
ALTER TABLE ONLY public.session_images
    ADD CONSTRAINT session_images_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_labels session_labels_label_id_fkey
ALTER TABLE ONLY public.session_labels
    ADD CONSTRAINT session_labels_label_id_fkey FOREIGN KEY (label_id) REFERENCES public.labels(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_labels session_labels_session_id_fkey
ALTER TABLE ONLY public.session_labels
    ADD CONSTRAINT session_labels_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_message_pins session_message_pins_session_id_fkey
ALTER TABLE ONLY public.session_message_pins
    ADD CONSTRAINT session_message_pins_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_message_pins session_message_pins_user_id_fkey
ALTER TABLE ONLY public.session_message_pins
    ADD CONSTRAINT session_message_pins_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_message_queue session_message_queue_session_id_fkey
ALTER TABLE ONLY public.session_message_queue
    ADD CONSTRAINT session_message_queue_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_message_queue session_message_queue_user_id_fkey
ALTER TABLE ONLY public.session_message_queue
    ADD CONSTRAINT session_message_queue_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_profiles session_profiles_account_id_fkey
ALTER TABLE ONLY public.session_profiles
    ADD CONSTRAINT session_profiles_account_id_fkey FOREIGN KEY (account_id) REFERENCES public.accounts(id) ON DELETE SET NULL;

-- FK CONSTRAINT: session_profiles session_profiles_pool_id_fkey
ALTER TABLE ONLY public.session_profiles
    ADD CONSTRAINT session_profiles_pool_id_fkey FOREIGN KEY (pool_id) REFERENCES public.account_pools(id) ON DELETE SET NULL;

-- FK CONSTRAINT: session_profiles session_profiles_user_id_fkey
ALTER TABLE ONLY public.session_profiles
    ADD CONSTRAINT session_profiles_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_reads session_reads_session_id_fkey
ALTER TABLE ONLY public.session_reads
    ADD CONSTRAINT session_reads_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_reads session_reads_user_id_fkey
ALTER TABLE ONLY public.session_reads
    ADD CONSTRAINT session_reads_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_transcript session_transcript_session_id_fkey
ALTER TABLE ONLY public.session_transcript
    ADD CONSTRAINT session_transcript_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_user_actions session_user_actions_session_id_fkey
ALTER TABLE ONLY public.session_user_actions
    ADD CONSTRAINT session_user_actions_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: session_voice_notes session_voice_notes_session_id_fkey
ALTER TABLE ONLY public.session_voice_notes
    ADD CONSTRAINT session_voice_notes_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: sessions sessions_room_id_fkey
ALTER TABLE ONLY public.sessions
    ADD CONSTRAINT sessions_room_id_fkey FOREIGN KEY (room_id) REFERENCES public.rooms(id) ON DELETE SET NULL;

-- FK CONSTRAINT: skill_registry skill_registry_uploaded_by_machine_fkey
ALTER TABLE ONLY public.skill_registry
    ADD CONSTRAINT skill_registry_uploaded_by_machine_fkey FOREIGN KEY (uploaded_by_machine) REFERENCES public.machines(id) ON DELETE SET NULL;

-- FK CONSTRAINT: skill_registry skill_registry_uploaded_by_user_fkey
ALTER TABLE ONLY public.skill_registry
    ADD CONSTRAINT skill_registry_uploaded_by_user_fkey FOREIGN KEY (uploaded_by_user) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: stream_events stream_events_session_id_fkey
ALTER TABLE ONLY public.stream_events
    ADD CONSTRAINT stream_events_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: triggers triggers_user_id_fkey
ALTER TABLE ONLY public.triggers
    ADD CONSTRAINT triggers_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: usage_notice_steps usage_notice_steps_session_id_fkey
ALTER TABLE ONLY public.usage_notice_steps
    ADD CONSTRAINT usage_notice_steps_session_id_fkey FOREIGN KEY (session_id) REFERENCES public.sessions(id) ON DELETE CASCADE;

-- FK CONSTRAINT: user_drafts user_drafts_user_id_fkey
ALTER TABLE ONLY public.user_drafts
    ADD CONSTRAINT user_drafts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: webauthn_challenges webauthn_challenges_user_id_fkey
ALTER TABLE ONLY public.webauthn_challenges
    ADD CONSTRAINT webauthn_challenges_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;

-- FK CONSTRAINT: webauthn_credentials webauthn_credentials_user_id_fkey
ALTER TABLE ONLY public.webauthn_credentials
    ADD CONSTRAINT webauthn_credentials_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;


--

-- Preserve provider-host access when expanding a populated core installation.
-- Seeds the admin upstream allowlist with every stored base_url host, so
-- existing accounts keep working after the upgrade.
WITH parsed AS (
    SELECT lower(scheme) AS scheme,
           rtrim(lower(host), '.') AS host,
           port
      FROM account_providers ap,
           regexp_match(
               btrim(ap.base_url),
               '^([A-Za-z][A-Za-z0-9+.-]*)://(?:[^@/?#]*@)?(\[[^\]]+\]|[^:/?#]+)(?::([0-9]+))?'
           ) AS m(parts),
           LATERAL (SELECT m.parts[1] AS scheme, m.parts[2] AS host, m.parts[3] AS port) p
     WHERE ap.base_url IS NOT NULL AND NOT ap.managed
),
hosts AS (
    SELECT DISTINCT host || ':' || COALESCE(
               NULLIF(ltrim(port, '0'), ''),
               CASE scheme WHEN 'http' THEN '80' WHEN 'https' THEN '443' END
           ) AS entry
      FROM parsed
     WHERE scheme IN ('http', 'https') AND host <> ''
)
INSERT INTO instance_settings (key, value, updated_at)
SELECT 'upstream_allowed_hosts', jsonb_agg(entry ORDER BY entry), now()
  FROM hosts
HAVING count(*) > 0
ON CONFLICT (key) DO UPDATE
   SET value = (
           SELECT jsonb_agg(DISTINCT e ORDER BY e)
             FROM jsonb_array_elements(instance_settings.value || EXCLUDED.value) AS t(e)
       ),
       updated_at = now();
