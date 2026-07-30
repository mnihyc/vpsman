# Tutorial 07: Backup, Restore, And Migration

Backups and restores are privilege-gated workflows. Agents emit plain tar
backup artifacts, and operators stage a verified archive on the target agent
before dispatching restore or migration restore jobs. The API stores backup
artifact metadata and local-disk object-store bytes by default. S3/MinIO-
compatible object storage is implemented as an optional adapter for deployments
that need remote backup or update artifact storage, and is covered by
adapter-specific smokes.

Object storage is deployment configuration, not a per-VPS configuration
preset. Backup readiness comes from the backup workflow and retained artifact
evidence; it is not represented by a synthetic “source” preset.

## Schedule Backup Policies

Create a policy for a client, pool, or tag selector. Privilege is verified when
the policy schedule is created or changed. After that, the worker creates due
runs and dispatches saved backup intent at schedule time, producing the same
per-target backup request and plain backup artifact history as a manual
`backup-run`.

```sh
cargo run -p vpsctl -- backup-policy-upsert \
  --name nightly-edge \
  --paths /etc/hostname \
  --include-config \
  --tags backup-critical \
  --cron-expr "0 3 * * *" \
  --retention-days 30 \
  --keep-last 7 \
  --rotation-generation keyring/v2 \
  --confirmed
```

Inspect policies:

```sh
cargo run -p vpsctl -- backup-policies
```

The CLI and VTY load 200 policies by default. If a page reaches its requested
cap, they explicitly warn that more may exist; continue with
`backup-policies --limit 200 --offset 200`.

Policies imported from older releases remain visible even when their stored
cron is malformed or has no future occurrence. These rows have an empty
`next_runs` list and an explicit `cadence_error`. If an enabled invalid policy
becomes due, the worker creates no job, disables it, records
`schedule.due_failed`, and emits `schedule.failed`.

Use **Edit** in the UI to repair the complete policy, or pass its schedule UUID
to the same CLI command:

```sh
cargo run -p vpsctl -- backup-policy-upsert \
  --schedule-id <policy_schedule_uuid> \
  --name nightly-edge \
  --paths /etc/hostname \
  --include-config \
  --tags backup-critical \
  --cron-expr "0 3 * * *" \
  --retention-days 30 \
  --keep-last 7 \
  --rotation-generation keyring/v2 \
  --disabled \
  --confirmed
```

The update replaces the full reviewed definition rather than patching selected
fields, so include every intended path, target, retention, and rotation option.
Updates require `--retention-days`, `--keep-last`, and an explicit rotation
choice; pass `--rotation-generation <id>` to retain/set one or
`--clear-rotation-generation` to clear it. Omission is rejected instead of
silently applying create defaults or clearing metadata. A valid update clears
the cadence and prior failure state. Direct API `PUT` callers must send every
material field; use an explicit `rotation_generation: null` to clear rotation
rather than omitting it. `--disabled` keeps
the repaired policy paused; omit it only when automatic runs should resume.

Preview or apply policy-linked retention pruning. Dry run is safe and returns
the per-policy rows that would be pruned. A confirmed non-metadata-only prune
also deletes linked object-store keys; `--metadata-only true` clears server
metadata while leaving stored objects untouched.

```sh
cargo run -p vpsctl -- backup-policy-prune --dry-run

cargo run -p vpsctl -- backup-policy-prune \
  --schedule-id <policy_schedule_uuid> \
  --metadata-only false \
  --confirmed
```

In VTY:

```text
backup-policies
backup-policy-upsert nightly-edge --path /etc/hostname --include-config tag:backup-critical --confirmed
backup-policy-upsert nightly-edge --schedule-id <policy_schedule_uuid> --path /etc/hostname --include-config --retention-days 30 --keep-last 7 --rotation-generation keyring/v2 --disabled tag:backup-critical --confirmed
backup-policy-prune --dry-run
```

For routine cleanup, run the worker with opt-in metadata-only retention pruning:

```sh
VPSMAN_POSTGRES_URL=postgres://... \
  target/debug/vpsman-worker --once \
  --backup-policy-prune-enabled \
  --backup-policy-prune-limit 50
```

