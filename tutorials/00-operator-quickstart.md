# Tutorial 00: Operator Quickstart

This is the shortest practical path for trying vpsman as an operator. It
assumes a local development control plane and one test VPS or VM.

## 1. Start The Control Plane

Use local disk object storage first:

```sh
export VPSMAN_API_BIND=127.0.0.1:8080
export VPSMAN_GATEWAY_BIND=127.0.0.1:9443
export VPSMAN_GATEWAY_CONTROL_BIND=127.0.0.1:9444
export VPSMAN_GATEWAY_CONTROL_URL=http://127.0.0.1:9444
export VPSMAN_INTERNAL_TOKEN=dev-internal-token
export VPSMAN_BACKUP_OBJECT_STORE_DIR=.tmp/objects/backups
export VPSMAN_UPDATE_OBJECT_STORE_DIR=.tmp/objects/updates

cargo run -p vpsman-api
cargo run -p vpsman-gateway
cargo run -p vpsman-worker
```

Run the panel in another shell:

```sh
cd frontend
npm run dev -- --port 5173
```

Open `http://127.0.0.1:5173`.

## 2. Bootstrap Access

```sh
export VPSMAN_API_URL=http://127.0.0.1:8080
export VPSMAN_OPERATOR_PASSWORD=<admin_password>
cargo run -p vpsctl -- bootstrap --username admin --password-env VPSMAN_OPERATOR_PASSWORD
cargo run -p vpsctl -- login --username admin --password-env VPSMAN_OPERATOR_PASSWORD
export VPSMAN_API_TOKEN=<operator_token>
```

Keep privileged proof material local:

```sh
export VPSMAN_SUPER_PASSWORD=<local_super_password>
export VPSMAN_SUPER_SALT_HEX=<64_hex_salt>
```

The API token authenticates the operator. The super password and salt are used
locally to build scoped proofs for privileged work.

## 3. Enroll One VPS

Create an enrollment token:

```sh
cargo run -p vpsctl -- enrollment-token-create \
  --allowed-client-id edge-01 \
  --default-tags edge \
  --ttl-secs 3600 \
  --purpose provision
```

Install the agent on the VPS using the enrollment token and the gateway
endpoint from `02-enroll-agents.md`. After it connects:

```sh
cargo run -p vpsctl -- agents
cargo run -p vpsctl -- gateway-sessions
```

## 4. Organize And Inspect

```sh
cargo run -p vpsctl -- tag-create --name edge
cargo run -p vpsctl -- tag-create --name provider:provider-a
cargo run -p vpsctl -- tag-create --name region:sfo
cargo run -p vpsctl -- agent-tag --client-id edge-01 --tag edge
cargo run -p vpsctl -- agent-tag --client-id edge-01 --tag provider:provider-a
cargo run -p vpsctl -- agent-tag --client-id edge-01 --tag region:sfo
cargo run -p vpsctl -- summary
cargo run -p vpsctl -- fleet-alerts
```

Use tags for provider/resource ownership and operating intent. Always resolve
targets before bulk work:

```sh
cargo run -p vpsctl -- bulk-resolve --tags edge,provider:provider-a,region:sfo
```

## 5. Run A Proof-Gated Command

```sh
cargo run -p vpsctl -- job-create --command uptime --clients edge-01 --confirmed
cargo run -p vpsctl -- jobs --limit 10
cargo run -p vpsctl -- job-follow <job_uuid> --max-polls 60
```

For interactive work:

```sh
cargo run -p vpsctl -- terminal-open --argv /bin/sh --clients edge-01 --confirmed
cargo run -p vpsctl -- terminal-input \
  --session-id <session_uuid> \
  --input-seq 1 \
  --text "uname -a\n" \
  --clients edge-01 \
  --confirmed
cargo run -p vpsctl -- terminal-poll \
  --session-id <session_uuid> \
  --replay-from-seq 1 \
  --clients edge-01 \
  --confirmed
```

## 6. Choose Data Sources

Use presets instead of editing hardcoded commands per VPS:

```sh
cargo run -p vpsctl -- data-source-presets --domain telemetry_metrics_source
cargo run -p vpsctl -- data-source-status --client-id edge-01
cargo run -p vpsctl -- data-source-hot-config --client-id edge-01 --format toml
```

Assign shared presets to tags or explicit clients, and reserve VPS-local presets
for machine-specific custom commands.

## 7. Back Up And Restore

```sh
cargo run -p vpsctl -- backup-request --client-id edge-01 --paths /etc/hostname --confirmed
cargo run -p vpsctl -- backup-run --paths /etc/hostname --clients edge-01 --confirmed
cargo run -p vpsctl -- backup-artifacts
```

Create a restore plan before changing a rebuilt VPS:

```sh
cargo run -p vpsctl -- restore-plan \
  --source-backup-request-id <backup_request_uuid> \
  --target-client-id edge-01 \
  --paths /etc/hostname \
  --destination-root /restore \
  --confirmed
```

For rebuilt-client migration, use `migration-run` so the migration link and
restore job are created together:

```sh
cargo run -p vpsctl -- migration-run \
  --restore-plan-id <restore_plan_uuid> \
  --confirmed
```

## 8. Daily Loop

Use this loop while managing 20+ VPSs:

1. Inspect `summary`, `agents`, `fleet-alerts`, and `gateway-sessions`.
2. Resolve exact targets with `bulk-resolve`.
3. Dispatch through panel, CLI, or VTY with confirmation and local proof.
4. Observe `jobs`, `job-targets`, `job-outputs`, and alerts.
5. Recover with rollback commands, re-enrollment tokens, or data-source preset
   changes instead of manual per-host edits.
