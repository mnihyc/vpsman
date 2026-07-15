ALTER TABLE terminal_sessions
    ADD COLUMN opened_at TIMESTAMPTZ;

UPDATE terminal_sessions
SET opened_at = observed_at
WHERE last_event = 'terminal_open';