This worker path uses policy `retention_days` and `keep_last`, records a
sanitized `backup_policy.retention_pruned` audit entry, and leaves object-store
bytes untouched unless local filesystem deletion is explicitly configured:

```sh
VPSMAN_POSTGRES_URL=postgres://... \
  target/debug/vpsman-worker --once \
  --backup-policy-prune-enabled \
  --backup-policy-prune-delete-objects \
  --backup-policy-prune-object-store-dir .tmp/objects/backups
```

Use the explicit API/CLI/VTY/panel prune action for previews, one-off cleanup,
or S3-backed object deletion until the worker S3 deletion adapter is selected.

## Request And Run A Backup

Create a metadata request:

```sh
cargo run -p vpsctl -- backup-request \
  --client-id edge-a \
  --paths /etc/hostname \
  --include-config \
  --confirmed
```

Run a privilege-gated backup job:

```sh
cargo run -p vpsctl -- backup-run \
  --paths /etc/hostname \
  --include-config \
  --clients edge-a \
  --confirmed
```

`backup-run` auto-creates a per-target backup request when no open request
already matches the client and payload hash. If the agent emits a valid
plain backup artifact and the backup object store is configured, the API
links the artifact automatically after the output row, target state, and parent
job terminal state are durable. Auto-linking is best-effort; if object storage
or artifact validation fails, the backup job still reaches a terminal state and
the retained output can be handed off manually.

Selected backup paths do not follow symlinks by default. Use
`--follow-symlinks` only when the reviewed backup scope intentionally includes
the symlink target bytes; the choice is recorded with the backup request,
policy, job, and artifact metadata.

A selected directory captures regular files recursively. Direct backups are
bounded configuration snapshots, not unbounded application-data backups: a
scanned-path ceiling bounds traversal, while file count, uncompressed bytes,
and archive bytes bound captured content.
The backup path presets therefore cover host, service, reverse-proxy, and Docker
daemon configuration; they do not claim to protect Docker volumes or arbitrary
`/srv` and `/opt` data.

Missing selected roots fail by default. For a reviewed cross-distribution or
heterogeneous-fleet scope, add `--skip-missing-paths` (or select **Skip missing
roots** in the panel). Only roots that do not exist are omitted; unreadable
paths, traversal errors, size limits, and an empty captured scope still fail.
The artifact status records omitted paths and reasons. The OS config, Web
config, and Docker config backup path presets select this policy explicitly,
while the Identity preset
remains strict.

```sh
cargo run -p vpsctl -- backup-run \
  --paths /etc/nginx,/etc/caddy \
  --skip-missing-paths \
  --tags web \
  --confirmed
```

Inspect artifacts:

```sh
cargo run -p vpsctl -- backup-artifacts
```

If the backup job completed but the artifact did not auto-link, promote the
retained plain stdout into the object store. Use `--job-id` when multiple
completed backup jobs used the same backup scope:

```sh
cargo run -p vpsctl -- backup-artifact-handoff \
  --backup-request-id <backup_request_uuid> \
  --job-id <backup_job_uuid> \
  --confirmed
```

If you already have a plain backup artifact file, upload it into the local
object-store-backed artifact registry:

```sh
cargo run -p vpsctl -- backup-artifact-upload \
  --backup-request-id <backup_request_uuid> \
  --object-key backups/edge-a/example.tar \
  --artifact-file ./artifact.tar \
  --confirmed
```

For larger plain backup artifact files, use the server-mediated chunked session.
The API validates the final size, SHA-256, tar manifest, and object-key
uniqueness before linking metadata:

```sh
cargo run -p vpsctl -- backup-artifact-upload-chunked \
  --backup-request-id <backup_request_uuid> \
  --object-key backups/edge-a/example-large.tar \
  --artifact-file ./artifact.tar \
  --chunk-size-bytes 4194304 \
  --confirmed
```

Stored artifact upload and download paths share the same configured API
artifact limit. The default maximum is 128 MiB; set
`api.artifact_max_bytes` in the suite config or `VPSMAN_ARTIFACT_MAX_BYTES` in
the API environment to change it. Values are clamped between 1 MiB and 4 GiB.
`api.job_output_artifact_min_bytes` remains only the threshold for externalizing
large job output chunks to object storage.

