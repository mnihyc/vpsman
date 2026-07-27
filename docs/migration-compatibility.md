# Migration Schema Notes

SQLx records a checksum for every applied migration and rejects startup when the
bytes for an already-applied version change. Published migration files are
therefore append-only release history: never edit, rename, or delete one. Put
every schema change in the next numbered migration, including constraint,
default, seed-data, and index changes.

`migrations/released-checksums.sha384` pins the exact SQLx checksums published
through its release anchor. `scripts/audit-migrations.sh` checks both that ledger
and the newest reachable release tag. Release CI fetches tags and requires this
audit, so changing both a migration and its local ledger entry cannot conceal a
rewrite of tagged history.

## Supported Upgrade Boundary

| Source database | Upgrade to current migrations |
| --- | --- |
| Fresh database | Supported. |
| `v0.1.4`, `v0.1.5`, `v0.1.6`, or `v0.1.7` | Supported. These releases share byte-identical migration history for all versions they have in common. |
| `v0.1.0` through `v0.1.3` | **Blocked pending an explicit compatibility bridge.** Those releases published different bytes and, in one case, a different filename for migration versions later folded into the baseline. The current SQLx migrator will correctly reject their recorded checksums. |

The tagged drift is finite but not checksum-only:

| Source release | Published files that differ from the `v0.1.4` baseline |
| --- | --- |
| `v0.1.0` | `0001` through `0007`; `0007` was also renamed. |
| `v0.1.1` | `0001` through `0005`, plus renamed `0007`. |
| `v0.1.2` | `0001`, `0003`, `0005`, and `0007`. |
| `v0.1.3` | `0003`, `0005`, and `0007`. |

Even the smallest (`v0.1.3`) delta removes the legacy `tunnels` model and
plan-lifecycle columns, tightens nullable observation identity to `NOT NULL`,
retires tunnel/OSPF template seed rows, and removes telemetry fields. Older
deltas also replace whole policy/template tables and change job, backup, and
update-release state. A bridge therefore needs explicit preservation and
mapping rules for live rows; changing recognized checksums without those rules
would silently bless the wrong schema and data.

Do not delete or manually rewrite rows in `_sqlx_migrations` to bypass the
pre-`v0.1.4` block. A safe bridge must start from a database fixture created by
each affected release, converge the actual schema and seed data
transactionally, update only recognized historical checksums, and then prove
that the normal migrator reaches the same schema and data as a fresh install.
Until that bridge and its upgrade fixtures ship, restore the matching release
and database snapshot instead of attempting an in-place upgrade.

Rules enforced by `scripts/audit-migrations.sh`:

- Migration filenames are sequential `NNNN_name.sql` files with no gaps.
- Every migration file is listed in this document.
- Every migration in the released checksum ledger keeps its exact bytes and
  filename.
- Every migration in the newest reachable release tag matches the checksum
  ledger; release CI must use full tag history.
- Destructive DDL is not accepted in migration files; make a deliberate new
  additive transition plan when performing a breaking schema change.
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
| `0020_traffic_counter_retention.sql` | Adds raw traffic-counter samples as an independently configurable, bounded history-retention domain. Retention keeps at least 32 days so the current monthly accounting cycle remains intact, while pruning preserves one pre-cutoff baseline per VPS/source/interface stream. |
| `0021_backup_policy_retention_fairness.sql` | Adds a durable per-policy retention-scan cursor so bounded worker batches rotate fairly through every backup policy; one failing or alphabetically early policy can no longer starve the rest. |
| `0022_auth_throttle_username_ip_scope.sql` | Adds the username/client-IP authentication throttle scope while retaining legacy scope values, preventing one hostile source from locking an operator out from every network. |
| `0023_job_approval_dispatch_binding.sql` | Binds each approval-backed job to the exact reviewed approval. Existing matching jobs are backfilled deterministically, so unrelated or duplicate approval rows cannot block or authorize dispatch. |
| `0024_fleet_alert_query_bounds.sql` | Persists validated capability-degradation metadata on terminal job targets and adds bounded fleet-alert/dashboard candidate indexes. Existing structured status output is safely backfilled; malformed or unrelated output remains ignored. |
