-- Network topology, tunnel plans, observations, and port forwarding.

SET LOCAL check_function_bodies = false;

-- Functions.

CREATE FUNCTION public.stamp_tunnel_plan_operational_alert_boundary() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF OLD.plan IS DISTINCT FROM NEW.plan
       OR OLD.builtin_credentials IS DISTINCT FROM NEW.builtin_credentials
       OR OLD.enabled IS DISTINCT FROM NEW.enabled
       OR OLD.deleted_at IS DISTINCT FROM NEW.deleted_at THEN
        NEW.operational_alert_runtime_boundary_at := clock_timestamp();
    END IF;
    RETURN NEW;
END;
$$;



-- Tables.

CREATE TABLE public.network_adapter_definitions (
    id uuid NOT NULL,
    adapter_kind text NOT NULL,
    name text NOT NULL,
    description text,
    definition jsonb NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT network_adapter_definitions_definition_object_check CHECK ((jsonb_typeof(definition) = 'object'::text)),
    CONSTRAINT network_adapter_definitions_description_check CHECK (((description IS NULL) OR (octet_length(description) <= 4096))),
    CONSTRAINT network_adapter_definitions_kind_check CHECK ((adapter_kind = ANY (ARRAY['runtime_tunnel'::text, 'routing_cost'::text]))),
    CONSTRAINT network_adapter_definitions_name_check CHECK (((length(name) > 0) AND (name = btrim(name)) AND (octet_length(name) <= 256) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT network_adapter_definitions_pkey PRIMARY KEY (id)
);



CREATE TABLE public.port_forward_rules (
    id uuid NOT NULL,
    actor_id uuid,
    client_id text NOT NULL,
    name text NOT NULL,
    protocol text NOT NULL,
    target_ip inet NOT NULL,
    target_hostname text,
    mappings jsonb NOT NULL,
    masquerade boolean DEFAULT true NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    deleted_reason text,
    removal_confirmed_at timestamp with time zone,
    forgotten_at timestamp with time zone,
    forgotten_by uuid,
    forget_reason text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT port_forward_rules_mappings_array_check CHECK ((jsonb_typeof(mappings) = 'array'::text)),
    CONSTRAINT port_forward_rules_name_check CHECK (((length(btrim(name)) >= 1) AND (length(btrim(name)) <= 128))),
    CONSTRAINT port_forward_rules_protocol_check CHECK ((protocol = ANY (ARRAY['tcp'::text, 'udp'::text, 'both'::text]))),
    CONSTRAINT port_forward_rules_revision_check CHECK ((revision >= 1)),
    CONSTRAINT port_forward_rules_target_hostname_check CHECK (((target_hostname IS NULL) OR (((length(target_hostname) >= 1) AND (length(target_hostname) <= 253)) AND (target_hostname = lower(target_hostname)) AND (target_hostname = btrim(target_hostname)) AND (target_hostname ~ '^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?(\.[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$'::text) AND (target_hostname !~ '^((25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])\.){3}(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])$'::text)))),
    CONSTRAINT port_forward_rules_pkey PRIMARY KEY (id),
    CONSTRAINT port_forward_rules_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id) ON DELETE SET NULL,
    CONSTRAINT port_forward_rules_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE RESTRICT,
    CONSTRAINT port_forward_rules_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.operators(id) ON DELETE SET NULL,
    CONSTRAINT port_forward_rules_forgotten_by_fkey FOREIGN KEY (forgotten_by) REFERENCES public.operators(id) ON DELETE SET NULL
);



CREATE TABLE public.port_forward_runtime_state (
    client_id text NOT NULL,
    snapshot jsonb NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT port_forward_runtime_state_pkey PRIMARY KEY (client_id),
    CONSTRAINT port_forward_runtime_state_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);



CREATE TABLE public.tunnel_plans (
    id uuid NOT NULL,
    actor_id uuid,
    name text NOT NULL,
    kind text NOT NULL,
    enabled boolean DEFAULT true NOT NULL,
    revision bigint DEFAULT 1 NOT NULL,
    left_client_id text NOT NULL,
    right_client_id text NOT NULL,
    input jsonb NOT NULL,
    plan jsonb NOT NULL,
    builtin_credentials jsonb,
    recommended_ospf_cost integer,
    ospf_status text DEFAULT 'disabled'::text NOT NULL,
    left_ospf_status text DEFAULT 'disabled'::text NOT NULL,
    right_ospf_status text DEFAULT 'disabled'::text NOT NULL,
    desired_ospf_cost integer,
    left_current_ospf_cost integer,
    right_current_ospf_cost integer,
    left_ospf_job_id uuid,
    right_ospf_job_id uuid,
    connection_assessment text DEFAULT 'automatic'::text NOT NULL,
    connection_assessment_note text,
    connection_assessed_at timestamp with time zone,
    connection_assessed_by uuid,
    automatic_ospf_scanned_at timestamp with time zone,
    pending_ospf_reconciled_at timestamp with time zone,
    deleted_at timestamp with time zone,
    deleted_by uuid,
    deleted_reason text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    operational_alert_runtime_boundary_at timestamp with time zone DEFAULT clock_timestamp() NOT NULL,
    CONSTRAINT tunnel_plans_connection_assessment_check CHECK ((connection_assessment = ANY (ARRAY['automatic'::text, 'connected'::text, 'disconnected'::text]))),
    CONSTRAINT tunnel_plans_connection_assessment_note_check CHECK ((((connection_assessment = 'automatic'::text) AND (connection_assessment_note IS NULL) AND (connection_assessed_at IS NULL) AND (connection_assessed_by IS NULL)) OR ((connection_assessment = ANY (ARRAY['connected'::text, 'disconnected'::text])) AND (connection_assessment_note IS NOT NULL) AND ((length(btrim(connection_assessment_note)) >= 1) AND (length(btrim(connection_assessment_note)) <= 500)) AND (connection_assessed_at IS NOT NULL) AND (connection_assessed_by IS NOT NULL)))),
    CONSTRAINT tunnel_plans_desired_ospf_cost_check CHECK (((desired_ospf_cost IS NULL) OR ((desired_ospf_cost >= 1) AND (desired_ospf_cost <= 65535)))),
    CONSTRAINT tunnel_plans_left_ospf_cost_check CHECK (((left_current_ospf_cost IS NULL) OR ((left_current_ospf_cost >= 1) AND (left_current_ospf_cost <= 65535)))),
    CONSTRAINT tunnel_plans_left_ospf_status_check CHECK ((left_ospf_status = ANY (ARRAY['disabled'::text, 'unverified'::text, 'pending'::text, 'verified'::text, 'failed'::text, 'stale'::text]))),
    CONSTRAINT tunnel_plans_name_check CHECK (((octet_length(name) >= 1) AND (octet_length(name) <= 128) AND (length(btrim(name)) >= 1) AND (name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT tunnel_plans_ospf_status_check CHECK ((ospf_status = ANY (ARRAY['disabled'::text, 'unverified'::text, 'pending'::text, 'verified'::text, 'partial'::text, 'failed'::text, 'stale'::text]))),
    CONSTRAINT tunnel_plans_revision_check CHECK ((revision >= 1)),
    CONSTRAINT tunnel_plans_right_ospf_cost_check CHECK (((right_current_ospf_cost IS NULL) OR ((right_current_ospf_cost >= 1) AND (right_current_ospf_cost <= 65535)))),
    CONSTRAINT tunnel_plans_right_ospf_status_check CHECK ((right_ospf_status = ANY (ARRAY['disabled'::text, 'unverified'::text, 'pending'::text, 'verified'::text, 'failed'::text, 'stale'::text]))),
    CONSTRAINT tunnel_plans_pkey PRIMARY KEY (id),
    CONSTRAINT tunnel_plans_actor_id_fkey FOREIGN KEY (actor_id) REFERENCES public.operators(id),
    CONSTRAINT tunnel_plans_connection_assessed_by_fkey FOREIGN KEY (connection_assessed_by) REFERENCES public.operators(id) ON DELETE SET NULL,
    CONSTRAINT tunnel_plans_deleted_by_fkey FOREIGN KEY (deleted_by) REFERENCES public.operators(id) ON DELETE SET NULL,
    CONSTRAINT tunnel_plans_left_client_id_fkey FOREIGN KEY (left_client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT tunnel_plans_right_client_id_fkey FOREIGN KEY (right_client_id) REFERENCES public.clients(id) ON DELETE CASCADE
);

CREATE TABLE public.telemetry_tunnels (
    client_id text NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    interface text NOT NULL,
    kind text NOT NULL,
    ownership_mode text NOT NULL,
    mutation_policy text NOT NULL,
    source text NOT NULL,
    operstate text,
    mtu bigint,
    link_type bigint,
    address text,
    rx_bytes bigint DEFAULT 0 NOT NULL,
    tx_bytes bigint DEFAULT 0 NOT NULL,
    counters_admitted_at_projection boolean DEFAULT false NOT NULL,
    traffic_source text,
    traffic_status text,
    traffic_reason text,
    traffic_checked_unix bigint,
    telemetry_plan_id uuid NOT NULL,
    telemetry_plan_name text NOT NULL,
    telemetry_plan_runtime_manager text,
    telemetry_endpoint_side text,
    telemetry_peer_client_id text,
    adapter_health jsonb,
    latency_monitoring_enabled boolean,
    latency_status text,
    latency_reason text,
    latency_primary_family text,
    latency_target text,
    latency_checked_unix bigint,
    latency_avg_ms double precision,
    packet_loss_ratio double precision,
    latency_healthy_windows integer,
    latency_missed_windows integer,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    telemetry_topology_identity_hash text,
    telemetry_runtime_evidence_identity_hash text,
    CONSTRAINT telemetry_tunnels_plan_name_check CHECK (((octet_length(telemetry_plan_name) >= 1) AND (octet_length(telemetry_plan_name) <= 128) AND (length(btrim(telemetry_plan_name)) >= 1) AND (telemetry_plan_name !~ '[[:cntrl:]]'::text))),
    CONSTRAINT telemetry_tunnels_runtime_evidence_identity_hash_check CHECK (((telemetry_runtime_evidence_identity_hash IS NULL) OR (telemetry_runtime_evidence_identity_hash ~ '^[0-9a-f]{64}$'::text))),
    CONSTRAINT telemetry_tunnels_topology_identity_hash_check CHECK (((telemetry_topology_identity_hash IS NULL) OR (telemetry_topology_identity_hash ~ '^[0-9a-f]{64}$'::text))),
    CONSTRAINT telemetry_tunnels_pkey PRIMARY KEY (client_id, interface),
    CONSTRAINT telemetry_tunnels_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT telemetry_tunnels_plan_id_fkey FOREIGN KEY (telemetry_plan_id) REFERENCES public.tunnel_plans(id) ON DELETE CASCADE
);



CREATE TABLE public.network_observation_series (
    id bigint GENERATED ALWAYS AS IDENTITY (
        SEQUENCE NAME public.network_observation_series_id_seq
        START WITH 1 INCREMENT BY 1 NO MINVALUE NO MAXVALUE CACHE 1
    ) NOT NULL,
    plan_id uuid NOT NULL,
    topology_identity_hash text NOT NULL,
    plan_name text NOT NULL,
    interface_name text NOT NULL,
    client_id text NOT NULL,
    peer_client_id text NOT NULL,
    endpoint_side text NOT NULL,
    address_family text NOT NULL,
    target text NOT NULL,
    active boolean DEFAULT true NOT NULL,
    last_seen_at timestamp with time zone DEFAULT now() NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT network_observation_series_address_family_check CHECK ((address_family = ANY (ARRAY['ipv4'::text, 'ipv6'::text]))),
    CONSTRAINT network_observation_series_endpoint_side_check CHECK ((endpoint_side = ANY (ARRAY['left'::text, 'right'::text]))),
    CONSTRAINT network_observation_series_pkey PRIMARY KEY (id),
    CONSTRAINT network_observation_series_plan_id_topology_identity_hash_c_key UNIQUE (plan_id, topology_identity_hash, client_id, peer_client_id, endpoint_side, address_family, interface_name, target),
    CONSTRAINT network_observation_series_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT network_observation_series_peer_client_id_fkey FOREIGN KEY (peer_client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT network_observation_series_plan_id_fkey FOREIGN KEY (plan_id) REFERENCES public.tunnel_plans(id) ON DELETE CASCADE
);



CREATE TABLE public.network_observation_latest (
    series_id bigint NOT NULL,
    observation_id uuid NOT NULL,
    stale_after_secs bigint NOT NULL,
    healthy boolean NOT NULL,
    transmitted integer NOT NULL,
    received integer NOT NULL,
    latency_min_ms double precision,
    latency_avg_ms double precision,
    latency_max_ms double precision,
    latency_mdev_ms double precision,
    packet_loss_ratio double precision NOT NULL,
    reason text,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    observed_at timestamp with time zone NOT NULL,
    received_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT network_observation_latest_check CHECK (((received >= 0) AND (received <= transmitted))),
    CONSTRAINT network_observation_latest_packet_loss_check CHECK (((packet_loss_ratio >= (0.0)::double precision) AND (packet_loss_ratio <= (1.0)::double precision))),
    CONSTRAINT network_observation_latest_stale_after_secs_check CHECK ((stale_after_secs >= 1)),
    CONSTRAINT network_observation_latest_transmitted_check CHECK ((transmitted >= 0)),
    CONSTRAINT network_observation_latest_observation_id_key UNIQUE (observation_id),
    CONSTRAINT network_observation_latest_pkey PRIMARY KEY (series_id),
    CONSTRAINT network_observation_latest_series_id_fkey FOREIGN KEY (series_id) REFERENCES public.network_observation_series(id) ON DELETE CASCADE
);



CREATE TABLE public.network_observation_rollups (
    series_id bigint NOT NULL,
    bucket_secs integer NOT NULL,
    bucket_start timestamp with time zone NOT NULL,
    health_state smallint NOT NULL,
    sample_count bigint NOT NULL,
    transmitted_total numeric(38,0) NOT NULL,
    transmitted_sample_count bigint NOT NULL,
    received_total numeric(38,0) NOT NULL,
    received_sample_count bigint NOT NULL,
    latency_sum_ms double precision DEFAULT 0.0 NOT NULL,
    latency_sample_count bigint NOT NULL,
    latency_min_ms double precision,
    latency_max_ms double precision,
    latency_mdev_sum_ms double precision DEFAULT 0.0 NOT NULL,
    latency_mdev_sample_count bigint NOT NULL,
    packet_loss_sum_ratio double precision DEFAULT 0.0 NOT NULL,
    packet_loss_sample_count bigint NOT NULL,
    packet_loss_min_ratio double precision,
    packet_loss_max_ratio double precision,
    latest_observation_id uuid NOT NULL,
    latest_stale_after_secs bigint NOT NULL,
    latest_healthy boolean NOT NULL,
    latest_transmitted integer NOT NULL,
    latest_received integer NOT NULL,
    latest_latency_min_ms double precision,
    latest_latency_avg_ms double precision,
    latest_latency_max_ms double precision,
    latest_latency_mdev_ms double precision,
    latest_packet_loss_ratio double precision NOT NULL,
    latest_reason text,
    latest_observed_at timestamp with time zone NOT NULL,
    latest_received_at timestamp with time zone NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT network_observation_rollups_bucket_alignment_check CHECK ((((EXTRACT(epoch FROM bucket_start))::bigint % (bucket_secs)::bigint) = 0)),
    CONSTRAINT network_observation_rollups_bucket_secs_check CHECK ((bucket_secs = ANY (ARRAY[60, 300, 1800, 3600, 10800, 21600, 86400]))),
    CONSTRAINT network_observation_rollups_check CHECK (((latest_received >= 0) AND (latest_received <= latest_transmitted))),
    CONSTRAINT network_observation_rollups_health_state_check CHECK ((health_state = ANY (ARRAY[0, 1]))),
    CONSTRAINT network_observation_rollups_latency_count_check CHECK ((((latency_sample_count = 0) AND (latency_min_ms IS NULL) AND (latency_max_ms IS NULL)) OR ((latency_sample_count > 0) AND (latency_min_ms IS NOT NULL) AND (latency_max_ms IS NOT NULL)))),
    CONSTRAINT network_observation_rollups_latency_mdev_sample_count_check CHECK ((latency_mdev_sample_count >= 0)),
    CONSTRAINT network_observation_rollups_latency_sample_count_check CHECK ((latency_sample_count >= 0)),
    CONSTRAINT network_observation_rollups_latest_packet_loss_check CHECK (((latest_packet_loss_ratio >= (0.0)::double precision) AND (latest_packet_loss_ratio <= (1.0)::double precision))),
    CONSTRAINT network_observation_rollups_latest_stale_after_secs_check CHECK ((latest_stale_after_secs >= 1)),
    CONSTRAINT network_observation_rollups_latest_transmitted_check CHECK ((latest_transmitted >= 0)),
    CONSTRAINT network_observation_rollups_packet_loss_count_check CHECK ((((packet_loss_sample_count = 0) AND (packet_loss_min_ratio IS NULL) AND (packet_loss_max_ratio IS NULL)) OR ((packet_loss_sample_count > 0) AND (packet_loss_min_ratio IS NOT NULL) AND (packet_loss_max_ratio IS NOT NULL)))),
    CONSTRAINT network_observation_rollups_packet_loss_sample_count_check CHECK ((packet_loss_sample_count >= 0)),
    CONSTRAINT network_observation_rollups_received_sample_count_check CHECK ((received_sample_count >= 0)),
    CONSTRAINT network_observation_rollups_received_total_check CHECK ((received_total >= (0)::numeric)),
    CONSTRAINT network_observation_rollups_sample_count_check CHECK ((sample_count > 0)),
    CONSTRAINT network_observation_rollups_transmitted_sample_count_check CHECK ((transmitted_sample_count >= 0)),
    CONSTRAINT network_observation_rollups_transmitted_total_check CHECK ((transmitted_total >= (0)::numeric)),
    CONSTRAINT network_observation_rollups_pkey PRIMARY KEY (series_id, bucket_secs, bucket_start, health_state),
    CONSTRAINT network_observation_rollups_series_id_fkey FOREIGN KEY (series_id) REFERENCES public.network_observation_series(id) ON DELETE CASCADE
);



CREATE TABLE public.network_observations (
    id uuid NOT NULL,
    job_id uuid,
    client_id text,
    seq integer,
    kind text,
    role text,
    plan_id uuid,
    topology_identity_hash text,
    plan_name text NOT NULL,
    interface_name text,
    peer_client_id text,
    target text,
    endpoint_side text,
    address_family text,
    stale_after_secs bigint,
    healthy boolean,
    transmitted integer,
    received integer,
    latency_min_ms double precision,
    latency_avg_ms double precision,
    latency_max_ms double precision,
    latency_mdev_ms double precision,
    packet_loss_ratio double precision,
    reason text,
    throughput_mbps double precision,
    bytes bigint,
    source text DEFAULT 'manual'::text NOT NULL,
    automatic_series_id bigint,
    automatic_sample_id uuid,
    automatic_payload_ordinal smallint,
    metadata jsonb DEFAULT '{}'::jsonb NOT NULL,
    observed_at timestamp with time zone DEFAULT now() NOT NULL,
    received_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT network_observations_address_family_check CHECK (((address_family IS NULL) OR (address_family = ANY (ARRAY['ipv4'::text, 'ipv6'::text])))),
    CONSTRAINT network_observations_automatic_series_check CHECK (
        (
            source = 'automatic'::text
            AND automatic_series_id IS NOT NULL
            AND automatic_sample_id IS NOT NULL
            AND automatic_payload_ordinal IS NOT NULL
            AND automatic_payload_ordinal BETWEEN 1 AND 512
            AND job_id IS NULL
            AND client_id IS NULL
            AND seq IS NULL
            AND kind IS NULL
            AND role IS NULL
            AND plan_id IS NULL
            AND topology_identity_hash IS NULL
            AND interface_name IS NULL
            AND peer_client_id IS NULL
            AND target IS NULL
            AND endpoint_side IS NULL
            AND address_family IS NULL
            AND stale_after_secs IS NULL
            AND healthy IS NULL
            AND transmitted IS NULL
            AND received IS NULL
            AND latency_min_ms IS NULL
            AND latency_avg_ms IS NULL
            AND latency_max_ms IS NULL
            AND latency_mdev_ms IS NULL
            AND packet_loss_ratio IS NULL
            AND reason IS NULL
            AND throughput_mbps IS NULL
            AND bytes IS NULL
            AND metadata = '{}'::jsonb
        )
        OR (
            source = 'manual'::text
            AND automatic_series_id IS NULL
            AND automatic_sample_id IS NULL
            AND automatic_payload_ordinal IS NULL
            AND client_id IS NOT NULL
            AND kind IS NOT NULL
            AND plan_id IS NOT NULL
            AND topology_identity_hash IS NOT NULL
            AND interface_name IS NOT NULL
            AND peer_client_id IS NOT NULL
        )
    ),
    CONSTRAINT network_observations_endpoint_side_check CHECK (((endpoint_side IS NULL) OR (endpoint_side = ANY (ARRAY['left'::text, 'right'::text])))),
    CONSTRAINT network_observations_packet_counts_check CHECK (((transmitted IS NULL) OR (received IS NULL) OR ((transmitted >= 0) AND (received >= 0) AND (received <= transmitted)))),
    CONSTRAINT network_observations_source_check CHECK ((source = ANY (ARRAY['automatic'::text, 'manual'::text]))),
    CONSTRAINT network_observations_stale_after_check CHECK (((stale_after_secs IS NULL) OR (stale_after_secs >= 1))),
    CONSTRAINT network_observations_pkey PRIMARY KEY (id),
    CONSTRAINT network_observations_automatic_sample_id_fkey FOREIGN KEY (automatic_sample_id) REFERENCES public.telemetry_samples(id) ON DELETE CASCADE,
    CONSTRAINT network_observations_automatic_series_id_fkey FOREIGN KEY (automatic_series_id) REFERENCES public.network_observation_series(id) ON DELETE CASCADE,
    CONSTRAINT network_observations_client_id_fkey FOREIGN KEY (client_id) REFERENCES public.clients(id) ON DELETE CASCADE,
    CONSTRAINT network_observations_job_id_client_id_fkey FOREIGN KEY (job_id, client_id) REFERENCES public.job_targets(job_id, client_id) ON DELETE CASCADE,
    CONSTRAINT network_observations_job_id_fkey FOREIGN KEY (job_id) REFERENCES public.jobs(id) ON DELETE CASCADE,
    CONSTRAINT network_observations_plan_id_fkey FOREIGN KEY (plan_id) REFERENCES public.tunnel_plans(id) ON DELETE RESTRICT
);



-- Views.

CREATE VIEW public.network_observation_exact_evidence AS
 SELECT observation.id,
    observation.job_id,
    observation.client_id,
    observation.seq,
    observation.kind,
    observation.source,
    observation.role,
    observation.plan_id,
    observation.topology_identity_hash,
    observation.plan_name,
    observation.interface_name,
    observation.peer_client_id,
    observation.target,
    observation.endpoint_side,
    observation.address_family,
    observation.stale_after_secs,
    observation.healthy,
    observation.transmitted,
    observation.received,
    observation.latency_min_ms,
    observation.latency_avg_ms,
    observation.latency_max_ms,
    observation.latency_mdev_ms,
    observation.packet_loss_ratio,
    observation.reason,
    observation.throughput_mbps,
    observation.bytes,
    observation.metadata,
    observation.observed_at,
    observation.received_at
   FROM public.network_observations observation
  WHERE observation.source = 'manual'
UNION ALL
 SELECT observation.id,
    NULL::uuid AS job_id,
    series.client_id,
    NULL::integer AS seq,
    'tunnel_reachability'::text AS kind,
    'automatic'::text AS source,
    'endpoint'::text AS role,
    series.plan_id,
    series.topology_identity_hash,
    observation.plan_name,
    series.interface_name,
    series.peer_client_id,
    series.target,
    series.endpoint_side,
    series.address_family,
    (raw.observation ->> 'stale_after_secs'::text)::bigint AS stale_after_secs,
    (raw.observation ->> 'healthy'::text)::boolean AS healthy,
    (raw.observation ->> 'transmitted'::text)::integer AS transmitted,
    (raw.observation ->> 'received'::text)::integer AS received,
    (raw.observation ->> 'latency_min_ms'::text)::double precision AS latency_min_ms,
    (raw.observation ->> 'latency_avg_ms'::text)::double precision AS latency_avg_ms,
    (raw.observation ->> 'latency_max_ms'::text)::double precision AS latency_max_ms,
    (raw.observation ->> 'latency_mdev_ms'::text)::double precision AS latency_mdev_ms,
    (raw.observation ->> 'packet_loss_ratio'::text)::double precision AS packet_loss_ratio,
    raw.observation ->> 'reason'::text AS reason,
    NULL::double precision AS throughput_mbps,
    NULL::bigint AS bytes,
    jsonb_build_object('type', 'tunnel_reachability', 'source', 'automatic') AS metadata,
    observation.observed_at,
    observation.received_at
   FROM public.network_observations observation
   JOIN public.network_observation_series series
     ON series.id = observation.automatic_series_id
   JOIN public.telemetry_samples sample
     ON sample.id = observation.automatic_sample_id
   CROSS JOIN LATERAL (
       SELECT sample.payload -> 'tunnel_reachability'::text
                    -> (observation.automatic_payload_ordinal::integer - 1)
                    AS observation
   ) raw
  WHERE observation.source = 'automatic'
    AND raw.observation IS NOT NULL
    AND (raw.observation ->> 'id'::text)::uuid = observation.id
UNION ALL
 SELECT latest.observation_id AS id,
    NULL::uuid AS job_id,
    series.client_id,
    NULL::integer AS seq,
    'tunnel_reachability'::text AS kind,
    'automatic'::text AS source,
    'endpoint'::text AS role,
    series.plan_id,
    series.topology_identity_hash,
    series.plan_name,
    series.interface_name,
    series.peer_client_id,
    series.target,
    series.endpoint_side,
    series.address_family,
    latest.stale_after_secs,
    latest.healthy,
    latest.transmitted,
    latest.received,
    latest.latency_min_ms,
    latest.latency_avg_ms,
    latest.latency_max_ms,
    latest.latency_mdev_ms,
    latest.packet_loss_ratio,
    latest.reason,
    NULL::double precision AS throughput_mbps,
    NULL::bigint AS bytes,
    latest.metadata,
    latest.observed_at,
    latest.received_at
   FROM (public.network_observation_latest latest
     JOIN public.network_observation_series series ON ((series.id = latest.series_id)))
  WHERE ((series.active = true) AND (NOT (EXISTS ( SELECT 1
           FROM public.network_observations observation
          WHERE (observation.id = latest.observation_id)))));



-- Indexes.

CREATE UNIQUE INDEX network_adapter_definitions_name_idx ON public.network_adapter_definitions USING btree (adapter_kind, lower(name));



CREATE INDEX network_observation_latest_observed_idx ON public.network_observation_latest USING btree (observed_at DESC, observation_id DESC);



CREATE INDEX network_observation_rollups_retention_idx ON public.network_observation_rollups USING btree (bucket_start, series_id) INCLUDE (bucket_secs);



CREATE INDEX network_observation_rollups_terminal_frontier_idx ON public.network_observation_rollups USING btree (bucket_secs, bucket_start, series_id, health_state);



CREATE INDEX network_observation_rollups_series_time_idx ON public.network_observation_rollups USING btree (series_id, bucket_start, bucket_secs, health_state);



CREATE INDEX network_observation_series_inactive_idx ON public.network_observation_series USING btree (last_seen_at, id) WHERE (active = false);



CREATE INDEX network_observation_series_active_client_idx ON public.network_observation_series USING btree (client_id, id) WHERE (active IS TRUE);



CREATE INDEX network_observation_series_plan_identity_idx ON public.network_observation_series USING btree (plan_id, topology_identity_hash, endpoint_side);



CREATE INDEX network_observations_automatic_series_observed_idx ON public.network_observations USING btree (automatic_series_id, observed_at DESC, id DESC) WHERE (automatic_series_id IS NOT NULL);



CREATE UNIQUE INDEX network_observations_automatic_sample_ordinal_idx ON public.network_observations USING btree (automatic_sample_id, automatic_payload_ordinal) WHERE (automatic_sample_id IS NOT NULL);



CREATE INDEX network_observations_client_observed_idx ON public.network_observations USING btree (client_id, observed_at DESC, id DESC) WHERE (source = 'manual'::text);



CREATE UNIQUE INDEX network_observations_job_sequence_unique ON public.network_observations USING btree (job_id, client_id, seq) WHERE ((job_id IS NOT NULL) AND (seq IS NOT NULL));



CREATE INDEX network_observations_kind_observed_idx ON public.network_observations USING btree (kind, observed_at DESC, id DESC) WHERE (source = 'manual'::text);



-- Manual evidence is the only exact-history class owned by the network
-- terminal worker.  Its global time frontier cannot use the client/kind-led
-- route indexes, while the unfiltered global time index below remains the
-- independent owner of mixed-source exact-history reads.
CREATE INDEX network_observations_manual_retention_idx ON public.network_observations USING btree (observed_at, id) WHERE (source = 'manual'::text);



CREATE INDEX network_observations_peer_client_observed_idx ON public.network_observations USING btree (peer_client_id, observed_at DESC, id DESC) WHERE (source = 'manual'::text);



CREATE INDEX network_observations_plan_identity_kind_observed_idx ON public.network_observations USING btree (plan_id, topology_identity_hash, kind, observed_at DESC, id DESC) WHERE (source = 'manual'::text);



CREATE INDEX network_observations_plan_identity_observed_idx ON public.network_observations USING btree (plan_id, topology_identity_hash, observed_at DESC, id DESC) WHERE (source = 'manual'::text);



CREATE INDEX network_observations_plan_kind_observed_idx ON public.network_observations USING btree (plan_id, kind, endpoint_side, observed_at DESC, id DESC) WHERE ((source = 'manual'::text) AND (kind = ANY (ARRAY['tunnel_reachability'::text, 'network_speed_test'::text])));



CREATE INDEX network_observations_observed_idx ON public.network_observations USING btree (observed_at DESC, id DESC);



CREATE INDEX network_observations_status_endpoint_observed_idx ON public.network_observations USING btree (plan_id, topology_identity_hash, client_id, observed_at DESC, id DESC) WHERE ((source = 'manual'::text) AND (kind = 'network_status'::text));



CREATE UNIQUE INDEX port_forward_rules_active_name_idx ON public.port_forward_rules USING btree (client_id, name) WHERE (deleted_at IS NULL);



CREATE INDEX port_forward_rules_client_state_idx ON public.port_forward_rules USING btree (client_id, enabled, updated_at DESC) WHERE (forgotten_at IS NULL);



CREATE INDEX port_forward_rules_removal_pending_idx ON public.port_forward_rules USING btree (client_id, deleted_at) WHERE ((deleted_at IS NOT NULL) AND (removal_confirmed_at IS NULL) AND (forgotten_at IS NULL));



CREATE INDEX telemetry_tunnels_latest_idx ON public.telemetry_tunnels USING btree (observed_at DESC, client_id, interface);



CREATE INDEX tunnel_plans_active_clients_idx ON public.tunnel_plans USING btree (left_client_id, right_client_id, updated_at DESC) WHERE (deleted_at IS NULL);



CREATE UNIQUE INDEX tunnel_plans_active_name_idx ON public.tunnel_plans USING btree (name) WHERE (deleted_at IS NULL);



CREATE INDEX tunnel_plans_automatic_controller_scan_idx ON public.tunnel_plans USING btree (automatic_ospf_scanned_at NULLS FIRST, id) WHERE ((deleted_at IS NULL) AND (enabled = true) AND (((plan -> 'ospf'::text) ->> 'mode'::text) = 'automatic'::text));



CREATE INDEX tunnel_plans_current_right_interface_idx ON public.tunnel_plans USING btree (right_client_id, ((plan ->> 'interface_name'::text)), id) WHERE ((deleted_at IS NULL) AND (enabled IS TRUE));



CREATE INDEX tunnel_plans_clients_idx ON public.tunnel_plans USING btree (left_client_id, right_client_id);



CREATE INDEX tunnel_plans_ospf_status_idx ON public.tunnel_plans USING btree (ospf_status, updated_at DESC);



CREATE INDEX tunnel_plans_pending_controller_scan_idx ON public.tunnel_plans USING btree (pending_ospf_reconciled_at NULLS FIRST, id) WHERE ((deleted_at IS NULL) AND (ospf_status = 'pending'::text));



-- Triggers.

CREATE TRIGGER network_observation_rollups_due_events_insert
AFTER INSERT ON public.network_observation_rollups
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'network_observation_rollups'
);



CREATE TRIGGER network_observation_rollups_due_events_update
AFTER UPDATE ON public.network_observation_rollups
REFERENCING NEW TABLE AS new_telemetry_history_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(
    'network_observation_rollups'
);



CREATE TRIGGER network_observation_rollups_retention_delete
AFTER DELETE ON public.network_observation_rollups
REFERENCING OLD TABLE AS old_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'network_observation_history_deleted'
);



CREATE TRIGGER network_observation_latest_retention_delete
AFTER DELETE ON public.network_observation_latest
REFERENCING OLD TABLE AS old_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'network_observation_latest_deleted'
);



CREATE TRIGGER network_observation_series_retention_deactivate
AFTER UPDATE ON public.network_observation_series
REFERENCING OLD TABLE AS old_telemetry_retention_rows
            NEW TABLE AS new_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'network_observation_series_deactivated'
);



CREATE TRIGGER network_observations_retention_delete
AFTER DELETE ON public.network_observations
REFERENCING OLD TABLE AS old_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'network_observation_history_deleted'
);



CREATE TRIGGER network_observations_retention_publish_insert
AFTER INSERT ON public.network_observations
REFERENCING NEW TABLE AS new_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'network_observation_history_published'
);



CREATE TRIGGER network_observations_retention_publish_update
AFTER UPDATE ON public.network_observations
REFERENCING NEW TABLE AS new_telemetry_retention_rows
FOR EACH STATEMENT
EXECUTE FUNCTION public.publish_telemetry_retention_effect(
    'network_observation_history_published'
);



CREATE TRIGGER tunnel_plans_operational_alert_boundary_trigger BEFORE UPDATE OF plan, builtin_credentials, enabled, deleted_at ON public.tunnel_plans FOR EACH ROW EXECUTE FUNCTION public.stamp_tunnel_plan_operational_alert_boundary();
