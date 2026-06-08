# Customizability And Data-Source Audit

This audit tracks environment-dependent business assumptions that must be
modeled as data-source presets, preset-backed adapters, or explicitly recorded
as gaps. The product target is 20+ heterogeneous VPSs, so one Linux layout, one
binary path, or one accounting source is not enough.

## Acceptance Principle

- Defaults are allowed only as documented presets.
- The managed business object is the preset and the VPS's assignment to that
  preset. Every applicable VPS should have an explicit selected preset for each
  data-source domain, even when that selected preset is the built-in default.
- Operators should manage built-in default presets, shared customizable
  presets, and VPS-local custom presets through preset management. Tag and bulk
  workflows are convenience tools for assigning, cloning, testing, or updating
  those preset objects at scale; they are not ad hoc command updates.
- Parsed custom sources must use absolute argv, bounded timeout/output, no
  implicit shell, typed JSON output, redacted status, and tests. Custom commands
  are implementation fields inside a preset, not the top-level abstraction.
- UI/CLI/VTY should show source/status where operators rely on the data.
- Thoroughness is required: auditing one path literal is not enough. Each
  workflow must be inspected across agent runtime, API/storage, CLI, VTY,
  frontend, tests, docs, degraded/unprivileged behavior, and operator
  migration. A converted backend source is still partial if the panel/headless
  tools cannot show the active preset, test it, and assign/clone/customize it
  for real 20+ VPS operations.

## Converted In Current Slice

- General agent telemetry:
  - Default preset: `linux_procfs`.
  - Selectable sources: `linux_procfs`, `custom_command`,
    `linux_procfs_and_custom_command`.
  - Configurable paths: `proc_root`, `sys_class_net_dir`, `hostname_file`,
    `os_release_file`.
  - Custom source: bounded JSON command can replace or overlay hostname,
    uptime, CPU, memory, disk, network, and tunnel metrics.
  - Preset-domain target: `telemetry_metrics_source`, with built-in default,
    shared custom, and VPS-local custom presets.
- Runtime tunnel traffic accounting:
  - Default source: interface counters.
  - Preset source: configurable `runtime_vnstat_argv`, with vpsman appending
    `--json -i <interface>`.
  - Custom source: per-plan bounded JSON command for provider/application
    accounting.
  - Preset-domain target: `runtime_traffic_accounting_source`; `vnstat` should
    become a shared customizable preset, not a hardcoded or one-off command.
- Runtime tunnel adapters:
  - External adapters can define startup, restart, status, traffic-limit, stop,
    and cleanup commands with typed placeholder expansion.
  - Preset-domain target: `runtime_tunnel_adapter`, where adapter command sets
    are preset fields and VPSs select the adapter preset applicable to the
    tunnel or provider.
- FOU runtime realization:
  - Default preset values: port `5555`, peer port `5555`, IP protocol `4`.
  - Selectable fields: per-plan typed FOU port, peer port, and IP protocol.
  - Covered surfaces: shared Rust model, ifupdown/systemd-networkd
    compatibility rendering, agent runtime `ip fou` and `ip tunnel`
    commands, adapter placeholders, CLI, VTY, and topology panel authoring.
  - Preset-domain target: `runtime_tunnel_realization_policy`; FOU defaults
    should eventually live alongside other tunnel realization defaults instead
    of only as shared model defaults.
- ICMP latency probes:
  - Configured source: `[network].probe_ping_argv`.
  - Default preset candidates: `/bin/ping`, `/usr/bin/ping`.
  - Status records whether configured argv or preset was used.
  - Preset-domain target: `latency_probe_source`, with built-in Linux ping and
    custom probe presets.
- Process inventory:
  - Default source: configurable Linux procfs root through
    `[execution].process_proc_root`.
  - Selectable source: bounded custom JSON command through
    `[execution].process_inventory_source = "custom_command"`.
  - Preset-domain target: `process_inventory_source`.
