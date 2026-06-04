# vpsman Design

## Status And Principle

`vpsman` is a traditional Rust C/S VPS management system for 20+ Linux VPSs.
Agents are headless Linux clients. Servers provide both a modern Google Cloud
style web panel and a complete headless CLI/VTY management surface.

The current repository is only a phase 0 foundation. It has useful workspace,
protocol, UI, schema, and smoke-test scaffolding, but it is not yet the full
system. Future agents must not treat placeholder APIs, mock UI data, or helper
types as completed features. A feature is complete only when the running system
implements it, tests verify it, and the panel and CLI expose it where required.

Core product principles:

- Correctness and robustness before feature claims.
- Thoroughness and product completeness before speed: every large workflow must
  be inspected across agent, server, API, CLI/VTY, frontend, storage, tests,
  operations, and migration paths before it can be called complete. Fast slices
  are acceptable only when they preserve a visible path to the full product
  model and record remaining gaps honestly.
- Low client overhead suitable for small VPSs.
- Explicit privilege boundaries and proof-gated control.
- User-friendly frequent professional use, especially in the frontend:
  day-to-day inspect, filter, approve, dispatch, rollback, and verify loops
  must be fast, predictable, durable across routine refreshes where practical,
  and comfortable for operators managing 20+ VPSs repeatedly.
- Traditional CRUD/list pages are a reusable product abstraction, not one-off
  tables. Record-heavy tabs must expose row counts, filtered counts, current
  page/total pages, field-selected search, all-field search, predictable page
  controls, and stable dense table layout so operators can manage many records
  without losing context.
- Data-source preset management instead of hidden hardcoded assumptions.
  Telemetry, traffic accounting, probes, speed tests, process inventory,
  user/session inventory, tunnel adapters, limits, paths, backup/update
  providers, and provider-specific operations must be modeled as data-source or
  adapter domains with named presets and explicit per-VPS preset assignment.
  Common Linux behavior should be available as built-in/default presets;
  operators can create shared customizable presets such as a `vnstat` traffic
  preset and VPS-local custom presets for one unusual host. Bulk workflows
  operate on data-source presets and VPS preset assignments with preview and
  audit; commands such as custom JSON argv are only implementation details
  inside a preset, not the primary business abstraction.
- Continuous verification with documented evidence.

Do not modify `AGENTS.md`. Frequent-use ergonomics are a primary design
principle, especially for the frontend; they are not a later polish item.
Customizability is also a primary product principle: an implementation that
works for only one assumed Linux layout, one command path, or one data source is
not complete unless that assumption is explicitly documented as a built-in
preset or compatibility adapter with a preset-backed replacement path.
Thoroughness is also an acceptance condition: a feature is not complete merely
because one command path or one UI button works. Completion requires the
operator workflow, headless workflow, privilege/degraded behavior, persistence,
observability, tests, configuration, and future extension boundary to all be
accounted for or listed as concrete gaps.

## Original Product Requirements

- Build two Rust versions: a headless Linux client agent and a server/control
  plane.
- Clients connect outbound to the server over raw TCP keepalive sessions.
- Clients support Ubuntu 18.04-24.04, Debian, and similar Linux distributions.
- Client binaries must be minimal, glibc-independent, cost-efficient, low RAM,
  low CPU, and suitable for 1-core/256MB VPSs.
- HTTPS fallback discovery can be used to obtain replacement server addresses
  when the current TCP endpoint is offline.
- Clients have a super password model. Without valid proof, commands are not
  executed and are reported only.
- Super password material must never be transferred or stored plaintext.
- The frontend may store the super password only for convenience in encrypted
  local storage.
- The agent protocol must be modern, minimal, efficient, resilient to high
  latency and disconnects, flexible, and extendible.
- Binary framing and compression are allowed only if CPU/RAM cost stays low.
- Clients must support file transfer, command execution, process management,
  scheduled tasks, `w`/user visibility when authorized, RAM/CPU/disk/network
  metrics, latency curves, hot config, and in-place updates.
- Agents start as root and autostart on boot across supported distributions.
- The server panel must have left navigation, top banner/command area, fleet
  statistics, VPS tabs/details, convenient access, and a Google Cloud style.
- The server panel must be friendly for frequent operation, not only visually
  polished: common workflows should be quick to repeat, easy to scan, low
  friction after refresh/reconnect, and safe for professional daily use across
  20+ VPSs.
- The server panel must support provider/resource-pool parent grouping, custom
  tags, and bulk operations over hierarchy and tags.
- BGP-tagged clients must support tunnel and Bird2/OSPF management workflows.
- The server must provide full CLI functionality, including router-like VTY.
- Backups, record history, easy VPS migration, restore, and useful utilities are
  in scope.
- Software development standards apply: structured layout, tests, code quality,
  reproducible build/test steps, and persistent progress documentation. Large
  source files above the recommended threshold must be split or justified in
  `docs/large-file-split-notes.md`, and frontend layout changes must pass a
  screenshot review smoke for main desktop/mobile console states.

## Negotiated Decisions

- Deployment target: Docker Compose split services first, portable to
  Kubernetes later.
- Server storage: PostgreSQL plus local filesystem-backed object storage as the
  baseline. The object-store abstraction remains backend-extensible so S3/MinIO
  can be enabled for explicit deployments, but S3 is not the default product or
  release target.
- Metrics storage: vanilla PostgreSQL with rollups, not TimescaleDB initially.
- Frontend: React + TypeScript.
- Frontend UX principle: optimize for user-friendly frequent professional use.
  Favor dense but readable data, saved context, quick filters, clear status,
  keyboard-friendly repeated actions, predictable bulk previews, and
  low-friction proof unlocks over decorative or demo-first layout. A screen can
  look modern and still fail the design if routine inspect-control-verify loops
  require avoidable repetition or context loss.
- Fleet-view principle: global fleet search and saved views are first-class
  operator context, not a decorative topbar. The console keeps saved non-secret
  view state locally and applies the active scope consistently to the visible
  Fleet, Pools, Jobs, Schedules, Topology, and Backups agent selectors. Search
  supports free text plus practical tokens such as `tag:`, `pool:`,
  `provider:`, `region:`, and `status:` so operators managing 20+ VPSs can
  switch between recurring resource slices without repeated manual filtering.
- CRUD/list abstraction principle: every traditional management page or tab
  that displays records must use a shared table-control model rather than an
  unbounded ad hoc list. The minimum surface is total row count, filtered row
  count, current page, total page count, row/page size, field selector,
  all-field search, previous/next page controls, actionable empty state, and a
  stable responsive layout. Current frontend slices apply this model to Active
  sources, the preset registry, pools, tags, audit records, job records, job
  target results, schedule records, operator/session/enrollment/key/gateway
  records, process supervisor inventory, file transfer sessions/source
  artifacts, terminal sessions, update releases/rollouts, and backup/
  restore/migration history. The shared component persists field/query/page/
  page-size preferences in local browser storage as non-secret UI state so
  frequent operator views survive routine refreshes. Diagnostic evidence
  drilldowns such as topology observation traces may use focused evidence
  tables when they are not CRUD management lists; future record tables should
  reuse the same component or a stricter compatible abstraction.
- Data-source preset principle: source choice is a managed product model, not
  an ad hoc config override. The control plane should maintain preset
  definitions per data-source domain, including built-in default presets,
  curated shared presets, operator-customized shared presets, and VPS-local
  custom presets. Each VPS records the selected preset for each applicable
  domain. Pool/tag/bulk workflows are convenience mechanisms for assigning,
  cloning, validating, and auditing those preset selections at scale; they do
  not replace per-VPS explicit assignment. A custom command is allowed only as a
  field inside a preset with validation, bounds, source/status reporting, and
  audit metadata.
- Current preset-management implementation slice: the control plane stores
  built-in/default, shared custom, and VPS-local custom data-source presets in
  memory/PostgreSQL; every existing VPS gets explicit default selected-preset
  assignments; operators can create, clone, diff, test, update, and assign
  selected presets by client, pool, or tag through API, CLI, VTY, and the Pools
  panel; multi-target assignment and multi-VPS preset-definition updates require
  confirmation and emit audit records. The control plane can also render a
  agent config patch from a VPS's selected presets for API, CLI, VTY, and panel
  preview. Preview is not itself privileged dispatch. Applying reviewed
  selected-preset output uses a separate proof-gated
  `data_source_config_patch` job that merges only allowed data-source sections
  (`telemetry`, `execution`, `network`) into the agent's current config, then
  reuses full hot-config validation before persisting. A PostgreSQL-backed live
  smoke now proves create/assign/render/apply for selected telemetry and
  command-execution-policy presets through the real API, gateway, enrolled
  agent, rollback file, audit/target/output records, no-proof rejection, live
  non-default execution behavior, and API restart persistence. The same preset
  model now has an active selected-source read model: the API exposes
  `/api/v1/data-source-status`; `vpsctl` and VTY expose `data-source-status`;
  the Pools panel shows Active source status rows; and status records tie each
  VPS/domain/module to the selected preset, source kind, status reason, and
  compact live evidence for the currently implemented telemetry,
  traffic-accounting, runtime-tunnel adapter, backup object-store, and update
  artifact-source domains. Workflow-only domains now expose typed
  `ready_on_demand` evidence for process inventory, user/session inventory,
  latency probes, speed tests, and command execution policy, including proof
  gating and selected policy metadata without exposing command secrets or env
  values. Process-inventory source status also carries supervisor workflow
  identity and agent capability evidence for process-limit availability,
  including root, unknown, and unprivileged/degraded modes, so operators can
  see whether selected process-management workflows are expected to enforce
  cgroup/process limits before dispatch. Backup/update source status is
  enriched with server runtime object-store kind/configuration,
  artifact/release counts, backup request counts, restore source/target
  counts, migration source/target counts, rollout counts, active rollout
  counts, failed rollout counts, and delegated rollout-proof readiness without
  exposing paths or credentials. Runtime tunnel and traffic source status now
  connects selected presets to saved tunnel plans, network observations,
  degraded samples, latency probes, speed tests, routing recommendations,
  OSPF update candidates, and traffic-limit plan/apply evidence. Remaining
  design work is deeper adapter-specific drilldown surfaces and richer curated
  preset libraries, not a separate command-override model.
- Current curated data-source library slice: built-in presets can now be
  defaults or non-default curated options. Fresh and existing PostgreSQL
  deployments seed selectable non-default presets for host-mounted proc/sys
  telemetry, `vnstat` JSON traffic accounting, pinned `/usr/bin/ping`, host
  mounted process inventory, pinned `w`/`who`, BusyBox `ash`, runtime
  iproute2/tc reconciliation, reserved S3/MinIO backup object storage, and
  signed HTTPS update artifacts. Default assignment still selects exactly one
  default preset per domain; curated built-ins must be explicitly assigned by
  client, pool, or tag before they affect a VPS.
- Current hardcode-audit consolidation slice: frontend operation defaults are
  being moved out of panels into explicit preset catalogs. Topology backend
  managed-file paths now come from a network-backend preset catalog;
  backup/restore default selected paths and placeholders come from backup-path
  presets; job-operation shell/file/backup/supervisor examples come from
  job-operation preset constants; agent network hook commands, Bird2/netplan
  reload paths, user-session command candidates, and latency probe executable
  candidates are named presets/constants instead of scattered literals.
  Installer root/service paths are now named installer-policy presets, `vnstat`
  traffic parsing/status naming explicitly identifies the selectable preset,
  and representative test/example paths are named fixtures rather than hidden
  runtime policy.
  `scripts/release-check.sh` now runs `scripts/audit-customizability.sh`.
  The latest scan reports 0 open candidates for the current scanner terms.
  Remaining customizability work is semantic rather than an unclassified
  hardcode backlog: still-open provider or policy models include speed-test
  providers, restore path mapping, terminal/PTY policy, and richer
  workflow-specific source/status surfaces.
- Current command preset/status hardening slice: the Jobs API and panel now
  include saved command templates as first-class preset records, scoped by
  global/provider/pool/tag/client and persisted in memory/PostgreSQL. Templates
  store typed operation JSON plus bounded non-secret defaults, require
  operator confirmation to upsert, emit audit records, and can be selected from
  the dispatch composer. Job creation now carries an optional idempotency key
  and reconnect policy. The server records those fields, returns the existing
  job when the same operator repeats the same idempotency key with the same
  payload, and rejects unsafe key reuse with a different payload. Delegated
  rollout activation/rollback dispatch uses stable idempotency keys as well,
  so worker restarts do not fan out duplicate client commands. Job output
  comparison is exposed as a reusable API/panel action for group command
  result verification.
- Data-source bulk update principle: bulk update means changing a shared
  data-source preset definition or metadata and letting all VPSs assigned to
  that preset naturally consume the new rendered model after review. Bulk
  assignment is a separate selection workflow for choosing which VPSs/pools/tags
  use a preset. Neither should be modeled as pushing ad hoc commands or
  mutating "commands" as the top-level business object; commands remain bounded
  implementation fields inside the selected data-source preset.
- API style: REST for commands/config and WebSocket for live streams.
- Operator auth: local users, password hashing, optional TOTP, roles/scopes,
  bearer access tokens, and refresh tokens.
- CLI: Rust `vpsctl` plus interactive VTY. Every panel action must have an
  equivalent CLI capability.
- Client identity: provisioned keypair, stable across hostname/IP changes.
- Agent enrollment: short-lived installer token plus locally generated keypair
  and locally stored secret verifier material.
- Inventory ownership: clients are loyal executors/reporters. They may present
  only stable identity and runtime capability facts during enrollment/hello;
  alias/display name, custom tags, resource-pool membership, country tags,
  presets, topology policy, rollout policy, and restore/migration policy are
  server-owned database state. Enrollment tokens can carry server-side default
  alias, pool name, and tags; ordinary client claims cannot override them.
- Transport: raw TCP sockets with Noise encryption above TCP.
- Wire format: custom binary TLV frames inside Noise.
- Compression: pure-Rust thresholded LZ4 for larger payloads. Avoid C
  compression dependencies that break static musl builds without external
  toolchains.
- Super password: global operator secret, but client-side verification uses
  per-client salted derived proof material.
- Proof generation: browser/CLI derives short-lived proof locally. Server signs
  and forwards commands; agents verify proof and server signature.
- Privilege boundary: every operation except autonomous health telemetry
  requires valid super-password proof.
- Unauthorized behavior: reject the operation, report audit metadata/hash/reason,
  and return no local command output.
- Command idempotency and reconnect policy: the server records operator-scoped
  idempotency keys and reconnect metadata, returns the existing job for
  same-key/same-payload retries, and rejects same-key/different-payload reuse.
  The agent also keeps a bounded recent-command cache across reconnect attempts
  in the same process. Exact duplicate completed-job delivery returns a typed
  `duplicate_job_suppressed` status without re-executing host mutations;
  same-job-id/different-payload conflicts and duplicate active deliveries are
  rejected. Disk-backed suppression across agent process restart is an optional
  hardening extension, not a hidden current guarantee.
- Super password rotation: staged privileged rollout using old proof to install
  new verifier material. Browser, CLI, and VTY rotation derive the next proof
  key locally when given the next password/salt and dispatch only the
  non-plaintext derived key in a proof-gated command; direct derived-key input
  is reserved for scripted operators that already hold generated proof
  material. The server exposes sanitized rotation history through API, panel,
  CLI, and VTY using job status, generation labels, target counts, payload
  hashes, and timestamps only; new proof keys and plaintext passwords are never
  stored or displayed in the history read model.
- Browser storage: WebCrypto-encrypted local vault.
- Lost super password recovery: root re-enrollment or proof-authorized staged
  rotation, never server plaintext escrow.
- Agent resource budget: target under 15MB RSS at idle, near-zero idle CPU,
  bounded buffers, no heavy background probes beyond telemetry.
- Release gates include `scripts/smoke-agent-resource-budget.sh` for static
  musl binary size, idle RSS, idle CPU, and thread-count budgets in a 128MB
  container, `scripts/smoke-agent-reconnect-churn.sh` for latency-injected
  forced connection loss followed by successful reconnect, plus
  `scripts/audit-agent-static-deps.sh` to fail the agent musl dependency graph
  on dynamic TLS/system-library crates or unexpected `*-sys` packages.
- Telemetry cadence: lightweight health every 15s and fuller health metrics every
  60s by default.
- Telemetry allowed without proof: uptime, CPU/RAM/disk/network counters,
  latency, agent version, OS, status, and tags. No process/user/file details.
