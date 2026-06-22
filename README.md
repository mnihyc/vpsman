# vpsman

> This project is highly personalized and managed by AI agents.

`vpsman` is a Rust-based VPS panel with extended special functions: lightweight
headless agents, a raw TCP gateway, an HTTP control plane, a CLI/VTY operator
tool, and a Vite-built web panel.

The public repository intentionally keeps source, migrations, deployment
templates, GitHub Actions build definitions, and operator tutorials. Local
planning notes, private smoke harnesses, runtime state, generated secrets, and
build artifacts are ignored and are not part of the public tree.

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

Operator access scopes are documented in
[operator access scopes](docs/operator-access-scopes.md). `fleet:read` is for
metadata/status views; payloads, terminal replay, integrations, templates,
schedules, rendered config, and full network plans require narrower read scopes.

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
The release workflow stamps it with the exact tag, commit, generated asset
list, and tag-pinned per-asset download URLs before uploading it to GitHub
Releases as `version.json`.

The GitHub release tag is the canonical shipped version. Release builds embed
that tag-derived version into the server, agent, and CLI binaries so the agent
updater compares against the same version published in `version.json`.

The frontend artifact is a static Vite build intended for Nginx, Apache2, or an
equivalent static server.

## Docker Compose Runtime

The compose template runs already-built release assets. It does not rebuild
Rust or frontend code.

Place release files into this deployment-directory layout. The repository
template names that directory `deploy/`, but the directory itself can be
renamed or copied outside a source checkout:

- server ZIP contents: `runtime/server/current/`
- extracted frontend `dist/`: `runtime/frontend/current/dist/`
- host CLI: `runtime/cli/current/vpsctl`
- suite config: `config/vpsman.toml`
- secret files referenced by suite config: generated under
  `config/secrets/`

For a first Docker Compose start from GitHub Releases, let the deploy updater
download the release, verify checksums, generate missing compose secrets, and
start the stack:

```sh
cd deploy
cp .env.example .env
# edit .env before real deployment; use a URL-safe random hex
# POSTGRES_PASSWORD because compose derives the API/worker Postgres URL from it
export VPSMAN_SUPER_PASSWORD='<local_super_password>'
./update.sh first-start latest
```

For manual asset placement or custom bootstrap, run
`cargo run -p vpsctl -- compose-secrets --secrets-dir deploy/config/secrets`
from a source checkout if a release `vpsctl` binary is not installed yet. The
command writes the three mounted compose secret files, a gateway public-key
file for agent installs, and `operator-privilege.env` containing the generated
`VPSMAN_SUPER_SALT_HEX`. Keep the super password in your operator password
manager; the API never receives it.

Persistent runtime data stays under the deployment directory:

- PostgreSQL: `runtime/postgres/data`
- local object storage: `runtime/data/objects/backups` for retained
  backup artifacts, large job outputs, file-transfer handoffs, and uploaded
  source artifacts

See `deploy/README.md` for the full compose directory layout.

In Docker, keep the `.env` object-store paths under `/var/lib/vpsman`
unchanged; compose maps them to `runtime/data`.
Compose also sets `VPSMAN_SUITE_CONFIG=/etc/vpsman/vpsman.toml`,
derives `VPSMAN_POSTGRES_URL` for API and worker from `.env`, and mounts
`config` at `/etc/vpsman`. `config/vpsman.toml` remains the single
authoritative compose suite config for non-secret runtime settings; the
database password stays in `.env`. The API receives that config directory as
writable so dashboard saves can atomically replace the TOML, while gateway,
worker, and secret mounts stay read-only. Compose mounts secret files per
service under `/run/secrets`; API and worker containers do not receive
gateway-only private-key or privilege-verifier material. Direct binary runs are
independent of the compose layout; set `VPSMAN_SUITE_CONFIG` and
`VPSMAN_POSTGRES_URL` yourself when you want a specific operator config file.

