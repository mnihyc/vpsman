ALTER TABLE agent_update_rollout_delegated_proofs
    ADD COLUMN force_unprivileged BOOLEAN NOT NULL DEFAULT false;