- Telemetry source model: Linux procfs/sysfs is the default low-cost preset, not
  an implicit universal assumption. Agents may currently select `linux_procfs`,
  `custom_command`, or `linux_procfs_and_custom_command`, with configurable proc
  root, sysfs network directory, hostname file, OS-release file, and bounded
  JSON custom source. The intended server model is a managed telemetry preset
  registry and explicit per-VPS telemetry preset assignment, with pool/tag/bulk
  workflows used to assign, clone, validate, and audit presets across many
  hosts.
- Command execution: explicit argv jobs by default. Bounded shell-script wrapper
  jobs and noninteractive PTY-backed argv jobs are separate proof-gated modes.
  Shell-script execution uses a configurable absolute argv prefix, with
  `/bin/sh -lc` only as the documented Linux preset. Command execution policy
  presets also carry fixed working directory, inherited/clean/minimal
  environment policy, explicit env values, PTY enabled/disabled policy, and
  process-group/direct-child cleanup policy. Live smoke verifies a selected
  non-default command policy by running proof-gated shell-script work from the
  configured directory with clean env values, proving direct-child timeout
  cleanup, and rejecting terminal open when PTY is disabled. Process inventory
  defaults to configurable Linux procfs and can use custom JSON commands; user-session
  inventory defaults to Linux `w`/`who` preset candidates and can use a
  configured command.
  A proof-gated terminal runtime, durable terminal-session inventory, durable
  replay from persisted PTY job outputs, and continuous idle output streaming
  now exist. Agents stream terminal output over the existing Noise TCP session
  with bounded queues and 80 ms / 16 KiB coalescing; gateways append it to
  server job-output history without blocking the active command slot; the API
  emits terminal-specific WebSocket metadata; and the Jobs panel plus
  `vpsctl`/VTY can follow replay deltas. Very long terminal-history retention
  policy and optional ack-based flow control remain future tuning, not the
  release-blocking live-terminal path.
- Process management: portable pid supervisor for vpsman-managed processes
  first. Do not require systemd integration for the first real release. The
  management model should still be extensible: process start requests carry a
  typed run policy and resource-limit intent so the panel/server can evolve
  toward restart policy, graceful stop, memory/PID/open-file/no-new-privileges
  enforcement, cgroup CPU shares, log budgets, and later systemd adapters
  without changing the privileged command envelope shape again.
- Scheduled tasks: stored and dispatched by the server scheduler.
- File transfer: chunked, resumable, hash-verified, conservatively rate-limited,
  with explicit override for large files.
- Agent update: atomic staged self-update with signed artifact, hash verification,
  side-by-side install, restart, heartbeat validation, and rollback.
  The rollout panel exposes policy presets, delegated activation/rollback
  proof escrow, pause/resume controls, forced-unprivileged visibility, and an
  Advance action that reuses the existing proof-gated delegate/activate paths
  instead of introducing another update mechanism.
- Backup encryption: client-side encryption to a panel public key.
- Grouping: provider/resource-pool parent nodes plus custom tags.
- Network MVP: observe/import/render plan first. Proof-gated apply comes after
  plan rendering is trustworthy.
- Network backend: client-managed runtime tunnel reconciliation is the default
  product direction. Ifupdown, netplan, and systemd-networkd are compatibility
  and migration adapters, not the primary future model.
  `network_status` reports read-only real-kernel namespace coverage when the
  agent is inspecting `/`, otherwise it reports rooted-sysfs-only evidence for
  staged/test roots. When a real namespace is available, bounded `ip -j`
  link/neigh/route probes use the configurable runtime `ip` argv source and
  are summarized as probe states rather than hidden assumptions. The topology
  graph derives drift policy/action labels, neighbor/probe state, runtime
  state/reasons, adapter/routing state, kernel link/neigh/route probe states,
  real-kernel namespace coverage, desired/stale/import drift counts, and
  recent per-tunnel latency series from persisted observations so
  panel/CLI/API consumers can distinguish degraded, observation-only,
  unsupported, and actionable states without parsing raw status JSON.
- Network file ownership: dedicated snippets only for compatibility persistent
  backends, especially `/etc/network/interfaces.d/vpsman-tunnels`, plus Bird2
  include files.
- Routing daemon: Bird2 first. FRR is future adapter work.
- Tunnel addressing: panel IPAM.
- Tunnel speed tests: agent-to-agent tests orchestrated by server with rate
  limits.
- OSPF cost: weighted formula using latency, packet loss, bandwidth tier, and
  panel preference, preferring higher bandwidth when latency is tolerable.
- Legacy Bird2 graph compatibility is import/migration support only.
- Secrets policy: templates only in repo. No real secrets.
- Testing policy: after design agreement, agents may run tests and Docker-based
  real integration verification freely.

## Architecture

### Agent

The agent is a static Rust binary for Linux `x86_64` and `aarch64`, built for
musl targets. Production installs should run as root because command execution,
file operations, update, backup, process limits, and network management often
require root capabilities. Root execution must be paired with strict
proof-gated authorization and audit reporting.

The agent also supports an explicit unprivileged mode for normal-user
deployments, rebuilt VPS bring-up, constrained hosting, and diagnostics. In
that mode the agent reports an authenticated capability snapshot in `AgentHello`
including privilege mode, effective UID, whether privileged operations can be
attempted, and whether runtime tunnels/process limits are expected to work.
Root-only operations must not be hidden: the panel/CLI should show capability
hints, default to safe ineffective/fail-closed behavior where appropriate, and
allow explicit forced best-effort attempts only when proof-gated, audited, and
clearly labeled. The current API filters explicit unprivileged agents out of
root-only network mutations by default for immediate jobs and scheduled
approval dispatch. It also degrades process starts that request unavailable
resource-limit enforcement, and agent update/restore host mutations when a
target reports unprivileged mode or no privileged host-mutation capability.
Skipped targets record `degraded_unprivileged` evidence with stable reason
codes. `force_unprivileged` remains the professional best-effort override
through API, CLI/VTY process start, CLI/VTY direct agent update and restore
commands, CLI/VTY scheduled dispatch, and the topology panel. This keeps
normal-user agents useful for telemetry, reporting, basic user-owned commands,
and recovery, while advanced root-only workflows expose why they are blocked or
degraded.

Frequent-use frontend behavior is part of this safety model. The panel now
shows target-impact previews for proof-gated update, topology/OSPF, and restore
workflows, grouping selected or resolved agents into ready, degraded,
proof-forced best-effort, and legacy/unknown buckets before dispatch. These
previews are not decorative: they are the operator-facing checkpoint that keeps
daily bulk actions fast while making root-vs-unprivileged consequences visible
before a command is sent.

Agent responsibilities:

- Maintain outbound TCP connection to gateway with keepalive, reconnect backoff,
  and bounded memory.
- Establish Noise session before application frames are exchanged.
- Send autonomous health telemetry without super-password proof.
- Send capability snapshots so the server can distinguish root-capable and
  unprivileged agents before dispatching root-only workflows.
- Reject every non-telemetry operation unless proof and server signature verify.
- Execute explicit argv jobs, bounded shell-script wrapper jobs, and future PTY
  jobs.
- Transfer files using chunked resumable protocol with hashes and rate limits.
- Manage vpsman-supervised processes using portable pid tracking.
- Apply hot config updates after proof verification.
- Perform atomic self-update with rollback.
- Encrypt backups client-side before upload.
- For BGP-tagged clients, import network state and later apply dedicated managed
  runtime tunnel changes only after proof-gated confirmation and capability
  checks, with persistent managed-file adapters kept for migration.

### Gateway

The gateway accepts outbound agent TCP sessions and owns session lifecycle. It
does not make security decisions from plaintext metadata.

Gateway responsibilities:

- Run Noise enrollment and enrolled handshakes.
- Authenticate client identity by key after enrollment.
- Multiplex TLV streams for telemetry, jobs, file transfer, PTY, updates, and
  network observations.
- Enforce frame limits, sequence/replay checks, timeouts, keepalive, and
  backpressure.
- Forward events to the API/worker persistence layer.
- Never log secret material or command payload plaintext beyond approved audit
  metadata.

### API Server

The API server is the single control-plane API for panel and CLI.

API responsibilities:

- Operator login, TOTP, bearer access/refresh token lifecycle, roles, and scopes.
- Client inventory, enrollment tokens, endpoint discovery, groups, tags, jobs,
  telemetry, audit logs, backups, schedules, topology, and tunnel plans.
- REST endpoints for durable state changes.
- WebSocket streams for live telemetry, job output, PTY, topology updates, and
  rollout progress.
- Equivalent capability surface for panel and CLI.

### Worker

The worker handles asynchronous and scheduled control-plane work.

Worker responsibilities:

- Scheduled job dispatch.
- Staged super-password rotation.
- Staged agent update rollout.
- Backup orchestration and retention.
- Metrics rollups.
- Durable task leases for multi-instance worker safety.
- Tunnel speed tests and latency probes.
- Network plan generation.
- Retry, timeout, and rollback workflows.

### Frontend

The frontend is a React + TypeScript cloud console. It must feel like a useful
operational dashboard, not a marketing page.

Frontend requirements:

- Left navigation and top command/search/action bar.
- Fleet overview statistics.
- VPS list and VPS detail tabs.
- Group and tag views.
- Job history, live output, and status.
- Privileged unlock flow using WebCrypto-encrypted local vault.
- Bulk operation confirmation for pools, tags, and explicit selections.
- Frequent-use ergonomics: persistent filters/search/scope where practical,
  quick repeat actions, keyboard-friendly controls, visible recent activity,
  and minimal clicks for common inspect-control-verify loops.
- Backup, restore, migration, topology, tunnel planning, and audit views.
- No static mock data in real release paths. Mock data is acceptable only in
  development fixtures clearly separated from production API usage.

### CLI And VTY

`vpsctl` must be a first-class headless management interface.

CLI requirements:

- Same API capabilities as the panel.
- Scriptable subcommands for all operations.
- Interactive router-like VTY mode.
- Privileged mode using local browser/CLI proof generation, not server plaintext
  password handling.
- Token-based operator login stored in user config with safe permissions.
- Machine-readable output modes are first-class for headless operation:
  `vpsctl --output raw|json|pretty-json` applies to every non-interactive
  operational command. Raw mode preserves historical stdout, while JSON modes
  normalize single JSON values, JSONL event streams, empty output, and
  plain-text command output into structured JSON. The interactive VTY shell is
  excluded because it is a terminal session, not a one-shot command.
- CLI commands that write destination files use explicit `--output-file`
  arguments so global `--output` remains reserved for output formatting. VTY
  accepts the same spelling and may keep legacy `--output` compatibility where
  there is no global flag parser.

## Protocol Design

### Transport

- TCP is the socket transport.
- Enrollment sessions use Noise XX with installer token validation.
- Enrolled sessions use Noise IK or another pattern with equivalent server-key
  pinning and client-key authentication.
- No application TLV frame may be processed before Noise transport mode.
- HTTPS fallback discovery uses normal TLS CA trust for transport and a signed
  discovery document for endpoint authenticity. It can suggest new TCP gateway
  addresses, but it does not replace Noise identity verification or command
  signing authority.
- Discovery signing can rotate independently from command signing through the
  agent auth field `discovery_trusted_server_ed25519_public_keys_hex`.
  Enrollment can deliver up to eight additional trusted discovery public keys
  from `VPSMAN_DISCOVERY_TRUSTED_SERVER_PUBLIC_KEYS_HEX`; hot config may update
  this discovery-only ring with proof, while the command-signing
  `server_ed25519_public_key_hex` remains immutable through hot config.
- For local development and smoke tests only, `http://localhost`,
  `http://127.0.0.1`, or `http://[::1]` discovery URLs may be accepted. Nonlocal
  HTTP discovery remains invalid so production discovery stays HTTPS-only.

### Framing

Application frames are custom binary TLV:

- Magic/version.
- Flags.
- Message kind.
- Stream id.
- Sequence number.
- Payload length.
- Payload.

Frame requirements:

- Version negotiation.
- Unknown message kind handling for forward compatibility.
- Max frame length and max decompressed length.
- Fragment-safe decoding.
- Replay/stale sequence rejection for every inbound stream direction.
- Optional thresholded LZ4 compression for large payloads only.
- Small telemetry and control frames remain uncompressed.

### Message Families

Required message families:

- Enrollment and hello.
- Keepalive and session health.
- Telemetry.
- Job request, ack, output, status, cancel.
- File transfer open/chunk/ack/complete/resume.
- PTY open/input/output/resize/close.
- Hot config update.
- Agent update.
- Backup artifact flow.
- Process supervisor operations.
- Network observe, tunnel plan, tunnel test, and later tunnel apply.
- Error/audit report.

## Authorization And Audit

Proof-gated operations:

- Command execution.
- PTY.
- File reads and writes.
- Process/user visibility such as `w`.
- Process supervisor operations.
- Scheduled task management.
- Hot config update.
- Agent update.
- Backup and restore.
- Network observation beyond allowed telemetry.
- Tunnel planning and all network mutations.
- Super password rotation.

Autonomous operations without proof:

- Connection/session establishment.
- Health telemetry allowed by this document.
- Update of last-seen/session metadata.

Audit requirements:

- Record operator, token/session id, target scope, command type, payload hash,
  proof status, server signature id, client result, timestamps, and error reason.
- For rejected operations, record metadata/hash/reason but not local output.
- For bulk operations, record the resolved target list at dispatch time.
- For destructive or bulk operations, require explicit confirmation in panel and
  CLI.

## Data Model

PostgreSQL is the source of truth for:

- Operators, roles, scopes, TOTP state, sessions, refresh tokens.
- Enrollment tokens and client key material metadata.
- Clients, resource pools, tags, and client-tag links.
- Gateway sessions and last-seen status.
- Jobs, job targets, output indexes, and result state.
- Telemetry samples and rollups.
- Audit logs.
- Backup metadata and restore jobs.
- Schedules.
- Process supervisor records.
- Discovery endpoint configuration.
- Tunnel models, IPAM pools, topology edges, observations, and generated plans.

Schema migrations are forward-only for the current release. The compatibility
contract is documented in `docs/migration-compatibility.md` and enforced by
`scripts/audit-migrations.sh`, which checks sequential migration numbering,
per-migration rollback notes, non-destructive DDL, safe `ADD COLUMN NOT NULL`
defaults, and duplicate index names. Downgrade is handled by restoring a
database snapshot that matches the older binary, not by unreviewed reverse SQL.

Object storage is used for:

- Backup artifacts.
- Large file transfer handoff.
- Agent update artifacts.
- Large job output artifacts when configured.

History retention is a first-class control-plane model instead of a hidden
worker constant. The `history_retention_policies` table defines built-in or
operator-updated domains for `telemetry_samples`, `telemetry_rollups`,
`audit_logs`, `job_outputs`, `backup_artifacts`, `network_observations`, and
`topology_history`. Operators can list and update policies through REST,
`vpsctl`, VTY, and the Audit panel, run dry-run previews, perform confirmed
prunes, and export a bounded JSON history bundle. Object-backed domains require
an explicit metadata-only choice or configured object storage so pruning cannot
silently orphan retained blobs. Backup artifact metadata pruning clears the
backup-request link before deletion, and every real prune run writes sanitized
audit metadata.

No real secrets are committed. Config examples must use placeholder values.

## Agent Operations

### Telemetry

Default telemetry:

- 15s lightweight health.
- 60s fuller resource metrics.
- Server-side rollups for older data.

Allowed without proof:

- Uptime.
- CPU load and core count.
- RAM total/available.
- Disk mount capacity/available.
- Network byte counters.
- Latency and connectivity health.
- Agent version.
- OS release.
- Architecture.
- Tags and pool metadata assigned by control plane.

Requires proof:

- Process list and command lines.
- Logged-in users and `w`.
- File metadata outside basic backup plan summaries.
- Service details.
- Network config details beyond BGP-tag import workflows authorized by proof.

### Command Execution

- Default command jobs are explicit argv.
- Optional shell jobs use a configurable absolute argv prefix, defaulting to
  the documented `/bin/sh -lc` preset, and must be visibly labeled as shell
  execution.
- Optional PTY sessions are audited privileged jobs and can be disabled through
  the selected command execution policy for batch-only or restricted agents.
