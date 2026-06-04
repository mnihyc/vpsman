# vpsman

Disclaimer: this is a highly personalized project and managed by AI agents.

`vpsman` is a Rust-based VPS management platform with lightweight headless
agents and a cloud-console-style server panel.

The repository is intentionally structured as a split control plane:

- `crates/agent`: low-overhead Linux client agent.
- `crates/gateway`: raw TCP gateway for long-lived agent sessions.
- `crates/api`: HTTP/WebSocket control-plane API.
- `crates/worker`: background scheduler and rollout worker.
- `crates/vpsctl`: scriptable CLI and interactive VTY shell.
- `crates/common`: shared protocol, auth, config, and telemetry types.
- `frontend`: React + TypeScript panel source.
- `deploy`: local Docker Compose templates.

See `DESIGN.md` for architecture and protocol decisions. See
`tutorials/README.md` for operator-facing setup and usage guides.

Release builds keep runtime roles separate and publish `version.json` metadata
for upgrade automation:

- `vpsman-api-linux-x86_64`, `vpsman-gateway-linux-x86_64`, and
  `vpsman-worker-linux-x86_64`: server/control-plane binaries.
- `vpsman-frontend-dist.tar.gz`: Vite static panel build for Nginx, Apache2,
  or another static web server.
- `vpsman-agent-*-musl`: static client agent binaries.
- `vpsctl-*`: operator CLI binaries.
- `version.json`: release tag, commit, and asset list.

For Docker Compose deployment, place released assets into the checkout-local
runtime layout, then start the persistent compose stack:

- server binaries: `deploy/runtime/server/current/bin/`
- migration SQL files: `deploy/runtime/server/current/migrations/`
- extracted Vite frontend `dist/`: `deploy/runtime/frontend/current/dist/`

The compose stack does not rebuild Rust or frontend assets. It mounts
`deploy/runtime/server/current` and `deploy/runtime/frontend/current`, keeps
PostgreSQL data under `deploy/runtime/postgres/data`, keeps local object
storage under `deploy/runtime/data`, and serves the Vite static build through
Nginx. It uses checkout-local bind mounts rather than Docker-managed named
volumes so the deployment directory stays portable.

## Smoke Verification

Run the aggregate release gate when handing off a broad change:

```sh
bash scripts/release-check.sh
```

Latest verified local-baseline release gate:
`release_check=ok log_dir=target/release-check/20260604-052636`. This covers
the Rust workspace, static musl clients, frontend build/layout smokes,
PostgreSQL success paths, local filesystem object storage, live agent/gateway
workflows, final E2E evidence, tutorial/migration/customizability audits, and
security/repository hygiene. S3/MinIO smokes are intentionally opt-in because
local disk object storage is the documented release baseline.

For a focused repository hygiene pass, run:

```sh
bash scripts/scan-repo-hygiene.sh
```

For a focused operator tutorial coverage pass, run:

```sh
bash scripts/audit-tutorials.sh
```

For a focused frontend cloud-console layout pass, run:

```sh
bash scripts/smoke-frontend-console-layout.sh
```

For a live API-backed frontend pass, run:

```sh
bash scripts/smoke-frontend-live-api.sh
```

For a focused `vpsctl` command-registry parser pass, run:

```sh
bash scripts/smoke-vpsctl-cli-help.sh
```

This smoke runs the built `vpsctl` binary and verifies root help plus every
supported subcommand help, including topology, backup/restore, process,
schedule, file, and proof-related commands. It catches command shape
regressions without direct in-process Clap parser tests.

For a focused `vpsctl` compiled-binary semantic pass, run:

```sh
bash scripts/smoke-vpsctl-cli-semantics.sh
```

This smoke executes local tunnel planning for GRE/IPIP/SIT/FOU, local proof
generation, Noise/signing key generation shape checks, and negative validation
for bad proof hashes and bounded network probe/speed-test arguments.

For a focused `vpsctl` structured output pass, run:

```sh
bash scripts/smoke-vpsctl-structured-output.sh
```

`vpsctl --output raw|json|pretty-json` is global. `raw` preserves historical
stdout. `json` and `pretty-json` normalize command stdout into compact or
pretty JSON for headless automation, including JSONL streams and plain-text
fallback wrappers; the interactive `vty` shell intentionally rejects this
mode. Commands that write files use `--output-file` so file destinations do not
conflict with global output formatting.

For focused terminal retention, replay, idle-timeout, and flow-window evidence,
run:

```sh
bash scripts/smoke-terminal-retention.sh
```

This gate runs the agent terminal retention boundary test, API terminal
read-model/replay tests, CLI/VTY terminal contract tests, and checks that the
live PostgreSQL gateway smoke still asserts terminal retention metadata.

For a live API-backed `vpsctl` workflow pass, run:

```sh
bash scripts/smoke-vpsctl-live-api.sh
```

This smoke starts the API, then uses the compiled `vpsctl` binary for operator
auth, enrollment-token listing, fleet summary, computed fleet alerts,
pool/tag/bulk targeting, tunnel-plan persistence, schedules, proof-gated
backup request, proof-gated restore plan, and audit visibility.

For a focused PostgreSQL persistence pass, run:

```sh
bash scripts/smoke-postgres-persistence.sh
```

This smoke restarts the API against a real Postgres container and verifies
operator auth/session state, agents, pools/tags, bulk targeting, tunnel plans,
schedules, fail-closed rejected job history and frozen job targets, backup
request metadata, restore plan metadata, and audit records.

For a PostgreSQL-backed live gateway/agent job-output pass, run:

```sh
bash scripts/smoke-postgres-live-job-output.sh
```

This smoke restarts the API against the same Postgres database after
proof-gated shell command execution through `vpsctl job-create`, file pull, file
push, network status inspection, tunnel latency probe, and two-endpoint tunnel
speed test through a real gateway and enrolled agents. It verifies job history,
target status, retained output, object-store-backed large output artifact
retention and `vpsctl job-output-artifact` retrieval, typed network
observations, audit records, and selected output leakage checks remain durable,
including OSPF recommendations and reviewed OSPF update plans across API
restart.
Large retained output chunks can be served as object-store artifacts, and the
transfer-session inventory at `/api/v1/file-transfers` derives durable
upload/download progress from retained job status output.
Completed resumable download sessions can now be materialized into the local
filesystem-backed object store through a server-side handoff, then downloaded
from `/api/v1/file-transfers/{client_id}/{session_id}/handoff/artifact` or
`vpsctl file-transfer-handoff`. Operators can also retain local source files as
verified source artifacts through `/api/v1/file-transfer-sources`,
`vpsctl file-transfer-source-upload`, VTY, or the Jobs panel. Source artifacts
are bounded, SHA-256 verified by the API, stored under content-addressed local
object keys, and kept ready for later transfer reuse. Directly driving new
agent uploads from retained source artifacts is supported by the resumable
upload composer, CLI, and VTY with `--source-artifact-id`.

For a focused agent install/autostart asset pass, run:

```sh
bash scripts/smoke-agent-install-assets.sh
```

This smoke stages the installer into a temporary root without `sudo`, verifies
the static binary placement, enrolled config permissions, systemd unit,
Debian-style init fallback, idempotent reinstall behavior, config replacement
backup, target-side enrollment through a transient `vpsctl` helper, and
explicit opt-in for development `dev_xx` config.

For a staged Debian/Ubuntu installer matrix pass, run:

```sh
bash scripts/smoke-agent-install-distro-matrix.sh
```

This smoke verifies the installer and static release agent inside Ubuntu
18.04/20.04/22.04/24.04, Debian stable, and one older Debian image. It stages
the install without touching the container root, checks systemd and SysV init
assets, verifies `0600` config permissions, and runs `vpsman-agent once` from
the installed static binary.

For a focused low-resource static agent pass, run:

```sh
bash scripts/smoke-agent-resource-budget.sh
bash scripts/smoke-agent-reconnect-churn.sh
bash scripts/audit-agent-static-deps.sh
```

