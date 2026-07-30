# Tutorial 01: Local Control Plane

This tutorial starts the API, gateway, worker, and frontend for local
development or smoke testing.

## Start With Docker Compose

Use the provided compose template when you want persistent PostgreSQL, local
object storage, Nginx-served Vite static assets, and all backend processes
running together from released assets.

The simplest release first start is:

```sh
cd deploy
cp .env.example .env
# Edit POSTGRES_PASSWORD before real deployment. Use URL-safe random hex,
# because compose derives the API/worker Postgres URL from this value.
export VPSMAN_SUPER_PASSWORD='<local_super_password>'
./update.sh first-start latest
```

`update.sh first-start` reads the release's `version.json`, downloads its
server, frontend, and host CLI assets, validates their layouts, installs them
under `runtime/`, generates missing compose secrets from
`VPSMAN_SUPER_PASSWORD`, and starts compose. On success it prints the generated
`VPSMAN_SUPER_SALT_HEX`; save that value for browser and CLI privilege unlock.
The persistent copy is `./config/secrets/operator-privilege.env`.

After the stack starts, open `http://127.0.0.1:5173`. The console discovers
whether any operator exists: an empty control plane shows **Create first
operator**, while an initialized control plane shows **Sign in**.

If you place release assets manually, use the deployment-directory runtime
layout. The repository template names that directory `deploy/`, but the
directory can be copied or renamed:

- server binaries: `runtime/server/current/bin/`
- migration SQL files: `runtime/server/current/migrations/`
- extracted Vite frontend `dist/`: `runtime/frontend/current/dist/`
- host CLI: `runtime/cli/current/vpsctl`
- suite config: `config/vpsman.toml`
- secret files referenced by suite config: generated under
  `config/secrets/`

Then start the stack:

```sh
cd deploy
cp .env.example .env
# Edit POSTGRES_PASSWORD before real deployment. Use URL-safe random hex,
# because compose derives the API/worker Postgres URL from this value.
export VPSMAN_SUPER_PASSWORD='<local_super_password>'
./runtime/cli/current/vpsctl compose-secrets --secrets-dir config/secrets
docker compose up -d
```

If the bundled CLI is not staged yet, run the same helper from the repository
root instead:
`cargo run -p vpsctl -- compose-secrets --secrets-dir deploy/config/secrets`.
It writes the mounted internal token, gateway private key, privilege verifier
key, a gateway public-key file for agent installs, and
`operator-privilege.env` with the generated `VPSMAN_SUPER_SALT_HEX`. Source that
file before using the host CLI for privileged work.

The default compose shape uses:

- browser and host CLI origin: `http://127.0.0.1:5173`
- API container: private `http://api:8080`, reached through the Nginx origin
- Gateway TCP: `127.0.0.1:9443`
- Gateway control API: private between API and gateway containers
- PostgreSQL: `runtime/postgres/data`
- Local object storage: `runtime/data`
- Suite config: `config/vpsman.toml`, mounted through the authoritative
  `/etc/vpsman` config directory in compose

For production, replace placeholder secrets in `.env`, review
`config/vpsman.toml`, and serve the panel through HTTPS while keeping the
operator API private behind the control-plane proxy. The API can atomically save
that same authoritative TOML from the dashboard; runtime data stays under
`runtime/`, and secrets stay in read-only mounts. Local disk object
storage is the default compose shape. Configure the S3/MinIO variables
only when the deployment should use the implemented S3-compatible adapter for
backup or update artifacts. For a reviewed production upgrade, run
`./update.sh vX.Y.Z` with the exact target tag from the deployment directory;
use `latest` only for disposable local evaluation. The updater refreshes the
server, frontend, and host CLI release payloads and recreates the compose
services. Runtime state stays under the deployment directory, not
Docker-managed named volumes.

The current canonical database is intentionally fresh-only and does not support
an in-place update from an earlier schema model; review
[migration compatibility](../docs/migration-compatibility.md) before updating
an older deployment.

## Start Processes Manually

Manual startup is useful while iterating:

