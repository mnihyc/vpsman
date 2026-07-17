-- Delivery rows are immutable operational evidence. Their snapshotted routing
-- identity remains useful after the corresponding configuration is deleted.
ALTER TABLE fleet_alert_notification_deliveries
    DROP CONSTRAINT fleet_alert_notification_deliveries_channel_id_fkey;

ALTER TABLE webhook_rule_deliveries
    DROP CONSTRAINT webhook_rule_deliveries_rule_id_fkey;