- Jobs have timeout, output limit, cancel, and exit status.
- Output streaming uses bounded buffers and backpressure.
- Agent-executed argv, shell, and PTY child processes run in their own process
  groups by default. Command execution policy presets may select direct-child
  cleanup for constrained environments. Timeout, active task abort, terminal
  close/idle cleanup, and supervisor stop/restart paths use bounded graceful
  termination before forced termination and emit typed cleanup evidence
  (`target_kind`, `target_id`, signals sent, graceful wait, fallback use, final
  running state, and errors) in status output where the operator needs to
  verify cleanup.

### File Transfer

- Chunked resumable transfer.
- Per-chunk hash or rolling verification plus final hash.
- Conservative default rate limits.
- Explicit large-file override.
- Object storage handoff for large artifacts when appropriate.
- All file reads and writes require proof.
- Current bounded implementation supports proof-gated inline and chunked file
  push plus file pull. Real agent sessions stream file-pull stdout chunks with
  backpressure and a rolling SHA-256 status record, avoiding full-file buffering
  on low-memory clients. Backup artifacts reuse this same payload-transfer
  primitive: the backup command emits bounded stdout chunks through the live
  gateway output sink when available and returns only final status metadata,
  while direct local execution keeps the existing bounded chunked-output shape.
  This keeps backup transport inside the file-transfer/object-store model
  instead of adding a special direct-upload protocol. A resumable upload
  protocol slice now exists with
  `file_transfer_start`, `file_transfer_chunk`, `file_transfer_commit`, and
  `file_transfer_abort` commands. The agent persists per-session temp metadata,
  ACKs each chunk with the next offset, accepts idempotent duplicate chunks,
  validates final SHA-256 before atomic rename, and can apply conservative
  per-chunk throttling. The CLI and VTY now include a resumable upload
  orchestrator that resolves and freezes targets once, streams hashing and
  chunks from local disk, submits proof-gated start/chunk/commit jobs, polls
  status ACKs, keeps strict `same-offset` behavior by default, and can use an
  explicit `independent-offsets` policy to group targets by current ACK offset
  and advance resumed sub-batches independently. It prints restartable
  session/resume-token progress events for professional repeated operation.
  The browser Jobs composer now has a limited resumable upload flow
  that reuses local proof generation, target-freezes the resolved client set,
  displays progress, keeps the generated session/resume token in the form for
  restart, verifies ACK offsets through job status output, keeps strict
  `same-offset` behavior by default, exposes the same `independent-offsets`
  policy for resumed multi-target batches, computes the full-file SHA-256 with
  sliced incremental hashing, and reads upload chunks from browser file slices
  instead of buffering the whole source file. Browser resumable upload is now
  capped at the shared protocol ceiling while the dedicated transfer object
  handoff remains future work for local server-side source state. This is not
  full product-complete transfer yet: typed download start/chunk commands
  now exist at the shared protocol, API, and agent layers. CLI/VTY can perform
  proof-gated resumable downloads that write through local `.part` files,
  verify chunk hashes, follow artifact-backed job-output chunks when object
  storage is enabled, verify final SHA-256 before rename, keep `single-target`
  behavior by default, and expose `per-target-files` for pulling the same remote
  path from each resolved VPS into deterministic client-named files under a
  destination directory. The browser Jobs composer now has a limited
  single-target resumable download workflow. The browser path reuses the local
  proof vault, sends only the resume-token hash, polls start/chunk status,
  retrieves retained stdout or object-store-backed chunks, verifies each chunk
  and the final file SHA-256, preserves restart material in the form, and saves
  either through the default verified Blob download path or a selectable
  stream-to-file sink backed by the browser File System Access API. The default
  browser-download sink remains capped to avoid full in-browser buffering;
  stream-to-file can run up to the shared protocol ceiling when the browser
  exposes a writable file handle. Operators can now retain local source files
  as confirmed source artifacts in the server object store before later
  transfer reuse. The API accepts bounded base64 source artifact uploads only
  with explicit confirmation, verifies declared size and SHA-256 against the
  decoded bytes, writes deterministic content-addressed local object keys, and
  records durable source metadata in memory or PostgreSQL. CLI, VTY, and the
  Jobs panel expose source-artifact list/upload/download workflows, and the
  browser computes SHA-256 before submitting while the API remains the final
  verifier. Stored source artifacts can now drive proof-gated resumable upload
  sessions through CLI, VTY, and the Jobs panel: the operator-side workflow
  downloads and verifies retained artifact bytes, then reuses the existing
  start/chunk/commit protocol and per-client proof envelopes without allowing
  the server to fabricate privileged commands. The API now exposes a durable
  transfer-session read model at `/api/v1/file-transfers`, derived from
  persisted job status outputs in memory or PostgreSQL. It merges latest status
  with older start metadata, reports upload/download direction, progress,
  hashes, configured and observed chunk sizes, rate limits, resume state, and
  latest source job, and is visible through CLI, VTY, and the Jobs panel.
  Completed download sessions can be promoted to a server-side handoff object:
  the API assembles retained inline or object-store-backed stdout chunks,
  verifies every chunk hash plus the final file SHA-256, commits a deterministic
  local filesystem object idempotently, and serves a verified artifact endpoint
  for browser, CLI, and VTY download. The Jobs panel supports selecting
  multiple completed download handoffs and saves each verified artifact under a
  deterministic client/session-prefixed filename to avoid same-path collisions
  across a fleet. Local filesystem object-store artifacts stream from verified
  server-side files when possible, and the browser handoff path can use a
  verified File System Access `stream-to-file` sink instead of buffering the
  whole artifact. S3 remains reserved behind the same object store adapter
  contract, with the local disk backend as the current default. The optional
  S3/MinIO adapter uses bounded path-style SigV4 requests, bounded response
  parsing, chunked GET decoding, no-body `HEAD` duplicate checks, and live
  MinIO smoke coverage for backup/update artifacts.
  PostgreSQL live gateway smoke now proves CLI resumable upload/download,
  upload/download policy metadata, verified file bytes, durable
  transfer-session inventory, and API restart persistence. The aggregate
  release gate now covers this local-backend transfer path; future work is
  optional S3 deployment hardening and deeper stress/provider coverage.

### Scheduled Tasks

- Server stores schedules and dispatches jobs.
- Agents do not write cron or systemd timers in the first release.
- Missed schedules are recorded with policy-controlled catch-up behavior.

### Process Supervisor

- First implementation supervises vpsman-launched processes with portable pid
  tracking.
- It supports start, stop, restart, status, logs/output references, and cleanup.
- Supervised processes are launched as their own process group and records store
  `process_group_id` for backward-compatible tree cleanup. Stop/restart first
  terminates that process group, then falls back to single-PID cleanup only for
  legacy records or unusual host behavior, and status output exposes the
  cleanup report.
- Process start supports typed extension fields for restart policy, retry
  budget, backoff, graceful stop timeout, and resource limits. The current agent
  enforces practical per-child Linux limits for address-space memory, process
  count, open files, and `no_new_privileges`. CPU shares are mapped to cgroup v2
  `cpu.weight` when a configured/available cgroup root exposes the CPU
  controller; unsupported hosts keep the process running but report the limit
  as desired-only with the exact reason. Status output includes restart
  attempts, last exit/restart evidence, cgroup path when attached, cgroup
  readback status (`cpu.weight`, process count, memory current, pids current,
  events, and `cpu.stat` when available), and compact limit-effectiveness
  evidence for API/panel inventory.
- Server dispatch uses agent capability snapshots before sending limit-bearing
  process starts. Explicit unprivileged agents, or root agents that report no
  process-limit capability, are completed as `degraded_unprivileged` by default
  unless the operator explicitly sets `force_unprivileged`.
- Host systemd/sysvinit service management is future work unless explicitly
  added later.

### Hot Config And Update

- Config updates are signed, proof-gated, versioned, and rollback-capable.
- Current interim implementation covers proof-gated hot-config updates:
  API/CLI/panel/VTY validate bounded agent TOML, agents reject client identity,
  Noise identity, proof key, and server signing key changes, then atomically
  replace the active config after writing a rollback copy.
- Current interim implementation also covers `artifact_staged_only` agent
  update dispatch: API/CLI/panel/VTY accept confirmed HTTPS artifact URL plus
  SHA-256 and optional detached Ed25519 artifact signature metadata. Agents
  download with bounded memory, hash-verify before writing, require matching
  signatures when a trusted update signing key is pinned in agent config, stage
  the new binary side-by-side, and create a rollback copy without echoing
  artifact URLs, signatures, signing keys, or trust anchors in status output.
- Current interim implementation also covers `manual_update_activation_rollback`:
  API/CLI/panel/VTY accept separate proof-gated activation and rollback
  commands. Activation requires the staged artifact SHA-256, re-hashes the
  side-by-side staged binary before replacing the active binary path, preserves
  a rollback copy, removes the staged file, and reports
  `activated_pending_restart`. It also writes a bounded activation marker next
  to the active binary so the next process start can report post-restart
  evidence. When activation completes, the API moves the matching rollout
  target to `activation_pending_restart` and records sanitized audit metadata.
  Rollback optionally verifies the rollback binary SHA-256 before restoring it,
  removes any pending activation marker, and reports
  `rolled_back_pending_restart`. When a rollback job completes, the API marks
  the matching rollout target `rolled_back` from
  `activation_pending_restart`, `activation_failed`, `heartbeat_timeout`, or
  `heartbeat_verified` and records sanitized
  `agent_update.rollback_completed` audit metadata. Both operations are
  explicit and target-scoped. Activation can optionally request `restart_agent`,
  which
  reports `supervised_exit_requested` and terminates the running agent after
  status delivery so systemd/supervisor can restart the replaced binary; normal
  activation still reports `manual_restart_required`.
- Current rollout persistence is `operator_canary_rollout_control`: accepted
  proof-gated `agent_update` jobs create durable rollout records with frozen
  targets, sanitized artifact hash/signing-key-hash metadata, target progress,
  stored canary count, manual staging activation policy, API/CLI/VTY/panel
  visibility, and restart-safe PostgreSQL persistence. CLI, VTY, and panel
  helpers can promote the next staged batch from a rollout with proof-gated
  `agent_update_activate` and can dispatch rollback to activation-pending or
  activation-failed targets, heartbeat-timeout targets, plus
  heartbeat-verified canaries that need operator rollback. Rollout records
  intentionally do not store or display
  artifact URLs, detached signatures, public signing keys, proof material, or
  trust anchors.
- Current implementation also covers `rollout_policy_presets`: operators can
  create/update named server-side rollout policy presets through API, CLI, VTY,
  and the Jobs panel. Policies match `global`, `tag`, `pool`, or `provider`
  scope, can be limited to a release channel inferred from the registered
  update artifact, carry priority/enabled state, and supply default
  `canary_count` plus `automation_health_gate`. Explicit per-dispatch
  `--canary-count` still wins, and final canary count is clamped to the
  resolved target count. Rollout rows store the applied `rollout_policy_id` and
  `rollout_policy_name` so operators can audit why a provider/tag/pool rollout
  used a specific default.
- Current interim implementation also covers
  `rollout_automation_recommendation_worker`: the durable worker reconciles
  rollout records into `automation_status`, `automation_next_action`,
  `automation_blocker`, `automation_targets`, `automation_updated_at`,
  `automation_paused`, `automation_pause_reason`,
  `automation_health_gate`, `automation_lease_owner`, and
  `automation_lease_expires_at`. It recommends the next canary/batch
  activation target set, flags heartbeat-timeout or activation-failed rollback
  targets, honors operator pause/resume state, supports selectable health gates
  (`heartbeat_verified`, `manual_after_canary`, and `manual_only`), records
  restart-safe worker lease ownership/expiry evidence, writes sanitized
  `agent_update.rollout_automation_reconciled` audit metadata, and the panel,
  CLI, and VTY use the recommended target set for frequent proof-gated
  operator actions. Operators can update rollout automation control through
  API, CLI, VTY, or panel controls without sending super-password proof because
  these controls change only server-side metadata.
- Current interim implementation also covers
  `delegated_rollback_proof_escrow`: an operator can preauthorize the exact
  `agent_update_rollback` payload for rollout targets through API, CLI, VTY, or
  panel. The browser/CLI/VTY derives per-client unsigned proof envelopes
  locally; the API stores only scoped command envelopes, payload hash, expiry,
  target, status, dispatch policy flags such as `force_unprivileged`, and audit
  metadata, never the super password and never raw artifact URLs/signatures/
  trust anchors. The `force_unprivileged` flag is not a server-edited command
  payload; it is an explicit operator dispatch policy that lets the API attempt
  a proof-gated rollback on known normal-user agents instead of preemptively
  degrading the target. The API reconciler may claim these
  ready delegations only after the matching rollout target is classified as
  `heartbeat_timeout` or `activation_failed`, then signs and dispatches the
  frozen rollback command through the normal gateway job path. The server cannot
  alter the command without invalidating the payload hash. This slice is
  automatic rollback escrow only; it is separate from delegated activation,
  proof renewal, or a generalized server-held proof facility.
- Current interim implementation also covers
  `delegated_activation_proof_escrow`: an operator can preauthorize the exact
  `agent_update_activate` payload for a rollout artifact through API, CLI, VTY,
  or panel. Activation delegation stores only per-client scoped envelopes,
  payload hash, staged artifact SHA-256, restart preference, expiry, status,
  dispatch job linkage, dispatch policy flags such as `force_unprivileged`, and
  sanitized audit metadata. As with rollback, `force_unprivileged` is an
  explicit best-effort dispatch policy for known normal-user agents and does not
  let the server rewrite the frozen command payload. The reconciler may claim
  these proofs only when the rollout worker recommends
  `operator_activate_batch`, the client is in the recommended
  `automation_targets`, the target is still `completed`, and the proof's
  staged artifact hash still matches the rollout. CLI, VTY, and panel default
  activation delegation to all rollout targets so future canary/next-batch
  recommendations can proceed without repeated proof entry; explicit client
  selection may narrow the escrow. The server cannot alter the activation
  command without invalidating the payload hash. This slice is proof-delegated
  assisted activation with manual renewal by recording fresh exact envelopes,
  not arbitrary command escrow or a full unattended rollback policy for every
  failure class.
- Current interim implementation also covers `activation_heartbeat_evidence`:
  after manual activation and an operator- or service-manager-triggered agent
  restart, the agent includes the activation marker in its next authenticated
  `AgentHello`. The API ingests that heartbeat, marks the matching rollout
  target `heartbeat_verified`, updates the rollout to `heartbeat_verified`
  when all targets report, and records `agent_update.heartbeat_verified` audit
  metadata without artifact URLs, signatures, signing keys, proof material, or
  trust anchors. Heartbeats can advance only targets that completed staging or
  are activation-pending; failed or pending staging targets are not overwritten.
- Current interim implementation also covers heartbeat timeout reconciliation:
  the API runs a lightweight configurable reconciler
  (`VPSMAN_AGENT_UPDATE_HEARTBEAT_TIMEOUT_SECS`, default 900 seconds;
  `VPSMAN_AGENT_UPDATE_RECONCILE_INTERVAL_SECS`, default 30 seconds), and the
  durable worker runs the same timeout classification when PostgreSQL-backed
  queues are enabled
  (`VPSMAN_WORKER_ROLLOUT_HEARTBEAT_TIMEOUT_SECS`, default 900 seconds). The
  reconciler marks stale `activation_pending_restart` rollout targets as
  `heartbeat_timeout`, updates rollout summary state, and writes sanitized
  `agent_update.heartbeat_timeout` audit metadata. Activation command failures
  mark the matching rollout target `activation_failed`, write sanitized
  `agent_update.activation_failed` audit metadata, and make that target eligible
  for the same delegated rollback claim path. When a matching
  `delegated_rollback_proof_escrow` record exists and is still valid, the API
  reconciler may dispatch the exact preauthorized rollback job. Without a
  recorded delegation, heartbeat timeout or activation failure remains
  classification and evidence only.
- Current interim implementation also covers
  `release_registry_metadata_only`: operators can record signed update release
  metadata through API/CLI/VTY/panel, the API verifies detached signatures,
  stores only hashes of artifact URLs/signatures/signing keys, stores optional
  rollback-bundle hash/signature/key/URL hashes, audits the write, exposes
  latest-by-name/channel lookup, enforces configured release channel and
  signing-key allow-lists when
  `VPSMAN_AGENT_UPDATE_ALLOWED_CHANNELS` or
  `VPSMAN_AGENT_UPDATE_TRUSTED_SIGNING_KEYS_HEX` are set, and can reject
  unregistered staged updates before gateway dispatch when
  `VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES=1`.
