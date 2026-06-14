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
| `0002_jobs_schedules_commands.sql` | Initial job, fixed-target schedule, durable job-target dispatch queue, output, worker-lease, terminal-session, server-job/artifact, and command-template schema. `jobs.id` is the retry identity and `request_fingerprint` rejects accidental ID reuse with different content. |
| `0003_telemetry_alerts_history.sql` | Initial telemetry, rollup, alert policy/state/notification, webhook, and history-retention schema. |
| `0004_backups_restores.sql` | Initial backup artifact, backup request, restore plan, migration link, and backup-policy schema using plain request metadata scoped by client/job. |
| `0005_network_tunnels.sql` | Initial tunnel, tunnel-plan, and network-observation schema. |
| `0006_agent_updates.sql` | Initial agent update release schema. Artifact verification remains intentionally scoped to agent update releases only. |
| `0007_data_sources_file_transfer.sql` | Initial data-source preset, client assignment, file-transfer session, and file-transfer source-artifact schema, including built-in presets. |
| `0008_system_metrics.sql` | Initial durable System Dashboard metric-rollup schema. Adds 60-second control-plane metric buckets. |
