-- Operators, access authority, client identity, and monitoring shares.

-- Tables.

CREATE TABLE public.operator_auth_throttle (
    scope_kind text NOT NULL,
    scope_key text NOT NULL,
    failed_attempts bigint DEFAULT 0 NOT NULL,
    window_started_at timestamp with time zone DEFAULT now() NOT NULL,
    locked_until timestamp with time zone,
    last_failed_at timestamp with time zone,
    last_failure_reason text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT operator_auth_throttle_failed_attempts_check CHECK ((failed_attempts >= 0)),
    CONSTRAINT operator_auth_throttle_scope_kind_check CHECK ((scope_kind = ANY (ARRAY['username'::text, 'username_ip'::text, 'ip'::text]))),
    CONSTRAINT operator_auth_throttle_pkey PRIMARY KEY (scope_kind, scope_key)
);



CREATE TABLE public.operators (
    id uuid NOT NULL,
    username text NOT NULL,
    password_hash text NOT NULL,
    totp_enabled boolean DEFAULT false NOT NULL,
    status text DEFAULT 'active'::text NOT NULL,
    role text NOT NULL,
    scopes jsonb DEFAULT '[]'::jsonb NOT NULL,
    totp_secret_ciphertext_hex text,
    totp_secret_nonce_hex text,
    totp_secret_salt_hex text,
    totp_last_accepted_step bigint,
    preferences jsonb DEFAULT '{}'::jsonb NOT NULL,
    session_refresh_ttl_secs bigint DEFAULT 31536000 NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    disabled_at timestamp with time zone,
    deleted_at timestamp with time zone,
    CONSTRAINT operators_preferences_json_object CHECK ((jsonb_typeof(preferences) = 'object'::text)),
    CONSTRAINT operators_scopes_json_array CHECK ((jsonb_typeof(scopes) = 'array'::text)),
    CONSTRAINT operators_session_refresh_ttl_check CHECK (((session_refresh_ttl_secs >= 86400) AND (session_refresh_ttl_secs <= 315360000))),
    CONSTRAINT operators_status_check CHECK ((status = ANY (ARRAY['active'::text, 'disabled'::text, 'deleted'::text]))),
    CONSTRAINT operators_totp_secret_hex CHECK ((((totp_secret_ciphertext_hex IS NULL) AND (totp_secret_nonce_hex IS NULL) AND (totp_secret_salt_hex IS NULL)) OR ((totp_secret_ciphertext_hex IS NOT NULL) AND (totp_secret_nonce_hex IS NOT NULL) AND (totp_secret_salt_hex IS NOT NULL) AND (totp_secret_ciphertext_hex ~ '^[0-9a-f]+$'::text) AND (totp_secret_nonce_hex ~ '^[0-9a-f]{24}$'::text) AND (totp_secret_salt_hex ~ '^[0-9a-f]{32}$'::text)))),
    CONSTRAINT operators_totp_state_check CHECK (((totp_enabled AND (totp_secret_ciphertext_hex IS NOT NULL) AND (totp_last_accepted_step IS NOT NULL) AND (totp_last_accepted_step >= 0)) OR ((NOT totp_enabled) AND (totp_last_accepted_step IS NULL)))),
    CONSTRAINT operators_pkey PRIMARY KEY (id),
    CONSTRAINT operators_username_key UNIQUE (username)
);



CREATE TABLE public.audit_logs (
    id uuid NOT NULL,
    actor_id uuid,
    action text NOT NULL,
    target text NOT NULL,
    command_hash text,
    metadata jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT audit_logs_canonical_metadata CHECK (((jsonb_typeof(metadata) = 'object'::text) AND (metadata ?& ARRAY['result'::text, 'origin_kind'::text, 'component'::text]) AND (jsonb_typeof((metadata -> 'result'::text)) = 'string'::text) AND (jsonb_typeof((metadata -> 'origin_kind'::text)) = 'string'::text) AND (jsonb_typeof((metadata -> 'component'::text)) = 'string'::text) AND (btrim((metadata ->> 'result'::text)) <> ''::text) AND (btrim((metadata ->> 'origin_kind'::text)) <> ''::text) AND (btrim((metadata ->> 'component'::text)) <> ''::text))),
    CONSTRAINT audit_logs_pkey PRIMARY KEY (id),
    CONSTRAINT audit_logs_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id)
);



