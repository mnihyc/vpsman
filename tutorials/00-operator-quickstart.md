# Tutorial 00: Operator Quickstart

This is the shortest practical path for trying vpsman as an operator. It
assumes a local development control plane and one test VPS or VM.

## 1. Start The Control Plane

Use local disk object storage first:

```sh
docker run --rm --name vpsman-quickstart-postgres \
  -e POSTGRES_DB=vpsman \
  -e POSTGRES_USER=vpsman \
  -e POSTGRES_PASSWORD=vpsman \
  -p 127.0.0.1:5432:5432 \
  postgres:16-alpine
```

In the service shells, point API and worker at that Postgres instance:

```sh
export VPSMAN_API_BIND=127.0.0.1:8080
export VPSMAN_API_URL=http://127.0.0.1:8080
export VPSMAN_POSTGRES_URL=postgres://vpsman:vpsman@127.0.0.1:5432/vpsman
export VPSMAN_GATEWAY_BIND=127.0.0.1:9443
export VPSMAN_GATEWAY_CONTROL_BIND=127.0.0.1:9444
export VPSMAN_GATEWAY_CONTROL_URL=http://127.0.0.1:9444
export VPSMAN_GATEWAY_SPOOL_DIR=.tmp/quickstart-gateway-spool
export VPSMAN_BACKUP_OBJECT_STORE_DIR=.tmp/objects/backups
export VPSMAN_ARTIFACT_MAX_BYTES=134217728

# This tutorial uses a disposable database. Start it with a matching empty
# gateway spool so a previous local run cannot replay events into new IDs.
rm -rf "$VPSMAN_GATEWAY_SPOOL_DIR"

# Generate one consistent secret set. Do not substitute unrelated random keys.
export VPSMAN_SUPER_PASSWORD='<local_super_password>'
cargo run -p vpsctl -- compose-secrets --secrets-dir .tmp/quickstart-secrets
unset VPSMAN_SUPER_PASSWORD
source .tmp/quickstart-secrets/operator-privilege.env
export VPSMAN_INTERNAL_TOKEN="$(<.tmp/quickstart-secrets/vpsman_internal_token)"

# In each of three shells, repeat the shared exports above, then run one service.
cargo run -p vpsman-api
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$(<.tmp/quickstart-secrets/vpsman_gateway_private_key_hex)" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$(<.tmp/quickstart-secrets/vpsman_privilege_verifier_key_hex)" \
  cargo run -p vpsman-gateway
cargo run -p vpsman-worker
```

Run the panel in another shell:

```sh
cd frontend
npm run dev -- --port 5173
```

Open `http://127.0.0.1:5173`.

## 2. Create First Operator

On an empty control plane, the browser shows **Create first operator**. Create
the initial admin operator there; the console signs in immediately after the
account is created. On later visits, the same screen shows **Sign in** for
existing operators.

If you also want to run the CLI examples below from a shell, sign in with
`vpsctl` and export the returned API token:

```sh
export VPSMAN_OPERATOR_PASSWORD=<admin_password>
cargo run -p vpsctl -- login --username admin --password-env VPSMAN_OPERATOR_PASSWORD
export VPSMAN_API_TOKEN=<operator_token>
```

Keep privilege unlock material local:

```sh
export VPSMAN_SUPER_PASSWORD=<local_super_password>
source .tmp/quickstart-secrets/operator-privilege.env
```

The API token authenticates the operator. The super password and salt are used
locally to build request-bound privilege assertions. The API forwards those
assertions to the private gateway for verification and never receives the
plaintext super password.

Access scopes separate fleet metadata from sensitive payloads. `fleet:read`
can inspect status and inventory, while job outputs, terminal replay,
integration payloads, saved templates, schedules, rendered config, and full
tunnel plans require the read scopes listed in
[`docs/operator-access-scopes.md`](../docs/operator-access-scopes.md).

## 3. Install One VPS

Register a direct gateway agent identity:

```sh
export EDGE_CLIENT_ID=1
cargo run -p vpsctl -- noise-keygen
cargo run -p vpsctl -- agent-identity-upsert \
  --client-id "$EDGE_CLIENT_ID" \
  --client-public-key-hex <agent_noise_public_key_hex> \
  --display-name edge-01 \
  --tags country:US,role:edge \
  --confirmed
```

Generate a different Noise keypair for every VPS. Public-key ownership is
global: a key already active or retired under any client ID is rejected rather
than allowing one agent to impersonate another.

Install the agent with `deploy/install-agent.sh` or follow
`02-install-agents.md`. After it connects, use the assigned display name in Fleet
and the numerical client ID in exact target expressions.

## 4. Organize And Inspect