- Current interim implementation also covers
  `hosted_update_artifact_streaming_bounded`:
  when the local `VPSMAN_UPDATE_OBJECT_STORE_DIR` backend is configured,
  operators can upload a bounded signed update artifact through API/CLI/VTY/
  panel. Existing JSON/base64 upload remains a compatibility path for small
  artifacts; the production path streams raw `application/octet-stream` bodies
  to `/api/v1/agent-update-artifacts/stream`, hashes while writing a temp file,
  verifies detached signatures before object-store commit, and records releases
  from hosted primary and rollback artifact hashes through
  `/api/v1/agent-update-releases/hosted`. Explicit `VPSMAN_UPDATE_OBJECT_*`
  S3/MinIO settings remain available as an optional adapter path when no local
  update object-store directory is configured. The API computes the artifact
  SHA-256,
  verifies the detached Ed25519 signature against that hash, stores bytes under
  a content-addressed object key, records a hosted release without raw artifact
  bytes/signatures/signing keys/full URLs, audits the upload, and serves the
  primary artifact and optional rollback artifact from
  `/api/v1/agent-update-artifacts/{sha256}` after re-checking object hash and
  size. When `VPSMAN_UPDATE_ARTIFACT_PUBLIC_BASE_URL` is configured with an
  HTTPS base URL, release views include computed public download URLs for
  operator handoff without storing those URLs in audit metadata. The optional
  S3 path uses bounded path-style SigV4 and has a dedicated opt-in live MinIO
  smoke. Agents still require an HTTPS URL, so production use should expose
  hosted paths behind HTTPS before dispatching `agent_update`.
- Server dispatch resolves agent capability snapshots before sending direct
  update staging, activation, or rollback host mutations. Explicit
  unprivileged agents, or root agents that report no privileged host-mutation
  capability, are completed as `degraded_unprivileged` by default unless the
  operator explicitly sets `force_unprivileged` through the API, CLI, VTY, or
  panel. The same policy applies to direct rollout activation/rollback and to
  delegated activation/rollback dispatch.
- Full autonomous agent update remains future work: full release-policy
  automation, automated proof-renewal policy, broader automatic rollback policy
  beyond heartbeat-timeout and activation-failure escrow, and richer
  restart-health policy are not yet complete. Streaming hosted release
  ingestion, basic configured channel allow-lists, basic trusted signing-key
  allow-lists, pause/resume, selectable health gates,
  restart-safe worker lease evidence, delegated activation proof escrow,
  delegated rollback proof escrow, explicit expiry status, rollout read-model
  summaries, panel renewal actions, latest channel lookup, public hosted
  download URL derivation, and rollback bundles exist, but they are not a
  substitute for full autonomous rollout orchestration.

## Installation And Runtime

Installer requirements:

- One-line root installer.
- Downloads static agent binary.
- Generates client keypair locally.
- Uses short-lived enrollment token.
- Stores server identity and client config.
- Installs autostart service.
- Supports systemd first with documented fallback/manual path for non-systemd.

Current implementation status:

- `scripts/install-agent.sh` installs or stages the static agent binary,
  writes an enrolled config with `0600` permissions, and refuses accidental
  config replacement unless a backup-producing force flag is set. Root mode is
  the default and renders both systemd and Debian-style init fallback assets
  under the normal system paths. Explicit unprivileged mode uses
  `VPSMAN_SERVICE_HOME`/`HOME` defaults for per-user install, config, state,
  and log paths, renders only a user systemd unit, and does not write root
  autostart assets. The installer automatically verifies rendered service
  assets for the selected install mode and, when installing live on `/` without
  skip-service mode, verifies the started service through the detected service
  manager. This verification is deliberately minimal and automatic; operators
  should not need to choose among service-manager implementation details.
- Production enrollment can be completed before install by `vpsctl
  enroll-config`, or on the target VPS by giving the installer a short-lived
  `VPSMAN_ENROLLMENT_TOKEN` plus a static transient `vpsctl` helper. The
  target-side path generates the client Noise keypair and proof key locally,
  consumes the enrollment token, writes the enrolled config, and then removes
  the helper.
- Rebuilt-client re-enrollment is part of the design, not a separate migration
  workaround. A rebuilt VPS may claim a fresh short-lived
  `rebuild_reenrollment` token while reusing the same stable `client_id`; the
  server rotates the enrolled Noise public key and updates display metadata
  while preserving server-side state keyed by `client_id`: resource-pool
  membership, custom tags, backup/restore relationships, migration links,
  tunnel plans, schedules, rollout history, and audit history. Normal
  provisioning tokens fail closed when they would replace an existing client
  key. Rebuild tokens must be bound to an allowed existing client id, require
  explicit operator confirmation at token creation, store the expected old
  public-key SHA-256 fingerprint, and reject claims if the current server-side
  key has changed since token issuance. API, Access panel, `vpsctl`, and VTY
  expose this workflow, and audit metadata records purpose, allowed client id,
  expected old-key fingerprint, and new-key fingerprint without storing private
  keys. Current-key revocation is a separate key-lifecycle workflow: operators
  can revoke the enrolled public-key fingerprint for a client, gateway identity
  validation rejects that key, audit records the reason, and a confirmed rebuild
  token can later rotate the same stable `client_id` to a new key while the old
  revocation record remains visible. Current implementation proves key rotation
  plus pool/tag preservation in memory and PostgreSQL, including post-restart
  verification, explicit current-key revocation, lifecycle reports, and
  rejection of old or revoked public keys after rotation. Remaining policy
  extensions are optional old-key proof from the rebuilt host, richer
  multi-operator approval, proactive client/server key rotation, and
  migration-plan binding.
- The transient `vpsctl` helper supports both `https://` API URLs with WebPKI
  roots and `http://` URLs for local/development deployments.
- URL-downloaded agent and `vpsctl` helper binaries require SHA-256 hash pins;
  copied local binaries can also be hash-verified before installation.
- Managed install and config directories must be absolute and cannot be `/`.
- Uninstall is explicit through `VPSMAN_UNINSTALL=1`; it removes the agent
  binary and the autostart assets for the selected install mode while
  preserving config, state, and logs by default. `VPSMAN_PURGE_CONFIG=1`
  additionally removes vpsman-owned config, state, and log paths.
- Installer post-release hardening: signed release metadata beyond SHA-256 hash
  pinning, live HTTPS enrollment smoke coverage with trusted certificates, and
  real distro VM/service-manager boot, enable, restart, disable, reboot, and
  uninstall tests across the supported matrix. The current release gate covers
  staged Debian/Ubuntu install assets, root/unprivileged config rendering,
  hash checks, explicit uninstall/purge behavior, static-agent resource
  budgets, and reconnect behavior.
- Development `dev_xx` config generation remains available only through an
  explicit opt-in flag for local testing.
- Unprivileged install/run mode is allowed as an explicit operator choice. It
  uses the same enrollment, Noise, proof, telemetry, and audit model, while the
  agent reports reduced capabilities and treats root-only network/update/
  restore/process-limit operations as ineffective/fail-closed by default unless
  the operator requests the existing proof-gated `force_unprivileged`
  best-effort path.

Build requirements:

- Use Rust toolchain from project/user environment.
- Do not install build software through `apt`.
- Static musl builds for `x86_64-unknown-linux-musl` and
  `aarch64-unknown-linux-musl` must pass.
- Static musl `vpsctl` builds for those same targets must pass because the
  target-side installer enrollment path uses `vpsctl` as a transient helper.
- Avoid dependencies that require unavailable C cross toolchains for agent
  targets.

## Groups, Bulk Operations, And UI Semantics

Grouping model:

- Provider/resource-pool parent node.
- Custom tags as independent labels.
- Explicit client selections.

Bulk operations:

- Can target pool, tag, or explicit list.
- Tags support explicit `any`/`all` matching; operators choose the data-source
  or job target expression instead of relying on hidden defaults.
- Must resolve target set at dispatch time.
- Must show confirmation with target count and operation hash.
- Must produce per-client result and audit records.

## BGP, Tunnel, And Bird2 Design

### Scope

Design change on 2026-06-01: agent-managed runtime tunnels are now the preferred
network architecture. The server stores desired topology, endpoint policy,
bandwidth/latency preference, OSPF cost policy, and custom tunnel adapters. When
an agent comes online it should reconcile its local runtime tunnel state to the
current desired topology using `ip link`, `ip tunnel`, `ip fou`, or configured
absolute-argv adapter commands. Tunnels are therefore created, updated, removed,
and cost-adjusted by the client while it is connected, not primarily by boot-time
ifupdown, netplan, NetworkManager, or systemd-networkd ownership. If the agent
is offline, no local tunnel mutation is attempted; the next authenticated
online session must converge the tunnel state and report evidence.

The first BGP/tunnel milestones remain observe/import/render plan plus bounded
proof-gated apply paths, but the permanent product direction is a runtime
overlay reconciler:

- `agent_iproute2_managed`: vpsman owns the tunnel interface lifecycle at
  runtime through typed idempotent operations, without writing distribution
  network-manager files by default.
- `external_observed`: vpsman imports and observes existing OpenVPN, WireGuard,
  TUN/TAP, or custom externally launched interfaces without taking ownership.
- `external_managed_adapter`: vpsman can call operator-configured absolute argv
  adapters for start/stop/status/probe when proof-gated and bounded.
- `legacy_persistent_backend`: ifupdown/netplan/systemd-networkd rendering is
  compatibility and migration support, useful for old hosts or operators who
  explicitly want persisted distro-manager files, but it is no longer the
  default future tunnel-management model.

Server-authored desired state can customize each managed tunnel's realization
strategy. For built-in agent-managed tunnels the client should translate desired
state into idempotent `ip link`, `ip tunnel`, `ip fou`, address/route, and
traffic-shaping actions. For custom/external tunnel types the server can attach
a typed adapter contract with absolute-argv startup, stop, cleanup, restart,
status, and traffic-limit apply commands, bounded timeouts, bounded captured
output, and placeholder expansion for interface/topology values. This covers
OpenVPN, custom TUN/TAP daemons, provider-specific tunnels, and future tunnel
families without forcing them into distro network-manager files. Adapter
execution is not shell based, is privileged only with proof, and reports command
evidence plus drift instead of treating "command dispatched" as success.
FOU realization parameters are part of the typed runtime tunnel model, not
hidden constants: the default Linux preset uses port `5555`, peer port `5555`,
and IP protocol `4`, while saved FOU plans may carry non-default port,
peer-port, and protocol values through API, panel, CLI, VTY, compatibility
backend rendering, and agent runtime command generation.

Runtime tunnel reconciliation must be typed and idempotent. Each converge cycle
freezes the desired topology version, validates endpoint side and proof scope,
compares existing links/routes/addresses/FOU state with desired state, applies
only the minimum delta, reports per-step evidence, and records drift or adapter
failure without inventing success. Apply must never silently mutate host
networking: privileged mutations still require local super-password proof,
explicit destructive confirmation or a policy-approved reconciler action,
agent config opt-in, bounded commands, and sanitized audit records.

Clients are also responsible for ongoing tunnel monitoring. Each client reports
observed tunnel inventory, ownership mode, link state, address/route state,
latency, loss, throughput evidence, adapter health, and drift against the
desired topology. The server-side topology graph is built from desired topology
plus these client tunnel reports. Bird2 configuration and OSPF cost mutation are
downstream routing-layer actions over that tunnel-management substrate: first
the tunnel exists and is measured, then Bird2 interface snippets/costs are
planned, applied, monitored, and rolled back. The product model is therefore
coherent: desired tunnel topology -> client runtime convergence and monitoring
-> topology evidence -> Bird2/OSPF policy updates.

The existing managed-file path remains implemented as compatibility. Agent-side
validation/reload hooks are available as explicit opt-in argv hooks with bounded
timeouts and rollback-on-hook-failure. Curated validation/reload presets exist
for Debian/Ubuntu-style networking with Bird2. `debian_ifupdown2_bird2` expands
to absolute argv commands for `ifreload -a -s`, `bird -p -c
/etc/bird/bird.conf`, `ifreload -a`, `birdc configure`, and rollback-time
`ifdown -f {interface}`. The legacy `debian_ifupdown_bird2` preset uses
`ifup --no-act {interface}`, `bird -p`, `ifdown -f {interface}`, `ifup
{interface}`, `birdc configure`, and rollback-time `ifdown -f {interface}`.
Neither preset invokes a shell. Netplan and systemd-networkd managed-file
renderers and proof hashes exist for compatibility, but new feature work should
prefer the agent runtime reconciler unless persistence through the host network
manager is explicitly requested.

Operator-triggered rollback exists for the bounded managed-file path: it removes
the exact vpsman-managed blocks for one endpoint side, backs up changed files,
and runs preset pre-rollback teardown before removal where configured, then runs
validation/reload hooks before accepting the rollback. Automated managed-file
status inspection also exists: `network_status` reports current managed backend
and Bird2 file existence, hashes, expected block presence, drift, and malformed
managed-block state for one endpoint side. It also performs a read-only runtime
check for the planned interface through sysfs under the agent-configured root
and discovers observed tunnel-like runtime interfaces. The status payload
distinguishes desired interfaces, declared stale interfaces that are still
present, and unrelated observed tunnels that are safe external-import
candidates rather than automatic failures. It can run an optional absolute-argv
Bird2 status probe with `{interface}` placeholder expansion, bounded output,
and heuristic parsing of common Bird2 OSPF neighbor-state output into
interface/full-neighbor health signals.
`network_probe` adds a proof-gated, read-only, bounded ICMP latency probe to the
peer tunnel address for one endpoint side; it reports parsed loss/latency plus
hashed/truncated stdout/stderr. `network_speed_test` is a separate proof-gated,
two-endpoint TCP throughput probe: the server-side agent listens on the planned
tunnel address/port while the peer agent sends bounded data under duration,
byte, connect-timeout, and rate-limit budgets, and both sides report status
metrics without shelling out. The Topology panel now has evidence drilldown plus
an API-backed applied-topology graph with search, health filtering,
deterministic dense-node layout for larger 20+ VPS views, endpoint health,
trend metrics, and selected-node inspection. The topology create-plan workflow
now exposes runtime owner selection, external/custom tunnel kinds, adapter argv,
traffic limits, and desired/stale topology intent. Deeper topology edit
workflows, adapter health/drift automation, and longer-retention analytics
remain later milestones.