CREATE TABLE public.clients (
    id text NOT NULL,
    display_name text NOT NULL,
    public_key bytea NOT NULL,
    status text DEFAULT 'never'::text NOT NULL,
    agent_version text,
    internal_build_number bigint DEFAULT 1 NOT NULL,
    process_incarnation_id uuid,
    os_release text,
    arch text,
    cpu_model text,
    kernel_release text,
    virtualization text,
    system_reported_at timestamp with time zone,
    capabilities jsonb DEFAULT '{}'::jsonb NOT NULL,
    registration_ip inet,
    last_ip inet,
    last_seen_at timestamp with time zone,
    stale_since timestamp with time zone,
    stale_reason text,
    stale_build_number bigint,
    hidden_at timestamp with time zone,
    hidden_by uuid,
    hidden_reason text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    operational_alert_status_at timestamp with time zone NOT NULL,
    operational_alert_tunnel_boundary_at timestamp with time zone,
    policy_scope_revision bigint DEFAULT 1 NOT NULL,
    suspended_at timestamp with time zone,
    suspended_by uuid,
    suspended_reason text,
    suspended_from_status text,
    CONSTRAINT clients_deleted_visibility_check CHECK (((status = 'deleted'::text) = (hidden_at IS NOT NULL))),
    CONSTRAINT clients_internal_build_number_check CHECK ((internal_build_number >= 1)),
    CONSTRAINT clients_policy_scope_revision_check CHECK ((policy_scope_revision >= 1)),
    CONSTRAINT clients_stale_build_number_check CHECK (((stale_build_number IS NULL) OR (stale_build_number >= 1))),
    CONSTRAINT clients_status_check CHECK ((status = ANY (ARRAY['never'::text, 'online'::text, 'disconnected'::text, 'offline'::text, 'stale'::text, 'suspended'::text, 'revoked'::text, 'deleted'::text]))),
    CONSTRAINT clients_suspended_reason_check CHECK (((suspended_reason IS NULL) OR ((length(btrim(suspended_reason)) >= 1) AND (length(btrim(suspended_reason)) <= 240)))),
    CONSTRAINT clients_suspension_state_check CHECK ((((status = 'suspended'::text) AND (suspended_at IS NOT NULL) AND (suspended_from_status = ANY (ARRAY['never'::text, 'disconnected'::text, 'offline'::text, 'stale'::text]))) OR ((status <> 'suspended'::text) AND (suspended_at IS NULL) AND (suspended_by IS NULL) AND (suspended_reason IS NULL) AND (suspended_from_status IS NULL)))),
    CONSTRAINT clients_pkey PRIMARY KEY (id),
    CONSTRAINT clients_hidden_by_fkey FOREIGN KEY (hidden_by) REFERENCES public.operators(id) ON DELETE SET NULL,
    CONSTRAINT clients_suspended_by_fkey FOREIGN KEY (suspended_by) REFERENCES public.operators(id) ON DELETE SET NULL
);