This smoke builds the release `x86_64-unknown-linux-musl` agent, verifies it is
static, runs it in a 128MB/1-vCPU Docker container against a local Noise
gateway, and enforces idle RSS, CPU, thread-count, and binary-size budgets. The
reconnect churn smoke runs the same static agent through a local latency/drop
proxy, forces an initial gateway session failure, and then requires a successful
reconnect. The dependency audit checks the musl-target agent dependency graph
and fails on dynamic TLS/system-library crates or unexpected `*-sys` packages.

For a focused Debian/Ubuntu ifupdown2 + Bird2 preset pass, run:

```sh
bash scripts/smoke-network-preset-container.sh
```

This smoke runs inside a Docker container with `NET_ADMIN`, validates the
curated preset commands against real ifupdown2/Bird2 packages, applies a GRE
tunnel, reloads Bird2, runs rollback-time `ifdown -f`, removes the managed
blocks, and verifies the live tunnel is gone.

For a focused live API/gateway/agent network apply pass, run:

```sh
bash scripts/smoke-live-network-apply.sh
```

This smoke uses a PostgreSQL-backed API and enrolled BGP-tagged agent with
`[network] apply_enabled = true` under a temporary sandbox root. It proves
no-proof rejection, compiled `vpsctl tunnel-apply` and `tunnel-rollback`
through the real gateway, managed-block write/removal, backup/status evidence,
output redaction, audit visibility, and API restart persistence. It also
promotes a saved `external_observed` OpenVPN-style plan into an
`external_managed_adapter` contract, then proves proof-gated adapter
apply/rollback, startup/status/traffic-limit/stop/cleanup command evidence,
Bird2-only managed-block write/removal, `try_external_adapters` unprivileged
best-effort policy, audit, redaction, and restart persistence.

For a focused Bird2 topology convergence pass, run:

```sh
bash scripts/smoke-bird2-topology-convergence.sh
```

This smoke runs two Linux network namespaces in Docker, builds a GRE tunnel
between them, starts one Bird2 OSPFv3 router on each side with vpsman-managed
interface snippets, and waits for `Full/PtP` neighbor state.

The discovery failover smoke starts a local API, gateway, and enrolled agent,
enrolls a discovery rotation public-key ring, puts a dead TCP endpoint first in
the agent config, restarts the API, then verifies signed localhost discovery
reconnects to the live gateway without weakening pinned Noise identity:

```sh
bash scripts/smoke-signed-discovery-failover.sh
```

Production discovery should use HTTPS and a signed document. The API can pass
additional discovery-only rotation public keys to enrolled agents with
`VPSMAN_DISCOVERY_TRUSTED_SERVER_PUBLIC_KEYS_HEX`; these keys do not change the
pinned command-signing key used for privileged command envelopes.

## Agent Installation

`scripts/install-agent.sh` installs the headless agent in one of two explicit
modes. `VPSMAN_INSTALL_MODE=root` is the production default: it installs under
`/opt/vpsman`, writes `/etc/vpsman/agent.toml` with `0600` permissions, and
renders both `/etc/systemd/system/vpsman-agent.service` and
`/etc/init.d/vpsman-agent`. Host root-mode installs require root.

`VPSMAN_INSTALL_MODE=unprivileged` installs for a normal user by default under
`$HOME/.local/lib/vpsman`, `$HOME/.config/vpsman`, and
`$HOME/.local/state/vpsman`, then renders a user systemd unit under
`$HOME/.config/systemd/user`. It intentionally does not write root autostart
assets. Operators can set `VPSMAN_SERVICE_HOME` to install/stage for another
user home. Test/staged installs can set `VPSMAN_INSTALL_ROOT` and never call
the service manager.

Production installs should pass a pre-rendered enrolled config from
`vpsctl enroll-config`; the super password remains local to the operator or
install environment, and only derived proof material is stored in the agent
config.

```sh
config_b64="$(base64 < ./agent.toml | tr -d '\n')"
env VPSMAN_AGENT_URL=https://updates.example/vpsman-agent \
  VPSMAN_AGENT_CONFIG_B64="$config_b64" \
  bash scripts/install-agent.sh
```

Unprivileged staged install example:

```sh
env VPSMAN_INSTALL_MODE=unprivileged \
  VPSMAN_SERVICE_HOME=/home/vpsman \
  VPSMAN_AGENT_URL=https://updates.example/vpsman-agent \
  VPSMAN_AGENT_SHA256_HEX=<64_hex_sha256> \
  VPSMAN_AGENT_CONFIG_B64="$config_b64" \
  bash scripts/install-agent.sh
```

For a one-line target-side enrollment flow, provide a short-lived enrollment
token and a static `vpsctl` helper. The installer calls `vpsctl enroll-config`
locally, carrying the token in the environment instead of a command-line
argument, writes the enrolled config, then removes the helper.

For a rebuilt VPS that should keep its existing server-side identity, create a
confirmed bound rebuild token instead of a normal provisioning token:

```sh
cargo run -p vpsctl -- reenrollment-token-create --client-id edge-01 --ttl-secs 1800 --default-tags rebuilt --confirmed
```

The server stores the expected old public-key fingerprint at token creation,
preserves pool/tag/history state by `client_id`, and rejects ordinary
provisioning tokens that would silently replace an existing client key.

Current client keys can also be revoked explicitly when a VPS is rebuilt or a
key is suspected compromised:

```sh
cargo run -p vpsctl -- client-key-revoke --client-id edge-01 --reason rebuilt --confirmed
cargo run -p vpsctl -- key-lifecycle-report
```

Revocation rejects that enrolled key at gateway identity validation. A bound
rebuild token can rotate the same `client_id` to a new key without discarding
pool, tag, history, backup, restore, or migration state.

```sh
env VPSMAN_AGENT_URL=https://updates.example/vpsman-agent \
  VPSMAN_AGENT_SHA256_HEX=<64_hex_sha256> \
  VPSMAN_VPSCTL_URL=https://updates.example/vpsctl \
  VPSMAN_VPSCTL_SHA256_HEX=<64_hex_sha256> \
  VPSMAN_API_URL=https://panel.example.com \
  VPSMAN_ENROLLMENT_TOKEN=... \
  VPSMAN_CLIENT_ID=edge-01 \
  VPSMAN_SUPER_PASSWORD=... \
  VPSMAN_SUPER_SALT_HEX=... \
  bash scripts/install-agent.sh
```

The installer refuses to overwrite an existing different config unless
`VPSMAN_FORCE_CONFIG=1` is set, in which case it first writes a timestamped
backup. The old `client_id`/`gateway` development config path is still
available only with `VPSMAN_ALLOW_DEV_CONFIG=1`. URL-downloaded agent and
`vpsctl` helper binaries require `VPSMAN_AGENT_SHA256_HEX` and
`VPSMAN_VPSCTL_SHA256_HEX`; copied local binaries may also provide those hashes
to fail closed on mismatch. Managed install and config directories must be
absolute paths and cannot be `/`.

Uninstall is explicit. `VPSMAN_UNINSTALL=1` stops/disables the service when a
real service manager is available, removes the agent binary and autostart
assets for the selected install mode, and preserves config, state, and logs by
default. Add
`VPSMAN_PURGE_CONFIG=1` to remove the vpsman-owned config, state, and log
paths.

Post-release installer hardening remains possible around signed release
metadata beyond SHA-256 hash pinning, trusted-certificate HTTPS enrollment
coverage, and real VM reboot/uninstall drills. The current release gate covers
the staged Debian/Ubuntu install matrix, root/unprivileged assets, hash checks,
config rendering, explicit uninstall/purge behavior, static-agent resource
budgets, and reconnect behavior.

## Current Panel Privileged Dispatch

