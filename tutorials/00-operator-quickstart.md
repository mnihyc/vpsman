# Tutorial 00: Operator Quickstart

This is the shortest practical path for trying vpsman as an operator. It
assumes a local development control plane and one test VPS or VM.

## 1. Start The Control Plane

Use local disk object storage first:

```sh
export VPSMAN_API_BIND=127.0.0.1:8080
export VPSMAN_API_URL=http://127.0.0.1:8080
export VPSMAN_GATEWAY_BIND=127.0.0.1:9443
export VPSMAN_GATEWAY_CONTROL_BIND=127.0.0.1:9444
export VPSMAN_GATEWAY_CONTROL_URL=http://127.0.0.1:9444
export VPSMAN_INTERNAL_TOKEN=dev-internal-token-change-me-32chars
export VPSMAN_BACKUP_OBJECT_STORE_DIR=.tmp/objects/backups
export VPSMAN_UPDATE_OBJECT_STORE_DIR=.tmp/objects/updates
export VPSMAN_ARTIFACT_MAX_BYTES=134217728

# Run each service in its own shell with the same environment.
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
export VPSMAN_OPERATOR_PASSWORD=<admin_password>
cargo run -p vpsctl -- bootstrap --username admin --password-env VPSMAN_OPERATOR_PASSWORD
cargo run -p vpsctl -- login --username admin --password-env VPSMAN_OPERATOR_PASSWORD
export VPSMAN_API_TOKEN=<operator_token>
```

Keep privilege unlock material local:

```sh
export VPSMAN_SUPER_PASSWORD=<local_super_password>
export VPSMAN_SUPER_SALT_HEX=<64_hex_salt>
```

The API token authenticates the operator. The super password and salt are used
locally to build request-bound privilege assertions. The API forwards those
assertions to the private gateway for verification and never receives the
plaintext super password.

## 3. Install One VPS

Register a direct gateway agent identity:

```sh
cargo run -p vpsctl -- noise-keygen
cargo run -p vpsctl -- agent-identity-upsert \
  --client-id edge-01 \
  --client-public-key-hex <agent_noise_public_key_hex> \
  --display-name edge-01 \
  --tags country:US,role:edge \
  --confirmed
```

Install the agent with `deploy/install-agent.sh` or follow
`02-install-agents.md`. After it connects, use the assigned display name in Fleet
and target expressions.

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
  --session-id <session_uuid> \
  --input-seq 1 \
  --text "uname -a\n" \
  --clients "$EDGE_CLIENT_ID" \
  --confirmed
cargo run -p vpsctl -- terminal-poll \
  --session-id <session_uuid> \
  --replay-from-seq 1 \
  --clients "$EDGE_CLIENT_ID" \
  --confirmed
```

## 6. Choose Data Sources

Use presets instead of editing hardcoded commands per VPS:

```sh
cargo run -p vpsctl -- data-source-presets --domain telemetry_metrics_source
cargo run -p vpsctl -- data-source-status --client-id "$EDGE_CLIENT_ID"
cargo run -p vpsctl -- data-source-hot-config --client-id "$EDGE_CLIENT_ID" --format toml
```

Assign shared presets to tags or explicit clients, and reserve VPS-local presets
for machine-specific custom commands.

## 7. Back Up And Restore

```sh
cargo run -p vpsctl -- backup-request --client-id "$EDGE_CLIENT_ID" --paths /etc/hostname --confirmed
cargo run -p vpsctl -- backup-run --paths /etc/hostname --clients "$EDGE_CLIENT_ID" --confirmed
cargo run -p vpsctl -- backup-artifacts
```

Create a restore plan before changing a rebuilt VPS:

```sh
cargo run -p vpsctl -- restore-plan \
  --source-backup-request-id <backup_request_uuid> \
  --target-client-id "$EDGE_CLIENT_ID" \
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
3. Dispatch through panel, CLI, or VTY with confirmation and local privilege unlock.
4. Observe `jobs`, `job-targets`, `job-target-status-download`,
   `job-outputs`, and alerts.
5. Recover with rollback commands, direct identity key rotation, or data-source preset
   changes instead of manual per-host edits.
