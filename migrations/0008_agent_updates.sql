-- Agent release catalogs.

-- Tables.

CREATE TABLE public.agent_update_releases (
    id uuid NOT NULL,
    actor_id uuid,
    name text NOT NULL,
    version text NOT NULL,
    channel text NOT NULL,
    status text NOT NULL,
    artifact_sha256_hex text NOT NULL,
    artifact_url_sha256_hex text NOT NULL,
    size_bytes bigint,
    rollback_artifact_sha256_hex text,
    rollback_artifact_url_sha256_hex text,
    rollback_size_bytes bigint,
    notes text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT agent_update_releases_status_check CHECK ((status = 'published_external'::text)),
    CONSTRAINT agent_update_releases_name_version_channel_key UNIQUE (name, version, channel),
    CONSTRAINT agent_update_releases_pkey PRIMARY KEY (id),
    CONSTRAINT agent_update_releases_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id)
);



-- Indexes.

CREATE INDEX agent_update_releases_artifact_idx ON public.agent_update_releases USING btree (artifact_sha256_hex, created_at DESC);



CREATE INDEX agent_update_releases_channel_created_idx ON public.agent_update_releases USING btree (channel, created_at DESC, id DESC);



CREATE INDEX agent_update_releases_rollback_artifact_idx ON public.agent_update_releases USING btree (rollback_artifact_sha256_hex, created_at DESC) WHERE (rollback_artifact_sha256_hex IS NOT NULL);