The Jobs panel contains the browser-side command composer for the current
proof-gated job slice. It resolves explicit VPS, pool, and tag targets through
`/api/v1/bulk/resolve`, derives one proof envelope per resolved client with
WebCrypto, and posts the signed job request shape expected by `/api/v1/jobs`.
The panel currently supports bounded shell argv jobs, bounded file pulls,
bounded inline/chunked file pushes, hot-config updates, staged agent-update
requests, signed update release metadata records, proof-gated user-session
visibility, process snapshots, and vpsman-managed process supervisor actions.
It also includes limited browser resumable upload and download composers that
run the start/chunk/commit or download-start/chunk protocols with ACK/progress,
hash verification, and session/resume-token restart fields. Browser upload
defaults to strict `same-offset` ACK fanout and exposes
`independent-offsets` for resumed multi-target batches. It hashes and reads
uploads from bounded browser file slices instead of buffering the whole source
file. Browser download exposes a default verified Blob-download sink plus a
selectable `stream-to-file` sink that writes verified chunks through the File
System Access API when available. The Jobs panel
shows a first-class File transfer sessions inventory backed by
`/api/v1/file-transfers`. Completed download sessions expose a server-side
handoff action that assembles retained inline or object-store-backed chunks,
verifies chunk and full-file SHA-256, commits a deterministic local object-store
artifact idempotently, and downloads it through the panel, CLI, or VTY. The
Jobs panel can also select multiple completed download handoffs and save each
verified artifact with a deterministic client/session-prefixed filename. Local
filesystem object-store artifacts stream from verified server files when
possible, and handoff downloads can use a verified browser `stream-to-file`
sink. The same panel includes Source artifacts for confirmed local source
uploads into the server object store, with browser-side SHA-256 computation and
API-side size/hash verification before durable metadata is recorded. The
current local-backend transfer path is covered by the aggregate release gate,
including live API/gateway/agent smokes and final E2E evidence. The optional
S3/MinIO adapter now has bounded parser hardening, fake-S3 regression tests,
and opt-in live MinIO backup/update smoke coverage. The
PostgreSQL live smoke now proves CLI resumable upload/download,
upload/download policy metadata, verified file bytes, durable transfer-session
inventory, and API restart persistence through
the real gateway path. CLI/VTY upload defaults to `same-offset` fanout and can
use `--multi-target-policy independent-offsets` when resumed targets have
different verified offsets. CLI/VTY download defaults to `single-target` and
can use `--multi-target-policy per-target-files` to pull the same remote path
from each resolved VPS into deterministic client-named files under the
destination directory.

The super password is not sent to the server. The browser can keep proof
material in memory for the session or store it in `localStorage` only as an
AES-GCM encrypted vault protected by an operator passphrase.

## Current VTY Privileged Mode

Scriptable `vpsctl job-create` can generate proof envelopes automatically when
no `--envelope-file` or `--envelopes-file` is supplied:

```sh
export VPSMAN_API_TOKEN=...
export VPSMAN_SUPER_PASSWORD=...
export VPSMAN_SUPER_SALT_HEX=...
cargo run -p vpsctl -- job-create --command uptime --tags edge
cargo run -p vpsctl -- job-create --command /bin/sh --argv '/bin/sh,-lc,tty' --pty --tags edge
```

The CLI resolves the selected clients through the API, derives the proof
locally, and sends only per-client proof envelopes. For bounded proof-gated file
pulls, use:

```sh
cargo run -p vpsctl -- file-pull --path /etc/hostname --tags edge
```

For bounded proof-gated file pushes, use an explicit local source, remote
absolute path, and confirmation. Files up to the inline cap are sent as one
payload; larger bounded files are split into per-chunk SHA-256 verified
payloads and still written atomically by the agent. This is not yet the full
resumable object-storage transfer protocol.

```sh
cargo run -p vpsctl -- file-push --source ./payload.txt --path /tmp/payload.txt --tags edge --confirmed
cargo run -p vpsctl -- file-transfers --limit 20
cargo run -p vpsctl -- file-transfer-handoff --client-id agent-fra-02 --session-id 51515151-2222-4333-8444-555555555555 --output-file ./bird.log --confirmed
cargo run -p vpsctl -- file-transfer-source-upload --source ./payload.txt --confirmed
cargo run -p vpsctl -- file-transfer-sources --limit 20
cargo run -p vpsctl -- file-transfer-source-download --artifact-id <source_artifact_uuid> --output-file ./payload.txt
cargo run -p vpsctl -- file-transfer-upload --source-artifact-id <source_artifact_uuid> --path /tmp/payload.txt --clients edge-01 --confirmed
```

The reusable live smoke below starts a local API, gateway, and enrolled agent,
then verifies no-proof rejection plus successful proof-gated inline and chunked
file pushes through the real gateway path:

```sh
bash scripts/smoke-live-file-push.sh
```

For proof-gated user-session visibility, use:

```sh
cargo run -p vpsctl -- user-sessions --tags edge
```

For proof-gated process visibility, use:

```sh
cargo run -p vpsctl -- process-list --limit 50 --tags edge
```

For vpsman-managed process supervision, use explicit argv commands. The agent
tracks only processes it starts and stores pid/log metadata locally:

```sh
cargo run -p vpsctl -- process-start --name edge-worker --argv /usr/bin/sleep --argv 60 --tags edge
cargo run -p vpsctl -- process-status --name edge-worker --tags edge
cargo run -p vpsctl -- process-logs --name edge-worker --tags edge
cargo run -p vpsctl -- process-restart --name edge-worker --tags edge
cargo run -p vpsctl -- process-stop --name edge-worker --tags edge
```

`process-start` accepts typed restart policy and resource-limit flags. When a
target reports that process-limit enforcement is unavailable, limit-bearing
starts are recorded as `degraded_unprivileged` by default; use
`--force-unprivileged` only for explicit best-effort attempts. The agent applies
memory, PID, open-file, and `no_new_privileges` limits in the child process.
CPU shares are enforced with cgroup v2 `cpu.weight` when
`VPSMAN_PROCESS_CGROUP_ROOT` or the default cgroup root exposes the CPU
controller; unsupported hosts continue running the process but report
`desired_only` limit evidence with the reason. Supervisor status/inventory also
includes restart attempts, last exit/restart evidence, and cgroup path when the
process was attached to a managed cgroup.

Active dispatching jobs and pending scheduled-approval jobs can be canceled
through the same job lifecycle:

```sh
cargo run -p vpsctl -- job-cancel --job-id <job_uuid> --reason "operator requested" --confirmed
```

Large retained stdout/stderr chunks are externalized to the configured object
store when local filesystem-backed `VPSMAN_BACKUP_OBJECT_STORE_DIR` is set and
the chunk is at least `VPSMAN_JOB_OUTPUT_ARTIFACT_MIN_BYTES` bytes
(`32768` by default). Status chunks remain inline so audits and observation
parsers keep working. The API exposes object key, SHA-256, and size metadata in
job outputs; the Jobs panel can download retained artifacts, and CLI/VTY can
retrieve the exact bytes. Gateway output frames are also ingested into the API
as they arrive, with WebSocket refresh events for retained output and job
completion. Non-PTY shell argv/script stdout and stderr stream as bounded
chunks before process exit. Noninteractive PTY-backed argv jobs are available
as proof-gated `shell_pty` operations and retain output on the `pty` stream;
`vpsctl job-follow` and VTY `job-follow` can poll retained output and decode
stdout/stderr/PTY/status chunks for headless operators. The terminal workflow
also supports proof-gated polled `terminal-open`, `terminal-input`,
`terminal-poll`, `terminal-resize`, and `terminal-close` operations plus a durable
`terminal-sessions` inventory derived from retained terminal status outputs.
Command execution policy presets can now select shell argv prefix, default
working directory, inherited/clean/minimal environment behavior, explicit env
values, PTY enabled/disabled state, and process-group or direct-child cleanup.
Terminal status records include retained-output, dropped-output, and replay
truncation counters so operators can tell when the bounded PTY window is no
longer sufficient for a requested replay cursor. Persisted terminal replay is
also available through the API, `vpsctl terminal-replay`, VTY
`terminal-replay`, and the Jobs panel Replay action; it reconstructs available
historical PTY chunks from retained job outputs and verifies
object-store-backed chunks before returning data. The Jobs panel can now turn a
retained open terminal-session row into a prepared attach/replay, durable
replay preview, poll, input, resize, or close job with the session id, target
VPS, replay cursor, window defaults, and next input sequence prefilled. The
PostgreSQL live smoke covers that polled lifecycle, attach replay, explicit
output polling, retention metadata, and API restart through the gateway when
run. Continuous push streaming and transport-level flow-control/backpressure
remain separate workflow gaps.