```sh
docker run --rm --name vpsman-local-postgres \
  -e POSTGRES_DB=vpsman \
  -e POSTGRES_USER=vpsman \
  -e POSTGRES_PASSWORD=vpsman \
  -p 127.0.0.1:5432:5432 \
  postgres:16-alpine
```

Run each control-plane process in its own shell. Repeat the shared export block
in every shell before starting that shell's service:

```sh
export VPSMAN_API_BIND=127.0.0.1:8080
export VPSMAN_API_URL=http://127.0.0.1:8080
export VPSMAN_POSTGRES_URL=postgres://vpsman:vpsman@127.0.0.1:5432/vpsman
export VPSMAN_GATEWAY_BIND=127.0.0.1:9443
export VPSMAN_GATEWAY_CONTROL_BIND=127.0.0.1:9444
export VPSMAN_GATEWAY_CONTROL_URL=http://127.0.0.1:9444
export VPSMAN_GATEWAY_SPOOL_DIR=.tmp/local-control-plane-gateway-spool
export VPSMAN_BACKUP_OBJECT_STORE_DIR=.tmp/objects/backups
export VPSMAN_ARTIFACT_MAX_BYTES=134217728
export VPSMAN_ALERT_MEMORY_AVAILABLE_WARNING_RATIO=0.20
export VPSMAN_ALERT_MEMORY_AVAILABLE_CRITICAL_RATIO=0.10
export VPSMAN_ALERT_DISK_AVAILABLE_WARNING_RATIO=0.20
export VPSMAN_ALERT_DISK_AVAILABLE_CRITICAL_RATIO=0.10
export VPSMAN_ALERT_CPU_LOAD_WARNING=2.0
export VPSMAN_ALERT_CPU_LOAD_CRITICAL=4.0
# Optional for manual runs. Set this to the operator config file you intend
# to use; compose sets its own container path.
# export VPSMAN_SUITE_CONFIG=.tmp/local-vpsman.toml

# Keep this spool with the database it belongs to. If you intentionally replace
# the local database with an empty one, clear this spool before starting the
# gateway so events from the retired control plane are not replayed.

# Generate the gateway key, privilege verifier, gateway public key, operator
# salt, and shared internal token as one consistent set.
export VPSMAN_SUPER_PASSWORD='<local_super_password>'
cargo run -p vpsctl -- compose-secrets --secrets-dir .tmp/local-control-plane-secrets
unset VPSMAN_SUPER_PASSWORD
export VPSMAN_INTERNAL_TOKEN="$(<.tmp/local-control-plane-secrets/vpsman_internal_token)"

cargo run -p vpsman-api
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$(<.tmp/local-control-plane-secrets/vpsman_gateway_private_key_hex)" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$(<.tmp/local-control-plane-secrets/vpsman_privilege_verifier_key_hex)" \
  cargo run -p vpsman-gateway
cargo run -p vpsman-worker
```

Keep `VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX` gateway-only. The API deliberately
rejects that verifier key in its environment. The generated gateway public-key
file is the value operators save once in the **Agent install command** panel.

In another shell:

```sh
cd frontend
npm run dev -- --port 5173
```

## Verify Basic Access

Check API health and CLI wiring:

```sh
cargo run -p vpsctl -- --api-url http://127.0.0.1:8080 health
cargo run -p vpsctl -- --api-url http://127.0.0.1:8080 bootstrap
```

After creating or obtaining an operator token, export it:

```sh
export VPSMAN_API_TOKEN=<operator_token>
cargo run -p vpsctl -- me
cargo run -p vpsctl -- summary
```

## Useful Local Verification

Run these before trusting a local environment:

```sh
bash scripts/smoke-vpsctl-live-api.sh
bash scripts/smoke-postgres-persistence.sh
bash scripts/smoke-frontend-live-api.sh
```

For a broad pre-release pass:

```sh
bash scripts/release-check.sh
```

The alert policy variables are fleet-wide startup defaults for built-in
resource alerts. For per-VPS traffic rules and environment-specific alert
logic, use Config > Rules or `vpsctl vps-rules`, then Observability > Alerts or
`vpsctl alert-policy`.
