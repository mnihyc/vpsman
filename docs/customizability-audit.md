# Customizability And Source Template Audit

This audit tracks environment-dependent business assumptions that must be
modeled as source templates, template-backed adapters, or explicitly recorded
as gaps. The product target is 20+ heterogeneous VPSs, so one Linux layout, one
binary path, or one accounting source is not enough.

## Acceptance Principle

- Defaults are allowed only as documented templates.
- The managed business object is the template and the VPS's assignment to that
  template. Every applicable VPS should have an explicit source template for each
  source template domain, even when that source template is the built-in default.
- Operators should manage built-in default templates, shared customizable
  templates, and VPS-local custom templates through template management. Tag and bulk
  workflows are convenience tools for assigning, cloning, testing, or updating
  those template objects at scale; they are not ad hoc command updates.
- Parsed custom sources must use absolute argv, bounded timeout/output, no
  implicit shell, typed JSON output, redacted status, and tests. Custom commands
  are implementation fields inside a template, not the top-level abstraction.
- UI/CLI/VTY should show source/status where operators rely on the data.
- Thoroughness is required: auditing one path literal is not enough. Each
  workflow must be inspected across agent runtime, API/storage, CLI, VTY,
  frontend, tests, docs, degraded/unprivileged behavior, and operator
  migration. A converted backend source is still partial if the panel/headless
  tools cannot show the active template, test it, and assign/clone/customize it
  for real 20+ VPS operations.

## Converted In Current Slice

- General agent telemetry:
  - One honest `telemetry_interval_secs` cadence controls complete samples;
    there is no unused light/full split or implied partial-sample freshness.
  - Linux and custom collectors share protocol cardinality ceilings (256
    filesystems, 512 interfaces, and 512 declared tunnel observations), and
    repeated filesystem sources are counted once. Declared runtime plans
    reserve their observation slots before custom tunnel data is truncated.
  - Default template: `linux_procfs`.
  - Selectable sources: `linux_procfs`, `custom_command`,
    `linux_procfs_and_custom_command`.
  - Configurable paths: `proc_root`, `sys_class_net_dir`, `hostname_file`,
    `os_release_file`.
  - Custom source: bounded JSON command can replace or overlay hostname,
    uptime, CPU, memory, disk, network, and tunnel metrics.
  - Template domain: `telemetry_metrics_source`, with built-in default,
    shared custom, and VPS-local custom templates.
- Runtime tunnel traffic accounting:
  - Default source: interface counters.
  - Template source: configurable `runtime_vnstat_argv`, with vpsman appending
    `--json -i <interface>`.
  - Custom source: per-plan bounded JSON command for provider/application
    accounting.
  - Template domain: `runtime_traffic_accounting_source`; `vnstat` should
    become a shared customizable template, not a hardcoded or one-off command.
- Runtime tunnel adapters:
  - Core GRE, IPIP, SIT, and FOU realization is an explicit agent iproute2
    ownership mode on each saved plan; it is not represented as an external
    source template.
  - Operator-created adapters can define startup, restart, status,
    traffic-limit, stop, and cleanup commands with typed placeholder expansion.
  - Template domain: `runtime_tunnel_adapter`. It has no built-in or default
    implementation. A plan binds one operator-owned template ID to each
    endpoint when `external_managed_adapter` is selected.
  - External-observed plans name one exact interface and never invoke a runtime
    adapter or scan for other tunnel-like interfaces.
- FOU runtime realization:
  - Port `5555`, peer port `5555`, and IP protocol `4` are visible per-plan
    defaults, not hidden host configuration or generated files.
  - Per-plan typed FOU port, peer port, and IP protocol are covered by shared
    models, agent `ip fou`/`ip tunnel` reconciliation, adapter placeholders,
    CLI, VTY, and the tunnel-plan editor.
- Routing cost adapters:
  - Template domain: `routing_cost_adapter`. It has no built-in or default
    implementation and does not identify any routing daemon.
  - Each OSPF-enabled plan binds one explicit adapter template to each endpoint.
    Versioned JSON stdin/stdout carries status and apply requests; the server
    owns reviewed or automatic decisions and the agent only executes explicit
    jobs.
  - vpsman never installs, edits, deletes, discovers, or chooses the external
    executable or routing-daemon configuration.
- ICMP latency probes:
  - Configured source: `[network].probe_ping_argv`.
  - Default template candidates: `/bin/ping`, `/usr/bin/ping`.
  - Status records whether configured argv or template was used.
  - Template domain: `latency_probe_source`, with built-in Linux ping and
    custom probe templates.