```sh
cargo run -p vpsctl -- job-outputs --job-id <job_uuid>
cargo run -p vpsctl -- job-follow --job-id <job_uuid> --interval-ms 1000 --max-polls 120
cargo run -p vpsctl -- job-output-artifact --job-id <job_uuid> --client-id edge-a --seq 0 --output-file ./stdout.bin
cargo run -p vpsctl -- terminal-sessions --limit 20
cargo run -p vpsctl -- terminal-replay --client-id edge-a --session-id <session_uuid> --from-seq 1 --output-file ./terminal.log
cargo run -p vpsctl -- terminal-poll --session-id <session_uuid> --replay-from-seq 1 --clients edge-a --confirmed
```

Hot-config updates are proof-gated and require explicit confirmation. This
slice updates validated agent TOML in place with an agent-side rollback copy.

```sh
cargo run -p vpsctl -- hot-config --config-file ./docs/agent-config.example.toml --tags edge --confirmed
```

The live hot-config smoke starts a PostgreSQL-backed API, authenticates an
operator, starts a gateway and enrolled agent, then verifies no-proof rejection,
proof-gated config apply with rollback, and persisted job/audit state after API
restart:

```sh
bash scripts/smoke-live-hot-config.sh
```

Agent update staging is also proof-gated and requires explicit confirmation.
The current slice accepts an HTTPS artifact URL plus SHA-256 and optional
detached Ed25519 artifact signature metadata. The agent hash-verifies the
download, verifies signatures when a trusted update signing key is pinned in
agent config, stages the binary side-by-side, and creates a rollback copy.
Operators can then run separate proof-gated activation and rollback commands:
activation re-hashes the staged binary before replacing the active agent path,
and rollback optionally verifies the rollback binary hash before restoring it.
Activation writes a non-secret restart marker. After the operator or service
manager restarts the activated agent, the next authenticated hello reports that
marker. The API moves the matching rollout target through
`activation_pending_restart` to `heartbeat_verified` and audits both the
activation-pending and heartbeat evidence. Rollback removes the marker, marks
the matching rollout target `rolled_back`, and audits
`agent_update.rollback_completed`.
Activation normally reports pending manual restart state, but
`--restart-agent` can request a delayed agent exit after the activation status
is sent so a service manager can restart the replaced binary.
The API also runs a configurable rollout reconciler
(`VPSMAN_AGENT_UPDATE_HEARTBEAT_TIMEOUT_SECS`, default 900 seconds;
`VPSMAN_AGENT_UPDATE_RECONCILE_INTERVAL_SECS`, default 30 seconds) that marks
stale activation-pending targets as `heartbeat_timeout` and writes sanitized
`agent_update.heartbeat_timeout` audit metadata.
Accepted update jobs also create rollout records with frozen target progress,
sanitized artifact hash/signing-key-hash metadata, stored canary count, manual
staging activation policy, and API/CLI/VTY/panel visibility across API restart.
Rollout records do not store or display artifact URLs, detached signatures,
public signing keys, proof material, or trust anchors. CLI, VTY, and panel
helpers can activate the next staged rollout batch and roll back
activation-pending, activation-failed, heartbeat-timeout, or
heartbeat-verified targets.
The assisted rollout worker records durable recommendations, heartbeat timeout
classification, worker lease owner/expiry evidence, and confirmed metadata-only
pause/resume plus health-gate controls. Use `heartbeat_verified` for normal
assisted promotion, `manual_after_canary` when the first verified canary should
stop for human inspection, and `manual_only` when the worker should never
recommend promotion. Autonomous privileged dispatch and automatic rollback for
a failed new version are still limited to proof-delegated escrow slices:
an operator can preauthorize exact activation commands for rollout targets, and
the API reconciler will dispatch only when the worker recommends
`operator_activate_batch` for a completed target with the same staged artifact
hash. Operators can also preauthorize exact rollback commands for rollout
targets, and the API reconciler will dispatch rollback only if the matching
target later enters `heartbeat_timeout` or `activation_failed`. Rollout records
expose activation and rollback proof summaries with ready, dispatching,
dispatched, expired, and failed counts plus proof expiry windows; the panel
turns expired or failed summaries into renewal actions that record fresh exact
envelopes. Broader release-policy automation and full multi-agent rollout E2E
remain future work.
A metadata-only signed release registry is available through
`/api/v1/agent-update-releases`, `vpsctl agent-update-release-publish`,
`vpsctl agent-update-release-latest`, `vpsctl agent-update-releases`, VTY
`agent-update-release-record`, VTY `agent-update-release-latest`, VTY
`agent-update-releases`, and the Jobs panel. It validates detached Ed25519
signatures before accepting records and stores only hashes of the artifact URL,
signature, signing key, and optional rollback-bundle URL/signature/signing key.
Set `VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES=1` on the API to reject staged
`agent_update` commands whose artifact hash/signing-key hash does not match a
registered release. Metadata-only records still do not host artifact bytes, and
hosted artifact records still do not perform autonomous rollout promotion.
Generate local detached signature metadata with:

```sh
cargo run -p vpsctl -- agent-update-signature --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent --signing-seed-hex <64_hex_seed>
```

To host bounded update artifacts from the API, configure the local
filesystem-backed `VPSMAN_UPDATE_OBJECT_STORE_DIR`. S3/MinIO remains available
as an optional adapter path through
`VPSMAN_UPDATE_OBJECT_ENDPOINT`, `VPSMAN_UPDATE_OBJECT_BUCKET`,
`VPSMAN_UPDATE_OBJECT_ACCESS_KEY`, `VPSMAN_UPDATE_OBJECT_SECRET_KEY`,
`VPSMAN_UPDATE_OBJECT_REGION`, and `VPSMAN_UPDATE_OBJECT_CREATE_BUCKET`.
Upload verifies the detached signature against the server-computed artifact
SHA-256, stores bytes under a content-addressed key, records only sanitized
metadata, and returns `artifact_download_path`. Optional rollback bundle bytes
can be uploaded in the same release record and are served from the same
content-addressed download route. For production-sized artifacts, prefer
`agent-update-artifact-upload --stream`: `vpsctl` streams the primary artifact
and optional rollback artifact as raw `application/octet-stream` bodies, the
API hashes while writing a temp file, verifies detached signatures before
object-store commit, then records the release from the hosted artifact hashes.
The old JSON/base64 upload path remains available for compatibility and small
local workflows. If `VPSMAN_UPDATE_ARTIFACT_PUBLIC_BASE_URL` is configured
with an HTTPS base URL, release views also include computed
`artifact_download_url` and `rollback_artifact_download_url` values for
operator handoff; those URLs are derived at read time and are not stored in
audit metadata. Production deployments can restrict release metadata with
`VPSMAN_AGENT_UPDATE_ALLOWED_CHANNELS=stable,canary` and
`VPSMAN_AGENT_UPDATE_TRUSTED_SIGNING_KEYS_HEX=<64_hex_public_key>[,<key>]`;
empty lists keep the compatibility default of accepting any signed channel/key
that validates cryptographically. The S3 path uses the same
bounded path-style SigV4 adapter as backup artifacts, including bounded
response parsing, chunked GET decoding, duplicate `HEAD` handling, and opt-in
live MinIO smoke coverage for adapter changes. Expose the download path behind
HTTPS before giving it to agents, because agent update dispatch still enforces
`https://` artifact URLs.

Publish sanitized release metadata or upload a bounded hosted artifact, then
dispatch the staged update:

