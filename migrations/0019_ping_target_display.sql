-- Console presentation is independent of probe definitions and generations.
CREATE TABLE public.ping_target_display (
    target_id UUID PRIMARY KEY REFERENCES public.ping_targets(id) ON DELETE CASCADE,
    display_order BIGINT NOT NULL CHECK (display_order >= 0),
    display_color TEXT CHECK (display_color ~ '^#[0-9a-f]{6}$')
);
