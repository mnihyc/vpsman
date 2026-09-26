-- Executable adapter edits use the existing per-client runtime reconciler.
-- Definitions remain referenced by stable IDs. Already accepted jobs retain
-- their immutable execution snapshots; agents adopt the new desired config.
CREATE OR REPLACE FUNCTION public.produce_network_adapter_reconcile() RETURNS trigger
LANGUAGE plpgsql AS $$
DECLARE
    affected text[];
BEGIN
    IF OLD.definition IS NOT DISTINCT FROM NEW.definition THEN
        RETURN NEW;
    END IF;

    IF NEW.adapter_kind = 'runtime_tunnel' THEN
        -- Canonical serialized plans retain the protocol's template_id names.
        SELECT array_agg(DISTINCT endpoint.client_id ORDER BY endpoint.client_id)
        INTO affected
        FROM (
            SELECT left_client_id AS client_id FROM public.tunnel_plans
            WHERE enabled AND deleted_at IS NULL
              AND plan #>> '{runtime_control,left_adapter_template_id}' = NEW.id::text
            UNION
            SELECT right_client_id AS client_id FROM public.tunnel_plans
            WHERE enabled AND deleted_at IS NULL
              AND plan #>> '{runtime_control,right_adapter_template_id}' = NEW.id::text
        ) endpoint;
        UPDATE public.tunnel_plans
        SET operational_alert_runtime_boundary_at = clock_timestamp()
        WHERE deleted_at IS NULL AND (
            plan #>> '{runtime_control,left_adapter_template_id}' = NEW.id::text
            OR plan #>> '{runtime_control,right_adapter_template_id}' = NEW.id::text
        );
    ELSIF NEW.adapter_kind = 'port_forward' THEN
        SELECT array_agg(DISTINCT rule.client_id ORDER BY rule.client_id)
        INTO affected
        FROM public.port_forward_rules rule
        WHERE (rule.adapter_definition_id = NEW.id AND rule.enabled AND rule.deleted_at IS NULL)
           OR EXISTS (
               SELECT 1 FROM public.port_forward_adapter_owners owner
               WHERE owner.rule_id = rule.id AND owner.adapter_definition_id = NEW.id
           );
    ELSIF NEW.adapter_kind = 'routing_cost' THEN
        -- An accepted old job keeps its immutable command payload, but may not
        -- certify the new adapter. Only affected endpoint verification is reset.
        -- This neither restarts OSPF nor changes reviewed/automatic control mode.
        -- Fence status/cost jobs whose adapters were resolved before this edit
        -- but which have not yet staged their immutable command snapshots.
        -- Revision-only changes do not enqueue runtime-tunnel reconciliation.
        UPDATE public.tunnel_plans
        SET revision = revision + 1,
            updated_at = clock_timestamp(),
            left_ospf_status = CASE
                WHEN plan #>> '{ospf,left_adapter_template_id}' = NEW.id::text
                THEN CASE WHEN enabled THEN 'unverified' ELSE 'disabled' END
                ELSE left_ospf_status END,
            right_ospf_status = CASE
                WHEN plan #>> '{ospf,right_adapter_template_id}' = NEW.id::text
                THEN CASE WHEN enabled THEN 'unverified' ELSE 'disabled' END
                ELSE right_ospf_status END,
            left_current_ospf_cost = CASE
                WHEN plan #>> '{ospf,left_adapter_template_id}' = NEW.id::text THEN NULL
                ELSE left_current_ospf_cost END,
            right_current_ospf_cost = CASE
                WHEN plan #>> '{ospf,right_adapter_template_id}' = NEW.id::text THEN NULL
                ELSE right_current_ospf_cost END,
            left_ospf_job_id = CASE
                WHEN plan #>> '{ospf,left_adapter_template_id}' = NEW.id::text THEN NULL
                ELSE left_ospf_job_id END,
            right_ospf_job_id = CASE
                WHEN plan #>> '{ospf,right_adapter_template_id}' = NEW.id::text THEN NULL
                ELSE right_ospf_job_id END,
            ospf_status = CASE WHEN enabled THEN 'unverified' ELSE 'disabled' END
        WHERE deleted_at IS NULL AND (
            plan #>> '{ospf,left_adapter_template_id}' = NEW.id::text
            OR plan #>> '{ospf,right_adapter_template_id}' = NEW.id::text
        );
    END IF;

    IF cardinality(affected) > 0 THEN
        PERFORM public.enqueue_runtime_config_reconcile(affected, 'network_adapter_definition_updated', NULL);
    END IF;
    RETURN NEW;
END;
$$;