In the official Compose deployment, Nginx allows `25m` per `/api/` request so
the current Base64-expanded JSON request envelopes fit. This does not reduce the
artifact limit: large backup artifacts use the chunked command above (4 MiB
chunks by default), and downloads use binary streaming rather than one large
JSON request.

For an explicit S3/MinIO-backed deployment, configure the full
`VPSMAN_OBJECT_*` set before starting the API. The adapter uses path-style
SigV4 over the configured endpoint, rejects duplicate objects with `HEAD`
before `PUT`, and streams verified downloads through a temporary spool file
with configured size and hash validation before responding to the client:
Remote object-store endpoints must use `https://`; plaintext `http://` is
accepted only for loopback/local MinIO endpoints such as `localhost` or
`127.0.0.1` used by development smoke tests.

```sh
bash scripts/smoke-minio-backup-artifact.sh
```

## Plan And Run Restore

```sh
cargo run -p vpsctl -- restore-plan \
  --source-backup-request-id <backup_request_uuid> \
  --target-client-id edge-b \
  --confirmed
```

Restore from an archive staged through a completed file-transfer upload on the
target agent. The restore request selects that transfer record; restore scope is
derived from the backup request, while path, size, and SHA-256 are derived from
the recorded transfer and matching backup artifact. Operators do not type
restore archive paths or hashes into restore-run; those values come from the
selected upload record.

The CLI and VTY generate restore destinations under
`/var/lib/vpsman/restores` by default. For sandbox agents, set
`VPSMAN_RESTORE_DESTINATION_ROOT_BASE` to another absolute base directory before
creating restore plans or restore runs.

```sh
cargo run -p vpsctl -- file-transfer-upload \
  --source ./backup.tar \
  --path /tmp/vpsman-restore-backup.tar \
  --clients edge-b \
  --confirmed

cargo run -p vpsctl -- restore-run \
  --source-backup-request-id <backup_request_uuid> \
  --target-client-id edge-b \
  --archive-transfer-session-id <completed_upload_session_uuid> \
  --confirmed
```

## Roll Back A Restore

Use retained successful restore status output to build the rollback command:

```sh
cargo run -p vpsctl -- restore-rollback \
  --restore-job-id <restore_job_uuid> \
  --target-client-id edge-b \
  --confirmed
```

Rollback first validates the full recorded destination set, then revalidates
each file immediately before replacing or removing it. If a service or local
writer changes one destination after restore, rollback preserves that changed
file, continues with independent destinations, and finishes as
`partial_failure` with both successful rollback evidence and per-file failures.
Inspect that evidence before retrying or applying a manual compensating change.

Unprivileged targets degrade by default for privileged restore paths. Use
`--force-unprivileged` only when a best-effort attempt is intentional.

## Run Rebuilt-VPS Migration

After a rebuild and direct identity rotation, prefer `migration-run` when you want one
audited operation that creates the migration link and dispatches the selected
restore plan:

```sh
cargo run -p vpsctl -- migration-run \
  --restore-plan-id <restore_plan_uuid> \
  --archive-transfer-session-id <completed_upload_session_uuid> \
  --confirmed
```

The command loads the restore plan, creates the migration link, and dispatches
the restore command with a request-bound privilege assertion. The API does not
receive inline restore archive bytes or plaintext super password material.

Use `migration-link` only when you need metadata linkage without running a
restore:

```sh
cargo run -p vpsctl -- migration-link \
  --restore-plan-id <restore_plan_uuid> \
  --confirmed
```

Use this with `agent-identity-upsert --replace-existing-key` from `02-install-agents.md` to keep
server-side state intact while replacing the VPS.

## Panel Workflow

Use the Backups panel for the same sequence:

1. Create or inspect backup request.
2. Choose strict roots for exact hosts or **Skip missing roots** for reviewed
   heterogeneous scopes; directory roots capture regular files recursively.
3. Save backup policies and use Policy prune for dry-run or confirmed
   retention cleanup.
4. Promote retained plain output or upload a plain backup artifact if needed.
5. Create restore plan.
6. Run restore with a selected completed archive upload record.
7. Roll back restore from retained restore evidence if needed.
8. Use Run migration restore for rebuilt targets, or link metadata only when
   restore has already been handled.
