# Tutorial 06: Tunnels, Topology, And Routing Adapters

vpsman manages only saved tunnel declarations. It does not discover interfaces,
import unmanaged tunnels, choose a runtime owner, install a routing daemon, or
write daemon configuration. An operator selects one runtime ownership mode for
each plan:

1. **Agent iproute2** creates and removes GRE, IPIP, SIT, or FOU links.
2. **External observed** inspects one exact declared interface without mutating it.
3. **External adapter** runs operator-owned lifecycle commands from source
   templates for implementations such as WireGuard, OpenVPN, TUN/TAP, or a
   custom tunnel program.

OSPF cost control is separate and off by default. Enabling it binds one
operator-owned routing-cost adapter to each endpoint. The adapter can integrate
with any routing stack; vpsman never installs, edits, or removes the adapter or
the routing stack it controls.

## Console Workflow

Open **Network > Tunnel plans**. The registry remains visible while create or
edit opens below it. Each plan shows:

- the exact endpoint VPSs and interface;
- runtime ownership and enabled state;
- current left/right runtime evidence derived from that declaration only;
- optional OSPF mode and adapter status;
- edit, export, enable, disable, and retire actions.

Choose **Create plan**, select the runtime owner, enter endpoints and addresses,
and optionally enable OSPF. The cost preview updates beside latency, loss,
bandwidth, and preference. Use the row edit action to replace an existing
declaration; plan identity is fixed during edit so a rename cannot accidentally
create another plan. The editor submits the displayed declaration revision; if
another operator changes the plan first, the update is rejected and the editor
must be reopened against current state.

Source templates live under **Automation > Source templates**. Runtime and
routing adapters are never built in. Create a shared template when the same
executable contract exists on several VPSs, or a VPS-local template when one
endpoint differs.

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
receive the complete desired state without that plan. Agent iproute2 removes
only the old declared interface and routes; an external runtime adapter runs
only its bound stop/cleanup command; external observed mode stops observation
without mutating the external tunnel. If an endpoint is offline or managed
cleanup fails, the retirement remains committed and runtime convergence remains
visible in Jobs and **Config > Overview**. The agent reconciles the
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
one side from the other. If a local source is empty, Agent iproute2 omits the
`local` argument and lets the host route select the source; an external adapter
receives an empty `{local_underlay}` placeholder. Each non-empty local source
must use the same address family as that endpoint's remote destination.

## Address Allocation

Allocation pools are optional. Configure persistent pools in **System > Suite
config**, pass a pool to `tunnel-allocate`, or enter endpoint addresses
manually. Pools must not overlap real networks.

```sh
cargo run -p vpsctl -- tunnel-allocate \
  --ipv4-pool-cidr 10.255.0.0/16 \
  --ipv6-pool-cidr fd80::/80 \
  --reserved-address 10.255.0.0,fd80:: \
  --include-ipv4 \
  --include-ipv6
```

The allocation call has no planning or runtime side effects. A saved plan must
contain an explicit IPv4 pair, IPv6 pair, or both. Each pair must contain two
different addresses in the declared prefix. Saved plans cannot reuse an overlay
address, or reuse the same interface name on a VPS; the API rejects both before
runtime config is queued.

## Workflow 1: Agent iproute2

Use this only for Linux tunnel kinds that the agent implements directly: GRE,
IPIP, SIT, and FOU. OSPF remains off unless `--ospf` is supplied.

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
for reuse:

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

Changing an immutable runtime identity, such as interface, kind, underlay, or
endpoint address, removes the old declared state before reconciling the new
state. Routes and stale interfaces remain explicit: the agent never guesses
what else is safe to delete. Agent-managed route and stale-interface fields are
accepted only in Agent iproute2 mode.

## Workflow 2: External Observed

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

## Workflow 3: External Runtime Adapter

Use an external adapter when vpsman should start, stop, clean up, or check an
operator-owned tunnel implementation. The executable must already exist on the
endpoint. vpsman invokes direct argv without a shell.

Create a shared runtime adapter template:

```sh
cargo run -p vpsctl -- source-template-create \
  --domain runtime_tunnel_adapter \
  --name shared:tunnel-lifecycle-v1 \
  --scope shared \
  --definition-json '{"manager":"external_managed_adapter","contract_version":1,"startup_command":{"argv":["/opt/operator/tunnel-adapter","start","--interface","{interface}","--kind","{kind}","--local-underlay","{local_underlay}","--remote-underlay","{remote_underlay}","--local-address","{local_address}","--remote-address","{remote_address}","--prefix-len","{prefix_len}"],"max_timeout_secs":20,"max_output_bytes":16384},"cleanup_command":{"argv":["/opt/operator/tunnel-adapter","cleanup","--interface","{interface}"],"max_timeout_secs":20,"max_output_bytes":16384},"status_command":{"argv":["/opt/operator/tunnel-adapter","status","--interface","{interface}"],"max_timeout_secs":10,"max_output_bytes":16384}}'
```