- User/session inventory:
  - Default preset: Linux `w`/`who` candidate source.
  - Selectable source: bounded configured/custom command through
    `[execution].user_sessions_command`.
  - Preset-domain target: `user_session_inventory_source`.
- Shell-script execution:
  - Default preset: `[execution].shell_script_argv = ["/bin/sh", "-lc"]`.
  - The shell prefix, working directory, environment policy, explicit env
    values, PTY policy, and cleanup policy are configurable, validated, and
    surfaced in command/terminal status metadata where relevant. Explicit argv
    remains the preferred command mode.
  - Preset-domain target: `command_execution_policy`; shell prefix,
    environment, PTY launcher, cwd, and cleanup policy live in a selected
    policy preset rather than scattered command defaults.
- Agent update restart request:
  - Activation no longer shells out to request a supervised restart; it uses an
    internal delayed `SIGTERM` request.
- Frontend operation defaults and compatibility paths:
  - Topology compatibility backend managed-file paths are now served from a
    network-backend preset catalog rather than embedded directly in panel and
    renderer logic.
  - Backup/restore selected-path defaults and placeholders are named backup
    path presets.
  - Job-operation command examples, terminal default argv, and backup path
    examples are named job-operation presets.
  - Preset-domain target: these are still frontend convenience/default
    catalogs, not the full server-side selected-preset model for every
    workflow. Future work should connect each operational panel to active
    source/status and assignment controls where the default affects real VPS
    behavior.
- Agent executable and compatibility command candidates:
  - Network compatibility backend validation/reload commands, Bird2/netplan
    paths, user-session executable candidates, and latency-probe executable
    candidates are now named preset/constants instead of scattered literals.
  - The audit scanner now treats vpsman-managed compatibility files as
    accepted adapter paths and avoids false positives where `/proc` appeared
    inside `/api/v1/process...`.

## Remaining High-Priority Gaps

- Process inventory:
  - Current state: Linux procfs and custom JSON command are selectable.
  - Remaining model: deeper supervisor-only source controls, systemd/cgroup
    enrichment beyond current supervisor limit evidence, and panel/CLI/VTY
    source controls.
- User/session inventory:
  - Current state: Linux `w`/`who` preset and custom command source are
    selectable.
  - Remaining model: typed parsed JSON output, degraded hints when unavailable
    or unprivileged, and panel/CLI/VTY source controls.
- Command execution:
  - Current state: shell-script argv prefix, environment policy,
    working-directory policy, PTY launcher policy, explicit env values, and
    process cleanup policy are configurable through the
    `command_execution_policy` preset domain. Explicit argv remains the
    preferred default.
  - Remaining model: richer UI/CLI/VTY preset-management ergonomics and live
    smoke evidence for every policy field.
- Network probes and speed tests:
  - Current state: ping argv is configurable; speed test is built-in TCP.
  - Required model: custom latency/speedtest providers with typed JSON output,
    provider presets, and source/status visibility.
- Traffic shaping and limits:
  - Current state: typed `tc` apply commands exist.
  - Required model: status source, rollback source, provider defaults, and
    non-tunnel flow-limit adapters.
- Routing daemon integration:
  - Current state: Bird2 first.
  - Required model: routing-daemon adapter contract for Bird2, FRR, and custom
    commands; topology and OSPF-like cost policy should remain daemon-neutral.
- Backup, restore, and update:
  - Current state: local filesystem object store with reserved S3 extension;
    data-source status reports selected backup/update presets, server
    object-store kind/configuration, backup artifact counts, backup request
    counts, restore/migration linkage, update release counts, rollout counts,
    failed rollout counts, and rollout automation readiness.
  - Required model: typed object-store adapters, direct/resumable artifact
    handoff, artifact hosting adapters, restore path mapping presets, update
    restart adapters, and heartbeat source selection.
- Install/runtime defaults:
  - Current state: agent config path and supervisor state path have sensible
    Linux defaults and command-line/config overrides in some paths.
  - Required model: document every install/runtime path as either installer
    policy, agent config, server config, or test fixture; avoid introducing
    new implicit global paths.
