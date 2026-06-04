ALTER TABLE fleet_alert_notification_deliveries
    ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN last_attempt_at TIMESTAMPTZ;

CREATE INDEX fleet_alert_notification_deliveries_attempt_idx
    ON fleet_alert_notification_deliveries (
        status,
        delivery_kind,
        attempt_count,
        created_at ASC
    );