CREATE TABLE public.client_key_revocations (
    id uuid NOT NULL,
    client_id text NOT NULL,
    public_key_sha256_hex text NOT NULL,
    reason text,
    revoked_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT client_key_revocations_sha256_hex_valid CHECK ((public_key_sha256_hex ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT client_key_revocations_client_id_public_key_sha256_hex_key UNIQUE (client_id, public_key_sha256_hex),
    CONSTRAINT client_key_revocations_pkey PRIMARY KEY (id),
    CONSTRAINT client_key_revocations_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT client_key_revocations_revoked_by_fkey FOREIGN KEY (revoked_by) REFERENCES public.operators(id) ON DELETE SET NULL
);



CREATE TABLE public.client_status_history (
    id uuid NOT NULL,
    client_id text NOT NULL,
    from_status text,
    to_status text NOT NULL,
    reason text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT client_status_history_from_check CHECK (((from_status IS NULL) OR (from_status = ANY (ARRAY['never'::text, 'online'::text, 'disconnected'::text, 'offline'::text, 'stale'::text, 'suspended'::text, 'revoked'::text, 'deleted'::text])))),
    CONSTRAINT client_status_history_metadata_object CHECK ((jsonb_typeof(metadata) = 'object'::text)),
    CONSTRAINT client_status_history_to_check CHECK ((to_status = ANY (ARRAY['never'::text, 'online'::text, 'disconnected'::text, 'offline'::text, 'stale'::text, 'suspended'::text, 'revoked'::text, 'deleted'::text]))),
    CONSTRAINT client_status_history_pkey PRIMARY KEY (id),
    CONSTRAINT client_status_history_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.gateway_sessions (
    id uuid NOT NULL,
    gateway_id text NOT NULL,
    client_id text NOT NULL,
    noise_public_key_hex text,
    remote_ip inet,
    status text NOT NULL,
    started_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    ended_at timestamp with time zone,
    end_reason text,
    CONSTRAINT gateway_sessions_status_check CHECK ((status = ANY (ARRAY['active'::text, 'ended'::text, 'expired'::text]))),
    CONSTRAINT gateway_sessions_pkey PRIMARY KEY (id),
    CONSTRAINT gateway_sessions_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.monitoring_share_links (
    id uuid NOT NULL,
    name text NOT NULL,
    token_secret text NOT NULL,
    selector_expression text NOT NULL,
    show_identity_context boolean DEFAULT false NOT NULL,
    show_billing boolean DEFAULT false NOT NULL,
    show_system_information boolean DEFAULT false NOT NULL,
    show_resources boolean DEFAULT true NOT NULL,
    show_network boolean DEFAULT true NOT NULL,
    show_traffic boolean DEFAULT true NOT NULL,
    show_ping boolean DEFAULT true NOT NULL,
    allow_detail_history boolean DEFAULT true NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    revoked_by uuid,
    created_by uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_share_links_check CHECK ((expires_at > created_at)),
    CONSTRAINT monitoring_share_links_name_check CHECK (((length(TRIM(BOTH FROM name)) >= 1) AND (length(TRIM(BOTH FROM name)) <= 128))),
    CONSTRAINT monitoring_share_links_selector_expression_check CHECK (((length(TRIM(BOTH FROM selector_expression)) >= 1) AND (length(TRIM(BOTH FROM selector_expression)) <= 65535))),
    CONSTRAINT monitoring_share_links_token_secret_check CHECK ((token_secret ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT monitoring_share_links_pkey PRIMARY KEY (id),
    CONSTRAINT monitoring_share_links_token_secret_key UNIQUE (token_secret),
    CONSTRAINT monitoring_share_links_created_by_fkey FOREIGN KEY (created_by) REFERENCES public.operators(id),
    CONSTRAINT monitoring_share_links_revoked_by_fkey FOREIGN KEY (revoked_by) REFERENCES public.operators(id)
);



CREATE TABLE public.monitoring_share_targets (
    share_id uuid NOT NULL,
    client_id text NOT NULL,
    public_client_key text NOT NULL,
    CONSTRAINT monitoring_share_targets_public_client_key_check CHECK ((public_client_key ~ '^[0-9a-f]{64}$'::text)),
    CONSTRAINT monitoring_share_targets_pkey PRIMARY KEY (share_id, client_id),
    CONSTRAINT monitoring_share_targets_share_id_public_client_key_key UNIQUE (share_id, public_client_key),
    CONSTRAINT monitoring_share_targets_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id),
    CONSTRAINT monitoring_share_targets_share_id_fkey FOREIGN KEY (share_id) REFERENCES public.monitoring_share_links(id) ON DELETE CASCADE
);



CREATE TABLE public.monitoring_share_visitors (
    share_id uuid NOT NULL,
    visitor_id uuid NOT NULL,
    source_ip inet,
    user_agent text,
    first_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT monitoring_share_visitors_user_agent_check CHECK (((user_agent IS NULL) OR (length(user_agent) <= 512))),
    CONSTRAINT monitoring_share_visitors_pkey PRIMARY KEY (share_id, visitor_id),
    CONSTRAINT monitoring_share_visitors_share_id_fkey FOREIGN KEY (share_id) REFERENCES public.monitoring_share_links(id) ON DELETE CASCADE
);



CREATE TABLE public.operator_sessions (
    id uuid NOT NULL,
    operator_id uuid NOT NULL,
    access_token_hash text NOT NULL,
    refresh_token_hash text NOT NULL,
    expires_at timestamp with time zone NOT NULL,
    refresh_expires_at timestamp with time zone NOT NULL,
    revoked_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT operator_sessions_access_token_hash_key UNIQUE (access_token_hash),
    CONSTRAINT operator_sessions_pkey PRIMARY KEY (id),
    CONSTRAINT operator_sessions_refresh_token_hash_key UNIQUE (refresh_token_hash),
    CONSTRAINT operator_sessions_operator_id_fkey FOREIGN KEY (operator_id) REFERENCES public.operators(id) ON DELETE CASCADE
);



CREATE TABLE public.tags (
    id uuid NOT NULL,
    name text NOT NULL,
    display_order bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT tags_name_key UNIQUE (name),
    CONSTRAINT tags_pkey PRIMARY KEY (id)
);



CREATE TABLE public.client_tags (
    client_id text NOT NULL,
    tag_id uuid NOT NULL,
    CONSTRAINT client_tags_pkey PRIMARY KEY (client_id, tag_id),
    CONSTRAINT client_tags_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT client_tags_tag_id_fkey FOREIGN KEY (tag_id) REFERENCES public.tags(id) ON DELETE CASCADE
);



-- Views.

CREATE VIEW public.visible_clients AS
 SELECT id,
    display_name,
    public_key,
    status,
    agent_version,
    internal_build_number,
    process_incarnation_id,
    os_release,
    arch,
    cpu_model,
    kernel_release,
    virtualization,
    system_reported_at,
    capabilities,
    registration_ip,
    last_ip,
    last_seen_at,
    stale_since,
    stale_reason,
    stale_build_number,
    hidden_at,
    hidden_by,
    hidden_reason,
    created_at,
    suspended_at,
    suspended_by,
    suspended_reason,
    suspended_from_status
   FROM public.clients
  WHERE (hidden_at IS NULL);



-- Indexes.

CREATE INDEX audit_logs_created_idx ON public.audit_logs USING btree (created_at DESC, id DESC);



CREATE INDEX client_key_revocations_client_created_idx ON public.client_key_revocations USING btree (client_id, created_at DESC);



CREATE UNIQUE INDEX client_key_revocations_public_key_unique_idx ON public.client_key_revocations USING btree (public_key_sha256_hex);



CREATE INDEX client_status_history_client_created_idx ON public.client_status_history USING btree (client_id, created_at DESC);



CREATE INDEX client_tags_tag_id_client_id_idx ON public.client_tags USING btree (tag_id, client_id);



CREATE UNIQUE INDEX clients_public_key_unique_idx ON public.clients USING btree (public_key) WHERE (octet_length(public_key) > 0);



CREATE UNIQUE INDEX clients_visible_display_name_key_idx ON public.clients USING btree (lower(btrim(display_name))) WHERE (hidden_at IS NULL);



CREATE INDEX clients_visible_last_ip_idx ON public.clients USING btree (last_ip) WHERE (hidden_at IS NULL);



CREATE INDEX clients_visible_status_idx ON public.clients USING btree (status, last_seen_at DESC) WHERE (hidden_at IS NULL);



CREATE INDEX gateway_sessions_client_status_idx ON public.gateway_sessions USING btree (client_id, status, last_seen_at DESC);



CREATE INDEX gateway_sessions_gateway_seen_idx ON public.gateway_sessions USING btree (gateway_id, last_seen_at DESC, id DESC);



CREATE INDEX monitoring_share_links_status_idx ON public.monitoring_share_links USING btree (revoked_at, expires_at, created_at DESC);



CREATE INDEX monitoring_share_targets_client_idx ON public.monitoring_share_targets USING btree (client_id, share_id);



CREATE INDEX monitoring_share_visitors_last_seen_idx ON public.monitoring_share_visitors USING btree (share_id, last_seen_at DESC);



CREATE INDEX operator_auth_throttle_locked_idx ON public.operator_auth_throttle USING btree (locked_until) WHERE (locked_until IS NOT NULL);



CREATE INDEX operator_sessions_operator_id_idx ON public.operator_sessions USING btree (operator_id);



-- Comments.

COMMENT ON COLUMN public.clients.suspended_at IS 'Current operator-approved monitoring/dispatch suspension boundary.';



COMMENT ON COLUMN public.clients.suspended_by IS 'Operator who initiated the current suspension; retained history owns past actors.';



COMMENT ON COLUMN public.clients.suspended_reason IS 'Optional operator reason for the current suspension.';



COMMENT ON COLUMN public.clients.suspended_from_status IS 'Non-online lifecycle state restored by manual unsuspend.';



COMMENT ON VIEW public.visible_clients IS 'Live operational VPS projection; use clients explicitly for lifecycle and historical evidence.';
