# Tutorial 06: Tunnels, Topology, And Routing Adapters

vpsman manages only saved tunnel declarations. It does not discover or import
unmanaged tunnels, silently choose a runtime owner, or install host networking
software. An operator selects one runtime ownership mode for each plan:

| Ownership mode | What vpsman manages | What remains externally owned | Disable or delete |
| --- | --- | --- | --- |
| **Agent builtin** | The vpsman agent's kind-specific built-in driver creates, configures, observes, and removes the exact declared endpoint. | The operator installs the required host tools or kernel support. OSPF and other routing daemons remain separate. | The agent removes only the interface, process, credentials, routes, and shaping state owned by that plan and driver. |
| **External observed** | The agent reads the exact declared interface for status, counters, probes, and tests. | Another system owns all configuration, credentials, processes, routes, and cleanup. | vpsman stops observing; it never mutates the tunnel. |
| **Custom adapter** | The agent invokes the selected operator-supplied lifecycle commands as bounded argv and records their results. | The adapter owns its implementation, credentials, process, routes, and daemon state. | vpsman invokes only the bound stop or cleanup command and never deletes the adapter executable. |

**Agent builtin** is an ownership boundary, not another name for iproute2. GRE,
IPIP, SIT, and FOU use the built-in iproute2 driver. WireGuard and OpenVPN fit
the same ownership mode when their kind-specific driver and endpoint
prerequisites are available. Missing prerequisites remain an explicit failed or
degraded state; vpsman never falls back to either non-builtin mode.

OSPF cost control is separate and off by default. Each endpoint normally uses
that VPS's effective `ospf_update_command` Configuration preset. A tunnel
plan may optionally select a routing-cost adapter definition as an endpoint
override. The plan override takes precedence; an invalid override is reported
and never bypassed with a preset fallback. vpsman never installs, edits, or
removes the command or routing stack it controls.

## Console Workflow

Open **Network > Tunnel plans**. The registry remains visible while create or
edit opens below it. Each plan shows:

- the exact endpoint VPSs and interface;
- runtime ownership and enabled state;
- current left/right runtime evidence derived from that declaration only;
- optional OSPF mode and updater status;
- edit, export, credential rotation where supported, enable, disable, and
  retire actions.

Choose **Create plan**, select the runtime owner, enter endpoints and addresses,
and optionally enable OSPF. The cost preview updates beside latency, loss,
bandwidth, and preference. To replace an existing declaration, select the plan
and choose **Actions > Edit**; plan identity is fixed during edit so a rename
cannot accidentally create another plan. The editor submits the displayed
declaration revision; if another operator changes the plan first, the update is
rejected and the editor must be reopened against current state.

Runtime adapters and optional per-plan OSPF overrides live under **Network >
Tunnel plans > Adapter definitions**. Reusable VPS-level OSPF commands live
under **Config > Sources** as `ospf_update_command` presets. No OSPF
implementation is built in.

**Network > Graph** contains only endpoints referenced by saved plans. Fleet
VPSs with no declared tunnel remain in Fleet rather than appearing as inferred
or disconnected topology nodes.

## Desired-State Convergence

The control plane sends the complete explicit enabled-plan set after create,
update, enable, disable, retirement, and agent reconnect. The agent reconciles
only those declarations and their previous exact snapshots. It never scans the
host to decide what belongs to vpsman.

Retiring a plan removes its declaration from control-plane desired state
immediately, whether the plan is currently enabled or disabled. Both endpoints
receive the complete desired state without that plan. Agent builtin removes
only the old state owned by its kind-specific driver; the Custom adapter
mode runs only its bound stop/cleanup command; External observed stops
observation without mutating the external tunnel. If an endpoint is offline or
managed cleanup fails, the retirement remains committed and runtime
convergence remains visible in Jobs and **Config > Overview**. The agent reconciles the
current plan-free desired state on reconnect; an omitted plan is not reported as
converged until its declared cleanup succeeds.

## Outer Address Semantics (NAT-Safe)

Each endpoint has its own remote destination and optional local source. These
are command arguments for the VPS on that side, not two names for one address:

| Command runs on | Required remote destination | Optional local source |
| --- | --- | --- |
| Left VPS | `left_remote_underlay`: peer address reachable from the left VPS | `left_local_underlay`: address bindable on the left VPS |
| Right VPS | `right_remote_underlay`: peer address reachable from the right VPS | `right_local_underlay`: address bindable on the right VPS |