```sh
cargo run -p vpsctl -- agent-update-release-publish --name vpsman-agent --version 0.1.0 --channel stable --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent --artifact-url https://updates.example/vpsman-agent --signing-seed-hex <64_hex_seed> --confirmed
cargo run -p vpsctl -- agent-update-release-publish --name vpsman-agent --version 0.1.1 --channel stable --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent --artifact-url https://updates.example/vpsman-agent --signing-seed-hex <64_hex_seed> --rollback-artifact-file ./target/previous/vpsman-agent --rollback-artifact-url https://updates.example/vpsman-agent.previous --confirmed
cargo run -p vpsctl -- agent-update-artifact-upload --name vpsman-agent --version 0.1.0 --channel stable --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent --signing-seed-hex <64_hex_seed> --confirmed
cargo run -p vpsctl -- agent-update-artifact-upload --name vpsman-agent --version 0.1.1 --channel stable --artifact-file ./target/x86_64-unknown-linux-musl/release/vpsman-agent --signing-seed-hex <64_hex_seed> --rollback-artifact-file ./target/previous/vpsman-agent --stream --confirmed
cargo run -p vpsctl -- agent-update-release-latest --name vpsman-agent --channel stable
cargo run -p vpsctl -- agent-update-releases --limit 10
cargo run -p vpsctl -- agent-update-rollout-policy-create --name hetzner-stable --scope-kind provider --scope-value hetzner --channel stable --canary-count 2 --health-gate manual_after_canary --priority 10 --confirmed
cargo run -p vpsctl -- agent-update-rollout-policies --limit 10
cargo run -p vpsctl -- agent-update --artifact-url https://updates.example/vpsman-agent --sha256-hex <64_hex_sha256> --artifact-signature-hex <128_hex_signature> --artifact-signing-key-hex <64_hex_public_key> --tags edge --canary-count 2 --confirmed
cargo run -p vpsctl -- agent-update-rollout-activate --rollout-id <rollout_uuid> --batch-size 2 --restart-agent --confirmed
cargo run -p vpsctl -- agent-update-rollout-control --rollout-id <rollout_uuid> --health-gate manual_after_canary --confirmed
cargo run -p vpsctl -- agent-update-rollout-control --rollout-id <rollout_uuid> --pause --pause-reason maintenance --confirmed
cargo run -p vpsctl -- agent-update-rollout-control --rollout-id <rollout_uuid> --resume --health-gate heartbeat_verified --confirmed
cargo run -p vpsctl -- agent-update-rollout-delegate-rollback --rollout-id <rollout_uuid> --proof-ttl-secs 3600 --confirmed
cargo run -p vpsctl -- agent-update-activate --staged-sha256-hex <64_hex_sha256> --tags edge --restart-agent --confirmed
cargo run -p vpsctl -- agent-update-rollback --rollback-sha256-hex <64_hex_rollback_sha256> --tags edge --confirmed
cargo run -p vpsctl -- agent-update-rollout-rollback --rollout-id <rollout_uuid> --confirmed
cargo run -p vpsctl -- agent-update-rollouts --limit 10
```

`agent-update-rollout-policy-create` is an upsert by policy name. Policies can
scope to `global`, `tag`, `pool`, or `provider`, optionally match a release
channel inferred from the registered artifact, and provide reusable
`canary_count` plus rollout health-gate defaults. Explicit `agent-update
--canary-count` still overrides the preset, and rollout records show the
applied policy id/name for audit.

`agent-update-rollout-delegate-rollback` locally derives per-client proof
envelopes for the exact `agent_update_rollback` payload and sends only those
envelopes to the API. By default it scopes to rollout automation targets, or to
all rollout targets when there is no current recommendation. Use
`--clients edge-a,edge-b` for an explicit subset and
`--rollback-sha256-hex <64_hex_rollback_sha256>` when the rollback binary hash
must be pinned. The API stores expiry/status/job linkage and sanitized audit
metadata, then signs/dispatches the frozen rollback command only after the
target becomes `heartbeat_timeout` or `activation_failed`.

Direct `agent-update`, `agent-update-activate`, and
`agent-update-rollback` dispatches default to `degraded_unprivileged` on
targets that report unprivileged mode or no privileged host-mutation
capability. Add `--force-unprivileged` only when an operator intentionally wants
a proof-gated best-effort attempt on those targets. Direct rollout
activation/rollback and delegated activation/rollback accept the same
`--force-unprivileged` flag; the API stores it as dispatch policy for the exact
proof envelope rather than as mutable command payload.

Agents use WebPKI roots for production HTTPS update downloads;
self-hosted/private-CA update infrastructure can add an explicit PEM root for
the agent process with
`VPSMAN_UPDATE_ROOT_CERT_PEM=/path/to/root-ca.pem`.

The live agent-update smoke starts a PostgreSQL-backed API, private-CA HTTPS
artifact endpoint, gateway, and enrolled agent, then verifies no-proof
rejection, pinned-key signature verification, proof-gated staging,
proof-gated activation, activated-agent restart heartbeat evidence,
proof-gated rollback marker cleanup, output redaction, API restart
persistence, forced activation failure classification, and delegated rollback
dispatch for an activation-failed normal-user agent:

```sh
bash scripts/smoke-live-agent-update.sh
```

Backup requests, encrypted artifact metadata, and restore plans are currently
control-plane records. Proof-gated backup jobs can also produce a bounded
encrypted artifact in job output after the agent is configured with
`backup.recipient_public_key_hex`. When the local filesystem-backed
`VPSMAN_BACKUP_OBJECT_STORE_DIR` is configured, the API can accept an
already-encrypted artifact file, validate the artifact envelope, write it under
a safe relative object key, compute SHA-256 server-side, reject overwrites, and
link artifact metadata to a backup request. Complete `VPSMAN_OBJECT_*` S3/MinIO
settings remain available as an optional adapter path. A completed backup
job also auto-links its encrypted stdout artifact when it matches an open backup
request by client id and payload hash. Operators can also promote retained
backup stdout later through `/api/v1/backups/{id}/artifact-handoff`,
`vpsctl backup-artifact-handoff`, VTY `backup-artifact-handoff`, or the
Backups panel; the handoff validates the encrypted artifact envelope, accepts an
optional source backup job id for repeated same-scope runs, writes a
deterministic object-store key idempotently, and links the artifact metadata.
Backup policies create approval-required scheduled backup jobs for
client/pool/tag selectors, and explicit policy pruning is available through
`/api/v1/backup-policies/prune`, `vpsctl backup-policy-prune`, VTY
`backup-policy-prune`, and the Backups panel Policy prune control with dry-run,
metadata-only, and confirmed object-store deletion modes. Operators can also
enable safe background metadata pruning in `vpsman-worker` with
`--backup-policy-prune-enabled`; this uses the policy `retention_days` and
`keep_last` values, takes a dedicated worker lease, records sanitized audit
metadata, and only deletes local filesystem object bytes when
`--backup-policy-prune-delete-objects` and
`--backup-policy-prune-object-store-dir` are explicitly set. S3 worker deletion
remains a reserved adapter path.
The current S3 path is a path-style HTTP
SigV4 adapter with fake-S3 parser regression tests and live MinIO byte
verification. The API can also serve the linked stored
encrypted artifact back to operators after re-checking object size, SHA-256, and
artifact envelope. `vpsctl restore-run`, VTY `restore-run`, and the Backups
panel Run restore action can decrypt a bounded artifact locally with
operator-held backup private key material and dispatch a proof-gated inline
restore job to an enrolled agent; when no local artifact file is supplied, they
download the linked stored encrypted artifact first. The agent writes selected
files and config under a destination root and creates rollback copies for
overwritten files. `vpsctl restore-rollback`, VTY `restore-rollback`, and the
Backups panel Rollback restore action can use retained successful restore status
output to build a proof-gated rollback command that restores snapshots for
overwritten files and removes files newly created by the restore. The panel
keeps the backup private key browser-local and does not send it to the API. A
direct restore or restore rollback defaults to `degraded_unprivileged` for
targets without privileged host-mutation capability; CLI and VTY expose
`--force-unprivileged` for explicit best-effort attempts.
metadata-only migration link can now tie an approved restore plan to the source
backup identity and rebuilt target client identity through the API, `vpsctl
migration-link`, VTY `migration-link`, and the Backups panel Link migration
form.
Server-mediated chunked backup artifact upload now exists for larger encrypted
artifacts; full agent-originated direct streaming with rate limits and reconnect
retry, HTTPS cloud-S3 hardening, large or resumable restore, and executable
rebuilt-VPS migration automation are still future work.