- Process inventory:
  - Default source: configurable Linux procfs root through
    `[execution].process_proc_root`.
  - Selectable source: bounded custom JSON command through
    `[execution].process_inventory_source = "custom_command"`.
  - Template domain: `process_inventory_source`.
- User/session inventory:
  - Default template: Linux `w`/`who` candidate source.
  - Selectable source: bounded configured/custom command through
    `[execution].user_sessions_command`.
  - Template domain: `user_session_inventory_source`.
- Shell-script execution:
  - Default template: `[execution].shell_script_argv = ["/bin/sh", "-lc"]`.
  - The shell prefix, working directory, environment policy, explicit env
    values, PTY policy, and cleanup policy are configurable, validated, and
    surfaced in command/terminal status metadata where relevant. Explicit argv
    remains the preferred command mode.
  - Template domain: `command_execution_policy`; shell prefix,
    environment, PTY launcher, cwd, and cleanup policy live in a selected
    policy template rather than scattered command defaults.
- Agent update restart request:
  - Activation no longer shells out to request a supervised restart; it uses an
    internal delayed `SIGTERM` request.
- Frontend operation defaults:
  - Tunnel authoring exposes only the three explicit runtime owners and filters
    adapter selectors to operator-created templates in the matching domain.
  - Backup/restore selected-path defaults and placeholders are named backup
    path templates.
  - Job-operation command examples, terminal default argv, and backup path
    examples are named job-operation templates.
  - Template domain: these are still frontend convenience/default
    catalogs, not the full server-side selected-template model for every
    workflow. Future work should connect each operational panel to active
    source/status and assignment controls where the default affects real VPS
    behavior.
- Agent executable candidates:
  - User-session and latency-probe executable candidates are named templates or
    constants instead of scattered literals. Network runtime/routing adapters
    require explicit operator-created absolute argv and have no managed-file
    compatibility path.
  - The audit scanner avoids false positives where `/proc` appears inside an
    API path.

## Remaining High-Priority Gaps

- Process inventory:
  - Current state: Linux procfs and custom JSON command are selectable.
  - Remaining model: deeper supervisor-only source controls, systemd/cgroup
    enrichment beyond current supervisor limit evidence, and panel/CLI/VTY
    source controls.
- User/session inventory:
  - Current state: Linux `w`/`who` template and custom command source are
    selectable.
  - Remaining model: typed parsed JSON output, degraded hints when unavailable
    or unprivileged, and panel/CLI/VTY source controls.
- Command execution:
  - Current state: shell-script argv prefix, environment policy,
    working-directory policy, PTY launcher policy, explicit env values, and
    process cleanup policy are configurable through the
    `command_execution_policy` template domain. Explicit argv remains the
    preferred default.
  - Remaining model: richer UI/CLI/VTY template-management ergonomics and live
    smoke evidence for every policy field.
- Network probes and speed tests:
  - Current state: ping argv is configurable; speed test is built-in TCP.
  - Required model: custom latency/speedtest providers with typed JSON output,
    provider templates, and source/status visibility.
- Traffic shaping and limits:
  - Current state: typed `tc` apply commands exist.
  - Required model: status source, rollback source, provider defaults, and
    non-tunnel flow-limit adapters.
- Routing-cost integration:
  - Current state: daemon-neutral operator-created adapter templates, explicit
    per-endpoint bindings, status-before-apply, stale-snapshot checks, and
    status-after-apply verification are implemented.
  - Remaining model: richer adapter preflight evidence and source-status links;
    no daemon-specific integration belongs in vpsman.
- Backup, restore, and update:
  - Current state: local filesystem and S3-compatible object stores are
    implemented for backup/update artifacts; source template status reports
    selected backup/update templates, server object-store kind/configuration,
    configured artifact maximums, backup artifact counts, backup request
    counts, restore/migration linkage, update release counts, and job execution evidence.
  - Required model: richer typed object-store template assignment, direct and
    resumable artifact handoff ergonomics, artifact hosting adapters, restore
    path mapping templates, update restart adapters, and heartbeat source
    selection.
- Install/runtime defaults:
  - Current state: agent config path and supervisor state path have sensible
    Linux defaults and command-line/config overrides in some paths.
  - Required model: document every install/runtime path as either installer
    policy, agent config, server config, or test fixture; avoid introducing
    new implicit global paths.