For example, suppose the left VPS has private address `10.0.0.10` behind public
address `198.51.100.10`, and the right VPS has private address `10.0.1.20`
behind public address `203.0.113.20`. The exact declaration is:

```text
left:  source 10.0.0.10 -> destination 203.0.113.20
right: source 10.0.1.20 -> destination 198.51.100.10
```

The public/NAT destination does not need to exist on the peer's interface, and
neither public address is copied into a local-source field. vpsman never derives
one side from the other. A built-in driver either binds the declared local
source when that tunnel kind supports it or rejects the unsupported
combination; it never invents policy routing. A custom adapter receives an
empty `{local_underlay}` placeholder when no source was declared. Each
non-empty local source must use the same address family as that endpoint's
remote destination.

## Address Allocation

Allocation pools are optional. Configure persistent pools in **System > Suite
config**, pass a pool to `tunnel-allocate`, or enter endpoint addresses
manually. Pools must not overlap real networks.

```sh
cargo run -p vpsctl -- tunnel-allocate \
  --ipv4-pool-cidr 10.255.0.0/16 \
  --ipv6-pool-cidr fd80::/80 \
  --reserved-addresses 10.255.0.0,fd80:: \
  --include-ipv4 \
  --include-ipv6
```

The allocation call has no planning or runtime side effects. A saved plan must
contain an explicit IPv4 pair, IPv6 pair, or both. Each pair must contain two
different addresses in the declared prefix. Saved plans cannot reuse an overlay
address, reuse the same interface name on a VPS, or claim the same Agent builtin
listener transport and port on one VPS. The API rejects these conflicts on both
create and edit before runtime config is queued. Disabled saved plans retain
their claims; TCP and UDP may use the same numeric port.

## Agent builtin inputs and prerequisites

The selected tunnel kind determines the driver and its small additional input
set. GRE, IPIP, SIT, FOU, WireGuard, and OpenVPN have built-in implementations.
Selecting one never reclassifies an existing External observed or Custom
adapter plan. Driver availability is endpoint evidence, not permission to
install packages or change firewall policy:

| Kind | Additional plan inputs | Endpoint prerequisites and ownership |
| --- | --- | --- |
| GRE, IPIP, SIT, FOU | Existing underlay addresses and kind-specific FOU ports/protocol where applicable. | Configured `ip` and `tc` commands, Linux support for the selected kind, and root execution under the default mutation gate. The agent owns the declared link, addresses, MTU, routes, and shaping. |
| WireGuard | Fixed VPS (`left`, `right`, or `both`, default `both`); left and right UDP listen ports (default `51820`); left and right persistent-keepalive seconds (`25` recommended, `0` disables it). | Configured `ip` and `wg` commands, kernel WireGuard support, and root execution. In a one-sided mode the roaming VPS receives the fixed VPS destination and should initiate traffic; the fixed VPS omits the roaming destination and learns it from authenticated WireGuard traffic. The enabled IPv4/IPv6 families use a full-family peer ACL so static and OSPF-learned routes can traverse the point-to-point link; this does not install a default route. WireGuard has no TCP mode or direct local-source bind setting. |
| OpenVPN | Transport (`UDP` or `TCP`), listener side (`left` or `right`), and listener port (default `1194`). | Configured `openvpn` 2.4 or newer (verified with 2.4–2.6), `/dev/net/tun`, and root execution under the default mutation gate. The agent selects the installed version's supported cipher directive. The listener is the TLS server; the other endpoint is the TLS client, including complementary `tcp-server`/`tcp-client` roles for TCP. |

The common plan fields continue to supply interface name, endpoint VPSs,
independent NAT-safe remote destinations, inner IPv4/IPv6 pairs, endpoint MTUs,
routes, bandwidth, shaping, and optional OSPF control. A WireGuard or OpenVPN
driver must report a missing command, kernel feature, TUN device, privilege, or
credential as explicit endpoint evidence. It must not reinterpret the plan or
run a custom adapter. Local runtime convergence and peer reachability remain
separate: use the existing probe and observation evidence for connectivity.

### Built-in credentials

The control plane generates both endpoint identities for Agent builtin
WireGuard and OpenVPN plans and stores them separately from the reviewed plan
declaration. Each endpoint runtime config receives its own private identity and
only the peer's public key or endpoint-specific issuer certificate. OpenVPN
2.4–2.6 therefore use the same exact peer trust even though their supported
cipher directive names differ. The console shows only public keys or
certificate fingerprints and credential generation. Audit and command output
omit private material.