Current runtime-reconciler slice: agents have an opt-in
`runtime_reconcile_enabled` path that runs during proof-gated `network_apply`
before compatibility managed-file mutation. It builds typed iproute2 commands
for link inspection, GRE/IPIP/SIT/FOU tunnel add/change, FOU setup, address
replacement, link-up, explicit stale route/interface cleanup, declared route
replacement, and `tc` ingress/egress traffic limits. Runtime topology intent now
travels on the tunnel plan with a bounded optional version, desired interface
set, explicit stale interfaces, declared routes, and stale routes. The API
validates this metadata before dispatch. It also executes bounded external
adapter startup/restart/status/traffic-limit commands with placeholder
expansion from the plan and endpoint, and `network_rollback` now performs a
runtime remove step: iproute2-managed tunnels delete declared routes and the
current link when present, while external managed adapters can run a bounded
stop command plus status. Every command is absolute argv, time/output bounded,
and reported as step evidence. If the agent is unprivileged, mutating runtime
steps are skipped and reported as `degraded_unprivileged` rather than silently
claimed unless the operator/config deliberately selects a bounded best-effort
policy. The current agent network policy is
`runtime_unprivileged_mutation_policy`: `skip` is the safe default,
`try_external_adapters` allows custom adapter commands that can run under a
normal user, and `try_all` is reserved for explicit lab/provider cases. Server
dispatch now also uses agent capability snapshots to avoid sending default
root-only network mutations to explicit unprivileged targets unless
`force_unprivileged` is set, including scheduled approval dispatch.
FOU add/delete and tunnel encapsulation now read the plan's typed FOU options,
so compatibility snippets, systemd-networkd previews, agent commands, CLI/VTY,
and the panel remain aligned when an operator chooses non-default port/protocol
values.
Runtime reconcile now stops after a failed required mutating step and records
bounded compensation evidence: for newly created iproute2-managed links it
attempts best-effort link deletion, and for external adapters it runs the
configured stop command when available. Existing links are preserved rather
than guessed back to an unknown prior state. Bird2 routing writes are now
gated when `runtime_reconcile_enabled=true`: `network_apply` only proceeds
from converged runtime evidence, from observed-only plans with the interface
already present, or from an explicit `allow_routing_without_runtime_ready`
compatibility override that records `degraded_allowed` evidence.
`network_ospf_cost_update` likewise requires the planned interface to exist in
sysfs before writing Bird2 cost changes unless the same override is enabled.
Status-discovered tunnel-like interfaces are read-only import candidates unless
the operator explicitly promotes them into an observed/adapter plan or declares
them as stale topology intent; runtime reconcile deletes only explicitly
declared stale interfaces. Continuous telemetry now carries tunnel
`mutation_policy` and `promotion_required` fields so frequent-use fleet views
can show import candidates without requiring an explicit proof-gated status job.
Telemetry tunnel readback also correlates observed interfaces with saved tunnel
plans. When an observed interface matches a saved endpoint-side plan, the API
returns `matched_saved_plan` metadata with the plan id/name, runtime manager,
endpoint side, peer client id, and `promotion_required=false`. Agent-managed
and adapter-managed matches use `managed_desired`; saved non-mutating external
observed plans use `observe_only_saved_plan`. This prevents approved
runtime/adapter tunnels from remaining in the import-candidate workflow after
the desired plan exists without flattening external-observed imports into
managed host control.
Approved external adapter status telemetry and selectable traffic accumulation
are now supported through local agent configuration.
`[network].runtime_status_telemetry_plans` stores a bounded list of
endpoint-side tunnel plans with explicit `external_managed_adapter` status
commands and a selected traffic source. Config validation requires absolute
argv, planner-validated runtime control, a status command, at most 16 plans,
and a bounded command when the selected traffic source is `custom_command` or
an explicit `vnstat` command. Interface counters are the default selected
source, `vnstat` is a configurable preset source when selected, and custom
JSON-producing absolute-argv sources are supported for provider/application
accounting. The agent runs these checks no more often than
`runtime_status_telemetry_interval_secs`, caches health between light telemetry
ticks, and reports selected traffic source/status plus only redacted
command/output health hashes, exit status, duration, timeout/output-limit flags,
and stable reasons. This is the approved continuous adapter command-health and
traffic-accounting path; arbitrary server-supplied commands are still never
executed as autonomous telemetry.
Proof-gated latency probes also use a selectable command source:
`[network].probe_ping_argv` can provide a custom absolute base argv, otherwise
the agent uses documented Linux ping preset candidates (`/bin/ping`,
`/usr/bin/ping`). Probe status records whether the configured source or preset
was used, plus a command hash and bounded output hashes.
The first promotion workflow is implemented through
`POST /api/v1/tunnel-plans/promote-telemetry`, `vpsctl
tunnel-promote-telemetry`, VTY `tunnel-promote-telemetry`, and the Topology
panel's Tunnel promotion form. It accepts only telemetry records marked
`observe_only_import_candidate` with `promotion_required=true`, combines them
with operator-supplied peer/underlay/IPAM context, saves a non-mutating
`external_observed` plan, records an audit row, and refreshes topology read
models. The topology graph also performs a server-side no-op drift pass: saved
plans whose endpoints are not currently connected are marked as
`convergence_blocked`, list their offline endpoint ids, and carry
`server_drift_reasons` without attempting any host mutation. This is still a
Saved observed tunnel plans can now be promoted into managed adapter contracts
without changing the plan id. `POST /api/v1/tunnel-plans/promote-adapter`, the
Topology panel's Adapter contract form, `vpsctl tunnel-promote-adapter`, and
VTY `tunnel-promote-adapter` require operator network write scope, explicit
confirmation, an existing `external_observed` plan,
`external_managed_adapter` runtime control, and a bounded status command.
Promotion resets endpoint apply state, preserves created-at/id history, writes
a sanitized `network.tunnel_plan_promoted_to_adapter` audit row, and leaves
actual host mutation proof-gated through later apply/rollback workflows.
External managed adapter remove semantics now support stop plus optional
cleanup commands before status verification; missing stop/cleanup is reported
as `remove_unavailable` rather than implied success. Remaining network
hardening gaps must be tracked in public issues or release notes rather than
agent-local planning files.
The PostgreSQL-backed live network apply smoke proves the current custom
adapter lifecycle through a real API/gateway/agent stack: a saved
`external_observed` OpenVPN-style plan is promoted into a managed adapter
contract, then proof-gated `tunnel-apply` and `tunnel-rollback` execute
startup, status, traffic-limit, stop, and cleanup adapter commands under
`try_external_adapters` best-effort unprivileged policy, write/remove only the
Bird2 managed block, redact proof material, update tunnel state, emit audit
evidence, and survive API restart.

### Tunnel Management Modes

- Primary future backend: agent-managed runtime overlay using typed `iproute2`
  operations and optional adapter commands. It converges only while clients are
  online and reports drift when a client is absent.
- Compatibility persistent backends: ifupdown, netplan, and systemd-networkd
  managed-file renderers. They write only vpsman-managed blocks under the
  agent-configured root after proof/hash verification and are not the default
  desired architecture for new tunnel work.
- Routing daemon: Bird2 first. Bird2 file ownership remains dedicated include
  files only until the runtime reconciler gains a typed routing-daemon adapter
  model.
- Legacy import: read existing `/etc/network/interfaces`, interfaces.d snippets,
  netplan/networkd evidence where available, runtime links, OpenVPN/TUN/TAP
  interfaces, and Bird2 config without assuming full ownership.
- Custom tunnel import: external tunnels can be represented as observed-only
  topology edges first, then optionally upgraded to managed adapters with
  explicit proof-gated start/stop/status/probe command contracts. API,
  `vpsctl`, VTY, and the topology panel can author OpenVPN, WireGuard, TUN/TAP,
  and custom tunnel plans without generating host network-manager files when
  the runtime owner is external observed or external adapter. Telemetry import
  candidates can also be promoted into saved `external_observed` plans through
  API, panel, CLI, and VTY after the operator supplies the missing peer and
  underlay context.

### Tunnel Model

Initial tunnel types:

- GRE.
- IPIP.
- SIT.
- FOU.

Topology data:

- Endpoint clients.
- Tunnel kind.
- Underlay endpoint addresses.
- Tunnel addresses from panel IPAM.
- Bandwidth tier: 10M, 100M, 1000M.
- Latency, jitter, loss, speed-test observations.
- Operator preference.
- Recommended and applied OSPF cost.

### OSPF Cost

The formula must account for:

- Latency.
- Packet loss.
- Bandwidth tier.
- Operator preference.
- Min/max clamps.
- Manual override.

Higher bandwidth is preferred when latency is tolerable. Cost recommendations
must be visible before apply.

### Legacy Compatibility

Legacy Bird2 graph compatibility exists for migration only. It should import:

- Router ID as node identity.
- `type ptp` OSPF interfaces.
- Interface names such as `YpeerX`.
- Existing costs.
- Existing peer relationships.

Do not make the legacy design the long-term architecture.

## Backups, Migration, And Restore

Backup scope:

- Control-plane database/config.
- Client config.
- Selected paths.
- vpsman-managed process state.
- Managed network snippets.
- Bird2 include files managed by vpsman.
- Restore metadata.

Backup encryption:

- Client encrypts artifacts to panel public key before upload.
- Object storage is not trusted with plaintext backup contents.

Restore and migration:

- Restore selected paths.
- Restore agent config.
- Re-enroll rebuilt VPSs.
- Recreate managed snippets after explicit proof-gated restore.
- Keep job/audit history for migration decisions.

## Current Implementation Gap

As of the current phase 0 scaffold plus the first transport correction:

- Runtime agent/gateway traffic now uses a Noise-protected frame stream. The
  code supports development XX mode and explicit enrolled IK mode with pinned
  server public key plus expected client public key. Active sessions enforce
  per-stream monotonic sequence checks after decryption, and tests now include a
  wire-smoke proving raw TCP-side Noise bytes do not expose TLV magic or a known
  plaintext payload. Production enrollment now includes installer-token
  enrollment, persistent identity allowlists, database lookup of client keys,
  rebuilt-client re-enrollment, and current-key revocation reports. Remaining
  production identity hardening is proactive server/client key rotation,
  stronger multi-operator approval policy, and durable command
  idempotency/cross-session replay policy.
- Privileged proof and command-envelope authorization helpers exist, including
  payload-hash, scope, expiry, server-signature, and active replay checks. In
  PostgreSQL mode, rejected job/audit records are attributed to the
  authenticated operator and bearer session. Rejected jobs freeze the resolved
  explicit-client, pool, and tag target set into per-client job-target records
  after enforcing destructive confirmation metadata, and authenticated
  API/CLI/panel history views can list persisted jobs, target results, and audit
  logs. The agent verifies command envelopes against locally stored per-client
  proof material plus server Ed25519 public key, rejects replay/missing auth,
  and executes bounded argv, bounded shell-script wrapper, and noninteractive
  PTY-backed argv commands received over the Noise command stream. The gateway
  has an internal
  `POST /internal/v1/gateway/command`
  control route on `VPSMAN_GATEWAY_CONTROL_BIND`, protected by
  `VPSMAN_INTERNAL_TOKEN` when configured, that routes signed `JobRequest`
  frames to connected agents over Noise and returns ack/output JSON. The API can
  now accept proof-bearing command envelopes, validate their target scope/hash
  shape, sign them with `VPSMAN_SERVER_SIGNING_KEY_HEX`, forward through
  `VPSMAN_GATEWAY_CONTROL_URL`, and persist per-target completed/failed dispatch
  state. The browser Jobs view can now resolve explicit clients, pools, and
  tags, derive local WebCrypto proof envelopes for each resolved client, and
  submit proof-gated bounded argv jobs, noninteractive PTY-backed argv jobs,
  bounded shell-script wrapper jobs,
  bounded file-pull jobs, bounded inline file-push jobs, and user-session
  visibility plus process-snapshot jobs
  without sending the super password to the server.
  A live memory-mode API/gateway/agent smoke verifies a proof-gated
  `argv=["true"]` job completes with exit code 0. A PostgreSQL-backed
  API/gateway/agent smoke now verifies compiled-CLI argv shell execution,
  noninteractive PTY-backed argv execution, shell-script wrapper execution,
  file pull, timed-out shell-job persistence,
  fail-closed no-proof user-session
  visibility, proof-gated user-session visibility, process supervisor
  start/status/logs/restart/stop, inline/chunked file push, persisted
  target/output/audit state, and API restart persistence through the real
  Noise-over-TCP gateway. It also exposes a durable
  process-supervisor inventory read model derived from persisted job outputs,
  available through REST, the Jobs panel, `vpsctl process-supervisor-inventory`,
  and VTY `process-supervisor-inventory`. Pending scheduled/approval jobs can
  now be canceled through REST, the Jobs panel, `vpsctl job-cancel`, and VTY
  `job-cancel`, with target rows moved to `canceled`, audit records written,
  and later scheduled dispatch blocked. Active in-flight command cancellation
  is implemented for gateway-dispatched agent jobs through a dedicated
  `CommandCancel` TLV, API `cancel_requested` lifecycle, gateway control
  route, agent task abort, typed `command_canceled` status output, CLI/VTY
  `job-cancel`, and Jobs-panel active-cancel control. Shell argv and
  shell-script jobs run in isolated process groups so timeout/drop paths can
  terminate child processes rather than only the immediate shell. Large
  non-status stdout/stderr chunks are now retained as object-store artifacts
  when local filesystem-backed `VPSMAN_BACKUP_OBJECT_STORE_DIR` storage is
  configured. Optional S3/MinIO object storage remains an adapter extension.
  Retained artifacts include SHA-256/size metadata in `job_outputs`, REST
  artifact
  download, `vpsctl job-output-artifact`, VTY `job-output-artifact`, Jobs-panel
  download action, and PostgreSQL restart smoke coverage. The gateway also
  forwards each command-output frame to
  `POST /internal/v1/gateway/command-output`; the API records chunks
  idempotently by `(job_id, client_id, seq)`, publishes WebSocket
  `job_output_recorded` and `job_finished` events, and keeps final dispatch
  result persistence as a fallback for older or disconnected gateway/API
  pairings. Non-PTY shell argv and shell-script stdout/stderr are streamed from
  the agent to the gateway/API as bounded chunks before process exit, with the
  final status output still sent last. Noninteractive PTY-backed argv jobs are
  accepted as proof-gated `shell_pty` jobs, run against a Linux PTY, and retain
  output on the `pty` stream. A first terminal-control contract now exists:
  shared `terminal_open`, `terminal_input`, `terminal_poll`,
  `terminal_resize`, and `terminal_close` operations carry session id,
  argv/cwd, dimensions, reconnect replay cursor, idle timeout, flow window,
  sequenced input, and close reason. The API validates those payloads,
  browser/CLI/VTY can submit them with local proof envelopes, and current
  agents keep a bounded PTY session registry: `terminal_open` spawns or
  attaches to an argv-backed PTY session, `terminal_input` writes sequenced
  base64 input and returns buffered PTY output, `terminal_poll` returns
  buffered PTY output without writing input, `terminal_resize` updates the PTY
  window size, and `terminal_close` kills/removes the session. Idle PTY output
  also streams continuously through `TerminalStreamOutput` frames on the same
  Noise TCP transport; the agent keeps bounded retained output, coalesces
  short bursts, reports dropped/truncated replay state, and emits final status
  on exit/close/idle timeout. The API now exposes a durable terminal-session
  read model at `/api/v1/terminal-sessions`, derived from persisted terminal
  status outputs in memory or PostgreSQL. It deduplicates by client/session,
  merges latest state with older open metadata, and reports state, argv/cwd,
  dimensions, idle/flow limits, output sequence cursors, input sequence,
  close reason, exit evidence, and source job through API, CLI, VTY, and the
  Jobs panel. The API also exposes durable output replay at
  `/api/v1/terminal-sessions/{client_id}/{session_id}/replay`, reconstructed
  from persisted PTY job-output chunks and able to hydrate object-store-backed
  chunks with size/SHA-256 verification. CLI `terminal-replay`, VTY
  `terminal-replay`, and the Jobs panel Replay action give operators a
  recoverable console-history path beyond the agent's bounded in-memory PTY
  ring. CLI `terminal-follow`, VTY `terminal-follow`, and the Jobs panel live
  Follow action fetch replay deltas after terminal-specific WebSocket metadata
  events instead of opening a separate browser-agent socket. The Jobs panel
  also turns retained session rows into prepared
  attach/replay, poll, input, resize, or close composer actions with the
  session id, target VPS, argv/window defaults, replay cursor, and next input
  sequence prefilled. PostgreSQL live gateway smoke coverage includes
  open/resize/input/attach-replay/poll/close, PTY output, durable
  terminal-session inventory, and API restart persistence. Remaining command
  gaps include retention-policy tuning for very long terminal histories,
  optional ack-based terminal flow control beyond the bounded queue/replay
  model,
  disconnect/reconnect idempotency for long-running commands, and advanced
  async timeout orchestration beyond synchronous dispatch. `vpsctl job-follow`
  and VTY `job-follow` provide a headless bridge over retained output by
  polling the API, decoding stdout/stderr/PTY/status chunks, and stopping on
  terminal job status; terminal-specific follow is now handled by
  `terminal-follow` and the Jobs-panel live Follow path.
