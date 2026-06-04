# Migration Compatibility Notes

The PostgreSQL schema is forward-only for the current release. Downgrade is a
database-snapshot operation, not an automatic reverse migration. Every migration
must remain append-only unless a later release adds a formally reviewed reverse
plan with data-export steps.

Compatibility rules enforced by `scripts/audit-migrations.sh`:

- Migration filenames are sequential `NNNN_name.sql` files with no gaps.
- Every migration is listed in this document.
- Destructive DDL such as dropping tables or columns is rejected.
- `ADD COLUMN ... NOT NULL` must include a `DEFAULT` so existing rows can be
  upgraded safely.
- Index names must be unique across migrations.

Operational rollback model:

- Before applying migrations in production, take a PostgreSQL snapshot or base
  backup.
- If an older binary must be restored after a schema upgrade, restore the
  matching database snapshot. Do not hand-edit schema down unless a future
  release provides a reviewed reverse migration.
- For additive columns and tables, older binaries should ignore unknown data
  only where their queries still compile. This is compatibility tolerance, not
  a downgrade guarantee.

## Migration Ledger

| Migration | Compatibility and rollback note |
| --- | --- |
| `0001_init.sql` | Baseline schema. Rollback requires dropping the new database or restoring the pre-install snapshot. |
| `0002_operator_sessions.sql` | Adds operator session table and indexes. Additive; rollback by snapshot restore if reverting auth-session support. |
| `0003_schedules.sql` | Adds schedule table and due index. Additive; older binaries should run against a snapshot without schedules. |
| `0004_tunnel_plans.sql` | Adds tunnel plan storage. Additive; rollback by snapshot restore if topology planning must be removed. |
| `0005_enrollment_tokens.sql` | Adds enrollment token storage. Additive; rollback by restoring the previous enrollment database state. |
| `0006_backup_requests.sql` | Adds backup request records. Additive; backup metadata rollback requires snapshot restore. |
| `0007_scheduled_job_dispatch.sql` | Adds nullable job dispatch linkage and index. Backward compatible for existing jobs. |
| `0008_restore_plans.sql` | Adds restore plan records. Additive; rollback by snapshot restore if restore planning is removed. |
| `0009_network_observations.sql` | Adds network observation records linked to job targets. Additive; rollback by snapshot restore. |
| `0010_agent_update_rollouts.sql` | Adds agent update rollout and target records. Additive; rollback by snapshot restore before running older rollout code. |
| `0011_gateway_sessions.sql` | Adds gateway session records. Additive; rollback by snapshot restore if gateway session history is incompatible. |
| `0012_telemetry_rollups.sql` | Adds telemetry rollup table. Additive; rollups can be recomputed after restore. |
| `0013_telemetry_rollup_disk_network.sql` | Adds non-null rollup aggregate columns with defaults. Existing rows upgrade safely. |
| `0014_operator_scopes.sql` | Adds operator scopes with default empty array and constraint. Existing operators remain valid. |
| `0015_telemetry_network_rates.sql` | Adds network rate rollup table. Additive; data can be regenerated from telemetry where retained. |
| `0016_operator_totp.sql` | Adds optional encrypted TOTP fields and hex constraint. Existing operators remain valid. |
| `0017_job_output_artifacts.sql` | Adds job-output object metadata with inline default. Existing outputs remain inline. |
| `0018_migration_links.sql` | Adds migration-link records. Additive; rollback by snapshot restore if migration workflows are removed. |
| `0019_agent_update_releases.sql` | Adds signed update release metadata. Additive; rollback by snapshot restore for release registry removal. |
| `0020_agent_update_artifact_hosting.sql` | Adds optional hosted artifact object fields. Existing release rows remain valid. |
| `0021_tunnel_plan_endpoint_status.sql` | Adds endpoint status columns with safe defaults and job references. Existing plans become `planned`. |
| `0022_agent_capabilities.sql` | Adds client capability JSON with default empty object. Existing clients remain valid. |
| `0023_rollout_automation.sql` | Adds rollout automation fields with defaults and nullable metadata. Existing rollouts become `unreconciled`. |
| `0024_enrollment_reenrollment_policy.sql` | Adds token purpose/policy fields with safe defaults. Existing tokens remain provisioning tokens. |
| `0025_data_source_presets.sql` | Adds preset tables and built-in default assignments. Rollback by snapshot restore; do not delete preset rows manually. |
| `0026_file_transfer_source_artifacts.sql` | Adds retained source-artifact metadata. Additive; rollback by snapshot restore plus object-store cleanup if needed. |
| `0027_rollout_control.sql` | Adds rollout pause/health-gate/lease fields with defaults. Existing rollouts keep heartbeat verification semantics. |
| `0028_rollout_delegated_rollback.sql` | Adds delegated proof escrow table. Additive; rollback by snapshot restore to remove escrow records. |
| `0029_rollout_delegated_activation.sql` | Adds activation-specific delegated proof fields and index. Existing rollback proofs remain valid. |
| `0030_rollout_delegated_force_unprivileged.sql` | Adds force-unprivileged flag with default false. Existing delegated proofs keep prior dispatch behavior. |
| `0031_agent_update_release_rollback_bundles.sql` | Adds optional rollback artifact metadata and indexes. Existing releases remain primary-only. |
| `0032_agent_update_rollout_policies.sql` | Adds rollout policy presets and optional rollout references. Existing rollouts remain policy-less. |
| `0033_fleet_alert_policies.sql` | Adds scoped alert policy table. Additive; rollback by snapshot restore if policy management is removed. |
| `0034_fleet_alert_states.sql` | Adds durable alert triage state. Additive; rollback by snapshot restore if triage history is incompatible. |
| `0035_fleet_alert_notifications.sql` | Adds notification channels and delivery records. Additive; rollback by snapshot restore before removing notification code. |
| `0036_fleet_alert_notification_attempts.sql` | Adds delivery attempt metadata with safe defaults. Existing deliveries start with attempt count zero. |
| `0037_schedule_policies_worker_leases.sql` | Adds schedule retry/catch-up fields with safe defaults and worker lease table. Existing schedules keep skip-missed semantics. |
| `0038_history_retention_policies.sql` | Adds retention policy table. Additive; rollback by snapshot restore if retention policy management is removed. |
| `0039_client_key_revocations.sql` | Adds current/old client public-key revocation records. Additive; rollback by snapshot restore if key-lifecycle revocation history must be removed. |
| `0040_backup_policies.sql` | Adds backup policy metadata linked to schedules. Additive; rollback by snapshot restore if scheduled backup policy management is removed. |
| `0041_backup_request_sources.sql` | Adds nullable source job/schedule links for backup requests. Additive; rollback by snapshot restore if policy-linked request provenance or retention pruning is removed. |
| `0042_enrollment_default_pool.sql` | Adds nullable server-owned enrollment token defaults for pool assignment and display name plus a partial lookup index. Additive; rollback by snapshot restore if enrollment default metadata must be removed. |
| `0043_command_templates_and_job_idempotency.sql` | Adds nullable job idempotency metadata, safe reconnect-policy defaults, command template storage, and scoped lookup indexes. Additive; rollback by snapshot restore if saved templates or idempotency records must be removed. |