```sh
cargo run -p vpsctl -- backup-request --client-id edge-a --paths /etc/hostname --include-config --confirmed
cargo run -p vpsctl -- backup-run --paths /etc/hostname --include-config --clients edge-a --confirmed
cargo run -p vpsctl -- backup-policy-upsert --name nightly-edge --paths /etc/hostname --include-config --tags backup-critical --interval-secs 86400 --retention-days 30 --keep-last 7 --confirmed
cargo run -p vpsctl -- backup-policy-prune --dry-run
cargo run -p vpsctl -- backup-policy-prune --schedule-id <policy_schedule_uuid> --metadata-only false --confirmed
cargo run -p vpsctl -- backup-artifact-record --backup-request-id <backup_request_uuid> --object-key backups/edge-a/example.cbor.zst.age --sha256-hex <64_hex_sha256> --size-bytes 4096 --confirmed
cargo run -p vpsctl -- backup-artifact-upload --backup-request-id <backup_request_uuid> --object-key backups/edge-a/example.json --artifact-file ./artifact.json --confirmed
cargo run -p vpsctl -- backup-artifact-upload-chunked --backup-request-id <backup_request_uuid> --object-key backups/edge-a/example-large.json --artifact-file ./artifact.json --chunk-size-bytes 4194304 --confirmed
cargo run -p vpsctl -- backup-artifact-handoff --backup-request-id <backup_request_uuid> --job-id <backup_job_uuid> --confirmed
cargo run -p vpsctl -- backup-artifacts
cargo run -p vpsctl -- restore-plan --source-backup-request-id <backup_request_uuid> --target-client-id edge-b --paths /etc/hostname --destination-root /restore --confirmed
cargo run -p vpsctl -- restore-run --source-backup-request-id <backup_request_uuid> --target-client-id edge-b --artifact-file ./artifact.json --paths /etc/hostname --destination-root /restore --confirmed
cargo run -p vpsctl -- restore-run --source-backup-request-id <backup_request_uuid> --target-client-id edge-b --paths /etc/hostname --destination-root /restore --confirmed
cargo run -p vpsctl -- restore-rollback --restore-job-id <restore_job_uuid> --target-client-id edge-b --confirmed
cargo run -p vpsctl -- restore-plans
```

Schedules can be registered through the API, panel, CLI, or VTY. Due schedules
materialize as `approval_required` job records with frozen targets. The server
does not generate super-password proofs on its own; an operator must approve a
due scheduled job with fresh per-client proof envelopes before the API dispatches
it through `/api/v1/jobs/{job_id}/dispatch-scheduled`. Schedule definitions also
carry catch-up and retry policy (`skip_missed`, `run_once`, or
`run_all_limited`) plus failure evidence. CLI, VTY, and browser panel approval
controls are available for this approval step.

```sh
cargo run -p vpsctl -- schedule-create --name hourly-uptime --command /usr/bin/uptime --tags edge --interval-secs 3600 --catch-up-policy run_once
cargo run -p vpsctl -- schedules
cargo run -p vpsctl -- schedule-dispatch --job-id <approval_required_job_uuid> --confirmed
```

Tunnel planning is available as a non-mutating observe/plan workflow. Without
`--save`, `vpsctl tunnel-plan` renders a local plan. With `--save`, the plan is
stored through the API and appears in the Topology panel. This does not apply
network changes.

```sh
cargo run -p vpsctl -- tunnel-plan --name edge-a-b --interface-name tunab --kind gre --left-client-id edge-a --right-client-id edge-b --left-underlay 203.0.113.10 --right-underlay 203.0.113.20 --address-pool-cidr 10.255.0.0/30 --bandwidth 100m --latency-ms 20 --save
cargo run -p vpsctl -- tunnel-plans
```

Runtime-owned tunnel plans can also represent imported or adapter-managed
tunnels without generating host network-manager files. The same flags are
accepted by VTY `tunnel-plan`.

```sh
cargo run -p vpsctl -- tunnel-plan --name openvpn-a-b --interface-name ovpn42 --kind openvpn --runtime-manager external_managed_adapter --runtime-startup-argv /usr/local/libexec/vpsman-openvpn-adapter,start,{interface} --runtime-status-argv /usr/local/libexec/vpsman-openvpn-adapter,status,{interface} --runtime-traffic-limit-argv /usr/local/libexec/vpsman-openvpn-adapter,shape,{interface} --traffic-egress-kbps 100000 --traffic-burst-kb 4096 --topology-desired-interfaces ovpn42 --topology-route 10.42.0.0/24,dev=ovpn42,metric=42 --left-client-id edge-a --right-client-id edge-b --left-underlay 203.0.113.10 --right-underlay 203.0.113.20 --address-pool-cidr 10.255.0.0/30 --bandwidth 100m --latency-ms 20 --save
```

Telemetry-discovered runtime tunnels that are marked as import candidates can
be promoted into saved non-mutating `external_observed` plans after the
operator supplies the missing peer, underlay, and IPAM context. This is exposed
through the API, the Topology panel Promote observed tunnel form,
`vpsctl tunnel-promote-telemetry`, and VTY `tunnel-promote-telemetry`; it does
not start, stop, or delete the observed interface.

`telemetry-tunnels` also shows saved-plan correlation. If an observed interface
already matches a saved endpoint-side plan, the record is returned as
`matched_saved_plan` with the plan id/name, runtime manager, endpoint side, and
peer client id, and it is no longer listed as a promotion candidate. Mutating
runtime managers show `managed_desired`; saved non-mutating external imports
show `observe_only_saved_plan`.

Agents can also report approved external-adapter status and selected traffic
accumulation as continuous telemetry when
`[network].runtime_status_telemetry_plans` is configured. Those plans are local
agent config, require bounded absolute-argv `external_managed_adapter` status
commands, run no more often than the configured interval, and expose only
redacted health fields such as status, exit code, duration, reason, command
hash, and stdout/stderr hashes. Traffic accumulation is selected per plan:
interface counters are the default source, `vnstat` is an optional preset when
available or explicitly configured, and custom JSON-producing absolute-argv
sources can be selected for provider/application-specific accounting. In the
intended server model, these choices are represented by managed data-source
presets: built-in defaults, shared customizable presets such as `vnstat`, and
VPS-local custom presets. Each VPS has an explicit selected preset for each
applicable data-source domain.
General agent telemetry follows the same source-selection principle:
`[telemetry] source = "linux_procfs"` is the cheap default for Linux procfs and
sysfs, `custom_command` can replace it with a bounded JSON-producing source,
and `linux_procfs_and_custom_command` can overlay provider-specific data. The
hostname and OS-release files, proc root, sysfs network directory, shell-script
argv prefix, user-session inventory command, process inventory source, ICMP
probe base argv, runtime `ip`/`tc` argv, `vnstat` argv, adapter commands, and
Bird2 hooks are config values or documented presets rather than one hidden host
layout. Process inventory defaults to Linux procfs with configurable root and
can be replaced by a bounded JSON command; user-session inventory defaults to
Linux `w`/`who` preset candidates and can use a configured command for
nonstandard images. Custom commands are implementation details inside presets;
bulk workflows should assign, clone, test, or update data-source presets, not
push ad hoc command strings as the primary management model.

The control plane now exposes the first preset-management registry through
`/api/v1/data-source-presets` and `/api/v1/data-source-assignments`. Built-in
defaults are seeded per data-source domain, operators can create shared presets
or VPS-local presets, and assignment can target explicit clients, resource
pools, or tags while each VPS still records its selected preset. The same
workflow is available in the Pools panel, `vpsctl data-source-presets`,
`vpsctl data-source-preset-create`, `vpsctl data-source-assignments`,
`vpsctl data-source-preset-assign`, and matching VTY commands. Operators can
also preview the agent config fragment rendered from a VPS's selected presets
through `/api/v1/data-source-hot-config`, the Pools-panel `Render selected
config` control, `vpsctl data-source-hot-config --client-id <id>`, and VTY
`data-source-hot-config --client-id <id>`. Bulk update means updating the
shared preset definition itself; assigning pools/tags/clients is only the
selection workflow for which VPSs use that preset.

