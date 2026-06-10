# vpsman

> This project is highly personalized and managed by AI agents.

`vpsman` is a Rust-based VPS panel with extended special functions: lightweight
headless agents, a raw TCP gateway, an HTTP control plane, a CLI/VTY operator
tool, and a Vite-built web panel.

The public repository intentionally keeps only source, migrations, deployment
templates, and GitHub Actions build definitions. Local planning notes,
operator tutorials, private smoke harnesses, and generated build artifacts are
ignored and are not part of the public tree.

## Components

- `crates/agent`: low-overhead Linux client agent.
- `crates/gateway`: raw TCP gateway for long-lived agent sessions.
- `crates/api`: HTTP/WebSocket control-plane API.
- `crates/worker`: background scheduler and automation worker.
- `crates/vpsctl`: scriptable CLI and interactive VTY shell.
- `crates/common`: shared protocol, auth, config, and telemetry types.
- `frontend`: React + TypeScript panel source.
- `deploy`: Docker Compose and Nginx templates for release binaries.

Targeting is tag-first. Provider/country/group labels are ordinary tags, while
resolver-only inner selectors such as `id:<client_id>` and
`name:<display_name>` are documented in [target selectors](docs/target-selectors.md).
Jobs and schedules execute fixed, reviewed target snapshots: frontend
confirmation and CLI preview resolve selectors to concrete VPS IDs, submit those
IDs to the API, and keep selector text only as audit context for deliberate
manual Target update.


## Operator Tutorials

The operator-facing tutorial index is [tutorials/README.md](tutorials/README.md).
It covers quickstart setup, local control-plane operation, direct gateway agent
installation, fleet organization, daily jobs/schedules, backups, updates, and
headless CLI/VTY workflows.

## Release Assets

GitHub Actions publishes separated runtime assets:

- `vpsman-server-linux-x86_64.zip`
- `vpsman-agent-*-musl`
- `vpsctl-*-musl`
- `vpsman-frontend-dist.tar.gz`
- `version.json`
- `SHA256SUMS`

The root `version-template.json` is the canonical release metadata template.
The release workflow stamps it with the exact tag, commit, and generated asset
list before uploading it to GitHub Releases as `version.json`.

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

In Docker, keep the `.env` object-store paths under `/var/lib/vpsman`
unchanged; compose maps them to `deploy/runtime/data`.

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

## Direct Gateway Agent Install

Remote VPS agents connect to the raw TCP gateway listener. They never contact
the browser panel, panel HTTP API, or a panel-side lookup endpoint during installation. Keep
`9444` private. For public agents, expose or proxy only the agent TCP gateway on
`9443`, provision each agent with gateway Noise identity material, and register
the matching public key as a direct gateway identity.

Typical flow:

```sh
vpsctl noise-keygen
vpsctl agent-identity-upsert \
  --client-id agent-nrt-04 \
  --client-public-key-hex <agent_noise_public_key_hex> \
  --display-name edge-nrt-04 \
  --tags country:JP,role:edge \
  --confirmed

curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh | env \
  VPSMAN_INSTALL_MODE=root \
  VPSMAN_AGENT_CLIENT_ID=agent-nrt-04 \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=<agent_noise_private_key_hex> \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_noise_public_key_hex> \
  VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10,backup=gw-backup.example.com:9443=20' \
  bash
```

Endpoint DNS names and priorities are part of the agent config; no separate
panel-side endpoint lookup is used. Gateway Noise sessions protect agent traffic
from tampering, so there is no extra server-side command-authentication key.
Privilege for mutating work is still request-bound through the local
super-password assertion verified by the private gateway. See
`deploy/AGENT_GATEWAY_INSTALL.md` and `deploy/install-agent.sh`.

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
