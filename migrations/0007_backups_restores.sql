-- Backup artifacts, requests, restore plans, and migration links.

-- Tables.

CREATE TABLE public.backup_artifacts (
    id uuid NOT NULL,
    client_id text NOT NULL,
    object_key text NOT NULL,
    sha256_hex text NOT NULL,
    size_bytes bigint NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT backup_artifacts_pkey PRIMARY KEY (id),
    CONSTRAINT backup_artifacts_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.backup_policies (
    schedule_id uuid NOT NULL,
    retention_days integer DEFAULT 30 NOT NULL,
    keep_last integer DEFAULT 7 NOT NULL,
    rotation_generation text,
    retention_scanned_at timestamp with time zone,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT backup_policies_keep_last_check CHECK (((keep_last >= 1) AND (keep_last <= 1000))),
    CONSTRAINT backup_policies_retention_days_check CHECK (((retention_days >= 1) AND (retention_days <= 3650))),
    CONSTRAINT backup_policies_pkey PRIMARY KEY (schedule_id),
    CONSTRAINT backup_policies_schedule_id_fkey FOREIGN KEY (schedule_id) REFERENCES public.schedules(id) ON DELETE CASCADE
);



CREATE TABLE public.backup_requests (
    id uuid NOT NULL,
    actor_id uuid,
    client_id text NOT NULL,
    paths text[] DEFAULT ARRAY[]::text[] NOT NULL,
    include_config boolean DEFAULT false NOT NULL,
    follow_symlinks boolean DEFAULT false NOT NULL,
    missing_path_policy text DEFAULT 'fail'::text NOT NULL,
    status text NOT NULL,
    payload_hash text NOT NULL,
    command_scope text NOT NULL,
    artifact_id uuid,
    source_job_id uuid,
    source_schedule_id uuid,
    note text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    terminal_at timestamp with time zone,
    causation_id uuid,
    schedule_lineage uuid[] DEFAULT ARRAY[]::uuid[] NOT NULL,
    CONSTRAINT backup_requests_missing_path_policy_check CHECK ((missing_path_policy = ANY (ARRAY['fail'::text, 'skip'::text]))),
    CONSTRAINT backup_requests_schedule_lineage_check CHECK (public.alert_uuid_array_is_unique_bounded(schedule_lineage, 16)),
    CONSTRAINT backup_requests_status_check CHECK ((status = ANY (ARRAY['requested_metadata_only'::text, 'artifact_metadata_recorded'::text, 'execution_failed'::text, 'execution_canceled'::text]))),
    CONSTRAINT backup_requests_terminal_at_check CHECK (((status = ANY (ARRAY['execution_failed'::text, 'execution_canceled'::text])) = (terminal_at IS NOT NULL))),
    CONSTRAINT backup_requests_pkey PRIMARY KEY (id),
    CONSTRAINT backup_requests_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id),
    CONSTRAINT backup_requests_artifact_id_fkey FOREIGN KEY (artifact_id) REFERENCES public.backup_artifacts(id),
    CONSTRAINT backup_requests_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT backup_requests_source_job_id_fkey FOREIGN KEY (source_job_id) REFERENCES public.jobs(id),
    CONSTRAINT backup_requests_source_schedule_id_fkey FOREIGN KEY (source_schedule_id) REFERENCES public.schedules(id)
);