The built-in registry now includes selectable curated alternatives beyond the
one default per domain: host-mounted proc/sys telemetry, `vnstat` JSON traffic,
pinned `/usr/bin/ping`, host-mounted process inventory, pinned `w`/`who`,
BusyBox `ash`, runtime iproute2/tc reconciliation, reserved S3/MinIO backup
object storage, and signed HTTPS update artifact sources. These are built-in
presets but not defaults; they must be explicitly assigned before a VPS uses
them. `vpsctl data-source-status` and VTY `data-source-status` show the active
selected preset/source/status read model, including compact runtime evidence
for traffic/tunnel samples and backup/update object-store readiness, artifact
counts, and release counts.

Saved observed tunnel plans can be upgraded into managed adapter contracts
without changing the tunnel-plan id. The API, Topology panel Adapter contract
form, `vpsctl tunnel-promote-adapter`, and VTY `tunnel-promote-adapter`
require confirmation and a bounded adapter status command; actual host mutation
still happens only through later
proof-gated apply/rollback commands. Unprivileged agents default to skipping
mutating runtime commands; set `[network].runtime_unprivileged_mutation_policy`
to `try_external_adapters` only when an adapter command is intentionally safe to
run as the normal agent user, and use `--force-unprivileged` for the explicit
best-effort dispatch.

```sh
cargo run -p vpsctl -- telemetry-tunnels --client-id edge-a
cargo run -p vpsctl -- tunnel-promote-telemetry --client-id edge-a --interface wg42 --peer-client-id edge-b --local-underlay 198.51.100.10 --peer-underlay 203.0.113.20 --address-pool-cidr 10.255.0.0/30 --side left --bandwidth 1000m --latency-ms 8
cargo run -p vpsctl -- tunnel-promote-adapter --plan-id <saved_plan_uuid> --runtime-status-argv /usr/local/libexec/wg-adapter,status,{interface} --runtime-startup-argv /usr/local/libexec/wg-adapter,start,{interface} --confirmed
```

The first bounded network-apply slice is available through the API, CLI, VTY,
and Topology panel for BGP/Bird2-managed agents that explicitly enable `[network]
apply_enabled = true`. It renders canonical side-specific snippets from a saved
plan, freezes snippet hashes in the proof payload, requires destructive
confirmation, targets exactly one endpoint side, and the agent writes only
vpsman-managed ifupdown and Bird2 include files under its configured root after
creating backups. Agents can optionally run configured validation hooks and
reload hooks after the atomic write, or use the curated
`debian_ifupdown2_bird2` preset for `/usr/sbin/ifreload -a -s`,
`/usr/sbin/bird -p -c /etc/bird/bird.conf`, `/usr/sbin/ifreload -a`, and
`/usr/sbin/birdc configure`; failed validation or reload rolls the managed
files back to their previous contents. The same proof-gated surfaces support
`network_rollback`, which runs preset pre-rollback interface teardown such as
`/usr/sbin/ifdown -f {interface}` while the old ifupdown snippet still exists,
then removes the exact managed blocks for one endpoint side of a saved plan,
backs up changed files, and runs validation/reload hooks before accepting the
rollback. `network_status` is also
available as a proof-gated read-only inspection job; it reports whether the
agent's current managed ifupdown and Bird2 files contain the expected blocks
and hashes for one endpoint side, checks live interface state through sysfs
under the configured root, and can run an optional read-only Bird2 status probe
such as `/usr/sbin/birdc ... {interface}`. The agent parses common Bird2 OSPF
neighbor-state output into `interface_seen`, `full_neighbor_seen`,
`state_counts`, and `healthy` fields while still returning bounded, hashed
stdout/stderr for audit. Hook execution is disabled by default, bounded by
timeout, and requires absolute argv entries in the agent config. The focused
`smoke-bird2-topology-convergence.sh` Docker check proves two Bird2 OSPFv3
routers can reach `Full/PtP` over the managed GRE/Bird2 snippet shape.
`network_probe` is available as a proof-gated read-only ICMP latency/loss check
against the peer tunnel address; it is bounded by count, interval, timeout, and
hashed/truncated output. Agents use configured `[network].probe_ping_argv` when
set, otherwise the documented Linux ping preset candidates are tried. The probe
status records whether it used the configured source or preset.
`network_speed_test` runs a separate proof-gated
two-endpoint TCP throughput check with exact endpoint targeting, concurrent
gateway dispatch, duration/byte/rate/port/connect-timeout limits, and per-side
status metrics. It is currently the built-in TCP speed-test provider; the
accepted model is to promote speed tests into managed provider presets with
built-in TCP, shared custom providers, and VPS-local custom providers. The API
persists typed summaries of network status, latency
probe, and speed-test status chunks in `network_observations`; the Topology
panel can load those summaries, grouped trend rollups, and retained network job
outputs to show recent status/probe/speed-test evidence, including a compact
latency history. The OSPF recommendation and update-plan views
combine saved tunnel plans with persisted probe/speed-test trends, downgrade
the effective bandwidth tier when measured throughput falls below the
configured burst tier, apply the saved tunnel-plan OSPF policy/preference,
report the recommended cost delta, and render reviewed left/right Bird2 cost
snippets with proof/approval metadata. Reviewed cost deltas can then be applied
with the proof-gated `network_ospf_cost_update` operation through the Topology
panel, `vpsctl tunnel-ospf-cost-update`, or VTY
`tunnel-ospf-cost-update`; the agent writes only the managed Bird2 block and
runs Bird2-only validation/reload hooks. Use
`/api/v1/network/observations`, `/api/v1/network/observation-trends`,
`/api/v1/network/ospf-recommendations`,
`/api/v1/network/ospf-update-plans`, and
`/api/v1/network/topology-graph` for headless inspection. The same read models
are available through `vpsctl network-observations`, `vpsctl network-trends`,
`vpsctl network-ospf-recommendations`,
`vpsctl network-ospf-update-plans`, `vpsctl topology-graph`, VTY
`network-observations`, VTY `network-trends`, VTY
`network-ospf-recommendations`, VTY `network-ospf-update-plans`, or VTY
`topology-graph`. The Topology panel renders the graph from saved tunnel plans,
endpoint state, network trends, and OSPF deltas. The graph marks saved tunnel
edges as convergence-blocked when an endpoint is missing or not connected, so
offline runtime drift is visible without dispatching a network mutation.
Longer-retention analytics, topology graph edit/drilldown workflows, and
higher-level bulk/canary policy automation around OSPF cost changes remain
later milestones.

```sh
cargo run -p vpsctl -- tunnel-apply --plan-file ./plan.json --side left --confirmed
cargo run -p vpsctl -- tunnel-rollback --plan-file ./plan.json --side left --confirmed
cargo run -p vpsctl -- tunnel-status --plan-file ./plan.json --side left
cargo run -p vpsctl -- tunnel-probe --plan-file ./plan.json --side left --count 3 --interval-ms 500
cargo run -p vpsctl -- tunnel-speed-test --plan-file ./plan.json --server-side left --duration-secs 3 --max-bytes 16777216 --rate-limit-kbps 100000
cargo run -p vpsctl -- tunnel-ospf-cost-update --plan-file ./plan.json --side left --current-ospf-cost 14 --recommended-ospf-cost 22 --confirmed
cargo run -p vpsctl -- network-observations --limit 50
cargo run -p vpsctl -- network-trends --limit 50
cargo run -p vpsctl -- network-ospf-recommendations --limit 50
cargo run -p vpsctl -- topology-graph --limit 50
```

Example curated validation/reload presets:

```toml
[network]
apply_enabled = true
preset = "debian_ifupdown2_bird2"
validate_enabled = true
reload_enabled = true
hook_timeout_secs = 10
```

`debian_ifupdown_bird2` is also available for legacy ifupdown systems. It uses
absolute argv for `ifup --no-act {interface}`, `bird -p`, `ifdown -f
{interface}`, `ifup {interface}`, and `birdc configure`, with rollback-time
`ifdown -f {interface}` before managed-block removal.

