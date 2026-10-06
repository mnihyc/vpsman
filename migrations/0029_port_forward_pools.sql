-- NULL preserves every existing fixed mapping; pools are an explicit opt-in.
ALTER TABLE public.port_forward_rules ADD COLUMN pool jsonb;

ALTER TABLE public.port_forward_rules
    ADD CONSTRAINT port_forward_rules_pool_check CHECK (
        pool IS NULL OR (
            jsonb_typeof(pool) = 'object' AND mode IN ('dnat', 'custom_adapter')
            AND target_ip IS NULL AND target_hostname IS NULL AND mappings = '[]'::jsonb
        )
    ),
    DROP CONSTRAINT port_forward_rules_mode_fields_check,
    ADD CONSTRAINT port_forward_rules_mode_fields_check CHECK (
        (mode = 'dnat' AND adapter_definition_id IS NULL AND address_family IS NOT NULL AND address_family IN ('ipv4', 'ipv6')
            AND ((pool IS NULL AND target_ip IS NOT NULL
                AND address_family = CASE family(target_ip) WHEN 4 THEN 'ipv4' ELSE 'ipv6' END)
                OR (pool IS NOT NULL AND target_ip IS NULL)))
        OR (mode = 'redirect' AND pool IS NULL AND target_ip IS NULL AND target_hostname IS NULL
            AND address_family IS NOT NULL AND address_family IN ('ipv4', 'ipv6', 'both')
            AND adapter_definition_id IS NULL AND NOT masquerade)
        OR (mode = 'custom_adapter' AND address_family IS NULL AND NOT masquerade
            AND (adapter_definition_id IS NOT NULL OR removal_confirmed_at IS NOT NULL OR forgotten_at IS NOT NULL))
    );

DROP TRIGGER port_forward_rules_runtime_config_reconcile ON public.port_forward_rules;
CREATE TRIGGER port_forward_rules_runtime_config_reconcile
BEFORE INSERT OR DELETE OR UPDATE OF client_id, name, protocol, target_ip, target_hostname,
    mappings, pool, masquerade, enabled, revision, deleted_at, mode, address_family, adapter_definition_id
ON public.port_forward_rules
FOR EACH ROW EXECUTE FUNCTION public.produce_port_forward_reconcile();
