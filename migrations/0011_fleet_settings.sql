-- Fleet-wide settings.

-- Tables.

CREATE TABLE public.fleet_tag_settings (
    setting_key text NOT NULL,
    value_json jsonb NOT NULL,
    updated_by uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT fleet_tag_settings_key_check CHECK ((((length(setting_key) >= 1) AND (length(setting_key) <= 128)) AND (setting_key ~ '^[a-z][a-z0-9_.-]*$'::text))),
    CONSTRAINT fleet_tag_settings_known_value_check CHECK (((setting_key <> 'order.namespace_natural_sort_enabled'::text) OR (jsonb_typeof(value_json) = 'boolean'::text))),
    CONSTRAINT fleet_tag_settings_pkey PRIMARY KEY (setting_key),
    CONSTRAINT fleet_tag_settings_updated_by_fkey FOREIGN KEY (updated_by) REFERENCES public.operators(id) ON DELETE SET NULL
);