- Frontend frequent-use configuration:
  - Current state: Automation > Source templates provides a source template
    manager backed by server storage. Operators can save shared templates, save
    VPS-local templates, see built-in/default templates and assignment counts,
    and assign the selected template by VPS or tag with API confirmation
    semantics. The panel, API, CLI, and VTY can clone, diff, test, update, and
    preview the read-only config patch rendered from a VPS's source templates.
    The first Active source status read model is visible in API, CLI/VTY, and
    Source templates, including backup/update object-store readiness evidence,
    privilege-gated on-demand workflow evidence, and process-limit capability
    readiness for root-capable, unknown, and unprivileged agents. Source
    templates now uses shared CRUD/list controls for active source rows and
    template registry rows, including total/filtered counts, current page,
    field search, and page controls; the same abstraction is used for audit,
    job, and schedule record tables.
  - Required remaining model: active-template badges in each operational module,
    deeper source/status linkage for restore/update/routing/traffic-limit
    workflows, richer curated provider libraries, and privilege-gated dispatch of
    rendered fragments.

## Audit Method For Future Batches

1. Search changed modules for hardcoded executable paths, filesystem paths,
   parser assumptions, fixed providers, fixed intervals, and UI-only option
   sets.
2. Classify each as `test fixture`, `built-in template`, `shared customizable
   template`, `VPS-local custom template`, `typed adapter field`, or `gap`.
3. Convert P0/P1 business assumptions into typed source models when the owner
   module is clear.
4. Add tests that reject unsafe custom-command template fields and prove at least
   one non-default template can be selected for a VPS.
5. Record converted assumptions and remaining gaps in public design notes or
   local private progress notes as appropriate.

## Latest Scan Notes

2026-06-02 later scan:

- Added `scripts/audit-customizability.sh`. The script scans command/path
  assumptions such as `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/etc`, `/proc`,
  `/sys/class`, `vnstat`, and `ping`; classifies template/config-backed matches;
  and reports open candidates without failing by default.
- Current result: `total_matches=565`, `classified_matches=463`,
  `open_candidates=102`.
- This historical open set became the audit backlog. It included frontend placeholders,
  compatibility backend paths, `w`/`who` discovery, former network hook templates,
  ping/vnstat parsing, installer paths, backup examples, and process/terminal
  default examples. Each later conversion was required to turn the assumption
  into a typed template/adapter field, mark it as a fixture/operator-input hint,
  or leave a concrete TODO with an owner module.
- Source template bulk update must remain template-centric: updating a shared template
  definition updates the model selected by all assigned VPSs after review.
  Tag/client assignment only changes which VPSs select a template.
- Curated built-in source templates now exist beyond defaults:
  host-mounted proc/sys telemetry, `vnstat` JSON traffic accounting, pinned
  `/usr/bin/ping`, host-mounted process inventory, pinned `w`/`who`, BusyBox
  `ash`, runtime iproute2/tc reconciliation, S3/MinIO backup object storage,
  and external HTTPS/GitHub update artifact metadata with SHA-256 verification.
  These are selectable templates, not automatic defaults.

2026-06-02 03:19 PDT scan:

- Accepted default templates already documented include ping, procfs, sysfs,
  local filesystem object storage, command execution, and bounded built-in
  network probes. External tunnel and routing-cost adapters deliberately have
  no built-in/default template and no managed compatibility files.
- Test fixture paths such as `/tmp/...`, `/etc/hostname`, `/bin/sleep`,
  `/bin/sh` inside `#[cfg(test)]` modules, and adapter example scripts appear
  heavily in API/CLI/VTY tests and are not product policy by themselves.
- Product-code hotspot: `crates/agent/src/network_speed.rs` implements one
  built-in TCP throughput provider. It is bounded and useful, but the business
  model is incomplete until speed tests become a selectable provider model with
  built-in TCP as a template plus shared custom and VPS-local custom
  JSON/provider templates with source/status reporting.

2026-06-02 FOU runtime option conversion:

- Converted the prior fixed FOU assumptions (`port=5555`, `peer_port=5555`,
  `ipproto=4`) into `RuntimeTunnelFouOptions` with serde defaults and
  validation. Non-default values flow through agent runtime commands, external
  adapter placeholders, CLI, VTY, and Network > Tunnel plans.
- This is a typed per-plan field model, not a source-template bulk command.
  Core iproute2 realization stays visible on each declaration; shared templates
  are reserved for external operator-owned implementations.