- Frontend frequent-use configuration:
  - Current state: the Tags view now has a data-source preset manager backed by
    server storage. Operators can save shared presets, save VPS-local presets,
    see built-in/default presets and assignment counts, and assign the selected
    preset by VPS or tag with API confirmation semantics. The panel, API, CLI,
    and VTY can clone, diff, test, update, and preview the read-only hot-config
    fragment rendered from a VPS's selected presets. The first Active source
    status read model is visible in API, CLI/VTY, and the Tags panel, including
    backup/update object-store readiness evidence, privilege-gated on-demand
    workflow evidence, and process-limit capability readiness for root-capable,
    unknown, and unprivileged agents. The Tags panel now uses shared CRUD/list
    controls for active source rows and preset registry rows, including
    total/filtered counts, current page, field search, and page controls; the
    same abstraction is used for audit, job, and schedule record tables.
  - Required remaining model: active-preset badges in each operational module,
    deeper source/status linkage for restore/update/routing/traffic-limit
    workflows, richer curated provider libraries, and privilege-gated dispatch of
    rendered fragments.

## Audit Method For Future Batches

1. Search changed modules for hardcoded executable paths, filesystem paths,
   parser assumptions, fixed providers, fixed intervals, and UI-only option
   sets.
2. Classify each as `test fixture`, `built-in preset`, `shared customizable
   preset`, `VPS-local custom preset`, `typed adapter field`, or `gap`.
3. Convert P0/P1 business assumptions into typed source models when the owner
   module is clear.
4. Add tests that reject unsafe custom-command preset fields and prove at least
   one non-default preset can be selected for a VPS.
5. Record converted assumptions and remaining gaps in public design notes or
   local private progress notes as appropriate.

## Latest Scan Notes

2026-06-02 later scan:

- Added `scripts/audit-customizability.sh`. The script scans command/path
  assumptions such as `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/etc`, `/proc`,
  `/sys/class`, `vnstat`, and `ping`; classifies preset/config-backed matches;
  and reports open candidates without failing by default.
- Current result: `total_matches=565`, `classified_matches=463`,
  `open_candidates=102`.
- The open set is now the audit backlog. It includes frontend placeholders,
  compatibility backend paths, `w`/`who` discovery, network hook presets,
  ping/vnstat parsing, installer paths, backup examples, and process/terminal
  default examples. Each future conversion should either promote the assumption
  into a typed preset/adapter field, mark it as a fixture/operator-input hint,
  or leave a concrete TODO with an owner module.
- Data-source bulk update must remain preset-centric: updating a shared preset
  definition updates the model selected by all assigned VPSs after review.
  Tag/client assignment only changes which VPSs select a preset.
- Curated built-in data-source presets now exist beyond defaults:
  host-mounted proc/sys telemetry, `vnstat` JSON traffic accounting, pinned
  `/usr/bin/ping`, host-mounted process inventory, pinned `w`/`who`, BusyBox
  `ash`, runtime iproute2/tc reconciliation, reserved S3/MinIO backup object
  storage, and signed HTTPS update artifacts. These are selectable presets, not
  automatic defaults.

2026-06-02 03:19 PDT scan:

- Accepted default presets already documented: runtime `ip`/`tc`, ifupdown,
  netplan, systemd-networkd, Bird2, ping, procfs, sysfs, local filesystem
  object store, and managed compatibility files. These remain accepted only
  while they are visible as presets or compatibility adapters instead of hidden
  product assumptions.
- Test fixture paths such as `/tmp/...`, `/etc/hostname`, `/bin/sleep`,
  `/bin/sh` inside `#[cfg(test)]` modules, and adapter example scripts appear
  heavily in API/CLI/VTY tests and are not product policy by themselves.
- Product-code hotspot: `crates/agent/src/network_speed.rs` implements one
  built-in TCP throughput provider. It is bounded and useful, but the business
  model is incomplete until speed tests become a selectable provider model with
  built-in TCP as a preset plus shared custom and VPS-local custom
  JSON/provider presets with source/status reporting.

