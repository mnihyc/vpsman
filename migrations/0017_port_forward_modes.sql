-- Native DNAT/REDIRECT and reusable external forwarding adapters.
ALTER TABLE public.network_adapter_definitions
    DROP CONSTRAINT network_adapter_definitions_kind_check,
    ADD CONSTRAINT network_adapter_definitions_kind_check
        CHECK (adapter_kind IN ('runtime_tunnel', 'routing_cost', 'port_forward'));

ALTER TABLE public.port_forward_rules
    ADD COLUMN mode text NOT NULL DEFAULT 'dnat',
    ADD COLUMN address_family text,
    ADD COLUMN adapter_definition_id uuid REFERENCES public.network_adapter_definitions(id) ON DELETE SET NULL,
    ALTER COLUMN target_ip DROP NOT NULL;

UPDATE public.port_forward_rules
SET address_family = CASE family(target_ip) WHEN 4 THEN 'ipv4' ELSE 'ipv6' END;

ALTER TABLE public.port_forward_rules
    ADD CONSTRAINT port_forward_rules_mode_check CHECK (mode IN ('dnat', 'redirect', 'custom_adapter')),
    ADD CONSTRAINT port_forward_rules_mode_fields_check CHECK (
        (mode = 'dnat' AND target_ip IS NOT NULL AND adapter_definition_id IS NULL AND address_family IS NOT NULL
            AND address_family = CASE family(target_ip) WHEN 4 THEN 'ipv4' ELSE 'ipv6' END)
        OR (mode = 'redirect' AND target_ip IS NULL AND target_hostname IS NULL
            AND address_family IS NOT NULL AND address_family IN ('ipv4', 'ipv6', 'both') AND adapter_definition_id IS NULL AND NOT masquerade)
        OR (mode = 'custom_adapter' AND address_family IS NULL AND NOT masquerade
            AND (adapter_definition_id IS NOT NULL OR removal_confirmed_at IS NOT NULL OR forgotten_at IS NOT NULL))
    );

CREATE INDEX port_forward_rules_adapter_definition_idx
    ON public.port_forward_rules (adapter_definition_id)
    WHERE adapter_definition_id IS NOT NULL;

-- Only adapters that can still own resources on a VPS are retained here.
-- Exact current apply/removal evidence retires superseded ownership.
CREATE TABLE public.port_forward_adapter_owners (
    rule_id uuid NOT NULL REFERENCES public.port_forward_rules(id) ON DELETE CASCADE,
    adapter_definition_id uuid NOT NULL REFERENCES public.network_adapter_definitions(id) ON DELETE RESTRICT,
    PRIMARY KEY (rule_id, adapter_definition_id)
);
CREATE INDEX port_forward_adapter_owners_definition_idx
    ON public.port_forward_adapter_owners (adapter_definition_id, rule_id);

DROP TRIGGER port_forward_rules_runtime_config_reconcile ON public.port_forward_rules;
CREATE OR REPLACE FUNCTION public.produce_port_forward_reconcile() RETURNS trigger
    LANGUAGE plpgsql AS $$
DECLARE
    affected text[] := ARRAY[]::text[];
    source_actor uuid;
BEGIN
    IF TG_OP <> 'INSERT' AND OLD.deleted_at IS NULL AND
       (OLD.enabled OR EXISTS (SELECT 1 FROM public.port_forward_adapter_owners WHERE rule_id=OLD.id)) THEN
        affected := affected || ARRAY[OLD.client_id];
        source_actor := OLD.actor_id;
    END IF;
    IF TG_OP <> 'DELETE' AND NEW.deleted_at IS NULL AND
       (NEW.enabled OR EXISTS (SELECT 1 FROM public.port_forward_adapter_owners WHERE rule_id=NEW.id)) THEN
        affected := affected || ARRAY[NEW.client_id];
        source_actor := COALESCE(NEW.actor_id, source_actor);
    END IF;
    IF cardinality(affected) > 0 THEN
        PERFORM public.enqueue_runtime_config_reconcile(affected, 'port_forward_rule_updated', source_actor);
    END IF;
    IF TG_OP = 'DELETE' THEN RETURN OLD; ELSE RETURN NEW; END IF;
END;
$$;

CREATE TRIGGER port_forward_rules_runtime_config_reconcile
BEFORE INSERT OR DELETE OR UPDATE OF client_id, name, protocol, target_ip, target_hostname, mappings, masquerade, enabled, revision, deleted_at, mode, address_family, adapter_definition_id ON public.port_forward_rules
FOR EACH ROW EXECUTE FUNCTION public.produce_port_forward_reconcile();
