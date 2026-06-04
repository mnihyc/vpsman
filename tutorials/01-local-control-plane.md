# Tutorial 01: Local Control Plane

This tutorial starts the API, gateway, worker, and frontend for local
development or smoke testing.

## Start With Docker Compose

Use the provided compose template when you want persistent PostgreSQL, local
object storage, Nginx-served Vite static assets, and all backend processes
running together from released assets.

First place release assets into the checkout-local runtime layout:

- server binaries: `deploy/runtime/server/current/bin/`
- migration SQL files: `deploy/runtime/server/current/migrations/`
- extracted Vite frontend `dist/`: `deploy/runtime/frontend/current/dist/`

Then start the stack:

```sh
docker compose -f deploy/compose.yml --env-file deploy/.env.example up -d
```

The default compose shape uses:

- API: `http://127.0.0.1:8080`
- Frontend: `http://127.0.0.1:5173`
- Gateway TCP: `127.0.0.1:9443`
- Gateway control API: private between API and gateway containers
- PostgreSQL: `deploy/runtime/postgres/data`
- Local object storage: `deploy/runtime/data`

For production, replace placeholder secrets in `deploy/.env.example` and serve
the panel/API through HTTPS. Leave S3/MinIO variables unset unless you are
deliberately testing that adapter; local disk object storage is the current
default deployment shape. To upgrade, replace the files under
`deploy/runtime/server/current` and
`deploy/runtime/frontend/current`, then restart the compose stack; no Rust or
frontend rebuild is required. Runtime state stays in checkout-local paths, not
Docker-managed named volumes.

## Start Processes Manually

Manual startup is useful while iterating:

```sh
export VPSMAN_API_BIND=127.0.0.1:8080
export VPSMAN_GATEWAY_BIND=127.0.0.1:9443
export VPSMAN_GATEWAY_CONTROL_BIND=127.0.0.1:9444
export VPSMAN_GATEWAY_CONTROL_URL=http://127.0.0.1:9444
export VPSMAN_INTERNAL_TOKEN=dev-internal-token
export VPSMAN_BACKUP_OBJECT_STORE_DIR=.tmp/objects/backups
export VPSMAN_UPDATE_OBJECT_STORE_DIR=.tmp/objects/updates
export VPSMAN_ALERT_MEMORY_AVAILABLE_WARNING_RATIO=0.20
export VPSMAN_ALERT_MEMORY_AVAILABLE_CRITICAL_RATIO=0.10
export VPSMAN_ALERT_DISK_AVAILABLE_WARNING_RATIO=0.20
export VPSMAN_ALERT_DISK_AVAILABLE_CRITICAL_RATIO=0.10
export VPSMAN_ALERT_CPU_LOAD_WARNING=2.0
export VPSMAN_ALERT_CPU_LOAD_CRITICAL=4.0

cargo run -p vpsman-api
cargo run -p vpsman-gateway
cargo run -p vpsman-worker
```

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
export VPSMAN_API_URL=http://127.0.0.1:8080
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

The alert policy variables are fleet-wide startup defaults. Change them for
your normal operating tolerance, then use data-source presets and future
per-scope policies for environment-specific behavior.