CREATE TABLE public.restore_plans (
    id uuid NOT NULL,
    actor_id uuid,
    source_backup_request_id uuid NOT NULL,
    source_client_id text NOT NULL,
    target_client_id text NOT NULL,
    paths text[] DEFAULT ARRAY[]::text[] NOT NULL,
    include_config boolean DEFAULT false NOT NULL,
    destination_root text,
    status text NOT NULL,
    payload_hash text NOT NULL,
    command_scope text NOT NULL,
    note text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT restore_plans_status_check CHECK ((status = 'planned_metadata_only'::text)),
    CONSTRAINT restore_plans_pkey PRIMARY KEY (id),
    CONSTRAINT restore_plans_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id),
    CONSTRAINT restore_plans_source_backup_request_id_fkey FOREIGN KEY (source_backup_request_id) REFERENCES public.backup_requests(id) ON DELETE CASCADE,
    CONSTRAINT restore_plans_source_client_id_fkey FOREIGN KEY (source_client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT restore_plans_target_client_id_fkey FOREIGN KEY (target_client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.migration_links (
    id uuid NOT NULL,
    actor_id uuid,
    restore_plan_id uuid NOT NULL,
    source_backup_request_id uuid NOT NULL,
    source_client_id text NOT NULL,
    target_client_id text NOT NULL,
    paths text[] DEFAULT ARRAY[]::text[] NOT NULL,
    include_config boolean DEFAULT false NOT NULL,
    destination_root text,
    status text NOT NULL,
    note text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT migration_links_status_check CHECK ((status = 'linked_metadata_only'::text)),
    CONSTRAINT migration_links_pkey PRIMARY KEY (id),
    CONSTRAINT migration_links_restore_plan_id_key UNIQUE (restore_plan_id),
    CONSTRAINT migration_links_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id),
    CONSTRAINT migration_links_restore_plan_id_fkey FOREIGN KEY (restore_plan_id) REFERENCES public.restore_plans(id) ON DELETE CASCADE,
    CONSTRAINT migration_links_source_backup_request_id_fkey FOREIGN KEY (source_backup_request_id) REFERENCES public.backup_requests(id) ON DELETE CASCADE,
    CONSTRAINT migration_links_source_client_id_fkey FOREIGN KEY (source_client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT migration_links_target_client_id_fkey FOREIGN KEY (target_client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



-- Indexes.

CREATE INDEX backup_artifacts_client_idx ON public.backup_artifacts USING btree (client_id);



CREATE INDEX backup_artifacts_created_idx ON public.backup_artifacts USING btree (created_at DESC, id DESC);



CREATE UNIQUE INDEX backup_artifacts_object_key_unique ON public.backup_artifacts USING btree (object_key);



CREATE INDEX backup_policies_retention_scan_idx ON public.backup_policies USING btree (retention_scanned_at NULLS FIRST, schedule_id);



CREATE INDEX backup_requests_client_created_idx ON public.backup_requests USING btree (client_id, created_at DESC, id DESC);



CREATE INDEX backup_requests_created_idx ON public.backup_requests USING btree (created_at DESC, id DESC);



CREATE INDEX backup_requests_failed_client_idx ON public.backup_requests USING btree (client_id, created_at DESC, id DESC) WHERE (status = 'execution_failed'::text);



CREATE INDEX backup_requests_source_schedule_created_idx ON public.backup_requests USING btree (source_schedule_id, client_id, created_at DESC, id DESC) WHERE (source_schedule_id IS NOT NULL);



CREATE INDEX backup_requests_status_created_idx ON public.backup_requests USING btree (status, created_at DESC, id DESC);



CREATE INDEX migration_links_source_created_idx ON public.migration_links USING btree (source_client_id, created_at DESC, id DESC);



CREATE INDEX migration_links_status_created_idx ON public.migration_links USING btree (status, created_at DESC, id DESC);



CREATE INDEX migration_links_target_created_idx ON public.migration_links USING btree (target_client_id, created_at DESC, id DESC);



CREATE INDEX restore_plans_source_client_idx ON public.restore_plans USING btree (source_client_id);



CREATE INDEX restore_plans_source_created_idx ON public.restore_plans USING btree (source_backup_request_id, created_at DESC, id DESC);



CREATE INDEX restore_plans_status_created_idx ON public.restore_plans USING btree (status, created_at DESC, id DESC);



CREATE INDEX restore_plans_target_created_idx ON public.restore_plans USING btree (target_client_id, created_at DESC, id DESC);
