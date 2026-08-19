# Migration Compatibility

The current schema is an intentional clean baseline for a fresh PostgreSQL
database. The supported in-place paths are applying
`0010_disabled_resource_alert_policies.sql` and then
`0011_operational_alert_lifecycle.sql` and
`0012_policy_owned_alerts_event_schedules.sql` and
`0013_revisioned_fleet_alert_state_bulk.sql` and
`0014_disk_telemetry_presence.sql` to the exact v0.4.4
`0001`–`0009` files and checksums, or applying
`0009_fleet_tag_settings.sql` through `0014` to the exact
v0.3.5 `0001`–`0008` baseline. Earlier or different canonical baselines are
not supported in place.

Before stopping services, let webhook delivery work drain and verify that no
delivery remains `queued`, `in_progress`, or retryable `failed`. A rendered
delivery body is immutable evidence of the rule version that produced it, so
the lifecycle-expression rewrite deliberately refuses to reinterpret a queued
body under the new generic alert vocabulary. After the queue is empty, stop
API/application and worker writers for the entire migration sequence. Start
only the new binaries after the full sequence through `0014` commits; startup performs the guarded
expression rewrite and waits for policy evidence reconciliation before
accepting ingress. Concurrent old-version writers and rolling binaries are not
a supported migration mode.

The following read-only check must return `0` immediately before services are
stopped:

```sql
SELECT COUNT(*) AS nonterminal_webhook_deliveries
FROM webhook_rule_deliveries
WHERE status IN ('queued', 'in_progress', 'failed');
```

If it does not, keep the existing worker running and correct the destination or
retry policy until every delivery is terminal. Do not delete, cancel, or edit a
delivery merely to pass the upgrade check.

Do not edit `_sqlx_migrations`, replace checksums in an existing database, or
mark migrations as applied. Retained data requires a separately reviewed
export/import into a fresh database.

## Current Canonical Baseline

| Migration                                      | Schema role                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| ---------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `0001_identity_access.sql`                     | Operators, sessions, scoped authentication throttling, VPS identity tombstones plus the `visible_clients` operational view, session-reported normalized host facts, globally unique active and retired Noise keys, tags, gateway sessions, lifecycle history, and audit logs.                                                                                                                                                                                                                                                                                                                        |
| `0002_jobs_schedules_commands.sql`             | Schedules, approval-bound jobs, durable per-target dispatch and capability evidence, outputs, terminal sessions, canary/batch rollouts, server cleanup jobs, worker leases, and command templates.                                                                                                                                                                                                                                                                                                                                                                                                   |
| `0003_telemetry_alerts_history.sql`            | Accepted high-resolution telemetry with transactionally derived indexed scalar/counter facts, compact logical Ping-series evidence, age-tiered resource/network/Ping history with sufficient statistics and exact current snapshots, ingest watermarks, Ping targets and frozen assignments, monitoring shares with frozen targets and explicit billing/system visibility, persisted random public target keys and visitor evidence, reset-safe exact and tiered traffic accounting, disabled starter policies, alert state and delivery history, webhook processing, and bounded retention domains. |
| `0004_backups_restores.sql`                    | Backup artifacts and requests with explicit missing-path policy, restore plans, migration links, and fair backup-policy retention scanning.                                                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `0005_network_tunnels.sql`                     | Tunnel plans, operator connection assessments, bounded OSPF reconciliation, exact current plus age-tiered automatic reachability evidence, network evidence indexes, and revision-bound port-forward desired/runtime state with retained hostname context for resolved literal targets.                                                                                                                                                                                                                                                                                                              |
| `0006_agent_updates.sql`                       | Agent update release and artifact-verification state.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| `0007_configuration_presets_file_transfer.sql` | System/custom configuration presets, explicit per-VPS overrides, runtime-config apply state, tunnel-plan adapter definitions, file-transfer sessions, and source artifacts.                                                                                                                                                                                                                                                                                                                                                                                                                          |
| `0008_system_metrics.sql`                      | Durable sufficient-stat control-plane metric rollups promoted from 60-second evidence into bounded long-term tiers for the System dashboard.                                                                                                                                                                                                                                                                                                                                                                                                                                                         |
| `0009_fleet_tag_settings.sql`                  | Fleet-wide tag-order settings stored beside the canonical flat tag display order.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| `0010_disabled_resource_alert_policies.sql`    | Explicit Triggered/Persisting/Unknown/Resolved policy-alert lifecycle with conservative Unknown state for existing alerts and confirmation-gated fleet/client current-alert priority indexes; disabled CPU-utilization, memory-availability, and disk-availability starter policies in the ordinary persisted alert-policy model; guarded replacement of untouched legacy CPU-load and memory starters; and state reset for rules affected by the corrected `cpu.load_saturation` meaning while retaining historical alerts.                                                                         |
| `0011_operational_alert_lifecycle.sql`         | The pre-unification operational condition/occurrence episode foundation with causal lifecycle and operator-resolution provenance constraints, a guarded application backfill fence, current/history and unresolved-occurrence cursor indexes, separate tunnel topology and runtime-evidence identities, and removal of the invalid webhook-delivery triage namespace without pruning legitimate triage or alert history.                                                                                                                                                                             |
| `0012_policy_owned_alerts_event_schedules.sql` | One policy-owned alert episode model for state, metric, and occurrence evidence; typed Trigger and Resolve meta conditions; deterministic enabled operational defaults; monotonic evidence receipts and lifecycle outbox; generic `alert.triggered`/`alert.resolved` vocabulary; and prospective, exactly deduplicated Alert-event schedules with frozen actors, targets, argv templates, and bounded causal lineage.                                                                                                                                                                                |
| `0013_revisioned_fleet_alert_state_bulk.sql`   | Monotonic fleet-alert triage revisions for compare-and-swap bulk mutations, with revision `0` reserved for an absent state row and existing persisted states upgraded to revision `1`; deterministic backfill and transactionally maintained coverage revisions for the exact hourly current-cycle traffic transition ledger, with raw-oracle fallback on incomplete coverage.                                                                                                                                                                                                                       |
| `0014_disk_telemetry_presence.sql`             | Versioned persistent-block-filesystem disk evidence with nullable raw failure state and an independent positive-capacity disk utilization sample count. Existing unversioned rollup numerics remain intact for forensics but receive count `0`, so every authoritative current/history consumer ignores them without a full-table rewrite.                                                                                                                                                                                                                                                            |

`scripts/audit-migrations.sh` verifies sequential filenames, a documented role
for every migration, unique active index names (or an explicit drop/recreate),
trailing newlines, and unsafe DDL patterns. Its destructive-DDL allowlist is
limited to the three reviewed retired-store statements in `0012`; any other
destructive statement still fails. That structural audit does not establish
upgrade compatibility; the checksum-pinned v0.3.5 regression through `0014`,
the exact v0.4.4
`0001`–`0009` through `0014` regression, and this explicit declaration are the
compatibility evidence for the supported in-place steps.

Except for the explicit checksum-pinned paths above, these migrations support
only a fresh database used by the current repository components. Copying
migration files must not be presented as a historical baseline. Moving retained
data from any other baseline requires the separately reviewed export/import
process described above.