```sh
cargo run -p vpsctl -- tag-create --name edge
cargo run -p vpsctl -- tag-create --name provider:provider-a
cargo run -p vpsctl -- tag-create --name region:sfo
cargo run -p vpsctl -- agent-tag --client-id "$EDGE_CLIENT_ID" --tag edge
cargo run -p vpsctl -- agent-tag --client-id "$EDGE_CLIENT_ID" --tag provider:provider-a
cargo run -p vpsctl -- agent-tag --client-id "$EDGE_CLIENT_ID" --tag region:sfo
cargo run -p vpsctl -- summary
cargo run -p vpsctl -- fleet-alerts
```

Use tags for provider/resource ownership and operating intent. Always resolve
targets before bulk work:

```sh
cargo run -p vpsctl -- bulk-resolve --tags edge,provider:provider-a,region:sfo
```

`bulk-resolve` is the headless equivalent. Browser workflows preview targets
in place; their **Review** step asks the server to resolve and freeze the exact
list, without a separate bulk page.

## 5. Run A Privileged Command

```sh
cargo run -p vpsctl -- job-create --command uptime --clients "$EDGE_CLIENT_ID" --confirmed
cargo run -p vpsctl -- jobs --limit 10
cargo run -p vpsctl -- job-follow --job-id <job_uuid> --max-polls 60
cargo run -p vpsctl -- job-target-status-download \
  --job-id <job_uuid> \
  --output-file ./job-status.tar
```

For interactive work:

```sh
cargo run -p vpsctl -- terminal-open --argv /bin/sh --clients "$EDGE_CLIENT_ID" --confirmed
cargo run -p vpsctl -- terminal-input \
  --client-id "$EDGE_CLIENT_ID" \
  --session-id <session_uuid> \
  --text "uname -a\n" \
  --confirmed
cargo run -p vpsctl -- terminal-poll \
  --session-id <session_uuid> \
  --replay-from-seq 1 \
  --clients "$EDGE_CLIENT_ID" \
  --confirmed
```

Terminal input order is assigned by the server for the selected client and
session; operators submit only the bytes to write.

## 6. Inspect Configuration Sources

Start with system-default inheritance and inspect what the VPS effectively uses:

```sh
cargo run -p vpsctl -- config-presets --behavior host_metrics
cargo run -p vpsctl -- config-sources --client-id "$EDGE_CLIENT_ID"
cargo run -p vpsctl -- config-render --client-id "$EDGE_CLIENT_ID" --format toml
```

Create and assign a custom preset only when the system choices do not fit.
Assignments are explicit per-VPS overrides; reset them to resume system-default
inheritance. See [Tutorial 05](05-configuration-presets.md).

## 7. Back Up And Restore

```sh
cargo run -p vpsctl -- backup-request --client-id "$EDGE_CLIENT_ID" --paths /etc/hostname --confirmed
cargo run -p vpsctl -- backup-run --paths /etc/hostname --clients "$EDGE_CLIENT_ID" --confirmed
cargo run -p vpsctl -- backup-artifacts
```

Backup selected directories capture regular files recursively under scan,
file-count, plaintext-byte, and archive-byte bounds. Missing roots fail by
default; add `--skip-missing-paths` only for a reviewed heterogeneous scope.
Selected paths reject symlinks by default. Add `--follow-symlinks` only when the
symlink target bytes are intentionally part of the reviewed backup.

Create a restore plan before changing a rebuilt VPS:

```sh
cargo run -p vpsctl -- restore-plan \
  --source-backup-request-id <backup_request_uuid> \
  --target-client-id "$EDGE_CLIENT_ID" \
  --confirmed
```

For rebuilt-client migration, use `migration-run` so the migration link and
restore job are created together. Stage the restore archive with
`file-transfer-upload` first, then use the completed upload session id. Restore
paths, size, and SHA-256 come from that selected upload record:

```sh
cargo run -p vpsctl -- file-transfer-upload \
  --source ./backup.tar \
  --path /tmp/vpsman-restore-backup.tar \
  --clients "$EDGE_CLIENT_ID" \
  --confirmed

cargo run -p vpsctl -- migration-run \
  --restore-plan-id <restore_plan_uuid> \
  --archive-transfer-session-id <completed_upload_session_uuid> \
  --confirmed
```

## 8. Daily Loop

Use this loop while managing 20+ VPSs:

1. Inspect `summary`, `agents`, `fleet-alerts`, and `gateway-sessions`.
2. Resolve exact targets with the browser workflow's inline preview and
   **Review**, or with `bulk-resolve` when operating headlessly.
3. Dispatch through panel, CLI, or VTY with confirmation and local privilege unlock.
4. Observe `jobs`, `job-targets`, `job-target-status-download`,
   `job-outputs`, and alerts.
5. Recover with rollback commands, direct identity key rotation, or reviewed
   configuration-preset changes instead of manual per-host edits.
