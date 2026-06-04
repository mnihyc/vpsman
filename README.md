# vpsman

Disclaimer: this is a highly personalized project and managed by AI agents.

`vpsman` is a Rust-based VPS management platform with lightweight headless
agents, a raw TCP gateway, an HTTP control plane, a CLI/VTY operator tool, and a
Vite-built web panel.

The public repository intentionally keeps only source, migrations, deployment
templates, and GitHub Actions build definitions. Local planning notes,
operator tutorials, private smoke harnesses, and generated build artifacts are
ignored and are not part of the public tree.

## Components

- `crates/agent`: low-overhead Linux client agent.
- `crates/gateway`: raw TCP gateway for long-lived agent sessions.
- `crates/api`: HTTP/WebSocket control-plane API.
- `crates/worker`: background scheduler and rollout worker.
- `crates/vpsctl`: scriptable CLI and interactive VTY shell.
- `crates/common`: shared protocol, auth, config, and telemetry types.
- `frontend`: React + TypeScript panel source.
- `deploy`: Docker Compose and Nginx templates for release binaries.

## Release Assets

GitHub Actions publishes separated runtime assets:

- `vpsman-server-linux-x86_64.zip`
- `vpsman-agent-*-musl`
- `vpsctl-*-musl`
- `vpsman-frontend-dist.tar.gz`
- `version.json`
- `SHA256SUMS`

The root `version.json` is the canonical release metadata template. The release
workflow stamps it with the exact tag, commit, and generated asset list before
uploading it to GitHub Releases.

The frontend artifact is a static Vite build intended for Nginx, Apache2, or an
equivalent static server.

## Docker Compose Runtime

The compose template runs already-built release assets. It does not rebuild
Rust or frontend code.

Place release files into this checkout-local layout:

- server ZIP contents: `deploy/runtime/server/current/`
- extracted frontend `dist/`: `deploy/runtime/frontend/current/dist/`

Then run:

```sh
cd deploy
cp .env.example .env
# edit .env before real deployment; VPSMAN_INTERNAL_TOKEN is mandatory and
# must be a random non-placeholder value of at least 32 characters
docker compose up -d
```

Persistent runtime data stays in checkout-local paths:

- PostgreSQL: `deploy/runtime/postgres/data`
- local object storage: `deploy/runtime/data`

The compose template publishes only Nginx on all host interfaces. API and
gateway host ports are bound to `127.0.0.1` by default; expose agent TCP through
your chosen public proxy, firewall, or tunnel when needed.

Update an existing Docker deployment from GitHub Releases:

```sh
cd deploy
./update.sh latest
# or pin a release:
./update.sh v0.1.0
```

Rollback swaps back to the previous server/frontend release directories:

```sh
cd deploy
./update.sh rollback
```

The update script downloads release assets, verifies `SHA256SUMS`, updates
`deploy/runtime/server/current` and `deploy/runtime/frontend/current`, and
recreates containers. It does not delete PostgreSQL or local object-storage
data.

## Local Build

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release -p vpsman-api -p vpsman-gateway -p vpsman-worker -p vpsctl
cargo build --release -p vpsman-agent --target x86_64-unknown-linux-musl
cargo build --release -p vpsctl --target x86_64-unknown-linux-musl
```

Frontend:

```sh
cd frontend
npm ci
npm run build
npm audit --audit-level=moderate
```
