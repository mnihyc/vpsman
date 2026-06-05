CREATE TABLE fleet_alert_notification_channels (
    id UUID PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    scope_kind TEXT NOT NULL,
    scope_value TEXT,
    min_severity TEXT NOT NULL,
    categories JSONB NOT NULL DEFAULT '[]'::jsonb,
    operator_states JSONB NOT NULL DEFAULT '[]'::jsonb,
    delivery_kind TEXT NOT NULL,
    target TEXT NOT NULL,
    cooldown_secs BIGINT NOT NULL DEFAULT 3600,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    notes TEXT,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (scope_kind IN ('global', 'provider', 'tag', 'client')),
    CHECK (
        (scope_kind = 'global' AND scope_value IS NULL)
        OR (scope_kind <> 'global' AND scope_value IS NOT NULL)
    ),
    CHECK (min_severity IN ('info', 'warning', 'critical')),
    CHECK (jsonb_typeof(categories) = 'array'),
    CHECK (jsonb_typeof(operator_states) = 'array'),
    CHECK (cooldown_secs >= 0 AND cooldown_secs <= 2592000)
);

CREATE INDEX fleet_alert_notification_channels_match_idx
    ON fleet_alert_notification_channels (
        enabled,
        scope_kind,
        scope_value,
        min_severity,
        delivery_kind,
        updated_at DESC
    );

CREATE TABLE fleet_alert_notification_deliveries (
    id UUID PRIMARY KEY,
    channel_id UUID NOT NULL REFERENCES fleet_alert_notification_channels(id) ON DELETE CASCADE,
    channel_name TEXT NOT NULL,
    alert_id TEXT NOT NULL,
    alert_severity TEXT NOT NULL,
    alert_category TEXT NOT NULL,
    status TEXT NOT NULL,
    delivery_kind TEXT NOT NULL,
    target TEXT NOT NULL,
    dedupe_key TEXT NOT NULL,
    payload JSONB NOT NULL,
    error TEXT,
    cooldown_until_unix BIGINT NOT NULL,
    actor_id UUID REFERENCES operators(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at TIMESTAMPTZ,
    CHECK (alert_severity IN ('info', 'warning', 'critical')),
    CHECK (cooldown_until_unix >= 0)
);

CREATE INDEX fleet_alert_notification_deliveries_status_idx
    ON fleet_alert_notification_deliveries (status, created_at DESC);

CREATE INDEX fleet_alert_notification_deliveries_dedupe_idx
    ON fleet_alert_notification_deliveries (dedupe_key, cooldown_until_unix DESC);

CREATE INDEX fleet_alert_notification_deliveries_alert_idx
    ON fleet_alert_notification_deliveries (alert_id, created_at DESC);