- The API has an optional PostgreSQL repository path for fleet summary, agent
  inventory, rejected job records, and audit records when `VPSMAN_POSTGRES_URL`
  is set. The gateway can optionally forward decrypted agent hello/telemetry to
  the API, protected by a shared internal bearer token when
  `VPSMAN_INTERNAL_TOKEN` is configured; the API can persist client status/tags,
  resource pools, raw telemetry samples, and rejected jobs/audits. Pool/tag
  hierarchy views, client-to-pool assignment, tag assignment, and bulk target
  resolution exist, including destructive confirmation-required metadata.
  PostgreSQL mode now supports first-operator bootstrap, Argon2id password
  hashing, opaque hashed bearer sessions, refresh tokens, and authenticated
  REST/WebSocket fleet access. A bounded role/scope/session lifecycle model is
  now implemented: operators have `admin`, `operator`, or `viewer` roles plus
  normalized scopes; admins default to `*`, operators default to scoped write
  permissions such as `jobs:write`, and viewers default to `fleet:read`. Only
  admins can create/list operator records, list operator sessions, and revoke
  sessions through REST, the Access panel, `vpsctl`, and VTY. Read routes for
  fleet/job/audit/history/topology state require `fleet:read`; write routes now
  require at least `operator` plus the matching scope: `jobs:write`,
  `inventory:write`, `schedules:write`, `backups:write`, or `network:write`.
  Enrollment token and client-key revocation administration are treated as
  inventory write access. The
  live `vpsctl` smoke proves admin-created viewer login, sanitized
  operator/scopes output, fail-closed viewer job dispatch denial, admin session
  revocation, revoked-token rejection, and an operator scoped only to
  `fleet:read` being allowed to read fleet summary and computed fleet alerts
  while failing closed on pool creation and job dispatch with
  `operator_scope_insufficient`. The alert read model is exposed as
  `/api/v1/fleet-alerts`, `vpsctl fleet-alerts`, VTY `fleet-alerts`, and the
  Fleet panel. It derives current read-only alerts from agent connection
  status, configurable telemetry resource threshold policy, runtime tunnel
  health/traffic, data-source readiness, failed backup/restore/update work, and
  unprivileged-degraded targets. Fleet-wide resource defaults are configured at
  API startup through `VPSMAN_ALERT_MEMORY_AVAILABLE_WARNING_RATIO`,
  `VPSMAN_ALERT_MEMORY_AVAILABLE_CRITICAL_RATIO`,
  `VPSMAN_ALERT_DISK_AVAILABLE_WARNING_RATIO`,
  `VPSMAN_ALERT_DISK_AVAILABLE_CRITICAL_RATIO`,
  `VPSMAN_ALERT_CPU_LOAD_WARNING`, and `VPSMAN_ALERT_CPU_LOAD_CRITICAL`, and
  fired resource-alert evidence includes the threshold that matched. Operators
  can also define durable scoped alert policies through
  `/api/v1/fleet-alert-policies`, `vpsctl fleet-alert-policies`,
  `vpsctl fleet-alert-policy-upsert`, VTY equivalents, and the Fleet panel.
  Policies can target global, provider, pool, tag, or client scopes, cascade
  from broad to specific matches, and audit each upsert. A `fleet:read` scoped
  operator can list alert policies but cannot modify them without
  `inventory:write`. Alert triage state is durable and action-oriented:
  `/api/v1/fleet-alert-states`, `vpsctl fleet-alert-state-update`,
  `vpsctl fleet-alert-states`, VTY equivalents, and Fleet panel row actions can
  acknowledge, mute with an expiry, escalate, or clear computed alerts. The
  alert list merges this state, hides muted alerts by default unless requested,
  and `/api/v1/fleet-alerts/export` plus `vpsctl fleet-alert-export` provide a
  filtered JSON export for incident handoff and review. Alert notification
  delivery now uses durable, scope-aware notification channels and an outbox:
  `/api/v1/fleet-alert-notification-channels`,
  `/api/v1/fleet-alert-notifications`, and
  `/api/v1/fleet-alert-notifications/dispatch`, plus matching `vpsctl`, VTY,
  and Fleet panel controls, support global/provider/pool/tag/client channel
  presets, category/operator-state filters, minimum severity, delivery kind,
  target, cooldown dedupe, dry-run matching, audit-log delivery, and queued
  adapter records for custom delivery kinds such as webhook or external
  command adapters. Queued delivery processing now has a bounded
  operator-triggered executor through
  `/api/v1/fleet-alert-notifications/process`,
  `vpsctl fleet-alert-notification-process`, matching VTY, and the Fleet panel:
  dry-run previews are non-mutating; confirmed runs increment attempt counts,
  mark delivered or failed, audit the run, send `webhook`/`webhook_json`
  payloads with HTTPS-only targets except localhost HTTP, and leave unsupported
  custom adapters as failed records with actionable errors for retry after an
  adapter is configured. The same outbox has automatic worker processing
  through `vpsman-worker`: queued `audit_log`, `webhook`, and `webhook_json`
  deliveries are processed in bounded batches, unsupported custom adapters fail
  visibly for later retry, old delivered/failed rows are pruned by a configurable
  retention window, and both processing and pruning create audit records.
  Worker knobs include `VPSMAN_WORKER_NOTIFICATION_DELIVERY_LIMIT`,
  `VPSMAN_WORKER_NOTIFICATION_RETENTION_DAYS`,
  `VPSMAN_WORKER_NOTIFICATION_RETENTION_PRUNE_LIMIT`, and
  `VPSMAN_WORKER_NOTIFICATION_WEBHOOK_TIMEOUT_SECS`, with matching CLI flags
  for one-shot and daemon operation. `fleet:read` operators can list channels
  and deliveries; channel writes, dispatch, and manual processing require
  `operator` plus `inventory:write`. Secret-vaulted webhook headers,
  custom-command delivery adapters, richer retry/backoff/dead-letter controls,
  broader explicit retention policy management, and deeper business-impact
  grouping remain future work. The browser no
  longer writes bearer or refresh
  tokens to plaintext `localStorage`; optional session persistence uses the
  same WebCrypto AES-GCM/PBKDF2 vault pattern as privileged proof material,
  with legacy plaintext token keys purged on load and logout. A browser test
  proves login, encrypted session-vault storage, reload unlock, and absence of
  raw bearer/refresh tokens, login password, or vault passphrase in storage.
  Optional operator TOTP is implemented through REST, `vpsctl`, VTY, and the
  Access panel. Setup requires the current operator password, encrypts the TOTP
  secret at rest with a password-derived ChaCha20-Poly1305 key, returns the
  secret/otpauth URI only during setup, requires confirmation before enabling,
  gates future login with a six-digit one-time code, and audits setup/enable/
  disable without logging the secret. Per-resource target constraints and
  richer permission administration UI remain gaps.
  Server-side schedules now have durable
  PostgreSQL and in-memory API support: operators can create/list interval
  schedules with client/pool/tag selectors and typed operations, and the worker
  can materialize due schedules as `approval_required` job/audit records with
  frozen per-run targets, persisted operation JSON, and source schedule ids.
  Schedules now expose policy fields for missed-run handling and materialization
  retry behavior: `catch_up_policy` (`skip_missed`, `run_once`, or
  `run_all_limited`), `catch_up_limit`, `retry_delay_secs`, `max_failures`,
  `failure_count`, and `last_error`. REST, `vpsctl`, VTY, and the Schedules
  panel can create/read these fields. The worker processes bounded catch-up
  runs, resets failure evidence after successful materialization, and records
  bounded failure evidence plus `schedule.due_failed` audit records when
  materialization cannot be completed. The same worker has a shared
  `worker_leases` table and `VPSMAN_WORKER_ID`/`VPSMAN_WORKER_LEASE_SECS`
  controls so repeated one-shot runs from the same worker can renew their own
  lease while competing worker instances skip singleton task families until
  the lease expires. Current shared-lease task names are `schedules`,
  `alert_notifications`, `telemetry_rollups`, `telemetry_network_rates`, and
  `telemetry_prune`; rollout automation keeps its rollout-specific
  worker/lease fields because work is claimed per rollout target set.
  The worker deliberately does not generate super-password proof material
  server-side. Instead, the API can dispatch an approval-required scheduled run
  through `/api/v1/jobs/{job_id}/dispatch-scheduled` only when an operator
  supplies fresh per-client proof envelopes for the frozen target set; a memory
  API/fake-gateway test covers this route. Scheduled dispatch now also resolves
  agent capability snapshots before gateway dispatch: explicit unprivileged
  targets for root-only network mutations, limit-bearing process starts on
  agents that cannot apply process limits, and update/restore host mutations on
  agents without privileged host-mutation capability are completed as
  `degraded_unprivileged` unless the operator explicitly sets
  `force_unprivileged`. The default no-database dev mode is still in-memory
  with an explicit auth bypass. Backup request metadata, backup
  artifact metadata, and restore plan metadata now have API persistence,
  proof-envelope validation where host action would be required, CLI/VTY/panel
  visibility, and audit records. Proof-gated backup jobs can now produce a
  bounded self-contained encrypted artifact in job output when the agent has a
  configured backup recipient public key. Direct and scheduled proof-gated
  backup dispatch now auto-creates one backup-request metadata row per target
  when no open row already matches the client and payload hash, so `backup-run`
  and due backup-policy jobs share the same request/artifact history model as
  explicit `backup-request`. When the primary filesystem-backed
  `VPSMAN_BACKUP_OBJECT_STORE_DIR` is configured, the API can accept an
  already-encrypted artifact, validate its encrypted artifact envelope and
  client id, write it under a safe relative object key, compute SHA-256
  server-side, reject overwrites, link metadata to the backup request, and audit
  the link. Complete `VPSMAN_OBJECT_*` S3/MinIO settings remain available as an
  optional adapter path when no local backup object-store directory is
  configured. When a proof-gated backup job completes for the same client and
  payload hash as an open backup request, the API can also auto-record the
  encrypted artifact from retained job stdout into the configured object store
  and publish a `backup_artifact_recorded` WebSocket event; direct and
  scheduled backup jobs therefore auto-link object-store artifacts without a
  separate hand-authored request as long as the agent emits a valid encrypted
  backup artifact on stdout. Backup stdout is produced by the same bounded
  payload streamer as file pull. Auto-record now prefers the persisted retained
  job-output path for the completed job, so inline chunks and local
  object-store-backed chunks are staged as a verified local file before the
  final backup artifact is committed. The retained-output handoff validates
  output chunk hash/size plus encrypted artifact envelope/client id, and commits
  through the object-store adapter with a configurable streaming ceiling
  (`VPSMAN_BACKUP_HANDOFF_MAX_BYTES`, defaulting to the chunked upload ceiling).
  Local filesystem storage is the primary release path; S3 remains a reserved
  adapter path. The S3 path is a reserved
  path-style HTTP SigV4 adapter with bounded
  parser behavior, non-overwrite duplicate checks, fake-S3 regression tests,
  and opt-in MinIO smoke coverage. The API can now serve the linked encrypted
  artifact back to an
  operator after checking object size, SHA-256, object ownership, client id, and
  encrypted artifact envelope. Backup policy scheduling now exists as a typed
  layer over schedules: policies target explicit clients, resource pools, and
  tags; carry selected paths/config scope, retention days, keep-last counts,
  optional rotation generation labels, and an optional backup recipient public
  key; persist in `backup_policies`; and materialize through the existing
  schedule worker as approval-required `scheduled_backup` jobs. The server
  never fabricates privileged proof for due policies. Operators approve
  materialized scheduled backup jobs with fresh proof through the normal
  schedule-dispatch path, and those dispatches produce the same per-target
  backup-request rows and artifact links as interactive backup runs. Retention
  policy metadata is recorded and visible. Policy-linked retention pruning is
  now an explicit operator workflow through `/api/v1/backup-policies/prune`,
  `vpsctl backup-policy-prune`, VTY `backup-policy-prune`, and the Backups
  panel Policy prune control. The prune path supports all policies or one
  schedule id, dry-run previews, metadata-only cleanup, confirmed object-store
  deletion, per-policy matched/pruned counters, object-key evidence, and audit
  records. The background worker also has an opt-in metadata-only retention
  policy (`--backup-policy-prune-enabled`) that runs under a dedicated lease,
  uses bounded policy limits, records sanitized worker audit metadata, and can
  delete local filesystem object bytes only when
  `--backup-policy-prune-delete-objects` and
  `--backup-policy-prune-object-store-dir` are explicitly configured. S3
  worker deletion remains a reserved adapter extension. `vpsctl restore-run`,
  VTY
  `restore-run`, and the
  Backups panel Run restore action can now decrypt a bounded backup artifact
  locally with operator-held backup private key material, dispatch a proof-gated
  inline restore job, and have the agent restore selected files and the archived
  config under a destination root while creating rollback copies for overwritten
  files. If no local artifact file is supplied, CLI, VTY, and panel download the
  linked stored encrypted artifact first; the backup private key still never
  goes to the API. If a later entry in the same bounded inline restore fails,
  the agent now automatically compensates already-applied entries in reverse
  order by restoring rollback copies or removing newly-created files.
  Operator-triggered
  rollback after a successful restore is now proof-gated through a typed
  `restore_rollback` command. The server, CLI, VTY, and panel rebuild the
  rollback operation from retained successful restore status output, hash that
  exact manifest into per-client proof envelopes, and the agent preflights the
  current restored files by size and SHA-256 before mutating anything. On
  success it restores prior snapshots for overwritten files and removes files
  that were created by the restore. The panel keeps backup private key material
  in browser state only and sends only the bounded archive bytes and proof
  envelope to the API. Server dispatch resolves agent capability snapshots
  before restore execution or rollback host mutations; explicit unprivileged
  agents, or root agents without privileged host-mutation capability, are
  completed as `degraded_unprivileged` by default unless the operator sets
  `force_unprivileged` through the API, CLI, or VTY. A first
  `migration_linked_metadata_only` slice now links
  an accepted proof-gated restore plan to the source backup identity and rebuilt
  target client identity, with API/CLI/VTY/panel visibility and audit records.
  Full resumable/direct backup upload beyond the matched job-output auto-link
  path, HTTPS cloud-S3 hardening, large or resumable restore, and executable
  rebuilt-VPS migration automation are not wired end to end.
  Gateway session lifecycle is now persisted through gateway-issued
  start/end events: the API stores active/ended session rows, updates client
  online/offline status only when no newer active session exists, and exposes
  the latest records through REST, the Access panel, `vpsctl
  gateway-sessions`, and VTY `gateway-sessions`. The PostgreSQL live gateway
  smoke proves active and ended session readback, peer disconnect status, and
  API restart persistence.
  The worker now materializes compact CPU/RAM/disk/network telemetry rollups
  from raw `telemetry_samples` into `telemetry_rollups`; operators can read
  the latest buckets through `/api/v1/telemetry/rollups`, the Fleet panel CPU/RAM
  columns plus telemetry detail tab, `vpsctl telemetry-rollups`, and VTY
  `telemetry-rollups`. The PostgreSQL persistence smoke seeds raw telemetry,
  runs `vpsman-worker --once`, verifies compiled-CLI CPU/RAM/disk/network
  rollup readback, and confirms API restart persistence.
  The same worker materializes per-interface network-rate buckets from raw
  telemetry network counters into `telemetry_network_rates`; operators can read
  latest interface rates through `/api/v1/telemetry/network-rates`, the Fleet
  panel Network tab, `vpsctl telemetry-network-rates`, and VTY
  `telemetry-network-rates`.
  The worker also prunes raw telemetry samples older than the configured
  retention window after rollups; the PostgreSQL persistence smoke now proves
  old raw sample removal. Operator-managed history retention policies now cover
  raw telemetry, rollups, audit, job output, backup artifact metadata, network
  observations, and topology history for manual/exported lifecycle control.
  Per-resource target constraints and richer permission administration UI
  remain incomplete; active in-flight job cancellation is now wired end to end
  for dispatching TCP command jobs.
- The frontend fleet shell now reads live summary/agent REST endpoints and
  handles fleet WebSocket events for snapshots, agent updates, telemetry
  updates, and rejected jobs. It can log in or bootstrap an operator, attach
  bearer tokens to REST/WebSocket requests, display API-backed job/audit
  history, and manage API-backed pools, tags, assignments, and bulk target
  previews. A Schedules view lists API-backed schedules and can register
  interval shell schedules against explicit clients, pools, or tags. The Jobs
  view now includes a command composer that parses argv,
  switches between shell argv, file-pull, file-push, hot-config, user-session,
  process-snapshot, and supervisor modes, previews target resolution, generates
  per-client WebCrypto proof envelopes, and can keep super-password material
  only in memory or inside an AES-GCM browser vault encrypted with an operator
  passphrase. Job output
  drilldown is backed by the retained output API. The Backups view can create
  proof-gated metadata-only backup requests, upload already-encrypted artifact
  files, create metadata-only restore plans, locally decrypt a bounded artifact
  for proof-gated Run restore, build proof-gated restore rollback commands from
  retained restore status output, and list linked encrypted artifact metadata,
  without sending the super password or backup private key to the server.
  Frontend code is no longer
  a single broad file: the React app shell, console shell, shared API/type/
  utility code, metrics, auth, fleet, audit, jobs, schedules, topology,
  pools/tags, backups, access, hooks, proof vault, and CSS are split into
  focused modules. The Jobs dispatch workflow now separates mode-specific
  operation editors under `frontend/src/panels/jobs/`, while target selection
  and dispatch options remain in shared dispatch controls and the parent panel
  owns proof, target resolution, and API submission. The Backups workflow now
  separates history tables, backup request, artifact upload, restore planning,
  executable restore, and restore rollback forms under
  `frontend/src/panels/backups/`, while the parent panel owns proof, decrypt,
  retained-output lookup, and dispatch orchestration.
  `frontend/src/App.tsx` is now a small view-composition layer, workflow data
  loading is split into domain hooks, the
  left navigation is grouped by workflow sections, and the topbar exposes
  resource scope, fleet search, live control-plane status, command access,
  session state, and unlock action in a denser console-style layout. The unused
  placeholder panel was removed so production frontend modules do not carry
  dead "not wired" UI.
  The Fleet view now reads API-backed telemetry rollups and per-interface
  network-rate buckets, showing latest CPU/RAM utilization in the instance table
  plus selected-instance telemetry/network details instead of leaving the core
  fleet health columns as placeholders. The Jobs view can cancel pending
  scheduled approval jobs without unlocking proof material because no client
  command is issued, and can request cancellation for active dispatching jobs
  through the same API/gateway/agent cancel lifecycle as CLI and VTY.
  The Access view now shows current-operator, bearer-session storage mode,
  proof-vault, WebSocket state, admin-only operator role/scope records, retained
  operator sessions, session revoke controls, and an admin-only create operator
  form with local clear controls. Browser bearer-session persistence is now a
  WebCrypto-encrypted session vault; sessions may also stay memory-only when no
  vault key is provided. The Access view also exposes optional TOTP
  setup/confirm/disable for the current operator. Deeper per-resource target
  constraints and permission administration, deeper Google Cloud-style polish,
  and some privileged workflows that remain metadata-only or observe/plan-only
  are still open.
