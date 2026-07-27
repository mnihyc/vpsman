# Migration Compatibility

v0.2.0 deliberately establishes a clean PostgreSQL schema baseline. The
previous incremental SQL patches are folded into eight domain migrations, so a
fresh database reaches the intended model directly without upgrade-only
`ALTER`, backfill, or constraint-replacement steps.

## Supported Boundary

| Source database | Start v0.2.0 |
| --- | --- |
| Fresh database | Supported. |
| v0.2.0 or a later compatible release | Supported through append-only migrations. |
| Any v0.1.x database | Not supported in place. The v0.2.0 baseline intentionally has different SQLx migration identities and fails closed. |

Keep a v0.1.x database with its matching application release. Moving retained
data into v0.2.0 requires a separately reviewed export/import procedure into a
fresh database; do not edit `_sqlx_migrations`, replace checksums, or mark
migrations as applied.

The deployment updater checks the target migration manifest before activation,
so attempting an in-place v0.1.x to v0.2.0 update is rejected before the new
application payload starts.

## Immutability From v0.2.0

`migrations/released-checksums.sha384` pins the eight v0.2.0 baseline files.
After the v0.2.0 tag exists, those filenames and bytes are immutable. Future
compatible schema changes must start at `0009` and remain append-only.

`scripts/audit-migrations.sh` enforces:

- sequential `NNNN_name.sql` filenames with no gaps;
- a compatibility note for every migration;
- exact SHA-384 matches for every released migration;
- agreement with the newest reachable release tag at or after v0.2.0;
- unique index names;
- no destructive DDL in later compatible migrations; and
- a default for any later `ADD COLUMN ... NOT NULL`.

During preparation of the not-yet-created v0.2.0 tag, the exact explicit
environment value
`VPSMAN_MIGRATION_BASELINE_CANDIDATE_TAG=v0.2.0` permits a ledger-only audit.
Release CI still requires the real tag.

## v0.2.0 Baseline

| Migration | Schema role |
| --- | --- |
| `0001_identity_access.sql` | Operators, sessions, scoped authentication throttling, agents, globally unique active and retired Noise keys, tags, gateway sessions, lifecycle history, and audit logs. |
| `0002_jobs_schedules_commands.sql` | Schedules, approval-bound jobs, durable per-target dispatch and capability evidence, outputs, terminal sessions, canary/batch rollouts, server cleanup jobs, worker leases, and command templates. |
| `0003_telemetry_alerts_history.sql` | Telemetry rollups and ingest watermarks, traffic accounting, disabled starter policies, alert state and delivery history, webhook processing, and bounded retention domains. |
| `0004_backups_restores.sql` | Backup artifacts and requests with explicit missing-path policy, restore plans, migration links, and fair backup-policy retention scanning. |
| `0005_network_tunnels.sql` | Tunnel plans, operator connection assessments, bounded OSPF reconciliation, network evidence indexes, and revision-bound port-forward desired/runtime state. |
| `0006_agent_updates.sql` | Agent update release and artifact-verification state. |
| `0007_source_templates_file_transfer.sql` | Source templates, client assignments, runtime-config state and generators, file-transfer sessions, and source artifacts. |
| `0008_system_metrics.sql` | Durable 60-second control-plane metric rollups for the System dashboard. |
