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
| `0001_identity_access.sql` | Initial identity, agent, tag, gateway-session, key-revocation, and audit schema. Rollback requires dropping the new database or restoring the pre-install snapshot. |
| `0002_jobs_schedules_commands.sql` | Initial job, schedule, output, worker-lease, and command-template schema. Rollback requires restoring the matching initial-release snapshot. |
| `0003_telemetry_alerts_history.sql` | Initial telemetry, rollup, alert policy/state/notification, and history-retention schema. Telemetry aggregates can be recomputed after snapshot restore. |
| `0004_backups_restores.sql` | Initial backup artifact, backup request, restore plan, migration link, and backup-policy schema. Rollback requires snapshot restore plus object-store cleanup if artifacts were written. |
| `0005_network_tunnels.sql` | Initial tunnel, tunnel-plan, and network-observation schema. Rollback requires restoring the matching initial-release snapshot. |
| `0006_agent_updates.sql` | Initial signed release, rollout, rollout policy, target, and rollout automation schema. Rollback requires snapshot restore before running older update-management code. |
| `0007_data_sources_file_transfer.sql` | Initial data-source preset, client assignment, and file-transfer source-artifact schema, including built-in presets. Rollback by snapshot restore; do not delete built-in preset rows manually. |

Future schema changes must start at `0008_...` and remain role-scoped when
practical. A future migration should describe the domain it changes, not the
implementation accident that created it.
