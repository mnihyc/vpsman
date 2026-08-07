-- Tunnel traffic accounting is no longer a runtime configuration behavior.
-- Runtime tunnel telemetry always uses live interface counters. Historical
-- host-interface counters can be imported explicitly with the vnStat traffic
-- import job instead.
DELETE FROM client_configuration_preset_overrides
WHERE behavior = 'tunnel_traffic';

DELETE FROM configuration_presets
WHERE behavior = 'tunnel_traffic';

ALTER TABLE configuration_presets
    DROP CONSTRAINT configuration_presets_behavior_check;

ALTER TABLE configuration_presets
    ADD CONSTRAINT configuration_presets_behavior_check
    CHECK (behavior IN (
        'host_metrics',
        'latency_probe',
        'ospf_update_command',
        'process_inventory',
        'user_sessions',
        'command_execution'
    ));