Example optional read-only Bird2 status config:

```toml
[network]
bird2_status_argv = ["/usr/sbin/birdc", "show", "ospf", "interface", "{interface}"]
status_probe_timeout_secs = 5
status_probe_max_output_bytes = 16384
```

The interactive shell is available with `vpsctl vty`. Before privileged
`job-create` commands, set local proof material and run `enable` inside the VTY:

```sh
export VPSMAN_API_TOKEN=...
export VPSMAN_SUPER_PASSWORD=...
export VPSMAN_SUPER_SALT_HEX=...
cargo run -p vpsctl -- vty
```

`enable` validates only local material. The super password is not sent to the
server. VTY `enrollment-tokens` lists retained token policies, and
`reenrollment-token-create --client-id <id> --confirmed` creates a bound
rebuild token. VTY `job-create <command> <target ...>` accepts `client:<id>`,
`pool:<uuid>`, `tag:<name>`, or bare tag names, plus `--pty`,
`--destructive`, and `--confirmed`; `job-cancel <job_uuid> --confirmed [reason]` cancels pending
scheduled approvals or active dispatching jobs; `file-pull --path
<remote-abs> <target ...>` and `file-push
--source <local-file> --path <remote-abs> <target ...> --confirmed` use the
same proof-gated file operation path; `process-list <target ...> [--limit
<1-512>]` uses the same privileged proof path. VTY process supervisor commands
also use the same proof path:
`process-start <name> --argv <abs> [--argv arg ...] <target ...> [--force-unprivileged]`,
`process-stop <name> <target ...>`, `process-restart <name> <target ...>`,
`process-status [--name <name>] <target ...>`, and
`process-logs <name> <target ...> [--max-bytes <n>]`. VTY `hot-config
--config-file <path> <target ...> --confirmed` uses the same proof-gated
config-update path. VTY `super-password-rotate (--new-proof-key-hex <hex>|--new-password-env <env> [--new-super-salt-hex <hex>])
[--rotation-generation <id>] <target ...> --confirmed` uses the old local
proof to install the next derived proof key without sending either plaintext
password to the server. `vpsctl super-password-rotations --limit 20` and VTY
`super-password-rotations [--limit <1-200>]` return the sanitized rotation
history from job state: generation label, status/counts, payload hash, and
timestamps only. VTY `agent-update --artifact-url <https-url> --sha256-hex
<64_hex_sha256> [--artifact-signature-hex <128_hex_signature>]
[--artifact-signing-key-hex <64_hex_public_key>] [--canary-count <n>]
<target ...> [--force-unprivileged] --confirmed`
uses the same proof-gated staged update path. VTY
`agent-update-activate --staged-sha256-hex <64_hex_sha256> <target ...>
--restart-agent [--force-unprivileged] --confirmed` and
`agent-update-rollback [--rollback-sha256-hex <64_hex_sha256>] <target ...>
[--force-unprivileged] --confirmed` use the same proof-gated
activation/rollback path. VTY `agent-update-rollouts` lists the
current sanitized rollout records, including `heartbeat_verified` evidence
after an activated agent reconnects, health-gate state, pause state, and worker
lease evidence. VTY `agent-update-rollout-policies [--limit <n>]` and
`agent-update-rollout-policy-create --name <name> --scope-kind
global|tag|pool|provider [--scope-value <value>] [--channel <name>]
[--canary-count <n>] [--health-gate
heartbeat_verified|manual_after_canary|manual_only] [--priority <n>]
[--disabled] --confirmed` manage reusable rollout policy presets. VTY also supports
`agent-update-rollout-activate --rollout-id <uuid> [--batch-size <n>]
[--restart-agent] [--force-unprivileged] --confirmed` and
`agent-update-rollout-rollback --rollout-id <uuid> [--force-unprivileged]
--confirmed` for operator-driven canary promotion and rollback dispatch.
`agent-update-rollout-delegate-rollback --rollout-id <uuid>
[--proof-ttl-secs <15-86400>] [--force-unprivileged] --confirmed` stores
scoped rollback proof escrow for heartbeat-timeout or activation-failure
recovery after VTY `enable`. `agent-update-rollout-delegate-activation
--rollout-id <uuid> [--proof-ttl-secs <15-86400>] [--restart-agent]
[--force-unprivileged] --confirmed` stores scoped activation proof for
worker-recommended rollout promotion. `agent-update-rollout-control
--rollout-id <uuid>
(--pause|--resume|--health-gate <heartbeat_verified|manual_after_canary|manual_only>)
[--pause-reason <text>] --confirmed` updates server-side rollout automation
metadata without sending privileged proof. VTY also supports
`agent-update-artifact-upload --name <name> --version <version> --artifact-file
<path> --signing-seed-hex <64_hex_seed> [--rollback-artifact-file <path>]
[--rollback-signing-seed-hex <64_hex_seed>] [--stream] --confirmed`,
`agent-update-release-record ... [--rollback-artifact-file <path>
--rollback-artifact-url <https-url> --rollback-signing-seed-hex <seed>]`, and
`agent-update-release-latest [--name <name>] [--channel stable]` for the
bounded hosted artifact and release-channel slice. The same VTY surface includes
`schedules`,
`schedule-create <name> <interval_secs> <command> [schedule policy flags] <target ...>`,
and `schedule-dispatch <job_uuid> [--force-unprivileged] --confirmed`, plus
`backups`,
`backup-request`, `backup-run`, `backup-artifacts`,
`backup-artifact-record`, `backup-artifact-upload`,
`backup-artifact-upload-chunked`,
`backup-artifact-handoff`, `restore-plans`, `restore-plan`,
`restore-run ... [--force-unprivileged]`,
`restore-rollback ... [--force-unprivileged]`, `migration-links`,
`migration-link <restore_plan_uuid> --confirmed`, `tunnel-plans`,
`tunnel-plan ... [--save]`, `tunnel-promote-telemetry ...`, and
`tunnel-apply --plan-file <plan.json> --side <left|right> --confirmed` /
`tunnel-rollback --plan-file <plan.json> --side <left|right> --confirmed` /
`tunnel-status --plan-file <plan.json> --side <left|right>` /
`tunnel-probe --plan-file <plan.json> --side <left|right> [--count <1-20>]` /
`tunnel-speed-test --plan-file <plan.json> --server-side <left|right>` /
`network-observations [--limit <1-200>]` /
`network-trends [--limit <1-200>]` /
`network-ospf-recommendations [--limit <1-200>]` /
`network-ospf-update-plans [--limit <1-200>]` /
`topology-graph [--limit <1-200>]`
for
encrypted backup output, S3/MinIO or filesystem-backed encrypted artifact
storage, metadata backup/restore planning, non-mutating tunnel planning,
bounded proof-gated network apply/rollback, managed-file status, and bounded
read-only tunnel latency and throughput probing plus persisted network
observation history and read-only OSPF cost recommendations.
VTY `job-outputs <job_uuid>` lists retained output metadata,
`job-follow <job_uuid> [--interval-ms <ms>] [--max-polls <n>] [--json]`
polls and decodes retained output until terminal job state, and
`job-output-artifact <job_uuid> <client_id> --seq <seq> <output_file>`
downloads externalized job-output artifacts through the same API route as the
panel and CLI. VTY `terminal-sessions [--limit <1-200>] [--client-id <id>]
[--session-id <uuid>]` exposes the same durable polled terminal-session
inventory as `vpsctl terminal-sessions` and the Jobs panel. VTY
`terminal-poll --session-id <uuid> [--replay-from-seq <n>] <target ...>`
retrieves retained PTY output through the same proof-gated terminal operation
as CLI and panel dispatch.
`vpsctl restore-run`, VTY `restore-run`, and the Backups panel Run restore
action keep backup private key material local before dispatch. They can use
either a local encrypted artifact file or the linked stored encrypted artifact.
`vpsctl restore-rollback`, VTY `restore-rollback`, and the Backups panel
Rollback restore action use retained restore output evidence to build the
rollback manifest locally before proof generation.
Privileged dispatch operations resolve targets through the API and submit
per-client proof envelopes.