- Telemetry promotion and inferred tunnel import are intentionally absent.
  Operators create declarations with explicit bandwidth, ownership, endpoints,
  and optional routing policy.
- Port forwarding is a typed, event-driven desired-state model rather than an
  adapter or discovery source. Operators explicitly claim ports, pin one
  literal same-family target address, and choose targeted masquerade or source
  preservation. The agent owns only `inet vpsman_port_forward`, never discovers
  external DNAT rules, never edits another nftables/iptables table, and never
  installs nftables or changes forwarding sysctls.
- Product-code hotspot: frontend topology controls contain fixed probe/speed
  defaults. These should evolve into saved operator templates with source/provider
  visibility and per-VPS selected-template display, while keeping safe bounded
  built-in defaults for frequent use.
- Product-code hotspot: backup/restore/update flows still need typed adapter
  models for object-store provider selection, restore path mapping, update
  artifact source, restart/heartbeat policy, and rollback source evidence.
- Product-code hotspot: command execution and terminal policy now have
  source-selectable environment, PTY launcher, working-directory, and cleanup
  templates; remaining risk is keeping frontend/CLI/VTY template-management
  ergonomics aligned as more policy fields are added.
- Product-code hotspot: UI/CLI/VTY source-selection controls are incomplete.
  Agent TOML can now select several sources, but professional 20+ VPS operation
  requires panel and headless controls to inspect active templates, test templates,
  clone/customize shared templates, create VPS-local custom templates, and bulk
  assign template selections with preview and audit.

2026-06-02 08:27 PDT scan:

- Converted another batch of hidden/default assumptions into explicit template
  catalogs or named template constants: backup/restore path defaults,
  job-operation placeholders, user-session executable candidates, and
  latency-probe executable candidates.
- `scripts/release-check.sh` now runs `scripts/audit-customizability.sh` after
  repository hygiene, making customizability review part of the aggregate
  release gate.
- Scanner fixes classify UI placeholders and search for the real `/proc` path
  family instead of matching `/api/v1/process...`. Network managed-file paths
  are no longer an accepted product category.
- Current result: `total_matches=548`, `classified_matches=510`,
  `open_candidates=38`.
- Remaining open candidates are not release-complete. The current list is
  mostly installer/autostart policy paths, shell/test fixtures, privilege/backup
  example paths, vnstat parser/status naming, process/terminal example argv,
  and protocol test payloads. Future batches should either convert each
  production assumption into a typed template/adapter model or classify it as a
  test fixture/operator-input example with an owner and rationale.

2026-06-02 08:38 PDT scan:

- Converted the remaining open audit classes for the current scanner terms:
  installer root/service locations are named installer-policy templates, `vnstat`
  traffic accounting parsing/status names explicitly identify the selectable
  template, and representative shell/path/protocol literals in tests are named
  fixtures.
- The scanner now classifies shebangs as script format requirements instead of
  source template assumptions.
- Current result: `total_matches=543`, `classified_matches=543`,
  `open_candidates=0`.

2026-07-15 port-forwarding ownership review:

- The only fixed system identifier is the deliberately exclusive nftables
  table name `inet vpsman_port_forward`. It is the cleanup and drift boundary,
  not an operator-customizable integration path.
- Interface names and local destination addresses are intentionally absent.
  Rules match only destinations the kernel classifies as local, so changing
  cloud addresses and interfaces does not require rule rewrites and transit
  forwarding traffic is not intercepted.
- Hostnames remain an authoring convenience. The control plane returns current
  candidates for an operator to select; desired state stores only the selected
  literal address and never changes it through background DNS heuristics.
- This closes the current hardcode-audit open-candidate backlog, but not the
  broader customizability program. Semantic gaps remain for speed-test provider
  templates, restore path mapping templates, terminal/PTY policy templates, richer
  workflow-specific active source/status surfaces, and new modules that have
  not yet been introduced.

2026-06-04 release-gate scan:

- `scripts/release-check.sh` passed
  (`release_check=ok log_dir=target/release-check/20260604-052636`) and ran
  `scripts/audit-customizability.sh`.
- Current result: `total_matches=594`, `classified_matches=594`,
  `open_candidates=0`.
- Release acceptance: no known hardcoded provider/path/command assumption is
  open for the documented local object-store baseline. Future modules must keep
  environment-specific behavior as built-in templates, shared custom templates,
  VPS-local custom templates, adapter fields, fixtures, or explicit operator
  inputs rather than hidden business policy.
