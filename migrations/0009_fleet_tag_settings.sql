CREATE TABLE fleet_tag_settings (
    setting_key TEXT PRIMARY KEY,
    value_json JSONB NOT NULL,
    updated_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT fleet_tag_settings_key_check
        CHECK (
            length(setting_key) BETWEEN 1 AND 128
            AND setting_key ~ '^[a-z][a-z0-9_.-]*$'
        ),
    CONSTRAINT fleet_tag_settings_known_value_check
        CHECK (
            setting_key <> 'order.namespace_natural_sort_enabled'
            OR jsonb_typeof(value_json) = 'boolean'
        )
);

INSERT INTO fleet_tag_settings (setting_key, value_json)
VALUES ('order.namespace_natural_sort_enabled', 'false'::jsonb);
