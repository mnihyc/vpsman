-- Tunnel display names are mutable metadata, not evidence identity.

CREATE OR REPLACE FUNCTION public.stamp_tunnel_plan_operational_alert_boundary()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF (OLD.plan - 'name') IS DISTINCT FROM (NEW.plan - 'name')
       OR OLD.builtin_credentials IS DISTINCT FROM NEW.builtin_credentials
       OR OLD.enabled IS DISTINCT FROM NEW.enabled
       OR OLD.deleted_at IS DISTINCT FROM NEW.deleted_at THEN
        NEW.operational_alert_runtime_boundary_at := clock_timestamp();
    END IF;
    RETURN NEW;
END;
$$;

CREATE OR REPLACE VIEW public.telemetry_current_tunnels AS
SELECT
    tunnel.*,
    current_plan.plan AS current_plan
FROM public.telemetry_tunnels tunnel
JOIN public.tunnel_plans current_plan
  ON current_plan.id = tunnel.telemetry_plan_id
 AND current_plan.deleted_at IS NULL
 AND current_plan.enabled
 AND current_plan.kind = tunnel.kind
 AND current_plan.plan->>'interface_name' = tunnel.interface
 AND (
    (
        tunnel.telemetry_endpoint_side = 'left'
        AND current_plan.left_client_id = tunnel.client_id
        AND current_plan.right_client_id = tunnel.telemetry_peer_client_id
    )
    OR (
        tunnel.telemetry_endpoint_side = 'right'
        AND current_plan.right_client_id = tunnel.client_id
        AND current_plan.left_client_id = tunnel.telemetry_peer_client_id
    )
 )
WHERE octet_length(tunnel.kind) BETWEEN 1 AND 64
  AND octet_length(tunnel.telemetry_plan_name) BETWEEN 1 AND 128;

-- Discard measurements attributed using the previous name-dependent hashes.
-- Remove leaf rows first; retain plans, jobs, credentials, and raw samples.
DELETE FROM public.network_observations
WHERE source = 'manual' AND plan_id IS NOT NULL;

DELETE FROM public.network_observations
WHERE source = 'automatic';

DELETE FROM public.network_observation_latest;
DELETE FROM public.network_observation_rollups;
DELETE FROM public.network_observation_series;
DELETE FROM public.telemetry_tunnels;

DELETE FROM public.telemetry_history_due_events
WHERE domain = 'network_observation_rollups';

DELETE FROM public.telemetry_history_due_spans
WHERE domain = 'network_observation_rollups';

-- Publish the new evidence identities through the existing configuration owner,
-- including visible endpoints that are currently offline.
SELECT public.enqueue_runtime_config_reconcile(
    ARRAY(
        SELECT endpoint.client_id
        FROM public.tunnel_plans plan
        CROSS JOIN LATERAL (VALUES
            (plan.left_client_id),
            (plan.right_client_id)
        ) endpoint(client_id)
        WHERE plan.enabled AND plan.deleted_at IS NULL
        GROUP BY endpoint.client_id
        ORDER BY endpoint.client_id
    ),
    'tunnel_plan_updated',
    NULL
);
