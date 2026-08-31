-- Restore the established live-rate selector tri-state without changing the
-- outer network.interfaces admission boundary. An absent rate rule follows
-- traffic.selectors; an exact rule with no selectors remains explicitly none.
CREATE OR REPLACE FUNCTION public.telemetry_dashboard_effective_network_selection(
    p_client_id TEXT
)
RETURNS public.telemetry_dashboard_network_selection
LANGUAGE plpgsql
STABLE
AS $$
DECLARE
    interface_rule JSONB;
    rate_rule JSONB;
    traffic_rule JSONB;
    interface_mode TEXT;
    rate_mode TEXT;
    traffic_mode TEXT;
    requested_all BOOLEAN := FALSE;
    admitted_all BOOLEAN := FALSE;
    requested_interfaces TEXT[] := ARRAY[]::TEXT[];
    admitted_patterns TEXT[] := ARRAY[]::TEXT[];
    interfaces TEXT[] := ARRAY[]::TEXT[];
BEGIN
    SELECT (
               SELECT value_json
               FROM public.vps_rule_values
               WHERE client_id = p_client_id
                 AND key = 'network.interfaces'
           ),
           (
               SELECT value_json
               FROM public.vps_rule_values
               WHERE client_id = p_client_id
                 AND key = 'network.rate.interfaces'
           ),
           (
               SELECT value_json
               FROM public.vps_rule_values
               WHERE client_id = p_client_id
                 AND key = 'traffic.selectors'
           )
    INTO interface_rule, rate_rule, traffic_rule;

    rate_mode := COALESCE(rate_rule ->> 'mode', 'reference');
    CASE rate_mode
        WHEN 'all' THEN
            requested_all := TRUE;
        WHEN 'exact' THEN
            SELECT COALESCE(
                array_agg(
                    DISTINCT (selector ->> 'interface') COLLATE "C"
                    ORDER BY (selector ->> 'interface') COLLATE "C"
                ),
                ARRAY[]::TEXT[]
            )
            INTO requested_interfaces
            FROM jsonb_array_elements(
                COALESCE(rate_rule -> 'selectors', '[]'::JSONB)
            ) selector
            WHERE selector ->> 'source' = 'host'
              AND octet_length(selector ->> 'interface') BETWEEN 1 AND 128;
        WHEN 'reference' THEN
            traffic_mode := COALESCE(traffic_rule ->> 'mode', 'exact');
            IF traffic_mode = 'all' THEN
                requested_all := TRUE;
            ELSIF traffic_mode = 'exact' THEN
                SELECT COALESCE(
                    array_agg(
                        DISTINCT (selector ->> 'interface') COLLATE "C"
                        ORDER BY (selector ->> 'interface') COLLATE "C"
                    ),
                    ARRAY[]::TEXT[]
                )
                INTO requested_interfaces
                FROM jsonb_array_elements(
                    COALESCE(traffic_rule -> 'selectors', '[]'::JSONB)
                ) selector
                WHERE selector ->> 'source' = 'host'
                  AND octet_length(selector ->> 'interface') BETWEEN 1 AND 128;
            ELSE
                RAISE EXCEPTION
                    'invalid traffic selection mode for client %',
                    p_client_id;
            END IF;
        ELSE
            RAISE EXCEPTION
                'invalid network-rate selection mode for client %',
                p_client_id;
    END CASE;

    -- Absence is the product default: admit ordinary e*/w* host interfaces.
    -- Operators may instead admit every host interface or a canonical list of
    -- exact/trailing-star prefixes.  Prefixes use explicit left comparison;
    -- operator text is never interpreted as a SQL pattern.
    IF interface_rule IS NULL THEN
        admitted_patterns := ARRAY['e*', 'w*']::TEXT[];
    ELSE
        interface_mode := interface_rule ->> 'mode';
        IF interface_mode = 'all' THEN
            admitted_all := TRUE;
        ELSIF interface_mode = 'patterns' THEN
            SELECT COALESCE(
                array_agg(
                    DISTINCT pattern.value COLLATE "C"
                    ORDER BY pattern.value COLLATE "C"
                ),
                ARRAY[]::TEXT[]
            )
            INTO admitted_patterns
            FROM jsonb_array_elements_text(
                COALESCE(interface_rule -> 'patterns', '[]'::JSONB)
            ) pattern(value)
            WHERE octet_length(pattern.value) BETWEEN 1 AND 128;
        ELSE
            RAISE EXCEPTION
                'invalid network-interface admission mode for client %',
                p_client_id;
        END IF;
    END IF;

    IF requested_all AND admitted_all THEN
        RETURN ROW(TRUE, ARRAY[]::TEXT[], ARRAY[]::TEXT[])
            ::public.telemetry_dashboard_network_selection;
    END IF;

    -- Preserve wildcard admission as a compact predicate.  It is expanded
    -- only when a generation is rebuilt, never once per arriving point.
    IF requested_all THEN
        IF NOT public.telemetry_dashboard_interfaces_are_canonical(
            admitted_patterns
        ) THEN
            RAISE EXCEPTION
                'noncanonical network-interface admission for client %',
                p_client_id;
        END IF;
        RETURN ROW(FALSE, ARRAY[]::TEXT[], admitted_patterns)
            ::public.telemetry_dashboard_network_selection;
    ELSE
        SELECT COALESCE(
            array_agg(
                candidate.interface
                ORDER BY candidate.interface COLLATE "C"
            ),
            ARRAY[]::TEXT[]
        )
        INTO interfaces
        FROM (
            SELECT requested.interface COLLATE "C" AS interface
            FROM unnest(requested_interfaces) requested(interface)
            WHERE admitted_all
               OR EXISTS (
                    SELECT 1
                    FROM unnest(admitted_patterns) pattern(value)
                    WHERE (
                        right(pattern.value, 1) = '*'
                        AND left(
                            requested.interface,
                            length(pattern.value) - 1
                        ) = left(pattern.value, length(pattern.value) - 1)
                    ) OR (
                        right(pattern.value, 1) <> '*'
                        AND requested.interface = pattern.value
                    )
               )
        ) candidate;
    END IF;

    IF NOT public.telemetry_dashboard_interfaces_are_canonical(interfaces) THEN
        RAISE EXCEPTION
            'noncanonical network-rate interface selection for client %',
            p_client_id;
    END IF;

    RETURN ROW(FALSE, interfaces, ARRAY[]::TEXT[])
        ::public.telemetry_dashboard_network_selection;
END
$$;

-- Existing projection heads may have been built with the wrong omitted-rule
-- default. Queue one immutable generation event per affected owner; the
-- established ready-owner consumer performs the rebuild after this commits.
INSERT INTO public.telemetry_dashboard_generation_events (
    client_id, domain, queued_at
)
SELECT head.client_id,
       'network',
       public.telemetry_dashboard_event_queued_at()
FROM public.telemetry_dashboard_network_projection_heads head
WHERE NOT EXISTS (
    SELECT 1
    FROM public.vps_rule_values rule
    WHERE rule.client_id = head.client_id
      AND rule.key = 'network.rate.interfaces'
)
AND EXISTS (
    SELECT 1
    FROM public.vps_rule_values rule
    WHERE rule.client_id = head.client_id
      AND rule.key = 'traffic.selectors'
)
ORDER BY head.client_id;