2026-06-02 FOU runtime option conversion:

- Converted the prior fixed FOU assumptions (`port=5555`, `peer_port=5555`,
  `ipproto=4`) into `RuntimeTunnelFouOptions` with serde defaults and
  validation. Non-default values now render through compatibility backends,
  agent runtime commands, CLI, VTY, and the Topology panel.
- This is a typed adapter-field conversion, not a data-source bulk command
  model. Fleet-wide changes should be modeled later as updating a named tunnel
  realization preset selected by relevant VPSs/plans, then validating/rendering
  the affected plans before privilege-gated mutation.
- Product-code hotspot: telemetry promotion defaults in
  `crates/api/src/routes_network.rs` currently fall back to fixed bandwidth,
  latency, packet-loss, preference, and OSPF policy values. Those should become
  named operator presets/profiles with explicit VPS or tag assignment helpers
  rather than compiled business policy.
- Product-code hotspot: frontend topology controls contain fixed probe/speed
  defaults. These should evolve into saved operator presets with source/provider
  visibility and per-VPS selected-preset display, while keeping safe bounded
  built-in defaults for frequent use.
- Product-code hotspot: backup/restore/update flows still need typed adapter
  models for object-store provider selection, restore path mapping, update
  artifact source, restart/heartbeat policy, and rollback source evidence.
- Product-code hotspot: command execution and terminal policy now have
  source-selectable environment, PTY launcher, working-directory, and cleanup
  presets; remaining risk is keeping frontend/CLI/VTY preset-management
  ergonomics aligned as more policy fields are added.
- Product-code hotspot: UI/CLI/VTY source-selection controls are incomplete.
  Agent TOML can now select several sources, but professional 20+ VPS operation
  requires panel and headless controls to inspect active presets, test presets,
  clone/customize shared presets, create VPS-local custom presets, and bulk
  assign preset selections with preview and audit.

2026-06-02 08:27 PDT scan:

- Converted another batch of hidden/default assumptions into explicit preset
  catalogs or named preset constants: frontend topology backend files,
  backup/restore path defaults, job-operation placeholders, agent network-hook
  compatibility commands, user-session executable candidates, and latency-probe
  executable candidates.
- `scripts/release-check.sh` now runs `scripts/audit-customizability.sh` after
  repository hygiene, making customizability review part of the aggregate
  release gate.
- Scanner fixes classify vpsman-managed compatibility files as accepted
  adapter paths, classify UI placeholders, and search for the real `/proc` path
  family instead of matching `/api/v1/process...`.
- Current result: `total_matches=548`, `classified_matches=510`,
  `open_candidates=38`.
- Remaining open candidates are not release-complete. The current list is
  mostly installer/autostart policy paths, shell/test fixtures, privilege/backup
  example paths, vnstat parser/status naming, process/terminal example argv,
  and protocol test payloads. Future batches should either convert each
  production assumption into a typed preset/adapter model or classify it as a
  test fixture/operator-input example with an owner and rationale.

2026-06-02 08:38 PDT scan:

- Converted the remaining open audit classes for the current scanner terms:
  installer root/service locations are named installer-policy presets, `vnstat`
  traffic accounting parsing/status names explicitly identify the selectable
  preset, and representative shell/path/protocol literals in tests are named
  fixtures.
- The scanner now classifies shebangs as script format requirements instead of
  data-source assumptions.
- Current result: `total_matches=543`, `classified_matches=543`,
  `open_candidates=0`.
- This closes the current hardcode-audit open-candidate backlog, but not the
  broader customizability program. Semantic gaps remain for speed-test provider
  presets, restore path mapping presets, terminal/PTY policy presets, richer
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
  environment-specific behavior as built-in presets, shared custom presets,
  VPS-local custom presets, adapter fields, fixtures, or explicit operator
  inputs rather than hidden business policy.
