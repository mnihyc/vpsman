# vpsman

[![CI](https://github.com/mnihyc/vpsman/actions/workflows/ci.yml/badge.svg)](https://github.com/mnihyc/vpsman/actions/workflows/ci.yml)
[![Release Build](https://github.com/mnihyc/vpsman/actions/workflows/release.yml/badge.svg)](https://github.com/mnihyc/vpsman/actions/workflows/release.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)

`vpsman` is a private VPS fleet control plane for operators who manage
long-lived Linux machines and want to stop falling back to SSH, VNC, shell
scripts, and one-off spreadsheets for routine work.

It combines lightweight agents, a raw TCP gateway, an HTTP/WebSocket API, a
background worker, a scriptable CLI/VTY tool, and a React console. The result is
one operator surface for fleet inventory, reviewed job dispatch, terminal
sessions, file transfer, backups, restores, runtime config, network topology,
agent updates, access control, and audit evidence.

## Contents

- [Why vpsman?](#why-vpsman)
- [Features](#features)
- [Architecture](#architecture)
- [Quick Start](#quick-start)
- [Add a VPS Agent](#add-a-vps-agent)
- [Operating Model](#operating-model)
- [Development](#development)
- [Release Assets](#release-assets)
- [Documentation](#documentation)
- [License](#license)

## Why vpsman?

Most VPS panels are either provider-specific dashboards or consumer hosting
UIs. `vpsman` targets a different operating model:

- You own the control plane and deploy it privately.
- Agents connect outward to your gateway; operators do not need inbound SSH for
  every routine task.
- Jobs, terminals, transfers, backups, and topology work are reviewed against
  explicit VPS targets before mutation.
- Tags and selectors make 20+ heterogeneous VPSs manageable without forcing a
  cloud-native business hierarchy.
- Access scopes, local privilege assertions, retained history, and release
  checks are built for production use.

## Features

| Area | What it covers |
| --- | --- |
| Fleet operations | Inventory, tags, groups, target preview, summaries, alerts, and per-VPS detail panels. |
| Remote work | Reviewed shell/script jobs, interactive terminal sessions, file browser, file transfer, process supervision, and schedules. |
| Backups | Backup requests, chunked artifacts, restore plans, rollback, migration links, and object-store retention. |
| Runtime config | Source templates, per-VPS overrides, bulk config patches, and visible runtime config sync jobs. |
| Network | Tunnel plans, runtime tunnel sync, topology graph/evidence, network tests, speed tests, Bird2/OSPF cost workflows. |
| Access and audit | Operator roles/scopes, sessions, TOTP, direct gateway identities, key rotation/revocation, audit logs, and evidence views. |
| Releases | GitHub release assets, checksum manifests, compose updater, agent update jobs, and rollback-friendly deployment layout. |

## Architecture

```text
Browser / vpsctl
      |
      | HTTPS or private HTTP
      v
vpsman-api  <---->  PostgreSQL  <---->  vpsman-worker
      |
      | private gateway control
      v
vpsman-gateway  <==== raw TCP + Noise ====>  vpsman-agent on each VPS
```

Core packages:

- `crates/api`: HTTP/WebSocket control-plane API.
- `crates/gateway`: long-lived raw TCP agent gateway.
- `crates/agent`: low-overhead Linux VPS agent.
- `crates/worker`: scheduler, retention, and background automation worker.
- `crates/vpsctl`: CLI and interactive VTY operator tool.
- `crates/common`: shared protocol, auth, config, telemetry, and network types.
- `frontend`: React + TypeScript operator console.
- `deploy`: Docker Compose runtime, Nginx config, release updater, and agent installer.

## Quick Start

For a real deployment, start from GitHub Releases through the compose updater.
For development or evaluation, use the local control-plane tutorial.

Prerequisites depend on the path you choose:

- Docker and Docker Compose for the release deployment.
- Rust via `rustup` for source builds; the repo pins the toolchain in
  [rust-toolchain.toml](rust-toolchain.toml).
- Node matching [frontend/.nvmrc](frontend/.nvmrc) for frontend development.
- `curl`, `env`, and systemd for the default root/user agent installer path;
  staged no-systemd installs are also supported.

### Deploy from GitHub Releases

```sh
cd deploy
cp .env.example .env

# Edit .env before production use. POSTGRES_PASSWORD should be a strong,
# URL-safe secret because compose derives service database URLs from it.
export VPSMAN_SUPER_PASSWORD='<local_super_password>'

./update.sh first-start latest
```

The updater downloads release assets, verifies `SHA256SUMS`, creates missing
compose secrets, stages server/frontend/CLI payloads under `deploy/runtime/`,
and starts the stack.

Open `http://127.0.0.1:5173` after first start. When no operator exists, the
console shows **Create first operator** and creates the initial admin session
directly in the browser. After any operator exists, the same page becomes the
normal **Sign in** screen.

By default:

- the browser console binds to `127.0.0.1:5173`;
- the API is private inside the compose network;
- the agent gateway binds to loopback on `9443`;
- persistent data stays under `deploy/runtime/`;
- non-secret suite config stays in `deploy/config/vpsman.toml`;
- generated secrets stay in `deploy/config/secrets/`.

See [deploy/README.md](deploy/README.md) for the full directory layout and
persistence model.

### Run locally from source

The shortest local operator walkthrough is
[tutorials/00-operator-quickstart.md](tutorials/00-operator-quickstart.md).

At a high level:

```sh
# 1. Start Postgres.
docker run --rm --name vpsman-quickstart-postgres \
  -e POSTGRES_DB=vpsman \
  -e POSTGRES_USER=vpsman \
  -e POSTGRES_PASSWORD=vpsman \
  -p 127.0.0.1:5432:5432 \
  postgres:16-alpine

# 2. Export shared service environment.
export VPSMAN_API_BIND=127.0.0.1:8080
export VPSMAN_API_URL=http://127.0.0.1:8080
export VPSMAN_POSTGRES_URL=postgres://vpsman:vpsman@127.0.0.1:5432/vpsman
export VPSMAN_GATEWAY_BIND=127.0.0.1:9443
export VPSMAN_GATEWAY_CONTROL_BIND=127.0.0.1:9444
export VPSMAN_GATEWAY_CONTROL_URL=http://127.0.0.1:9444
export VPSMAN_INTERNAL_TOKEN="$(openssl rand -hex 32)"
export VPSMAN_BACKUP_OBJECT_STORE_DIR=.tmp/objects/backups

# 3. Run each service in its own shell with that environment.
cargo run -p vpsman-api
cargo run -p vpsman-gateway
cargo run -p vpsman-worker

# 4. Run the console.
cd frontend
npm run dev -- --port 5173
```

Open `http://127.0.0.1:5173`. The console asks the API whether an operator
already exists, then shows either **Create first operator** for first start or
**Sign in** for normal access. Follow the tutorials to register agents and
dispatch work.

## Add a VPS Agent

The recommended path is the Access -> VPS identities workflow in the web
console:

1. Open **Access -> VPS identities**.
2. Choose **Register VPS**.
3. Keep the default numerical VPS ID or edit it for an imported legacy ID.
4. Generate a Noise keypair.
5. Review and register the public identity.
6. Fill gateway install defaults once.
7. Copy the generated one-line installer to the VPS.

The generated command installs the latest GitHub release by default and supports
root service, user service, and staged no-systemd installs.

CLI/manual equivalent:

```sh
vpsctl noise-keygen

export VPSMAN_SUPER_PASSWORD='<local_super_password>'
export VPSMAN_SUPER_SALT_HEX='<64_hex_salt>'

vpsctl agent-identity-upsert \
  --client-id 1 \
  --client-public-key-hex <agent_noise_public_key_hex> \
  --display-name edge-nrt-04 \
  --tags country:JP,role:edge \
  --confirmed

curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh | env \
  VPSMAN_INSTALL_MODE=root \
  VPSMAN_AGENT_CLIENT_ID=1 \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=<agent_noise_private_key_hex> \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_noise_public_key_hex> \
  VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10,backup=gw-backup.example.com:9443=20' \
  bash
```

Agents do not call the browser panel, HTTP API, or a panel-side endpoint lookup
during installation. They receive a stable client ID, their private Noise key,
the gateway public key, and a prioritized gateway endpoint list. See
[deploy/AGENT_GATEWAY_INSTALL.md](deploy/AGENT_GATEWAY_INSTALL.md).

## Operating Model

### Targets

Targeting is tag-first. Provider, country, role, and ownership labels are
ordinary tags. Resolver-only selectors such as `id:<client_id>` and
`name:<display_name>` are available for precise work.

Jobs and schedules execute fixed, reviewed target snapshots. Selector text is
kept as audit context, but the submitted API payload contains the concrete VPS
IDs that were reviewed.

Read more in [docs/target-selectors.md](docs/target-selectors.md).

### Access and privilege

Operator API tokens authenticate to the API. Privileged mutations also require a
request-bound assertion created locally from the operator's super password and
salt; the API never receives the plaintext super password.

Access scopes intentionally separate broad fleet metadata from sensitive
payloads, terminal replay, integrations, schedules, templates, rendered config,
and full network plans. See
[docs/operator-access-scopes.md](docs/operator-access-scopes.md).

### Persistence and updates

Compose deployments keep durable state under `deploy/runtime/`:

- PostgreSQL data: `runtime/postgres/data`
- filesystem object store: `runtime/data/objects/backups`
- active server payload: `runtime/server/current`
- active frontend payload: `runtime/frontend/current`
- active CLI payload: `runtime/cli/current`

Update an existing deployment:

```sh
cd deploy
./update.sh latest

# or pin a tag:
./update.sh v0.1.3
```

Rollback swaps server, frontend, and CLI payload directories back together:

```sh
cd deploy
./update.sh rollback
```

The updater verifies checksums and does not delete PostgreSQL or object-store
data.

## Development

Rust uses the repository `rust-toolchain.toml`; Node is pinned through
`frontend/.nvmrc`.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Frontend:

```sh
cd frontend
npm ci
npm run build
npm audit --audit-level=moderate
```

Static Linux agent and CLI builds:

```sh
cargo build --release -p vpsman-agent --target x86_64-unknown-linux-musl
cargo build --release -p vpsctl --target x86_64-unknown-linux-musl
```

Release-gate smoke checks are aggregated by:

```sh
bash scripts/release-check.sh
```

More build notes are in [docs/build.md](docs/build.md).

## Release Assets

The release workflow publishes:

- `vpsman-server-linux-x86_64.zip`
- `vpsman-agent-linux-x86_64-musl`
- `vpsman-agent-linux-aarch64-musl`
- `vpsctl-linux-x86_64-musl`
- `vpsctl-linux-aarch64-musl`
- `vpsman-frontend-dist.tar.gz`
- `version.json`
- `SHA256SUMS`

The release tag is the canonical shipped version. `version.json` is generated
from [version-template.json](version-template.json), stamped with the tag,
commit, asset list, checksum manifest, and tag-pinned download URLs.
Tag-triggered releases run the release version gate first, then the reusable
release quality workflow in
[.github/workflows/release-quality-gate.yml](.github/workflows/release-quality-gate.yml)
before any release artifacts or GitHub release are published.

## Documentation

- [Tutorial index](tutorials/README.md)
- [Operator quickstart](tutorials/00-operator-quickstart.md)
- [Local control plane](tutorials/01-local-control-plane.md)
- [Install agents](tutorials/02-install-agents.md)
- [Daily operations](tutorials/04-daily-operations.md)
- [Backup, restore, and migration](tutorials/07-backup-restore-migration.md)
- [Agent updates](tutorials/08-agent-updates.md)
- [Headless CLI/VTY](tutorials/09-headless-cli-vty.md)
- [Deploy layout](deploy/README.md)
- [Direct gateway agent install](deploy/AGENT_GATEWAY_INSTALL.md)
- [Target selectors](docs/target-selectors.md)
- [Operator access scopes](docs/operator-access-scopes.md)
- [Job status model](docs/job-status-model.md)
- [Build notes](docs/build.md)

## Project Status

`vpsman` is intended for private or production VPS fleet operation by expert
operators. It is not a hosted SaaS product and does not try to imitate provider
business models. Its design goal is an expert-simple control plane: expose
powerful VPS operations directly, keep common tasks fast, and add friction only
where an action is broad, destructive, privileged, or difficult to reverse.

## License

Licensed under either [MIT](LICENSE) or [Apache-2.0](LICENSE), at your option.
