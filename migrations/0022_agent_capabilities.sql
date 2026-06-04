ALTER TABLE clients
    ADD COLUMN capabilities JSONB NOT NULL DEFAULT '{}'::jsonb;