Long-running job control uses `max_timeout_secs` as the agent execution budget.
The fleet-wide accepted maximum is `timeout.max_job_timeout_secs` in
`config/vpsman.toml`, defaulting to 3600 seconds and configurable up to
seven days. Requests above the configured maximum are rejected so the browser,
CLI, API, worker, and agent agree on the exact budget. The API adds
dispatch/event grace through `timeout.control_deadline_grace_secs`, and the
gateway keeps forwarder delivery RAM-first with overflow and graceful-shutdown
spool settings under `[gateway]`. Controlled shutdown defers pending forwarder
events to the spool; hard crashes before RAM-resident events are spooled remain
a residual loss boundary. Spool replay reposts saved command-output event
bodies through normal API ingest so duplicate, conflict, late-output, and
payload-hash checks use the same path as live delivery. Active operator cancellation interrupts
agent shell/script/PTY children and long-running backup, restore, network, and
terminal operations; canceled targets become terminal only after the agent sends
structured `command_canceled` output. Resumable file-transfer steps use the
same structured cancellation path: upload chunks cancel before a temp-file write
starts, upload chunk completion wins after that write has succeeded, commits
cancel before the final move starts, and download chunks cancel before emitting
stdout/status. Command-output retry retention defaults to 24 hours and can be
tuned with
`[gateway].command_output_event_ttl_secs` or
`VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS`; this remains a best-effort
gateway forwarder spool, not an end-to-end gateway-agent ACK protocol.

Tunnel endpoint allocation pools live under `[network]` and are empty by
default. Operators must set IPv4 and/or IPv6 pool CIDRs, or pass pools on the
allocation request, before endpoint generation can produce suggestions.

The compose template does not publish the API host port. Nginx reaches the API
over the private Docker network, and the dashboard binds to `127.0.0.1:5173` by
default through `VPSMAN_FRONTEND_BIND`. Gateway TCP also stays loopback-bound by
default, and gateway control uses a shared Unix socket under
`runtime/data`; expose agent TCP through your chosen public proxy,
firewall, or tunnel when needed. Gateway-to-API forwarding is intentionally
plain HTTP and should stay on localhost, a Unix-adjacent private compose
network, or another trusted private network; TLS termination belongs in the
operator-facing reverse proxy, not this internal forwarding link.
Because the API is a private service behind the dashboard proxy, operator
login throttling and auth history trust `X-Forwarded-For` by default, including
IPv6 client addresses forwarded by an external TLS provider. Deployments that
bind the API directly can restrict that trust with `[api].trusted_proxy_cidrs`
or `VPSMAN_TRUSTED_PROXY_CIDRS`.

Update an existing Docker deployment from GitHub Releases:

```sh
cd deploy
./update.sh latest
# or pin a release:
./update.sh v0.1.0
```

Rollback swaps back to the previous server/frontend/CLI release directories:

```sh
cd deploy
./update.sh rollback
```

The update script downloads release assets, verifies `SHA256SUMS`, updates
`runtime/server/current`, `runtime/frontend/current`, and
`runtime/cli/current/vpsctl`, then recreates containers. It does not
delete PostgreSQL or local object-storage data.

## Direct Gateway Agent Install

Remote VPS agents connect to the raw TCP gateway listener. They never contact
the browser panel, panel HTTP API, or a panel-side lookup endpoint during
installation. Gateway control must stay private; the default compose deployment
uses a local Unix socket for it. For public agents, expose or proxy only the
agent TCP gateway on `9443`, provision each agent with gateway Noise identity
material, and register the matching public key as a direct gateway identity.

Typical flow:

```sh
vpsctl noise-keygen
export VPSMAN_SUPER_PASSWORD='<local_super_password>'
export VPSMAN_SUPER_SALT_HEX='<server_super_salt_hex>'
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
panel-side endpoint lookup is used. The installer enables and starts the agent
service by default; set `VPSMAN_AGENT_ENABLE_SERVICE=0` only when deliberately
staging files without starting the service. Gateway Noise sessions protect
agent traffic from tampering, so there is no extra server-side
command-authentication key.
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
