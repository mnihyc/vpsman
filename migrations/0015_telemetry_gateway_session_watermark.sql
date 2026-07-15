ALTER TABLE telemetry_ingest_watermarks
    ADD COLUMN gateway_session_id UUID NOT NULL
        DEFAULT '00000000-0000-0000-0000-000000000000';

-- Existing process-only watermarks use the sentinel session. The next sample
-- arrives with a real gateway session ID and atomically replaces the watermark.
ALTER TABLE telemetry_ingest_watermarks
    ALTER COLUMN gateway_session_id DROP DEFAULT;