- Shared network planning code can now parse legacy Bird2 OSPF peer snippets,
  parse ifupdown tunnel snippets, calculate OSPF costs, allocate tunnel address
  pairs from a panel pool, render non-mutating GRE/IPIP/SIT/FOU ifupdown plus
  Bird2 plan snippets, and render canonical side-specific endpoint snippets for
  apply. The shared network domain is split into model, cost policy, legacy
  parser, planner/rendering, and test modules so network apply does not grow a
  monolithic business-logic file. Tunnel plans now have API persistence and
  audit records through `/api/v1/tunnel-plans`, `vpsctl tunnel-plan --save`,
  and the Topology panel. The first bounded proof-gated apply slice exists
  through API job validation, `vpsctl tunnel-apply`, VTY `tunnel-apply`, the
  Topology panel Network apply controls, and agent-side managed-file writes
  with backups, validation/reload hooks, rollback-on-write-failure, and
  rollback-on-hook-failure. `network_rollback` now has API/CLI/VTY/panel/agent
  parity for removing exact managed blocks with proof, confirmation, backups,
  validation/reload hooks, and idempotent no-block behavior. `network_status`
  now has API/CLI/VTY/panel/agent parity for proof-gated read-only managed-file
  inspection, live sysfs interface observation, and optional bounded Bird2
  status probes with OSPF neighbor-state parsing. `debian_ifupdown2_bird2`
  provides the first curated validation/reload/rollback preset, and a Docker
  smoke now exercises real Debian ifupdown2/Bird2 validation, reload, and
  rollback teardown. A second Docker smoke now builds two Linux namespaces,
  creates a GRE tunnel, starts two Bird2 OSPFv3 routers with vpsman-managed
  interface snippets, and waits for `Full/PtP` neighbor convergence.
  `network_probe` now has API/CLI/VTY/panel/agent parity for bounded,
  proof-gated read-only ICMP latency/loss checks against the peer tunnel
  address. `network_speed_test` has API/CLI/VTY/panel/agent parity for a
  bounded two-endpoint TCP throughput test with exact endpoint targeting,
  concurrent gateway dispatch, duration/byte/rate/port/connect-timeout limits,
  and retained per-side status metrics. Network status, latency-probe, and
  speed-test status chunks are also summarized into typed
  `network_observations` records and grouped trend rollups so topology
  performance evidence survives beyond retained-output inspection.
  `/api/v1/network/observations`, `/api/v1/network/observation-trends`,
  `/api/v1/network/ospf-recommendations`, `vpsctl network-observations`,
  `vpsctl network-trends`, `vpsctl network-ospf-recommendations`, VTY
  `network-observations`, VTY `network-trends`, VTY
  `network-ospf-recommendations`, and the Topology panel evidence table expose
  those summaries while still allowing retained job-output drilldown. The
  API also exposes a derived applied-topology graph at
  `/api/v1/network/topology-graph`; `vpsctl topology-graph`, VTY
  `topology-graph`, and the Topology panel render saved tunnel endpoints,
  endpoint state, applied/degraded health, latency/loss/throughput trends, and
  OSPF cost deltas from the same read model.
  current OSPF recommendation and update-plan slices combine saved tunnel plans
  with persisted probe/speed-test trends, downgrade effective bandwidth when
  measured throughput falls below the configured burst tier, apply the saved
  tunnel-plan OSPF policy/preference, report the derived cost delta, and render
  reviewed left/right Bird2 cost snippets with proof/approval metadata.
  `/api/v1/network/ospf-update-plans`, `vpsctl network-ospf-update-plans`,
  VTY `network-ospf-update-plans`, and the Topology panel expose that reviewed
  surface. Applying a reviewed cost delta is a separate proof-gated
  `network_ospf_cost_update` operation through API validation, agent runtime,
  `vpsctl tunnel-ospf-cost-update`, VTY `tunnel-ospf-cost-update`, and the
  Topology panel OSPF cost apply controls. It hash-freezes the proposed Bird2
  snippet, checks the expected current cost, writes only the managed Bird2
  include block, runs Bird2-only validation/reload hooks, and reports
  `rollback_mode: apply_previous_cost` rather than performing a full tunnel
  reapply. Tunnel plans now persist left/right endpoint status, aggregate
  topology status, last apply job id, last rollback job id, update timestamps,
  and sanitized audit records when proof-gated `network_apply` or
  `network_rollback` jobs complete. The Topology panel exposes those endpoint
  states alongside the plan list and graph. The graph now supports search,
  health filtering, dense deterministic layout, and selected-node inspection
  for larger fleets. Typed ifupdown, netplan, and systemd-networkd
  managed-file compatibility renderers now exist with backend proof hashes, but
  the design direction has changed: the primary release path is the
  agent-managed runtime reconciler for `ip link`/`ip tunnel`/custom adapter
  lifecycle. Custom external tunnel import/management, runtime status
  telemetry, topology drift policy/action labels, latency curves, and
  proof-gated apply/rollback are implemented for the local release baseline.
  Remaining work is longer-retention analytics over observation records,
  graph edit/drilldown workflows, broader provider/namespace coverage, and
  higher-level bulk/canary policy automation around reviewed OSPF cost changes.
- `vpsctl` supports health, operator bootstrap/login/refresh/me, summary,
  admin-only operator list/create/session-list/session-revoke,
  agent/pool/tag listing, durable gateway-session listing, telemetry-rollup
  listing, bulk target resolution, offline tunnel-plan rendering, Noise key
  generation, local proof envelope generation, and scriptable
  `job-create` auto-proof generation for explicit client, pool, and tag target
  selectors when local super-password material is available. The partial VTY
  can enter privileged mode only after reading local
  `VPSMAN_SUPER_PASSWORD`/`VPSMAN_SUPER_SALT_HEX` material and can generate
  per-client proof envelopes for `job-create` selectors including
  `client:<id>`, `pool:<uuid>`, and `tag:<name>`. VTY privileged mode now has
  router-style operator affordances: `enable`, `disable`, `show privilege`,
  `show capabilities`, and `show degraded-policy`. These commands expose
  redacted local proof state, command-family capability coverage, and explicit
  `degraded_unprivileged`/`--force-unprivileged` guidance without printing
  plaintext super-password or salt material. `vpsctl file-pull`, VTY
  `file-pull`, `vpsctl user-sessions`, and VTY `user-sessions` submit the same
  proof-gated bounded file-pull and user-visibility operations as the panel;
  real agent file pulls stream chunks through the gateway/API output path, so
  large pulls can be retained as object-store-backed job-output artifacts when
  that storage is configured instead of forcing the agent to buffer the file;
  `vpsctl process-list` and VTY `process-list` cover the current proof-gated
  process snapshot slice.
  `vpsctl file-push --source ... --path ... --confirmed`, VTY `file-push`, and
  the panel File push mode cover the current `chunked_file_push_operation`
  slice: local data is bounded, SHA-256 hashed into the proof payload, split
  into per-chunk SHA-256 verified payloads when it exceeds the inline cap, and
  atomically written by the agent after full-file verification.
  `vpsctl process-start`, `process-stop`, `process-restart`, `process-status`,
  and `process-logs`, plus the matching VTY commands, cover the first
  proof-gated vpsman-managed process supervisor slice.
  `vpsctl hot-config --config-file ... --confirmed` and VTY
  `hot-config --config-file ... --confirmed` cover the current proof-gated
  config-update-only slice with local proof generation. `vpsctl
  agent-update-signature` creates detached Ed25519 metadata for a local update
  artifact. `vpsctl agent-update --artifact-url ... --sha256-hex ...
  [--artifact-signature-hex ... --artifact-signing-key-hex ...]
  [--canary-count ...] --confirmed`,
  VTY `agent-update`, and the panel Agent update mode cover the current
  `artifact_staged_only` slice with HTTPS URL validation, local proof
  generation, hash verification, optional signature verification, and
  agent-side staging. `vpsctl agent-update-activate --staged-sha256-hex ...
  [--restart-agent]` and `vpsctl agent-update-rollback
  [--rollback-sha256-hex ...]`, plus matching VTY commands and panel modes,
  cover the current proof-gated activation/rollback slice. `vpsctl
  agent-update-rollouts`, `agent-update-rollout-activate`,
  `agent-update-rollout-rollback`, and
  `agent-update-rollout-control`, plus matching VTY commands and panel rollout
  actions, expose durable rollout state, operator-driven canary batch
  promotion, rollout pause/resume, selectable health gates, post-restart
  `heartbeat_verified` evidence, worker lease evidence, and explicit rollback
  dispatch for activation-pending, activation-failed, heartbeat-timeout, or
  heartbeat-verified targets.
  `vpsctl schedules`, `vpsctl schedule-create`, VTY `schedules`, and VTY
  `schedule-create` cover the current schedule registry slice. `vpsctl backups`,
  `vpsctl backup-policies`, `vpsctl backup-policy-upsert`, `vpsctl
  backup-policy-prune`, `vpsctl backup-request`, `vpsctl backup-run`, `vpsctl
  backup-artifacts`, and `vpsctl backup-artifact-record`, plus `vpsctl
  backup-artifact-upload`, `vpsctl
  backup-artifact-upload-chunked`, and `vpsctl backup-artifact-handoff`, VTY
  `backups`, `backup-policies`, `backup-policy-upsert`,
  `backup-policy-prune`, `backup-request`, `backup-run`, `backup-artifacts`,
  `backup-artifact-record`, `backup-artifact-upload`,
  `backup-artifact-upload-chunked`, and
  `backup-artifact-handoff`, cover the current backup policy scheduler,
  policy-linked retention pruning, metadata, encrypted-artifact-output,
  filesystem-backed encrypted artifact upload, server-mediated chunked
  encrypted artifact upload, and durable retained-output handoff slices. The
  prune workflow is explicit and auditable: dry-runs compute per-policy
  candidates without mutation, confirmed runs clear policy-linked artifact
  metadata and, unless metadata-only is selected, delete object-store keys
  through the configured object-store adapter. The chunked upload-session path
  creates a server-side staging session, accepts offset-aware base64 chunks,
  allows idempotent retry of already-written chunks, validates final size,
  SHA-256, encrypted envelope, and client identity, commits through the local
  object-store abstraction with reserved S3 compatibility, rejects duplicate
  object keys, and is exposed in the Backups panel upload mode selector. The
  handoff validates retained backup stdout by client, payload hash, completed
  target, optional selected job id, encrypted artifact envelope, output
  size/hash, and deterministic object-store key before linking artifact
  metadata. Retained inline chunks and object-store-backed job-output chunks are
  staged to a verified local file before object-store commit, so the handoff
  path no longer depends on the small inline-only upload ceiling. The S3/MinIO
  upload adapter remains optional compatibility coverage. Agent-originated
  direct backup streaming with reconnect retry/rate limits remains open beyond
  the server-mediated chunked/staged handoff paths. `vpsctl restore-run`,
  VTY `restore-run`, and the panel Run restore action cover the first bounded
  restore execution slice with local artifact files or linked stored encrypted
  artifact retrieval; CLI and VTY also expose `--force-unprivileged` for
  explicit best-effort attempts on incapable targets. `vpsctl
  restore-rollback`, VTY
  `restore-rollback`, and the panel Rollback restore action cover the current
  proof-gated rollback slice for completed successful restore jobs with
  retained status evidence; CLI and VTY also expose `--force-unprivileged` for
  explicit best-effort attempts. `vpsctl restore-plans`, `vpsctl restore-plan`,
  VTY
  `restore-plans`, and VTY `restore-plan` cover the metadata-only restore plan
  slice. `vpsctl tunnel-plans`, `vpsctl tunnel-plan`, VTY `tunnel-plans`, and
  VTY `tunnel-plan` cover non-mutating tunnel-plan listing, local rendering,
  optional API persistence with `--save`, runtime owner flags, external adapter
  argv, traffic limits, desired/stale interfaces, and topology routes. `vpsctl
  tunnel-promote-telemetry`, VTY `tunnel-promote-telemetry`, and the Topology
  panel Promote observed tunnel form cover promotion of observed telemetry
  import candidates into saved non-mutating `external_observed` plans. `vpsctl
  tunnel-promote-adapter`, VTY `tunnel-promote-adapter`, and the Topology
  panel Adapter contract form cover promotion from saved observed plans into
  managed adapter contracts. `vpsctl tunnel-apply`, VTY `tunnel-apply`,
  `vpsctl tunnel-rollback`, VTY
  `tunnel-rollback`, `vpsctl tunnel-status`, VTY `tunnel-status`, `vpsctl
  tunnel-probe`, VTY `tunnel-probe`, `vpsctl tunnel-speed-test`, and VTY
  `tunnel-speed-test` cover the bounded proof-gated network
  apply/rollback/status/read-only latency-probe/two-endpoint speed-test slice
  for saved plans. `vpsctl
  network-observations`, `vpsctl network-trends`, `vpsctl
  network-ospf-recommendations`, VTY `network-observations`, VTY
  `network-trends`, and VTY `network-ospf-recommendations` list typed
  persisted network status/probe/speed summaries, rollups, and read-only OSPF
  cost recommendations from the same API used by the Topology panel. The
  Topology panel now exposes the same bounded
  apply/rollback/status/probe/speed-test path with local proof generation.
  API/CLI/VTY/panel parity is now present for the current release spine:
  resumable transfer sessions, terminal sessions/replay, executable
  migration-run orchestration, update rollout/rollback/delegation, runtime
  tunnel apply/rollback/status, command templates, server-owned enrollment
  aliases/tags, staged retained-output backup handoff, and PostgreSQL
  success-path persistence all have focused smoke evidence. The 2026-06-04
  aggregate release gate passed for the documented local filesystem object-store
  baseline. Remaining work is post-release hardening: longer-retention terminal
  and network analytics, policy automation, broader provider/namespace tunnel
  coverage, and multi-agent rollout stress.
