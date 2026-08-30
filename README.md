# vpsman

[![CI](https://github.com/mnihyc/vpsman/actions/workflows/ci.yml/badge.svg)](https://github.com/mnihyc/vpsman/actions/workflows/ci.yml)
[![Release Build](https://github.com/mnihyc/vpsman/actions/workflows/release.yml/badge.svg)](https://github.com/mnihyc/vpsman/actions/workflows/release.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)

`vpsman` is a private VPS fleet control plane for operators who manage
long-lived Linux machines and want to stop falling back to SSH, VNC, shell
scripts, and one-off spreadsheets for routine work.

It combines lightweight agents, a raw TCP gateway, an HTTP/WebSocket API, a
background worker, a scriptable CLI/VTY tool, and a React console. The result is
one operator surface for fleet inventory, reviewed job dispatch, terminal
sessions, file transfer, backups, restores, runtime config, network topology,
agent updates, access control, and audit evidence.

![vpsman Home overview with live fleet posture and operations](docs/assets/vps-overview.png)

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
- [Support and Contributing](#support-and-contributing)
- [Security](#security)
- [License](#license)

## Why vpsman?

Most VPS panels are either provider-specific dashboards or consumer hosting
UIs. `vpsman` targets a different operating model:

- You own the control plane and deploy it privately.
- Agents connect outward to your gateway; operators do not need inbound SSH for
  every routine task.
- Mutating work is bound to explicit VPS targets; broad, destructive,
  privileged, or difficult-to-reverse actions add a frozen review step.
- Tags and selectors make 20+ heterogeneous VPSs manageable without forcing a
  cloud-native business hierarchy.
- Access scopes, local privilege assertions, retained history, and release
  checks are built for production use.

## Features

| Area             | What it covers                                                                                                                                                                                                                                                                                             |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Fleet operations | Inventory, tags, groups, target preview, summaries, alerts, and per-VPS detail panels.                                                                                                                                                                                                                     |
| Remote work      | Reviewed shell/script jobs, interactive terminal sessions, file browser, file transfer, host and managed processes, native services, read-only storage inventory, and schedules.                                                                                                                           |
| Host maintenance | Explicit APT/DNF/YUM/Pacman update plans, stale-plan rejection, per-VPS apply evidence, and durable canary/batch job rollouts.                                                                                                                                                                             |
| Backups          | Bounded recursive configuration snapshots, chunked artifacts, restore plans, rollback, migration links, and object-store retention.                                                                                                                                                                        |
| Runtime config   | Immutable system presets, reusable custom presets, a typed per-VPS desired-config tree with explicit inherit/reset controls, reviewed bulk incremental patches, and visible runtime sync state.                                                                                                            |
| Network          | Explicit NAT-safe tunnel plans, per-VPS owned nftables port forwarding, exact endpoint evidence, topology, bounded network tests, and optional daemon-neutral routing-cost adapters.                                                                                                                       |
| Observability    | Clickable Home posture, an all-VPS visual grid with Comfortable and Compact densities, canonical per-VPS resource/network/Ping history, reusable Ping targets, managed read-only shared views, explicit freshness and coverage, alert policies, event webhooks, and bounded automatic telemetry retention. |
| Access and audit | Operator roles/scopes, searchable and revocable bearer sessions, QR-assisted TOTP enrollment, direct gateway identities, key rotation/revocation, audit logs, and evidence views.                                                                                                                          |
| Releases         | Authoritative `version.json` metadata, GitHub release assets, compose updater, agent update jobs, and rollback-friendly deployment layout.                                                                                                                                                                 |

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

For a real deployment, start from an exact-tag, versioned GitHub release
bundle. For development or evaluation, use the local control-plane tutorial.

Prerequisites depend on the path you choose:

- An x86-64 Linux host with Docker and Docker Compose for the release
  control-plane deployment; agents and standalone `vpsctl` also support ARM64.
- Rust via `rustup` for source builds; the repo pins the toolchain in
  [rust-toolchain.toml](rust-toolchain.toml).
- Node matching [frontend/.nvmrc](frontend/.nvmrc) for frontend development.
- `awk`, `curl`, `env`, `flock`, and `mktemp` for the generated agent installer
  command, plus systemd for the default root/user service path;
  staged no-systemd installs are also supported.

### Deploy from GitHub Releases

```sh
export VPSMAN_RELEASE_TAG=vX.Y.Z
release_url="https://github.com/mnihyc/vpsman/releases/download/${VPSMAN_RELEASE_TAG}"
curl -fLO "${release_url}/vpsman-deploy-${VPSMAN_RELEASE_TAG}.tar.gz"
tar -xzf "vpsman-deploy-${VPSMAN_RELEASE_TAG}.tar.gz"
cd "vpsman-deploy-${VPSMAN_RELEASE_TAG}"

cp .env.example .env

# Edit .env before production use. POSTGRES_PASSWORD should be a strong,
# URL-safe secret because compose derives service database URLs from it.
export VPSMAN_SUPER_PASSWORD='<local_super_password>'

./update.sh first-start "$VPSMAN_RELEASE_TAG"
unset VPSMAN_SUPER_PASSWORD
```

The updater reads the release's authoritative `version.json`, downloads its
server/frontend/CLI assets, validates their layouts, creates missing compose
secrets, stages the payloads under `runtime/`, and starts the stack. On success,
first-start prints `VPSMAN_SUPER_SALT_HEX`; save that line with the super
password for browser and CLI privilege unlock. Its persistent deployment copy
is `./config/secrets/operator-privilege.env`.

Open `http://127.0.0.1:5173` after first start. When no operator exists, the
console shows **Create first operator** and creates the initial admin session
directly in the browser. After any operator exists, the same page becomes the
normal **Sign in** screen.

By default:

- the browser console binds to `127.0.0.1:5173`;
- the API is private inside the compose network;
- the agent gateway binds to loopback on `9443`;
- persistent data stays under `runtime/`;
- non-secret suite config stays in `config/vpsman.toml`;
- generated secrets stay in `config/secrets/`.

See [deploy/README.md](deploy/README.md) for the full directory layout and
persistence model. Before exposing a real control plane, follow the
[production deployment, backup, restore, and upgrade runbook](docs/production-deployment.md).

The bundled host CLI is `./runtime/cli/current/vpsctl`. Point it at the Nginx
console/API origin, for example:

```sh
./runtime/cli/current/vpsctl \
  --api-url http://127.0.0.1:5173 \
  health
```

### Run locally from source

The shortest local operator walkthrough is
[tutorials/00-operator-quickstart.md](tutorials/00-operator-quickstart.md).

At a high level:

```sh
# 1. Start Postgres.
docker run -d --rm --name vpsman-quickstart-postgres \
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
export VPSMAN_GATEWAY_SPOOL_DIR=.tmp/quickstart-gateway-spool
export VPSMAN_BACKUP_OBJECT_STORE_DIR=.tmp/objects/backups

# The disposable Postgres above starts an empty control plane. Pair it with an
# empty quickstart spool so events from an older local run cannot be replayed.
rm -rf "$VPSMAN_GATEWAY_SPOOL_DIR"

# Generate the mutually consistent gateway and privilege secrets once.
export VPSMAN_SUPER_PASSWORD='<local_super_password>'
cargo run -p vpsctl -- compose-secrets --secrets-dir .tmp/quickstart-secrets
unset VPSMAN_SUPER_PASSWORD
export VPSMAN_INTERNAL_TOKEN="$(<.tmp/quickstart-secrets/vpsman_internal_token)"

# 3. In each of three shells, repeat the shared exports above, then run one service.
cargo run -p vpsman-api
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$(<.tmp/quickstart-secrets/vpsman_gateway_private_key_hex)" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$(<.tmp/quickstart-secrets/vpsman_privilege_verifier_key_hex)" \
  cargo run -p vpsman-gateway
cargo run -p vpsman-worker

# 4. Run the console.
cd frontend
npm run dev -- --port 5173
```

Open `http://127.0.0.1:5173`. The console asks the API whether an operator
already exists, then shows either **Create first operator** for first start or
**Sign in** for normal access. Follow the tutorials to register agents and
dispatch work. Stop the disposable database afterward with
`docker stop vpsman-quickstart-postgres`.

## Add a VPS Agent

The recommended path is the Access > VPS identities workflow in the web
console:

1. As an admin, save the reusable gateway public key, prioritized endpoints,
   and install mode under **Access > Gateway sessions**.
2. Open **Access > VPS identities**.
3. Choose **Register VPS**.
4. Keep the next numbered VPS ID (`v-1`, `v-2`, …) or edit it for an existing imported ID.
5. Generate a unique Noise keypair for this VPS.
6. Review and register the public identity.
7. Review the saved gateway install defaults.
8. Copy the generated one-line installer to the VPS.

The generated command downloads the stable repository installer. That
bootstrap reads authoritative `version.json` metadata and installs the
control-plane build's tagged agent release, or the latest stable release from a
source build. Root service, user service, and explicitly staged no-systemd
installs are supported. Staging prints the exact foreground command needed to
start the agent.

CLI/manual equivalent:

```sh
vpsctl noise-keygen

export VPSMAN_SUPER_PASSWORD='<local_super_password>'
# Source checkout; release deployments use ./config/secrets/operator-privilege.env.
source .tmp/quickstart-secrets/operator-privilege.env

vpsctl agent-identity-upsert \
  --client-id v-1 \
  --client-public-key-hex <agent_noise_public_key_hex> \
  --display-name edge-nrt-04 \
  --tags country:JP,role:edge \
  --confirmed

# Download the stable installer once, then run it with the reviewed release:
curl -fLO https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/install-agent.sh
env \
  VPSMAN_AGENT_RELEASE=vX.Y.Z \
  VPSMAN_INSTALL_MODE=root \
  VPSMAN_AGENT_CLIENT_ID=v-1 \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=<agent_noise_private_key_hex> \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=<gateway_noise_public_key_hex> \
  VPSMAN_GATEWAY_ENDPOINTS='primary=gw.example.com:9443=10,backup=gw-backup.example.com:9443=20' \
  bash ./install-agent.sh
```

Agents do not call the browser panel, HTTP API, or a panel-side endpoint lookup
during installation. They receive a stable client ID, their private Noise key,
the gateway public key, and a prioritized gateway endpoint list. See
[deploy/AGENT_GATEWAY_INSTALL.md](deploy/AGENT_GATEWAY_INSTALL.md).

One Noise public key identifies exactly one VPS. The control plane rejects a
key already active under another client ID, and rotated, revoked, or deleted
keys remain retired rather than becoming reusable identities.

## Operating Model

### Targets

Targeting is tag-first. Provider, country, role, and ownership labels are
ordinary tags. Resolver-only selectors such as `id:<client_id>` and
`name:<display_name>` are available for precise work.

Jobs and schedules execute fixed, reviewed target snapshots. Selector text is
kept as audit context, but the submitted API payload contains the concrete VPS
IDs that were reviewed. **System > Maintenance > Stale selectors** consolidates
deliberate target refresh for mutable Schedule and Ping snapshots. Active
shared-view definitions are mutable only through reviewed **Edit** or their
saved-selector-only **Update targets** shortcut. Edit can change the name,
selector, exact frozen targets, and visible-data scope; every revision retains
immutable approval and audit evidence.

Schedules have two explicit trigger modes. **Time · cron** retains the reviewed
recurring workflow. **Alert event** listens only to policy-confirmed
`alert.triggered` and `alert.resolved` edges, then renders a strict argv template
and dispatches the same fixed target snapshot. It never reacts directly to a
raw status, job, or telemetry event, so policy Sustained/Count confirmation and
Resolve hysteresis absorb flapping before automation runs.

Read more in [docs/target-selectors.md](docs/target-selectors.md).

### Expected-offline suspension

Fleet inventory can mark any `never`, `disconnected`, `offline`, or `stale`
VPS as **Suspended** from the row action beside agent Stop/Restart. Suspension
keeps the VPS and all retained history in Fleet, but removes it from monitoring
and Network Metrics, suppresses warnings and client-scoped alert delivery, and
neutral-skips new/unstarted work as `target_suspended`. Manual unsuspend
restores its saved prior state; an authenticated online event also clears
suspension. See [Operator access scopes](docs/operator-access-scopes.md) for the
exact API, authorization, and alert boundary.

### Monitoring and shared views

**Fleet > Monitor** is the primary fleet overview and defaults to every matching
VPS. Comfortable cards retain identity context and fuller histories; Compact
cards use a materially denser metric hierarchy. Both show exact values alongside
visual CPU, RAM, aggregate disk, load, network, configured traffic, and primary
Ping evidence. When explicitly shared and available, the same compact surfaces
also present billing-cycle, uptime, connection-count, swap, and normalized
system-information evidence. Shared-but-unavailable current facts remain
visible as `-`, while data groups outside the share's current reviewed scope remain
absent. Selecting a card opens the canonical VPS detail with shared
**15m**, **1h**, **8h**, **1d**, **7d**, **30d**, **90d**, **180d**, **1y**,
**All**, and **Custom** ranges. 15m is the rolling realtime view.

Private and shared VPS cards can sort by Traffic raw volume or quota ratio,
Realtime speed, Connections, CPU utilization, and RAM, Disk, or Load in raw or
ratio form. A metric sort compares only that selected value, highest first,
then places missing values last and breaks ties by VPS name. Stale or warning
state and unlimited traffic receive no implicit metric-sort rank; **Warnings
first** remains the explicit status-aware choice.

Accepted high-resolution samples default to 1 day. Authoritative resource,
network, Ping, and reset-safe traffic history is promoted through UTC-aligned
age tiers. System metrics write only one-minute source buckets; closed buckets
are promoted through the same age tiers, and terminal daily history is retained
through 3,650 days. Charts never fabricate fine points.

**Observability > Ping targets** manages reusable ICMP/TCP definitions, frozen
VPS assignments, and an explicit primary target for each card.
**Observability > Alerts** owns every alert through typed state, metric, or
occurrence evidence. Each rule has a Trigger condition, an optional Trigger
meta condition (Immediate by default), an optional Resolve condition, and a
Resolve meta condition. Conditions recover automatically; occurrences expire
automatically after their configured elapsed window and may be resolved early.
Alert lifecycle automation emits only the generic `alert.triggered` and
`alert.resolved` edges; either edge can reach webhooks and Alert-event
schedules. Webhooks may also independently consume their documented raw event
contexts.
**Observability > Shared views** creates expiring public read-only projections,
then retains the lifecycle needed to edit an active definition, update frozen
targets from its saved selector, copy the URL, extend the link, or revoke it. A
reviewed **Edit** can change the name, selector, frozen VPSs, and visible-data
scope while preserving the bearer URL, visitor history, and unchanged VPS
keys. **Extend** remains the only action that changes expiry. Public projections use
persisted random share-specific VPS keys and never expose internal VPS IDs or
network-address fields, raw host files, internal configuration, actions, jobs, terminals,
files, backups, audit data, or operator identity. Operator-entered public labels
remain verbatim. Visitor bootstrap/data reads live only below
`/api/v1/public/monitoring-shares`; share management remains authenticated.

See [Telemetry metric definitions](docs/telemetry-metrics.md) for aggregation,
retention, gaps, and source semantics.

### Access and privilege

Operator API tokens authenticate to the API. Privileged mutations also require a
request-bound assertion created locally from the operator's super password and
generated deployment salt; the API never receives either local unlock input.

Access scopes intentionally separate broad fleet metadata from sensitive
payloads, terminal replay, integrations, schedules, configuration presets, rendered config,
and full network plans. See
[docs/operator-access-scopes.md](docs/operator-access-scopes.md).

### Backups and file integrity

Directory selections create bounded recursive snapshots of regular files. They
are intended for host and service configuration, not unbounded application or
container-volume data. Missing selected roots fail by default; operators may
explicitly skip only missing roots for a reviewed heterogeneous-fleet backup.

Remote text replacement is revision-bound: an existing file must carry the
hash read by the editor, and the agent rechecks it immediately before atomic
replacement. Restore rollback applies the same commit-time stale-content guard
per destination and reports partial completion without overwriting a concurrent
local change. See [Tutorial 07](tutorials/07-backup-restore-migration.md) and
[Tutorial 04](tutorials/04-daily-operations.md).

### Persistence and updates

Compose deployments keep durable state under their `runtime/` directory:

- PostgreSQL data: `runtime/postgres/data`
- filesystem object store: `runtime/data/objects/backups`
- active server payload: `runtime/server/current`
- active frontend payload: `runtime/frontend/current`
- active CLI payload: `runtime/cli/current`

API and worker apply the bundled ordinary SQLx migrations to PostgreSQL.

Update an existing deployment:

```sh
cd /path/to/the/versioned-deployment
./update.sh vX.Y.Z

# Use latest only for a disposable environment where change review is not
# required:
./update.sh latest
```

Create an immediate, validated PostgreSQL snapshot without stopping the
application services:

```sh
./update.sh backup
```

The custom-format archive is written with mode `0600` under
`runtime/update-backups/`. It is database-only; retain a separate encrypted,
off-host backup of deployment configuration, secrets, and object data.

Rollback swaps server, frontend, and CLI payload directories back together:

```sh
cd /path/to/the/versioned-deployment
./update.sh rollback
```

The updater validates the selected version manifest and payload layouts and
does not delete PostgreSQL or object-store data.

### Desired state and runtime evidence

Host-managed configuration separates durable desired state, per-VPS dispatch,
matching applied evidence, and current runtime observation. The single-VPS
configuration workspace edits one complete sparse override: adding a field
overrides its inherited value, while removing it restores inheritance; resetting
the last field deletes the override. Its Advanced TOML view is the same sparse
document, not an incremental command. Bulk changes instead use an explicit
incremental patch language and freeze the exact previewed VPS IDs before apply.
Both paths revalidate their preview and desired-state revisions before writing.
For headless bulk changes, `vpsctl config-patch` first prints the server preview;
the confirmed invocation must receive that separately reviewed value through
`--preview-hash`.

An explicit agent config read is optional live evidence and never becomes the
editable base. VPS display names and tags remain server-owned inventory and are
not copied into agent runtime configuration. Saving a tunnel, port-forward rule,
configuration-preset override, or config patch never implies that every target
has applied it. The console reports partial queue failures by VPS and keeps
runtime removal visible as pending until the agent confirms cleanup.
Deleting a tunnel plan retires its desired-state declaration immediately and
returns per-endpoint removal queue outcomes; offline agents remove it when they
next reconcile current desired state. Agents continue using their last accepted
config through control-plane outages and reconcile current database desired
state on reconnect. Identity deletion, key rotation, and key revocation
similarly report the committed change separately from gateway disconnect and
peer-cleanup outcomes, preventing ambiguous retries after a successful durable
mutation. See
[docs/job-status-model.md](docs/job-status-model.md#desired-state-reconciliation).

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

More build notes are in [docs/build.md](docs/build.md).

## Release Assets

The release workflow publishes:

- `vpsman-server-linux-x86_64.zip`
- `vpsman-agent-linux-x86_64-musl`
- `vpsman-agent-linux-aarch64-musl`
- `vpsctl-linux-x86_64-musl`
- `vpsctl-linux-aarch64-musl`
- `vpsman-frontend-dist.tar.gz`
- `vpsman-deploy-vX.Y.Z.tar.gz`
- `LICENSE-APACHE`
- `LICENSE-MIT`
- `version.json`

`version.json` is the authoritative asset manifest for its immutable release
tag. It is generated from [version-template.json](version-template.json) and
stamped with the tag, commit, asset list, and tag-pinned download URLs.
Tag-triggered releases first validate the version and require a successful
`main` CI push for the exact tagged commit before publishing any artifacts.
The workflow refuses to rebuild an existing release or replace its assets and
detects release-tag movement before publication; repository hosting should also
protect the `v*` tag namespace. Prerelease tags are marked as prereleases and
are never promoted to the latest stable release.
Published GitHub release notes are the canonical per-version change record; the
project does not maintain a separate rolling changelog.

## Documentation

- [Build notes](docs/build.md)
- [Tutorial index](tutorials/README.md)
- [Operator quickstart](tutorials/00-operator-quickstart.md)
- [Local control plane](tutorials/01-local-control-plane.md)
- [Install agents](tutorials/02-install-agents.md)
- [Fleet organization](tutorials/03-fleet-organization.md)
- [Daily operations](tutorials/04-daily-operations.md)
- [Configuration presets](tutorials/05-configuration-presets.md)
- [Tunnels and routing adapters](tutorials/06-tunnels-routing-adapters.md)
- [Backup, restore, and migration](tutorials/07-backup-restore-migration.md)
- [Agent updates](tutorials/08-agent-updates.md)
- [Headless CLI/VTY](tutorials/09-headless-cli-vty.md)
- [Deploy layout](deploy/README.md)
- [Production deployment and recovery](docs/production-deployment.md)
- [Direct gateway agent install](deploy/AGENT_GATEWAY_INSTALL.md)
- [Target selectors](docs/target-selectors.md)
- [Operator access scopes](docs/operator-access-scopes.md)
- [Job status model](docs/job-status-model.md)
- [Host management and Linux support](docs/host-management.md)
- [Telemetry metric definitions](docs/telemetry-metrics.md)
- [Port forwarding](docs/port-forwarding.md)
- [Build notes](docs/build.md)

## Support and Contributing

Use GitHub issues for reproducible non-security bugs, documentation gaps, and
bounded feature proposals. Community support is best-effort and has no response
or resolution SLA. See [CONTRIBUTING.md](CONTRIBUTING.md) for the information
to include, local quality gates, and change guidelines.

## Security

See [SECURITY.md](SECURITY.md) for supported releases and private vulnerability
reporting guidance. Do not post exploit details, live credentials, private
keys, or production data in a public issue.

## Project Status

`vpsman` is intended for private or production VPS fleet operation by expert
operators. It is not a hosted SaaS product and does not try to imitate provider
business models. Its design goal is an expert-simple control plane: expose
powerful VPS operations directly, keep common tasks fast, and add friction only
where an action is broad, destructive, privileged, or difficult to reverse.

## License

Licensed under either [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE), at your option. Unless explicitly stated
otherwise, contributions intentionally submitted for inclusion in vpsman are
licensed under the same terms.
