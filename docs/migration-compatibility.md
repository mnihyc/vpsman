# Migration Schema Notes

This repository currently treats the SQL files as the clean release-baseline
schema for a fresh deployment. The project does not carry compatibility shims or
reverse migrations in this branch. Operational rollback means restoring the
matching database snapshot and binaries together.

`vpsman-main(4)` breaking changes are folded directly into the baseline schema:
client status includes `never`, gateway sessions use `active` / `ended` /
`expired`, jobs and schedules store fixed target snapshots, durable dispatch
state lives on `job_targets`, and backup/restore request metadata only stores
plain request metadata scoped by client/job.

Rules enforced by `scripts/audit-migrations.sh`:

- Migration filenames are sequential `NNNN_name.sql` files with no gaps.
- Every migration file is listed in this document.
- Destructive DDL is not accepted in migration files; make a deliberate new
  baseline when performing breaking schema work.
- `ADD COLUMN ... NOT NULL` must include a `DEFAULT` if a future migration uses
  additive changes.
- Index names must be unique across migration files.

## Migration Ledger

| Migration | Schema role |
| --- | --- |
| `0001_identity_access.sql` | Initial identity, operator, token, agent, tag, gateway-session, key-revocation, and audit schema. Gateway lifecycle state is `active`, `ended`, or `expired`; newly imported agents may be `never` connected. |
| `0002_jobs_schedules_commands.sql` | Initial job, fixed-target schedule, durable job-target dispatch queue, output, worker-lease, terminal-session, bounded terminal-output chunk, server-job/artifact, and command-template schema. `jobs.id` is the retry identity and `request_fingerprint` rejects accidental ID reuse with different content. |
| `0003_telemetry_alerts_history.sql` | Initial telemetry, traffic accounting, per-VPS rule values, policy group/rule/state/alert, notification, webhook, and history-retention schema. |
| `0004_backups_restores.sql` | Initial backup artifact, backup request, restore plan, migration link, and backup-policy schema using plain request metadata scoped by client/job. |
| `0005_network_tunnels.sql` | Initial tunnel, tunnel-plan, and network-observation schema. |
| `0006_agent_updates.sql` | Initial agent update release schema. Artifact verification remains intentionally scoped to agent update releases only. |
| `0007_source_templates_file_transfer.sql` | Initial source template, client assignment, file-transfer session, and file-transfer source-artifact schema, including built-in templates. |
| `0008_system_metrics.sql` | Initial durable System Dashboard metric-rollup schema. Adds 60-second control-plane metric buckets. |
| `0009_job_approvals.sql` | Initial persisted job-approval queue schema. Approval rows preserve fixed target snapshots, payload hash, request fingerprint, requester/decision metadata, and risk/privilege state for audited operator decisions. |
| `0010_predefined_alert_policies.sql` | Adds three disabled, operator-editable starter policy groups for CPU load, memory pressure, and traffic quota warnings. No predefined policy evaluates until an operator explicitly enables it. |
| `0011_agent_noise_key_ownership.sql` | Enforces global one-key-per-VPS ownership for non-empty active client public keys and retired-key fingerprints. Migration aborts if existing duplicates would make ownership ambiguous. |
| `0012_backup_missing_path_policy.sql` | Adds the explicit backup missing-root policy. Existing and new requests default to strict `fail`; reviewed heterogeneous scopes may select `skip`. |
| `0013_tunnel_connection_assessments.sql` | Adds an audited, revision-bound operator connectivity assessment. Manual connected/disconnected labels require a note and remain separate from runtime reconciliation, measured reachability, and automatic routing control. |
| `0014_telemetry_ingest_integrity.sql` | Adds the bounded per-VPS telemetry process/sequence watermark used for idempotent ordering, plus indexes for efficient latest resource/per-interface snapshots and timestamp-ordered retention pruning. |
| `0015_telemetry_gateway_session_watermark.sql` | Binds telemetry sequence watermarks to authenticated gateway sessions because sequence numbers restart on reconnect. Existing process-only rows use a sentinel session until the first post-upgrade sample atomically replaces them. |
| `0016_terminal_session_opened_at.sql` | Persists the first reported terminal-open timestamp so session evidence does not depend on retained audit rows. Existing rows are backfilled only when their latest retained event is the original open event. |
| `0017_port_forwarding.sql` | Adds revision-bound per-VPS port-forward desired state, cleanup tombstones, and the latest agent-observed owned-table snapshot. The agent remains the sole owner of `inet vpsman_port_forward`; no system or Docker firewall table is imported. |
| `0018_preserve_delivery_history.sql` | Removes current-config ownership cascades from webhook and notification delivery evidence. Rule or destination retirement cancels active work before deletion while completed, failed, and cancelled delivery rows remain immutable audit history. |
| `0019_job_rollouts.sql` | Adds durable explicit canary/batch rollout policy and deterministic per-target batch assignments for direct jobs. Active rollout state gates dispatcher claims and persists pause, delay, failure baseline, completion, and abort evidence. |
