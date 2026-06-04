CREATE TABLE tunnel_plans (
    id UUID PRIMARY KEY,
    actor_id UUID REFERENCES operators(id),
    name TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    left_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    right_client_id TEXT NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
    input JSONB NOT NULL,
    plan JSONB NOT NULL,
    recommended_ospf_cost INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'planned',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX tunnel_plans_clients_idx ON tunnel_plans(left_client_id, right_client_id);
