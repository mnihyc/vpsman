# Tutorial 07: Backup, Restore, And Migration

Backups and restores are privilege-gated workflows. Backup private key material
stays local to the operator or browser. The API stores encrypted artifact
metadata and local-disk object-store bytes by default. S3/MinIO-compatible
object storage is implemented as an optional adapter for deployments that need
remote backup or update artifact storage, and is covered by adapter-specific
smokes.

## Schedule Backup Policies

Create a policy for a client, pool, or tag selector. Privilege is verified when
the policy schedule is created or changed. After that, the worker creates due
runs and dispatches saved backup intent at schedule time, producing the same
per-target backup request and encrypted artifact history as a manual
`backup-run`.

```sh
cargo run -p vpsctl -- backup-policy-upsert \
  --name nightly-edge \
  --paths /etc/hostname \
  --include-config \
  --recipient-public-key-hex <32_byte_hex_public_key> \
  --tags backup-critical \
  --interval-secs 86400 \
  --retention-days 30 \
  --keep-last 7 \
  --rotation-generation keyring/v2 \
  --confirmed
```

Inspect policies:

```sh
cargo run -p vpsctl -- backup-policies
```

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
encrypted backup artifact and the backup object store is configured, the API
links the artifact automatically after the output row, target state, and parent
job terminal state are durable. Auto-linking is best-effort; if object storage
or artifact validation fails, the backup job still reaches a terminal state and
the retained output can be handed off manually.

Inspect artifacts:

```sh
cargo run -p vpsctl -- backup-artifacts
```

If the backup job completed but the artifact did not auto-link, promote the
retained encrypted stdout into the object store. Use `--job-id` when multiple
completed backup jobs used the same backup scope:

```sh
cargo run -p vpsctl -- backup-artifact-handoff \
  --backup-request-id <backup_request_uuid> \
  --job-id <backup_job_uuid> \
  --confirmed
```

If you already have an encrypted artifact file, upload it into the local
object-store-backed artifact registry:

```sh
cargo run -p vpsctl -- backup-artifact-upload \
  --backup-request-id <backup_request_uuid> \
  --object-key backups/edge-a/example.json \
  --artifact-file ./artifact.json \
  --confirmed
```

For larger encrypted artifact files, use the server-mediated chunked session.
The API validates the final size, SHA-256, encrypted artifact envelope, and
object-key uniqueness before linking metadata:

```sh
cargo run -p vpsctl -- backup-artifact-upload-chunked \
  --backup-request-id <backup_request_uuid> \
  --object-key backups/edge-a/example-large.json \
  --artifact-file ./artifact.json \
  --chunk-size-bytes 4194304 \
  --confirmed
```

Stored artifact upload, download, and restore-preparation paths share the same
configured API artifact envelope. The default maximum is 128 MiB; set
`api.artifact_max_bytes` in the suite config or `VPSMAN_ARTIFACT_MAX_BYTES` in
the API environment to change it. Values are clamped between 1 MiB and 4 GiB.
`api.job_output_artifact_min_bytes` remains only the threshold for externalizing
large job output chunks to object storage.

For an explicit S3/MinIO-backed deployment, configure the full
`VPSMAN_OBJECT_*` set before starting the API. The adapter uses path-style
SigV4 over the configured endpoint, rejects duplicate objects with `HEAD`
before `PUT`, and streams verified downloads through a temporary spool file
with configured size and hash validation before responding to the client:

```sh
bash scripts/smoke-minio-backup-artifact.sh
```

## Plan And Run Restore

```sh
cargo run -p vpsctl -- restore-plan \
  --source-backup-request-id <backup_request_uuid> \
  --target-client-id edge-b \
  --paths /etc/hostname \
  --destination-root /restore \
  --confirmed
```

Restore from a local encrypted artifact:

```sh
cargo run -p vpsctl -- restore-run \
  --source-backup-request-id <backup_request_uuid> \
  --target-client-id edge-b \
  --artifact-file ./artifact.json \
  --paths /etc/hostname \
  --destination-root /restore \
  --confirmed
```

Restore from the linked stored artifact:

```sh
cargo run -p vpsctl -- restore-run \
  --source-backup-request-id <backup_request_uuid> \
  --target-client-id edge-b \
  --paths /etc/hostname \
  --destination-root /restore \
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

Unprivileged targets degrade by default for privileged restore paths. Use
`--force-unprivileged` only when a best-effort attempt is intentional.

## Run Rebuilt-VPS Migration

After a rebuild and direct identity rotation, prefer `migration-run` when you want one
audited operation that creates the migration link and dispatches the selected
restore plan:

```sh
cargo run -p vpsctl -- migration-run \
  --restore-plan-id <restore_plan_uuid> \
  --confirmed
```

Use `--artifact-file` when the encrypted artifact is local instead of linked
from the backup object store:

```sh
cargo run -p vpsctl -- migration-run \
  --restore-plan-id <restore_plan_uuid> \
  --artifact-file ./artifact.json \
  --private-key-env VPSMAN_BACKUP_PRIVATE_KEY_HEX \
  --confirmed
```

The command loads the restore plan, creates the migration link, decrypts the
artifact locally, and dispatches the restore command with a request-bound
privilege assertion. The API does not receive the backup private key or
plaintext super password.

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
2. Save backup policies and use Policy prune for dry-run or confirmed
   retention cleanup.
3. Promote retained encrypted output or upload an encrypted artifact if needed.
4. Create restore plan.
5. Run restore with local key material.
6. Roll back restore from retained restore evidence if needed.
7. Use Run migration restore for rebuilt targets, or link metadata only when
   restore has already been handled.