**Export** returns the reviewed tunnel declaration without credentials. Control
plane backups retain the persistent database state needed for service recovery.
**Rotate credentials** replaces both endpoint identities at one reviewed plan
revision. An enabled plan then queues both endpoint runtime syncs; a disabled
plan receives the new identities when next enabled. Ordinary bandwidth, MTU,
route, or shaping edits keep the current credential generation.

```sh
cargo run -p vpsctl -- tunnel-plan-rotate-credentials \
  --plan-id <plan_uuid> \
  --expected-revision <reviewed_revision> \
  --confirmed
```

## Workflow 1: Agent builtin

Use this when the selected kind has a built-in driver and both endpoints meet
that driver's prerequisites. This iproute2 example uses GRE. OSPF remains off
unless `--ospf` is supplied.

```sh
cargo run -p vpsctl -- tunnel-plan \
  --name edge-a-edge-b \
  --interface-name gre42 \
  --kind gre \
  --runtime-manager builtin \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-remote-underlay 203.0.113.20 \
  --left-local-underlay 10.0.0.10 \
  --right-remote-underlay 198.51.100.10 \
  --right-local-underlay 10.0.1.20 \
  --left-tunnel-ipv4-cidr 10.255.0.0/31 \
  --right-tunnel-ipv4-cidr 10.255.0.1/31 \
  --bandwidth-mbps 1000 \
  --save \
  --enabled \
  --confirmed
```

Enabling pushes the exact declaration to both agents. Disabling removes only
the tunnel state owned by this plan and stops its telemetry and future OSPF
control. It does not revert the current cost in an operator-owned routing
daemon; use that daemon's own workflow when a cost rollback is required:

```sh
cargo run -p vpsctl -- tunnel-plan-disable \
  --plan-id <plan_uuid> \
  --expected-revision <revision> \
  --confirmed

cargo run -p vpsctl -- tunnel-plan-enable \
  --plan-id <plan_uuid> \
  --expected-revision <current_revision> \
  --confirmed
```

Deletion is a separate retirement step, not another spelling of disable. It is
accepted for an enabled or disabled plan at the exact reviewed revision.
Retirement commits immediately, records whether the plan was active in audit
metadata, queues the complete plan-free desired state for both endpoints, and
releases the plan name, per-VPS interface reservation, and endpoint addresses
and local listener reservations for reuse:

```sh
cargo run -p vpsctl -- tunnel-plan-delete \
  --plan-id <plan_uuid> \
  --expected-revision <reviewed_plan_revision> \
  --confirmed
```

The console exposes the same direct delete action for either lifecycle state and
shows the frozen revision, endpoints, current state, runtime effect, and OSPF
impact before confirmation. Deletion does not wait for an endpoint response. If
a local mutation gate or insufficient privilege blocks managed cleanup, the
agent retains the old plan snapshot and reports the failed removal job; the plan
remains retired, and the endpoint retries against current desired state on its
next reconnect or authoritative sync. Do not treat runtime removal as converged
until both endpoint sync jobs succeed.

Changing an immutable runtime identity, such as interface, kind, endpoint VPS,
or endpoint address, removes the old declared state before reconciling the new
state. GRE, IPIP, SIT, and FOU underlay changes are also identity changes.
WireGuard applies underlay, listener, roaming, keepalive, and MTU edits to its
existing interface; OpenVPN restarts only its owned process when an underlay or
runtime setting changes. Routes and stale interfaces remain explicit: the
agent never guesses what else is safe to delete. Agent-owned route and
stale-interface fields are accepted only in Agent builtin mode.

## Workflow 2: External observed

Use this when another system owns the tunnel process and configuration, but
vpsman should show the exact interface in topology, telemetry, probes, and
speed tests.

```sh
cargo run -p vpsctl -- tunnel-plan \
  --name edge-a-edge-b-openvpn \
  --interface-name ovpn42 \
  --kind openvpn \
  --runtime-manager external_observed \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-remote-underlay 203.0.113.20 \
  --left-local-underlay 10.0.0.10 \
  --right-remote-underlay 198.51.100.10 \
  --right-local-underlay 10.0.1.20 \
  --left-tunnel-ipv4-cidr 10.255.10.0/31 \
  --right-tunnel-ipv4-cidr 10.255.10.1/31 \
  --bandwidth-mbps 750 \
  --save \
  --enabled \
  --confirmed
```

