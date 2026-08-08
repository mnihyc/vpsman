# Migration Compatibility

The current schema is an intentional clean baseline for a fresh PostgreSQL
database. It does not support upgrading a database created by an earlier schema
model in place.

Do not edit `_sqlx_migrations`, replace checksums in an existing database, or
mark migrations as applied. Retained data requires a separately reviewed
export/import into a fresh database.

## Current Canonical Baseline

| Migration | Schema role |
| --- | --- |
| `0001_identity_access.sql` | Operators, sessions, scoped authentication throttling, VPS identity tombstones plus the `visible_clients` operational view, session-reported normalized host facts, globally unique active and retired Noise keys, tags, gateway sessions, lifecycle history, and audit logs. |
| `0002_jobs_schedules_commands.sql` | Schedules, approval-bound jobs, durable per-target dispatch and capability evidence, outputs, terminal sessions, canary/batch rollouts, server cleanup jobs, worker leases, and command templates. |
| `0003_telemetry_alerts_history.sql` | Accepted high-resolution telemetry, adaptive minute-derived resource/network/Ping history including independently covered swap evidence, ingest watermarks, Ping targets and frozen assignments, monitoring shares with frozen targets and explicit billing/system visibility, persisted random public target keys and visitor evidence, authoritative traffic accounting, disabled starter policies, alert state and delivery history, webhook processing, and bounded retention domains. |
| `0004_backups_restores.sql` | Backup artifacts and requests with explicit missing-path policy, restore plans, migration links, and fair backup-policy retention scanning. |
| `0005_network_tunnels.sql` | Tunnel plans, operator connection assessments, bounded OSPF reconciliation, network evidence indexes, and revision-bound port-forward desired/runtime state. |
| `0006_agent_updates.sql` | Agent update release and artifact-verification state. |
| `0007_configuration_presets_file_transfer.sql` | System/custom configuration presets, explicit per-VPS overrides, runtime-config apply state, tunnel-plan adapter definitions, file-transfer sessions, and source artifacts. |
| `0008_system_metrics.sql` | Durable 60-second control-plane metric rollups for the System dashboard. |
| `0009_traffic_counter_epoch_indexes.sql` | Covering counter-epoch indexes that keep no-reset traffic accounting bounded. |

`scripts/audit-migrations.sh` verifies sequential filenames, a documented role
for every migration, unique index names, trailing newlines, and unsafe DDL
patterns. It intentionally does not claim compatibility with a tagged or
deployed database.

When a deployed schema is declared as a compatibility boundary, record that
boundary explicitly, pin its migration bytes, and make all later compatible
changes append-only. Until then, do not describe this canonical fresh-database
model as an upgrade path.
