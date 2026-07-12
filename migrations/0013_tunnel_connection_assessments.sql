ALTER TABLE tunnel_plans
    ADD COLUMN connection_assessment TEXT NOT NULL DEFAULT 'automatic',
    ADD COLUMN connection_assessment_note TEXT,
    ADD COLUMN connection_assessed_at TIMESTAMPTZ,
    ADD COLUMN connection_assessed_by UUID REFERENCES operators(id) ON DELETE SET NULL,
    ADD CONSTRAINT tunnel_plans_connection_assessment_check
        CHECK (connection_assessment IN ('automatic', 'connected', 'disconnected')),
    ADD CONSTRAINT tunnel_plans_connection_assessment_note_check
        CHECK (
            (connection_assessment = 'automatic'
                AND connection_assessment_note IS NULL
                AND connection_assessed_at IS NULL
                AND connection_assessed_by IS NULL)
            OR
            (connection_assessment IN ('connected', 'disconnected')
                AND connection_assessment_note IS NOT NULL
                AND length(btrim(connection_assessment_note)) BETWEEN 1 AND 500
                AND connection_assessed_at IS NOT NULL
                AND connection_assessed_by IS NOT NULL)
        );