The agent checks only `ovpn42`. It does not scan for other tunnel-like
interfaces and cannot promote an interface into management. Disabling or
retiring the declaration stops observation; it does not stop or delete the
externally owned tunnel.
Mutation controls, traffic shaping, FOU options, and agent-owned route cleanup
are rejected in this mode. Observation works without enabling network mutation.

## Workflow 3: Custom adapter

Use a custom adapter when vpsman should start, stop, clean up, or check an
operator-owned tunnel implementation. The executable must already exist on the
endpoint. vpsman invokes direct argv without a shell.

In **Adapter definitions**, choose **Tunnel runtime adapter** and enter each
command as one argument per line. The form produces the following contract,
which is also visible under **Advanced contract preview**:

```json
{
  "manager": "custom_adapter",
  "contract_version": 1,
  "startup_command": {
    "argv": ["/opt/operator/tunnel-adapter", "start", "--interface", "{interface}", "--kind", "{kind}", "--local-underlay", "{local_underlay}", "--remote-underlay", "{remote_underlay}", "--local-address", "{local_address}", "--remote-address", "{remote_address}", "--prefix-len", "{prefix_len}"],
    "max_timeout_secs": 20,
    "max_output_bytes": 16384
  },
  "cleanup_command": {
    "argv": ["/opt/operator/tunnel-adapter", "cleanup", "--interface", "{interface}"],
    "max_timeout_secs": 20,
    "max_output_bytes": 16384
  },
  "status_command": {
    "argv": ["/opt/operator/tunnel-adapter", "status", "--interface", "{interface}"],
    "max_timeout_secs": 10,
    "max_output_bytes": 16384
  }
}
```

Runtime definitions require `status_command`, one of `startup_command` or
`restart_command`, and one of `stop_command` or `cleanup_command`. Optional
commands are `traffic_limit_command`, `stop_command`, `cleanup_command`, and
`restart_command`.

Supported argv placeholders include:

```text
{interface} {plan} {kind}
{local_client_id} {peer_client_id}
{local_underlay} {remote_underlay}
{local_address} {remote_address} {prefix_len}
{local_ipv4} {remote_ipv4} {prefix_len_ipv4}
{local_ipv6} {remote_ipv6} {prefix_len_ipv6}
{fou_port} {fou_peer_port} {fou_ipproto}
{ingress_kbps} {egress_kbps} {burst_kb}
```

`{remote_underlay}` is always the exact destination declared for the endpoint
where the command runs. `{local_underlay}` is that endpoint's optional local
source and expands to an empty argument when no source was declared. Adapter
scripts must not infer either value from the peer or host inventory.

The status command succeeds with exit code `0`. Output is retained only as
bounded command evidence; it is not parsed to discover or infer another
tunnel. A random script can implement the contract if it accepts the declared
argv, is idempotent, stays within the configured timeout/output bounds, and
returns truthful exit status.

Bind the definition explicitly to both endpoints:

```sh
cargo run -p vpsctl -- tunnel-plan \
  --name edge-a-edge-b-wireguard \
  --interface-name wg42 \
  --kind wireguard \
  --runtime-manager custom_adapter \
  --left-runtime-adapter-definition-id <runtime_definition_uuid> \
  --right-runtime-adapter-definition-id <runtime_definition_uuid> \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-remote-underlay 203.0.113.20 \
  --left-local-underlay 10.0.0.10 \
  --right-remote-underlay 198.51.100.10 \
  --right-local-underlay 10.0.1.20 \
  --left-tunnel-ipv4-cidr 10.255.20.0/31 \
  --right-tunnel-ipv4-cidr 10.255.20.1/31 \
  --bandwidth-mbps 1500 \
  --save \
  --enabled \
  --confirmed
```

An adapter definition is desired-state input, not proof that its executable is
installed or converged. A definition referenced by a tunnel plan cannot be
edited or deleted: create a replacement, review the plan change, and then
verify runtime evidence. Disabling a plan runs only its declared stop/cleanup
command and then stops telemetry. vpsman never deletes the adapter executable.
The adapter owns any routes or daemon state its commands need; Agent builtin
route/cleanup fields are rejected for this mode.

## Optional OSPF Cost Control

