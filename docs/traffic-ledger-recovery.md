# Traffic-ledger migration and recovery

The supported v0.4.7 upgrade path applies migrations `0015` through `0020`
after all older writers have stopped. Take a verified database backup first,
hold the traffic-ledger advisory resource while checking the schema, and start
only v0.4.7 writers after the migration audit succeeds.

The migration set adds the effective network-rate index, bounded hourly refresh,
suspension state, the import-class stream index, and the same-shape import
trigger contract and retires the unused `traffic_cycle_usage` prototype table
without `CASCADE`. The application proves a complete dense keyset while holding
the client and traffic-stream locks; the trigger validates the transition-table
primary-key, source-class, accounting, and revision projection.

Recovery is conservative: restore into a disposable database, run the release
migration audit and smoke checks, then compare raw samples, hourly bytes,
revisions, epochs, and job lineage before cutover. Never copy live PostgreSQL
relation files or WAL between installations. If any contract check fails, keep
writers stopped and restore the last verified backup.
