# Migration Compatibility

The current schema is an intentional clean baseline for a fresh PostgreSQL
database. The sole supported in-place step is applying
`0009_fleet_tag_settings.sql` to a database whose applied migrations are the
exact v0.3.5 `0001`–`0008` files and checksums. Earlier or different canonical
baselines are not supported in place.

Do not edit `_sqlx_migrations`, replace checksums in an existing database, or
mark migrations as applied. Retained data requires a separately reviewed
export/import into a fresh database.

## Current Canonical Baseline

| Migration | Schema role |
| --- | --- |
| `0001_identity_access.sql` | Operators, sessions, scoped authentication throttling, VPS identity tombstones plus the `visible_clients` operational view, session-reported normalized host facts, globally unique active and retired Noise keys, tags, gateway sessions, lifecycle history, and audit logs. |
| `0002_jobs_schedules_commands.sql` | Schedules, approval-bound jobs, durable per-target dispatch and capability evidence, outputs, terminal sessions, canary/batch rollouts, server cleanup jobs, worker leases, and command templates. |
| `0003_telemetry_alerts_history.sql` | Accepted high-resolution telemetry with transactionally derived indexed scalar/counter facts, compact logical Ping-series evidence, age-tiered resource/network/Ping history with sufficient statistics and exact current snapshots, ingest watermarks, Ping targets and frozen assignments, monitoring shares with frozen targets and explicit billing/system visibility, persisted random public target keys and visitor evidence, reset-safe exact and tiered traffic accounting, disabled starter policies, alert state and delivery history, webhook processing, and bounded retention domains. |
| `0004_backups_restores.sql` | Backup artifacts and requests with explicit missing-path policy, restore plans, migration links, and fair backup-policy retention scanning. |
| `0005_network_tunnels.sql` | Tunnel plans, operator connection assessments, bounded OSPF reconciliation, exact current plus age-tiered automatic reachability evidence, network evidence indexes, and revision-bound port-forward desired/runtime state with retained hostname context for resolved literal targets. |
| `0006_agent_updates.sql` | Agent update release and artifact-verification state. |
| `0007_configuration_presets_file_transfer.sql` | System/custom configuration presets, explicit per-VPS overrides, runtime-config apply state, tunnel-plan adapter definitions, file-transfer sessions, and source artifacts. |
| `0008_system_metrics.sql` | Durable sufficient-stat control-plane metric rollups promoted from 60-second evidence into bounded long-term tiers for the System dashboard. |
| `0009_fleet_tag_settings.sql` | Fleet-wide tag-order settings stored beside the canonical flat tag display order. |

`scripts/audit-migrations.sh` verifies sequential filenames, a documented role
for every migration, unique index names, trailing newlines, and unsafe DDL
patterns. That structural audit does not establish upgrade compatibility; the
checksum-pinned v0.3.5 regression for `0009` and this explicit declaration are
the compatibility evidence for the sole supported in-place step.

Except for the explicit v0.3.5 `0001`–`0008` to `0009` step above, these
migrations support only a fresh database used by the current repository
components. Copying migration files must not be presented as a historical
baseline. Moving retained data from any other baseline requires the separately
reviewed export/import process described above.