Runtime templates require `status_command`, one of `startup_command` or
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

Bind the template explicitly to both endpoints:

```sh
cargo run -p vpsctl -- tunnel-plan \
  --name edge-a-edge-b-wireguard \
  --interface-name wg42 \
  --kind wireguard \
  --runtime-manager external_managed_adapter \
  --left-runtime-adapter-template-id <runtime_template_uuid> \
  --right-runtime-adapter-template-id <runtime_template_uuid> \
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

Template updates use a frozen affected-endpoint preview. Updating a bound
runtime template pushes new snapshots to affected agents. Disabling a plan runs
only its declared stop/cleanup command and then stops telemetry. vpsman never
deletes the adapter executable. The adapter owns any routes or daemon state its
commands need; agent iproute2 route/cleanup fields are rejected for this mode.

## Optional OSPF Cost Control

Routing cost is a second adapter contract, independent of tunnel ownership.
Create one shared or VPS-local template per executable contract:

```sh
cargo run -p vpsctl -- source-template-create \
  --domain routing_cost_adapter \
  --name shared:routing-cost-v1 \
  --scope shared \
  --definition-json '{"contract_version":1,"status_command":{"argv":["/opt/operator/routing-cost-adapter","status"],"max_timeout_secs":10,"max_output_bytes":16384},"update_command":{"argv":["/opt/operator/routing-cost-adapter","apply"],"max_timeout_secs":10,"max_output_bytes":16384}}'
```

Both commands receive one JSON object on stdin. A status request resembles:

```json
{
  "contract_version": 1,
  "operation": "status",
  "plan_id": "<plan_uuid>",
  "plan_name": "edge-a-edge-b",
  "interface_name": "gre42",
  "endpoint_side": "left",
  "client_id": "edge-a",
  "peer_client_id": "edge-b",
  "local_underlay": "10.0.0.10",
  "remote_underlay": "203.0.113.20",
  "local_address": "10.255.0.0",
  "remote_address": "10.255.0.1",
  "prefix_len": 31,
  "expected_current_cost": null,
  "desired_cost": null
}
```

The executable writes exactly one contract response to stdout:

```json
{
  "contract_version": 1,
  "interface_name": "gre42",
  "ready": true,
  "current_cost": 14,
  "applied_cost": null,
  "adapter_version": "1.0.0",
  "message": null
}
```

For an apply request, `expected_current_cost` and `desired_cost` are populated.
The update response must set `applied_cost` to the desired value. The agent then
runs status again and accepts the job only when `current_cost` equals that
value. A changed current cost, interface, template hash, recommendation, or
endpoint snapshot rejects stale confirmation.

Enable reviewed OSPF on a plan:

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
  --left-routing-adapter-template-id <left_template_uuid> \
  --right-routing-adapter-template-id <right_template_uuid> \
  --save \
  --update-plan-id <plan_uuid> \
  --expected-revision <revision> \
  --enabled \
  --confirmed
```

Check both endpoint adapters, then fetch the current frozen recommendation:

```sh
cargo run -p vpsctl -- tunnel-ospf-status-refresh --plan-id <plan_uuid>
cargo run -p vpsctl -- network-ospf-update-plans --limit 50
```

Apply only fields from that current update-plan record:

```sh
export VPSMAN_SUPER_PASSWORD='<local-super-password>'
export VPSMAN_SUPER_SALT_HEX='<super-salt-hex>'

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
jobs with bound template snapshots. Those internal routing jobs cannot be
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

The API resolves a bound runtime adapter snapshot for external-managed status
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
  The built-in iproute2 modes use IPv4 outer addresses.
- Keep each overlay address unique across saved plans and each interface name
  unique per endpoint VPS. This prevents two declarations from claiming the
  same runtime resource.
- Use external observed mode when vpsman must never mutate the tunnel.
- Use external adapter mode only for an executable the operator installed and
  owns.
- Keep OSPF off for tunnel-only use; enabling it always requires two explicit
  routing adapter bindings.
- Inspect endpoint status, probe, and throughput evidence before reviewed cost
  changes.
- Reviewed mode may explicitly apply the operator-declared baseline when no
  recent probe exists, or a recommendation containing degraded recent samples;
  both cases use a warning confirmation. Automatic mode never does either.
- Treat adapter definition updates as code changes. Review affected endpoints
  and recheck routing status after an update.
- Do not expect vpsman to discover, import, install, or clean up anything that
  was not named by the saved declaration and its bound adapter commands.