Routing cost uses a second command contract, independent of tunnel ownership.
For the normal reusable path, open **Config > Sources**, create an **OSPF
updater command** preset, and assign it to the applicable VPSs. When one tunnel
plan needs different commands, create a **Routing cost adapter** under
**Adapter definitions** and select it as that endpoint's optional override.
Both forms use the same contract:

```json
{
  "contract_version": 2,
  "status_command": {
    "argv": ["/opt/operator/routing-cost", "status", "--plan-id", "{plan_id}", "--interface", "{interface}", "--side", "{endpoint_side}"],
    "max_timeout_secs": 10,
    "max_output_bytes": 16384
  },
  "update_command": {
    "argv": ["/opt/operator/routing-cost", "apply", "--plan-id", "{plan_id}", "--interface", "{interface}", "--side", "{endpoint_side}", "--cost", "{desired_cost}"],
    "max_timeout_secs": 10,
    "max_output_bytes": 16384
  }
}
```

vpsman invokes each command as exact argv, without a shell, and closes stdin.
The routing commands can use the runtime endpoint placeholders plus:

```text
{plan_id} {endpoint_side} {expected_current_cost} {desired_cost}
```

Use `{plan_id}`, `{interface}`, and `{endpoint_side}` so one reusable helper can
locate the exact tunnel endpoint. The update command receives the requested
cost through `{desired_cost}`. `{expected_current_cost}` is also available when
the helper wants the reviewed previous value, although the agent independently
checks it before invoking update.

Status must exit zero and print exactly one decimal cost from 1 through 65535.
Update success or failure is its exit code; bounded stdout is retained as the
operator-facing result message. The agent reads status before update, rejects a
stale reviewed cost, then reads status again and accepts the job only when it
equals `{desired_cost}`. When no cost has been recorded yet, that first status
read establishes the baseline instead of inventing one. A changed current cost, adapter-definition hash,
recommendation, or endpoint snapshot rejects stale confirmation.

Enable reviewed OSPF on a plan after each endpoint resolves a configured updater
from its effective `ospf_update_command` preset or explicit per-plan override:

First read the plan UUID and current declaration revision with `tunnel-plans`.
The console row editor carries these fields automatically. CLI/VTY updates name
the exact plan and revision explicitly:

```sh
cargo run -p vpsctl -- tunnel-plan \
  --name edge-a-edge-b \
  --interface-name gre42 \
  --kind gre \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-remote-underlay 203.0.113.20 \
  --left-local-underlay 10.0.0.10 \
  --right-remote-underlay 198.51.100.10 \
  --right-local-underlay 10.0.1.20 \
  --left-tunnel-ipv4-cidr 10.255.0.0/31 \
  --right-tunnel-ipv4-cidr 10.255.0.1/31 \
  --bandwidth-mbps 1000 \
  --ospf \
  --ospf-mode reviewed \
  --ospf-latency-ms 20 \
  --save \
  --update-plan-id <plan_uuid> \
  --expected-revision <revision> \
  --enabled \
  --confirmed
```

To override only one endpoint for this plan, add its optional flag:

```text
--left-routing-adapter-definition-id <left_definition_uuid>
```

The other endpoint still uses its effective VPS Configuration preset. An
explicit override always wins; if it is missing or invalid, vpsman rejects the
operation instead of falling back.

Check both resolved endpoint updaters, then fetch the current frozen
recommendation:

```sh
cargo run -p vpsctl -- tunnel-ospf-status-refresh --plan-id <plan_uuid>
cargo run -p vpsctl -- network-ospf-update-plans --limit 50
```

Apply only fields from that current update-plan record:

```sh
source ./path/to/secrets/operator-privilege.env
export VPSMAN_SUPER_PASSWORD='<local-super-password>'

cargo run -p vpsctl -- tunnel-ospf-cost-update \
  --plan-id <plan_uuid> \
  --plan-revision <plan_revision> \
  --recommendation-id <recommendation_id> \
  --left-current-ospf-cost <left_cost> \
  --right-current-ospf-cost <right_cost> \
  --desired-ospf-cost <recommended_cost> \
  --left-adapter-definition-hash <left_sha256> \
  --right-adapter-definition-hash <right_sha256> \
  --confirmed
```

For automatic mode, use `--ospf-mode automatic`. The server controller, not an
agent-local loop, refreshes unverified or stale adapters, retries failed checks
after five minutes, refreshes verified costs every ten minutes, and applies only
after the configured minimum delta and consecutive healthy-probe count pass in
the recent ten-minute evidence window. A degraded sample resets that streak; it
does not block the plan forever. The agent still executes only explicit server-issued status/apply
jobs with bound adapter-definition snapshots. Those internal routing jobs cannot be
submitted as ordinary public operator mutations; operator-reviewed jobs retain
the approving operator's authority.

