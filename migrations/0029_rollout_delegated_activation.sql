ALTER TABLE agent_update_rollout_delegated_proofs
    ADD COLUMN staged_sha256_hex TEXT,
    ADD COLUMN restart_agent BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX agent_update_rollout_delegated_action_ready_idx
    ON agent_update_rollout_delegated_proofs (action, status, proof_expires_unix, rollout_id, client_id);