- Command execution is partially implemented end to end for proof-gated bounded
  non-PTY argv jobs, noninteractive PTY-backed argv jobs, bounded shell-script
  wrapper jobs, bounded file-pull jobs, bounded inline/chunked file-push jobs,
  user-session visibility jobs, and process snapshot jobs through API, gateway,
  and agent. A first
  vpsman-managed portable process supervisor slice now supports start, stop,
  restart, status, and log-tail operations through proof-gated job commands on
  the API, agent, `vpsctl`, and browser command composer. It tracks only
  vpsman-launched processes with local pid records and stdout/stderr log files.
  Bounded stdout/stderr/status chunks, file-pull chunks, user-session output,
  process snapshot output, and supervisor outputs are persisted in
  `job_outputs` and exposed through REST plus completed clients. Active
  in-flight job cancellation now works through REST, CLI, VTY, gateway,
  agent, persisted target state, audit history, and the Jobs panel for
  dispatching jobs.
  A hot-config update slice is implemented through the same proof-gated job
  channel: API validates bounded TOML shape, CLI/VTY/panel build local proof
  envelopes, and the agent applies permitted config fields atomically with a
  rollback copy. A reusable live smoke proves no-proof rejection and successful
  proof-gated hot-config application through API, gateway, and an enrolled
  agent, including rollback-copy creation, persisted target/output/audit
  records, and status-output redaction of TOML secrets and trust anchors. An
  `artifact_staged_only` agent-update slice is also
  implemented: API/CLI/VTY/panel validate confirmed HTTPS artifact URL plus
  SHA-256 and optional detached Ed25519 signature metadata, the agent downloads
  with bounded memory, decodes normal and chunked HTTP responses, verifies the
  hash and pinned-key signature policy before writing, stages the binary
  side-by-side, and writes a rollback copy. A reusable live smoke proves
  private-CA HTTPS artifact retrieval, pinned-key Ed25519 signature
  verification, no-proof rejection, proof-gated compiled-CLI dispatch through
  the gateway, staging/rollback-copy creation, persisted target/output/audit
  state, rollout record persistence/readback, proof-gated manual activation,
  activated-agent restart heartbeat evidence, proof-gated rollback cleanup, and
  redaction across API restart. A signed update release metadata registry now
  exists as `agent_update_releases`: API, CLI, VTY, and the Jobs panel can
  record/list version/channel metadata for signed artifacts, look up the latest
  release by name/channel, store only hashes of artifact URLs/signatures/signing
  keys, record optional rollback-bundle hashes, audit the record, and
  optionally require staged updates to match a registered signing-key/hash pair
  with `VPSMAN_REQUIRE_REGISTERED_AGENT_UPDATES=1`. A bounded hosted artifact
  slice is also implemented: local `VPSMAN_UPDATE_OBJECT_STORE_DIR` storage
  enables upload of signed primary and optional rollback artifacts through API,
  CLI, VTY, and panel compatibility paths. The production CLI/VTY path streams
  raw artifact bodies to the API, records hosted releases from verified primary
  and rollback artifact hashes, stores bytes under content-addressed object
  keys, records only sanitized hosted-release metadata, serves
  `/api/v1/agent-update-artifacts/{sha256}` after hash/size re-check, and can
  derive public HTTPS download URLs when
  `VPSMAN_UPDATE_ARTIFACT_PUBLIC_BASE_URL` is configured. Configured release
  channel and trusted signing-key allow-lists are available through
  `VPSMAN_AGENT_UPDATE_ALLOWED_CHANNELS` and
  `VPSMAN_AGENT_UPDATE_TRUSTED_SIGNING_KEYS_HEX`. The current release path
  supports assisted canary automation, operator pause/resume, health gates,
  supervised self-restart, and proof-delegated activation/rollback. Full
  unattended release-policy automation, automatic proof renewal, and broader
  multi-agent rollout stress remain post-release hardening items.
  Explicit `VPSMAN_UPDATE_OBJECT_*` S3/MinIO settings remain a reserved adapter
  path.
  Operator-driven canary activation and rollback helpers now exist, including
  optional supervised-exit restart request on activation; the assisted worker
  recommends batches and dispatches only exact delegated proof envelopes.
  The local-backend file transfer release path supports bounded inline and
  chunked proof payloads, resumable upload has API/agent/CLI/VTY/browser
  slices, and resumable download has API/agent/CLI/VTY plus limited browser
  single-target slices. A durable transfer-session read model now derives
  latest upload/download state from retained job status outputs and is exposed
  through API, CLI, VTY, and the Jobs panel. CLI/VTY upload now supports
  strict same-offset and independent resumed-offset sub-batch policies.
  Browser upload now exposes the same strict/default and independent-offset
  policies, uses sliced incremental hashing/chunk reads, and is capped at the
  shared protocol maximum rather than a browser memory cap. CLI/VTY download
  now supports explicit `single-target` and `per-target-files` policies.
  Browser download now has selectable default Blob-download and File System
  Access stream-to-file sinks; the stream-to-file sink avoids retaining all file
  bytes in memory when supported. Completed download sessions now also support
  server-side local object-store handoff from retained chunks with verified
  browser/CLI/VTY artifact retrieval. Confirmed source artifacts can now be
  uploaded to the server object store through API, CLI, VTY, and the Jobs panel
  with size/SHA-256 verification and durable metadata; those retained artifacts
  can now drive proof-gated resumable upload sessions through CLI, VTY, and the
  Jobs panel without server-side privileged command fabrication. The Jobs panel
  can batch-select completed download handoffs and save deterministic
  client/session-prefixed browser downloads. Local filesystem artifact delivery
  now streams from verified server files, and browser handoff downloads can use
  verified `stream-to-file`. The aggregate release gate covers this path for
  the local object-store baseline; future work is provider/stress coverage and
  optional S3 production hardening.
  A reusable live smoke now proves inline/chunked file-push paths plus CLI
  resumable upload/download through API, gateway, and an enrolled agent,
  including no-proof rejection, destination hash/mode verification, resumable
  file-byte verification, durable transfer-session inventory, persisted
  target/output state, audit actions, no raw inline file bytes in status
  output, and API restart persistence.
  The same PostgreSQL-backed live smoke now proves no-proof user-session
  visibility is rejected and proof-gated `vpsctl user-sessions` reaches a
  Noise-connected agent with persisted output/audit across API restart.
  It also proves proof-gated `vpsctl process-start`, `process-status`,
  `process-logs`, `process-restart`, and `process-stop` through the same
  Noise-connected path with a disposable supervisor root and no super-password
  leakage in decoded outputs. The same persisted evidence now populates a
  durable supervisor inventory that deduplicates latest per-client process
  state and survives API restart.
  Bounded shell-script wrapper jobs are a distinct `shell_script` operation:
  API and panel validate a compact script payload, local proof envelopes hash
  that exact operation, CLI/VTY expose `job-shell`, the agent runs it through
  the selected command execution policy with configurable shell argv, cwd,
  environment, PTY, and cleanup behavior, and a PostgreSQL live smoke proves
  the path through the Noise gateway plus API restart persistence.
  Object-store-backed large output artifact retention is implemented for
  retained non-status chunks above `VPSMAN_JOB_OUTPUT_ARTIFACT_MIN_BYTES`; the
  API stores inline status chunks for observation/audit parsing and exposes
  artifact metadata plus download through REST, CLI, VTY, and the Jobs panel.
  Gateway/API per-frame retained-output ingestion and WebSocket refresh events
  are implemented, so selected job output can refresh as persisted chunks
  arrive. Non-PTY shell argv and shell-script stdout/stderr now stream as
  bounded chunks before command completion, and noninteractive PTY-backed argv
  jobs retain `pty` stream output. CLI/VTY `job-follow` can now poll and decode
  retained output for headless operators. Terminal open/input/poll/resize/close
  sessions, durable terminal-session inventory, and durable persisted-output
  replay now exist through API, browser, CLI, and VTY. Terminal status records
  include retained-output window counters and replay-truncation hints so
  operators can see when a replay cursor has fallen outside the bounded PTY
  buffer, while the replay endpoint reconstructs available historical PTY
  chunks from persisted job outputs and verifies object-store-backed chunks.
  PostgreSQL live smoke coverage includes the polled lifecycle, attach replay,
  explicit output polling, retention metadata, and API restart through the
  Noise gateway. The Jobs panel now provides retained-row actions to prepare
  attach/replay, durable replay preview, poll, input, resize, and close jobs
  for the selected session without manually copying ids or targets.
  Continuous terminal streaming now uses the server-owned event/replay model:
  agent-originated stream frames, gateway/API append-only persistence,
  terminal-specific WebSocket metadata, and panel/CLI/VTY replay-delta follow.
  Very long terminal-history retention policy remains future tuning.
  Encrypted
  backup output, auto-created per-target request metadata for direct/scheduled
  backup dispatch, filesystem-backed encrypted artifact storage, bounded
  CLI/VTY/panel restore execution, and
  operator-triggered restore rollback exist, including automatic object-store
  linking for completed direct/scheduled backup jobs. Full
  resumable/direct backup upload, migration, update rollout, final tunnel
  workflow integration, and full final integration are not yet implemented end
  to end.
- Code organization has improved but remains incomplete. The API is no longer a
  3,000+ line `main.rs`: models, route groups, gateway dispatch client,
  repositories, job validation, ingest persistence, auth, enrollment, backup,
  network, and schedule code are split into focused Rust modules; `main.rs` is
  now a small composition root. `vpsctl` has separate command-family modules,
  proof helpers, VTY backup, VTY inventory, VTY network, VTY OSPF cost-update,
  VTY process-supervisor parsing, and privileged job-operation helpers. The
  frontend app, data hooks, panels, proof vault, command-dispatch operation
  model, command-router dispatch, and CSS have been split by workflow. The
  shared network planner is now split by domain responsibility.
  API tests are now partially split into
  auth, backup, process-supervisor, schedule, network-plan, and OSPF
  cost-update modules; the remaining general API test module is below the
  800-line split-warning threshold but should still be split further by
  enrollment, inventory, and job workflow before substantial API test growth.
  Scheduled-dispatch persistence is
  isolated from the broader jobs repository in a focused repository module. The
  gateway's internal control HTTP path, API-forwarding client, shared session
  state, and API-persisted session lifecycle events have been split out of the
  binary entrypoint; the remaining gateway design-quality work is to split the
  agent session loop and transport handlers before expanding enrollment,
  command idempotency, or replay policy. `vpsctl` job command handling is now
  split by workflow into generic jobs, schedules, file transfer,
  process/user-session command modules, and a network command module that owns
  topology subcommand arguments and bounded tunnel operation dispatch. The
  top-level `vpsctl` command router is also split into
  access/inventory/schedule, job/config/process/update, and
  backup/restore/migration/network/key dispatch modules. The VTY target/proof
  submission helpers are split out of the interactive shell loop. API
  job-output persistence is split out of the broader job repository. Remaining
  design-quality work should continue by splitting the inventory repository,
  integration tests, and any file that grows past the recommended split
  threshold instead of growing entrypoints or broad repository modules again.
  Files around 1,000 lines should get a split plan or a cohesion rationale;
  hard line-count failures are role-based, with lower limits for implementation
  modules, roughly 2,000 lines for broad implementation files, and higher
  limits, roughly 5,000 lines, for focused integration tests, fixtures, and
  role-separated CLI/VTY command-parser modules. The
  CLI command
  enum declarations are now partially split, with access, enrollment,
  inventory, bulk, telemetry, and schedule argument structs isolated in
  `cli_access.rs`. API job route handling is also split so read-only
  job/history/output/audit/network-observation endpoints live in
  `routes_job_history.rs`, while proof-gated dispatch and cancel lifecycle
  remain in `routes_jobs.rs`. The built-in data-source preset catalog is split
  into `data_source_builtin_presets.rs`, keeping
  `repository_data_source_presets.rs` focused on persistence, lifecycle,
  assignment, and audit behavior.
  VTY direct, non-proof command handling is split from the interactive proof
  shell loop so the loop can stay focused on prompt state and proof-gated
  workflow dispatch.
  `scripts/release-check.sh` now aggregates the current release quality gate,
  includes `scripts/scan-repo-hygiene.sh` for repository secret/path hygiene,
  includes a staged Debian/Ubuntu installer matrix smoke for Ubuntu 18.04,
  20.04, 22.04, 24.04, Debian stable, and one older Debian image, including
  staged uninstall-preserve and explicit purge behavior,
  includes a constrained-container static-agent resource smoke for binary size,
  idle RSS, idle CPU, and thread count,
  includes a constrained static-agent reconnect-churn smoke with a local
  latency/drop proxy that forces a failed session before successful reconnect,
  includes `scripts/audit-customizability.sh` so environment-dependent
  hardcodes are classified as preset/config-backed assumptions or recorded
  open candidates,
  includes `scripts/audit-tutorials.sh` so the operator-facing `tutorials/`
  directory, quickstart, enrollment, re-enrollment, unprivileged mode,
  data-source presets, tunnel/topology/Bird2, backup/migration, updates, and
  headless CLI/VTY usage guides remain indexed and actionable,
  includes a built-binary `vpsctl` help smoke for the root command and every
  supported subcommand,
  includes a built-binary UI/CLI/VTY parity smoke that cross-checks completed
  panel workflow tokens, CLI command help, and VTY help vocabulary,
  includes a built-binary `vpsctl` semantic smoke for local tunnel-plan
  rendering, local proof/key generation, and fail-closed argument validation,
  includes a built-binary `vpsctl` structured-output smoke for global
  `--output json|pretty-json` help, compact JSON, pretty JSON, proof redaction,
  and interactive VTY rejection,
  includes `scripts/smoke-terminal-retention.sh` for terminal flow-window tail
  retention, replay truncation boundaries, idle-timeout/read-model fields, and
  CLI/VTY terminal contracts,
  includes a live API-backed `vpsctl` workflow smoke for operator auth,
  inventory, computed fleet alerts, bulk targeting, saved tunnel plans,
  schedules, backup request, restore plan, and audit visibility,
  includes the Playwright-backed frontend console layout smoke plus a live
  API-backed frontend smoke, runs a Docker-backed PostgreSQL restart
  persistence smoke covering auth, inventory, hierarchy, worker-generated
  telemetry rollups and per-interface network rates, tunnels, schedules,
  pending scheduled job cancellation, fail-closed rejected job history/targets,
  backup/restore metadata, worker-driven backup policy retention prune with
  source-schedule provenance, and audit records, runs a
  PostgreSQL-backed live gateway/agent smoke covering
  `vpsctl job-create` shell execution, proof-gated file pull, proof-gated file
  push, timed-out shell-job state, fail-closed no-proof user-session
  visibility, proof-gated `vpsctl user-sessions`, proof-gated process supervisor
  start/status/logs/restart/stop, durable supervisor inventory readback,
  durable gateway session start/end readback, network status, tunnel latency
  probe, two-endpoint tunnel speed test, accepted job target/output/audit,
  typed network observation durability, and compiled-CLI network trend rollups
  across API restart, runs a PostgreSQL-backed live network apply smoke
  covering no-proof rejection, managed-file apply/rollback, saved observed-plan
  to adapter promotion, external adapter lifecycle apply/rollback,
  unprivileged best-effort adapter policy, audit, redaction, and API restart
  persistence, runs a
  PostgreSQL-backed live hot-config smoke covering
  operator auth persistence, no-proof rejection, proof-gated config mutation,
  rollback-copy creation, accepted job target/output/audit durability, and
  output redaction across API restart, runs a PostgreSQL-backed live
  data-source config patch smoke covering selected-preset create/assign/render,
  no-proof rejection, proof-gated `data_source_config_patch` dispatch through
  the real gateway and enrolled agent, rollback-copy creation,
  target/output/audit durability, output redaction, and API restart
  persistence, runs a PostgreSQL-backed live
  agent-update smoke covering private-CA HTTPS artifact download, pinned-key
  Ed25519 signature verification, no-proof rejection, compiled `vpsctl
  agent-update` dispatch through the real gateway, hash-verified staging,
  rollback-copy creation, output redaction, accepted job target/output/audit
  durability, and API restart persistence, runs a
  PostgreSQL-backed live network
  apply smoke covering no-proof rejection, compiled `vpsctl tunnel-apply` and
  `tunnel-rollback` through the real gateway and enrolled BGP-tagged agent,
  sandboxed managed ifupdown/Bird2 file mutation/removal, backup/status/audit
  durability, and output redaction across API restart, and writes per-step logs
  under `target/release-check/`.
  public release gates include explicit software-quality acceptance conditions
  for correctness/robustness, maintainability, low complexity,
  security/reliability, style consistency, testability, observability,
  evolvability, and product evolution discipline.

Future agents must close these gaps in dependency order rather than adding UI
surface that pretends the backend exists.

## Continuous Inspection And Verification Goals

Public verification objective:

> Inspect and verify vpsman against DESIGN.md and the release-check scripts. Keep the implementation aligned with the original VPS management requirements and all negotiated decisions: Rust headless agents, Noise-over-TCP TLV transport, proof-gated non-telemetry operations, low-resource static musl clients, persistent server control plane, Google Cloud style panel, full CLI/VTY parity, groups/tags/bulk operations, backups/restore, and BGP/Bird2 tunnel observe-plan-apply workflows. Run `scripts/release-check.sh` plus targeted module smokes for changed areas, and do not commit real secrets.

Detailed inspection tasks should live in public issues, release check scripts,
or private local notes, depending on whether they are user-facing release
criteria or agent-only execution planning.