## Cost Model

Bandwidth is an operator-entered integer from 10 through 10000 Mbps. It is not
limited to presets. The default policy computes:

```text
raw = latency_ms * latency_weight
    + packet_loss_ratio * loss_weight
    + bandwidth_weight * sqrt(100 / bandwidth_mbps)

cost = clamp(round(raw * preference_bias / preference), min_cost, max_cost)
```

Measured throughput can lower effective bandwidth to the observed value but
never raises it above configured bandwidth. Higher preference lowers cost;
lower preference raises it. The console previews the cost beside these fields
while the operator edits them.

## Status, Probe, And Speed Evidence

Target an exact registry plan for direct CLI tests. Read-only status works for
enabled and disabled plans so cleanup can be verified after disable. Probe and
speed tests require an enabled plan. The CLI fetches the current declaration,
and the API rejects stale snapshots:

```sh
cargo run -p vpsctl -- tunnel-status \
  --plan-id <plan_uuid> \
  --side left

cargo run -p vpsctl -- tunnel-probe \
  --plan-id <plan_uuid> \
  --side left \
  --count 3 \
  --interval-ms 500

cargo run -p vpsctl -- tunnel-speed-test \
  --plan-id <plan_uuid> \
  --server-side left \
  --duration-secs 3 \
  --max-bytes 16777216 \
  --rate-limit-kbps 100000 \
  --confirmed
```

The API resolves a bound runtime adapter snapshot for Custom adapter status
jobs before dispatch. Status, probe, and speed evidence is bound from the
authorized job's plan UUID and frozen declaration, never inferred from result
labels. Reusing a name or interface cannot carry evidence into another plan.

Runtime reconciliation and reachability are deliberately separate. Runtime
state reports whether the exact declared interface and lifecycle command
converged. Reachability reports only the result of the configured probe; a
failed or blocked ICMP probe is `probe_failed` and is not proof that the tunnel
is disconnected. A saved enabled plan can therefore remain visible while an
endpoint sync is failed, or be runtime-healthy while reachability is unverified.
Inspect the endpoint sync job output for creation/cleanup errors.

When application evidence is more authoritative than a probe, expand the plan
row and record an operator assessment of connected or disconnected with a note.
This audited, revision-bound annotation changes display state only. It never
changes runtime reconciliation or automatic OSPF decisions, and plan edits,
enable/disable changes, or retirement clear it back to automatic measurement.

## Operator Rules

- Save a plan before expecting runtime observation or management.
- Create rejects duplicate names. Updates identify one UUID and current
  declaration revision; a stale revision never replaces a newer plan.
- Endpoints must be two different registered VPSs. Each remote destination is
  required. Its optional local source is validated only against that endpoint's
  address family; no address equality or cross-endpoint derivation is allowed.
  The GRE, IPIP, SIT, and FOU built-in driver uses IPv4 outer addresses. Other
  built-in drivers validate their own transport family without assuming every
  VPS is dual-stack.
- Keep each overlay address unique across saved plans and each interface name
  unique per endpoint VPS. Agent builtin FOU, WireGuard, and OpenVPN listeners
  must also have a unique transport and local port on each VPS. This prevents
  two declarations from claiming the same runtime resource.
- Use Agent builtin only when vpsman should own the declared endpoint and the
  required kind-specific prerequisites are available on both VPSs.
- Use External observed when vpsman must never mutate the tunnel.
- Use Custom adapter only for an executable the operator installed and owns.
- Keep OSPF off for tunnel-only use. When it is enabled, each endpoint needs an
  effective OSPF updater command from its configuration preset unless the plan
  explicitly binds an override; the per-plan override wins.
- Inspect endpoint status, probe, and throughput evidence before reviewed cost
  changes.
- Reviewed mode may explicitly apply the operator-declared baseline when no
  recent probe exists, or a recommendation containing degraded recent samples;
  both cases use a warning confirmation. Automatic mode never does either.
- Treat adapter replacements as code changes. A referenced definition is
  immutable: create its replacement, review each explicit plan binding, and
  recheck endpoint and routing status after the plan change.
- Do not expect vpsman to discover, import, install, or clean up anything that
  was not named by the saved declaration and its bound adapter commands.
