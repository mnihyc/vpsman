CREATE TABLE gateway_sessions (
    id UUID PRIMARY KEY,
    gateway_id TEXT NOT NULL,
    client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    noise_public_key_hex TEXT,
    status TEXT NOT NULL,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at TIMESTAMPTZ,
    end_reason TEXT
);

CREATE INDEX gateway_sessions_client_status_idx
    ON gateway_sessions (client_id, status, last_seen_at DESC);

CREATE INDEX gateway_sessions_gateway_seen_idx
    ON gateway_sessions (gateway_id, last_seen_at DESC, id DESC);
