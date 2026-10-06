# Port Forwarding

Port forwarding is an explicit per-VPS desired-state workflow under **Network >
Port forwards**. It is intended for direct TCP/UDP requests addressed to the
VPS itself. It does not discover, import, or manage Docker, system-firewall,
iptables, or third-party nftables rules. Choose **DNAT**, **REDIRECT**, or a
reusable **Custom adapter** in the rule editor.

## Host Contract

An enabled built-in DNAT or REDIRECT rule requires the following on the VPS:

- the `nft` executable is already installed;
- the agent runs as root or has `CAP_NET_ADMIN` in the host network namespace;
- nftables and the kernel accept the required `inet` NAT, local-destination,
  port-map, counter, connection-tracking, and masquerade expressions;
- for DNAT, IP forwarding is enabled by the operator when the target is not local.

The agent probes this exact capability and reports a reason when it is not
available. `vpsman` does not install nftables, write firewall configuration,
change sysctls, or select a distribution-specific persistence service. A rule
may be saved disabled on an unsupported VPS, but it cannot be enabled or
reapplied until the capability is reported as supported.

Custom adapters do not require nftables. They require an agent advertising
custom-adapter support and the programs named by the selected definition.

For built-in modes, the agent owns one table only:

```text
table inet vpsman_port_forward
```

The table carries a human-readable `vpsman-owned desired=...` comment and a
structural `vpsman_ownership_v1` marker set. The structural marker is used for
ownership checks because older supported nftables JSON output omits table
comments. If a same-name table lacks the exact marker, the agent reports an
ownership conflict and leaves it unchanged. Every apply atomically checks and
replaces the complete marked table when its native desired state changes or
needs repair. Custom-only changes do not rebuild it. The agent never flushes the ruleset and
never edits another table.
Prerouting and output rules
use `fib daddr type local`, so they claim requests to current local addresses
without depending on an interface name or fixed local IP and do not intercept
unrelated transit forwarding traffic. Their destination-NAT priority is before
the conventional `dstnat` priority, so a port explicitly claimed in vpsman wins
for new matching connections. Existing conntrack entries can continue after a
rule is changed or removed.

Desired forwarding rules are server-managed runtime state. The bootstrap agent
TOML rejects a local `network.port_forwarding` declaration, so every rule that
can change the host is represented in the API, UI, audit log, and cleanup
lifecycle. It follows the shared desired-state, dispatch, apply, and observation
contract in [Job Status Model](job-status-model.md#desired-state-reconciliation).
On startup, the agent reconciles forwarding first from a validated
last-accepted runtime cache, independently of tunnel reconciliation. If that
cache is absent or unreadable, it preserves existing host state instead of
treating missing state as a deletion request. After authentication, a cacheless
agent forces an authoritative sync. A cached agent reports its current content
hash, while the API always compares it with current database desired state; an
older applied snapshot is evidence only and is never resent as desired state.
Explicit/reconnect forwarding repair syncs require successful access to the
owned table even when current desired state is empty. A generic authoritative
sync still accepts empty forwarding state on a host that reports nftables as
unsupported, so port forwarding remains optional. When a cacheless supported
agent observes a marked table, its separate reconnect-drift signal makes table
access mandatory and prevents unknown cleanup from being acknowledged.

### Recovery And Integrity

- **Gateway or API disconnect:** the kernel table continues forwarding. The
  agent keeps its last accepted config. After reconnect it inspects the exact
  owned table and requests a forwarding-only reconciliation when that table is
  missing, structurally drifted, or could not be inspected. The API supplies
  current database desired state; unchanged tunnel adapters are not rerun.
- **Agent process restart:** forwarding is reconciled from the atomically stored,
  hash-verified last accepted cache before potentially slow tunnel commands.
- **Startup reconciliation failure:** the agent keeps an explicit authoritative
  sync requirement until one full retry is durably accepted. A matching cached
  configuration hash therefore cannot hide a failed reboot-time host apply.
- **System reboot:** nftables runtime state may be empty until the agent starts;
  the same cached reconciliation recreates the owned table. The packaged
  systemd service uses `Restart=always`, but the operator remains responsible
  for enabling the service and any required IP-forwarding sysctl persistently.
- **Lost completion event:** when a reconnecting authenticated agent reports the
  exact hash of a pending snapshot, the API records that snapshot as applied
  instead of leaving it queued forever.
- **Concurrent or reordered syncs:** each server snapshot has a strictly
  increasing persisted generation. Pending server evidence never regresses to
  an older applied or queued generation. The agent rejects every lower
  generation; an exact-generation replay is accepted only when its content
  matches the current snapshot. Delayed jobs therefore cannot replace newer
  forwarding state or make the control plane report an older generation as
  current.
- **Partial network failure:** forwarding-only changes do not run unchanged
  tunnel adapters. When forwarding succeeds but an independently changed tunnel
  fails, the forwarding portion is retained in the last accepted cache and the
  full server desired state remains failed/pending for retry.

The dynamic connection-ID set used for targeted masquerade is excluded from
structural drift comparison; live traffic therefore does not create false drift.
Static maps, chains, expressions, and comments remain part of the comparison.
There is intentionally no periodic auto-repair while a session remains
connected: external changes are reported as Drifted, and **Reapply** is the
explicit immediate repair action. Reconnect is a lifecycle boundary and also
repairs a missing or drifted owned table from current database desired state.

Capability is advertised when the agent connects. After installing `nft` or
changing the agent's host-network privileges, reconnect or restart the agent so
the control plane receives a fresh capability snapshot before enabling rules.

## Rule Workflow

1. Select one VPS and enter a unique rule name.
2. Select the mode, then TCP, UDP, or Both.
3. Choose **Port mapping** for fixed translation, or **Upstream pool** when the
   selected DNAT host/custom adapter supports it. Enter the incoming ports.
4. For DNAT, enter a target IP, or resolve a hostname and select a literal
   address. REDIRECT selects IPv4, IPv6, or Both instead. Custom selects a
   reusable adapter, with an optional target IP or resolved hostname.
5. For DNAT, choose **Masquerade** or **Preserve source**.
6. Review the frozen VPS, mapping, target, and return-path snapshot, then apply.

DNAT rules apply to every current local address in the target IP's family. IPv4
targets create IPv4 DNAT rules and IPv6 targets create IPv6 DNAT rules; NAT46
and NAT64 are not inferred. The selected literal address is the desired state.
When a hostname is resolved, its normalized name is retained alongside that
address as operator context so Edit can resolve it again; the hostname is not
sent to the agent, included in forwarding desired state, or refreshed
automatically. Use Resolve again, select the intended literal address, and save
a new revision when DNS changes.

### REDIRECT And Custom Adapters

REDIRECT translates the destination port to a local listener in the selected
family (IPv4 by default). Incoming traffic uses the receiving interface's local
address; it does **not** make a listener bound only to `127.0.0.1` or `::1`
externally reachable. It has no remote target or masquerade setting.

Custom adapters can manage listeners such as nginx or socat, including relaying
to a loopback destination. Define their required **Apply**, **Remove**, and
**Status** argv commands in the existing adapter registry. Commands execute
directly, without implicit shell parsing. An Apply command must start or reload
its service and return; it must not remain attached to a foreground daemon.

Available placeholders are `{rule_id}`, `{client_id}`, `{revision}`, `{protocol}`,
`{incoming_ports}`, `{target_ports}`, and `{target_ip}`. Port expressions retain
their corresponding ordering; `both` remains one protocol value. An omitted
target IP expands to an empty argument. Family selection and listener conflicts
belong to the adapter; built-in overlap validation does not claim custom ports.

Apply must idempotently establish the exact supplied rule under its stable rule
ID. Remove must remove all resources for that ID, including partial applies.
Status must exit successfully and return JSON with `state` equal to `applied`,
`absent`, or `drifted`, and an optional `message`. Apply is verified by `applied`;
Remove by `absent`. Failed commands or invalid status output report failure.
Each command uses its configured timeout/output budget from the adapter editor.

Definition command edits use the existing affected-resource review and dispatch
workflow. An edit cannot remove capabilities required by an attached pool,
including disabled drafts. Definitions cannot be deleted while referenced.
Switching adapters or modes removes the previous owner's resources first. The
agent records exact custom ownership before Apply, including commands and rule
inputs needed to remove interrupted/partial applies after a restart. Failed
cleanup remains explicit evidence even when a replacement is subsequently applied.

### Upstream Pools

Pools explicitly balance every incoming port over the entire configured set of
upstream IP:port endpoints. There is no positional mapping between incoming and
target ranges: `443` can balance across `192.0.2.1:8000-8002` and
`192.0.2.2:9000`, and multiple incoming ports can use that same pool. Fixed port
mapping retains its original equal-range/multiple-to-one behavior. REDIRECT has
no pool policy.

Each compact upstream row owns its literal IP, port range, positive integer
weight, primary/backup role, enabled state and optional failure policy. Weight
applies to **each expanded endpoint**: weight 2 on three ports contributes six
shares, while weight 1 on one port contributes one. Disabled endpoints retain
their settings but receive no new selections. At least one endpoint must be
enabled and one primary must remain configured; enabled backups may take over
while all configured primaries are disabled. Duplicate/overlapping port ranges
on the same IP are rejected, including disabled rows.

A hostname groups its explicitly selected resolved IPs in the editor. It is
provenance only, never a runtime DNS target. Resolve again to review membership;
existing IP settings and disappeared answers remain until explicitly removed.
New IPs copy the selected row template. No background DNS refresh or automatic
backup promotion occurs.

| Forwarder | Strategies | Other supported pool settings |
| --- | --- | --- |
| Native DNAT | Weighted round robin, random, source-IP hash | One address family per pool; existing masquerade/preserve-source behavior |
| Custom adapter v2 | Only strategies advertised by its definition | Only advertised families, mixed-family support, backup strategies, failure exclusion and connection settings |

Native selection occurs on a flow's first packet, then conntrack retains its
destination. Round robin rotates weighted shares; random chooses a weighted
share; source-IP hash keeps a source on the same endpoint while ordered
membership/weights remain unchanged. Its rule-stable seed preserves affinity
across unrelated native table rebuilds. Shared source addresses behind NAT share
affinity. Native pools do not probe health, count active connections, retry a
failed connection, or provide backup roles.

The agent checks native pool grammar independently using `nft --check`; failure
hides pools without disabling supported fixed DNAT/REDIRECT. Pools/custom v2
require forwarding schema 3 and command protocol 12. Existing mappings continue
using their previous schema/protocol. Upgrade the server, frontend and affected
agents before enabling pools. Existing database rows migrate with no pool.

### Pool Settings Have Fixed Scopes

These meanings are part of vpsman's contract, not configurable adapter scopes:

| Setting | Scope and behavior |
| --- | --- |
| `connect_timeout_secs` | This rule; deadline for each upstream connection attempt, independent of retry being on/off/default |
| `retry_policy` | One incoming connection in this rule; Off or retry connection-establishment failures |
| `max_attempts` | Total attempts, including the first, for that connection |
| `retry_budget_secs` | Elapsed limit for starting another attempt; not an established-connection lifetime or a hard cap on an already-started attempt |
| Temporary failure threshold/window/exclusion | Each expanded IP:port separately within this rule; no shared state with sibling ports, domain IPs or other rules |
| Backup role | Select only when this rule's eligible primary endpoints are unavailable; supported strategies only |

Omitted settings inherit the adapter's documented defaults. Explicit failure
Off disables failure counting/exclusion, and explicit retry Off disables trying
another endpoint. Positive values are required when a threshold/limit is supplied.
Whole-range failure aggregation is not implemented. Runtime exclusion is
temporary eligibility evidence, not a permanent edit of the configured enabled
flag or a failure to apply the rule.

For an NGINX stream implementation, rule-specific stream `server` blocks can own
`proxy_connect_timeout` and `proxy_next_upstream*`, while distinct rule-owned
upstream groups own each IP:port's `max_fails`/`fail_timeout`. This permits rules
on one NGINX instance to have different settings without changing shared stream
defaults. NGINX links the failure-count window and exclusion interval, does not
support backup with hash/random, and ignores passive-failure settings for a
single-server group. Advertise these restrictions; do not silently approximate
unsupported settings. Standard NGINX does not expose per-peer live exclusion
state through this contract automatically. Omit unavailable observations.
See [NGINX upstream directives](https://nginx.org/en/docs/stream/ngx_stream_upstream_module.html#server)
and [connection/retry directives](https://nginx.org/en/docs/stream/ngx_stream_proxy_module.html#proxy_connect_timeout).

### Custom Adapter Contract 2

vpsman ships the contract, validation and lifecycle, not an NGINX implementation.
Operators supply and install their adapter on each target VPS. Register its
definition through the network-adapter API with `adapter_kind: "port_forward"`;
the existing registry edits its commands with the usual impact review.

```json
{
  "contract_version": 2,
  "pool_capabilities": {
    "strategies": ["round_robin", "least_connections", "source_ip_hash", "consistent_source_ip_hash", "random"],
    "protocols": ["tcp", "udp"],
    "address_families": ["ipv4", "ipv6"],
    "mixed_families": true,
    "backup_strategies": ["round_robin", "least_connections"],
    "failure_exclusion": {
      "protocols": ["tcp"], "linked_timeout": true, "min_endpoints": 2,
      "description": "Temporary TCP connection-failure exclusion per IP:port in this rule."
    },
    "connect_timeout": {"protocols": ["tcp"], "description": "Deadline per connection attempt in this rule."},
    "retries": {"protocols": ["tcp"], "description": "Retry failed connection establishment within this rule."}
  },
  "apply_command": {"argv": ["/opt/operator/forward-adapter", "apply", "{rule_config_path}"], "max_timeout_secs": 30, "max_output_bytes": 16384},
  "remove_command": {"argv": ["/opt/operator/forward-adapter", "remove", "{rule_config_path}"], "max_timeout_secs": 30, "max_output_bytes": 16384},
  "status_command": {"argv": ["/opt/operator/forward-adapter", "status", "{rule_config_path}"], "max_timeout_secs": 30, "max_output_bytes": 16384}
}
```

This illustrates an NGINX-like capability profile, not executable behavior.
Advertise only implemented features. Unsupported controls are hidden. Capability
protocol/family lists contain concrete values (`tcp`/`udp`, `ipv4`/`ipv6`), not
`both`. A Both rule needs support for both protocols. Optional feature objects
and `backup_strategies` may be omitted; an adapter v2 may omit pool capabilities
entirely and support only fixed mappings. `min_endpoints` counts configured
expanded endpoints, including disabled/backup entries; it defaults to 1.

Every v2 command must include `{rule_config_path}`. The agent writes an immutable
mode-0600 JSON file in its private state directory for that invocation. Read it
before returning; do not retain its path, modify it, or start a listener that
depends on that file. It is removed after the command exits or is canceled.
Each request has the following structure (the embedded rule omits adapter
commands/capabilities and DNS provenance):

```json
{
  "contract_version": 2,
  "client_id": "v-example",
  "config_hash": "64 lowercase hexadecimal characters",
  "rule": {
    "id": "11111111-1111-4111-8111-111111111111",
    "revision": 1, "name": "application", "mode": "custom_adapter",
    "protocol": "tcp", "masquerade": false, "mappings": [],
    "pool": {
      "incoming": [{"start": 443, "end": 443}], "strategy": "round_robin",
      "upstreams": [{
        "id": "22222222-2222-4222-8222-222222222222",
        "target_ip": "192.0.2.10", "ports": {"start": 8000, "end": 8002},
        "weight": 2, "role": "primary", "enabled": true,
        "failure_policy": {"mode": "temporary", "threshold": 2, "window_secs": 10, "retry_after_secs": 10}
      }],
      "connect_timeout_secs": 3,
      "retry_policy": {"mode": "connect_failure", "max_attempts": 3, "retry_budget_secs": 10}
    }
  }
}
```

Fixed mappings instead have no `pool`, retain `mappings` and may omit `target_ip`
for an adapter-defined local target. V1 argv placeholders remain available, but
pool rules have no scalar target/target-port mapping: use the JSON file. The
configuration hash is opaque to the adapter; store it with the successfully
loaded generation and report that observed value. Merely echoing the incoming
hash or matching a file on disk is not proof that a service loaded it.

Apply must be idempotent and return after the exact generation is active. Remove
receives the saved owner's inputs and must remove **all** resources for that
stable rule ID, including previous revisions/partial applies. Preserve unrelated
rules and other service configuration. Status must be able to verify absence
by rule ID even when rejected Apply inputs cannot be rendered, so a failed
creation or edit remains removable. Define serialization/atomic replacement,
syntax checks, reload verification and recovery appropriate to your service.
The agent saves the owner before Apply and verifies Status after Apply/Remove.
Nonzero exit, timeout, truncated output, malformed status or a mismatched loaded
hash cannot acknowledge success.

```json
{
  "state": "applied",
  "config_hash": "the hash of the loaded generation",
  "message": "optional diagnostic",
  "upstream_observations": [{
    "upstream_id": "22222222-2222-4222-8222-222222222222",
    "port": 8001, "state": "excluded", "reason": "connection failures",
    "retry_after_unix": 1791244810
  }]
}
```

`state` is `applied`, `absent` or `drifted`. Only Applied requires a matching
`config_hash`; Absent confirms cleanup. Optional upstream observations use
`eligible`, `excluded` or `unknown`, with configured row IDs/ports, no duplicate
endpoints, and reasons of at most 1024 bytes. Do not invent eligibility when the
service cannot observe it. The UI shows unknown when evidence is unavailable;
stale configuration evidence is never shown as live endpoint health. Management
command timeout/output bounds remain separate from forwarding connection settings.

Create/update API requests use `pool` with `mappings: []` and `target_ip: null`.
Pool hostnames may be included in upstream rows as API-only provenance. Updating
an existing pool must send its complete pool or explicit `pool: null` when
converting back to mapping/REDIRECT; omission is rejected to protect against old
clients erasing pools. Existing mapping requests may continue omitting `pool`.
The usual revision fence, confirmation, enable/disable, bulk deletion, audit and
cleanup receipt workflows apply unchanged.

### Port Expressions

Expressions are comma-separated `PORT` or `START-END` items. Incoming ranges
must not overlap. The target side supports either one port for every incoming
item or one corresponding item for each incoming item.

| Incoming | Target | Result |
| --- | --- | --- |
| `443` | `8443` | One port to one port |
| `80,443` | `8080` | Multiple ports to one port |
| `10000-10010` | `20000-20010` | Position-preserving range translation |
| `80,443,10000-10010` | `8080,8443,20000-20010` | Corresponding ports and ranges |

A corresponding target range must contain the same number of ports as its
incoming range. Port 0, reversed ranges, overlapping built-in claims for the
same VPS, family, and protocol, and native desired states that exceed the bounded nftables
program limit are rejected before dispatch.

### Return Path

**Masquerade** is the default. The owned table masquerades only connections
that one of its own DNAT rules accepted; unrelated forwarded packets are not
masqueraded.

**Preserve source** keeps the original source address. Select it only when the
target has a return route through this VPS or an equivalent symmetric route.
The UI reports IPv4/IPv6 forwarding state as evidence but never changes it.

## Desired And Runtime State

Create, update, enable, disable, delete, bulk mutation, agent startup, reconnect
after owned-table drift, and explicit Reapply are reconciliation events.
Telemetry reports completed observations; it never repairs drift in the
background or waits for a custom command. Custom Status runs at the configured
network runtime-status interval and immediately after mutations.

- **Pending**: the latest desired hash has not yet been observed from the agent.
- **Applied**: the observed normalized owned table matches the exact desired
  revision set.
- **Applied · warning**: the table matches, but forwarding for the target family
  is disabled outside vpsman.
- **Disabled**: a current applied/absent snapshot confirms this disabled rule is
  omitted.
- **Drifted**: the owned table is missing, unexpected, or structurally changed.
- **Unsupported / Failed**: capability or inspection/apply evidence includes a
  reason.
- **Removal pending**: the rule is omitted from desired state, but host cleanup
  has not yet been confirmed.

NAT matches count first-packet NAT rule matches since the latest complete table
apply. They are not bytes, throughput, active connections, or health checks.
Custom rules instead show their own status, observation time, and command error;
they do not have a native NAT counter.

Delete keeps a tombstone until the agent reports the exact current table (or no
owned table when no rules remain). Custom deletion additionally requires
per-rule verified removal; an absent nftables table is not custom-cleanup
evidence. Native tombstones keep table inspection active even after the last
native rule is removed, including when custom rules or cleanup requests remain.
An admin may **Forget** a tombstone only for
a permanently unreachable or decommissioned VPS and must provide a reason.
Forgetting clears that VPS's cached forwarding snapshot, so any other active
rules show Pending until fresh telemetry arrives. It does not remove any
nftables or custom-adapter state from that host. Agent deletion is
blocked while desired, pending-removal, or observed owned-table state can remain.
When no host state can remain, agent deletion archives clean disabled drafts
with the agent record instead of leaving orphaned forwarding definitions.
Transient inspection failures retain the last known owned-table presence for
this deletion guard. Only a successful observation that the table is absent,
or the explicit admin Forget override, clears that evidence.

## CLI And VTY

List and resolve without changing desired state:

```sh
vpsctl port-forwards
vpsctl port-forward-resolve --hostname app.internal
```

Create and apply a reviewed rule:

```sh
vpsctl port-forward-create \
  --client-id v-12 \
  --name public-web \
  --protocol both \
  --incoming 80,443 \
  --target 8080,8443 \
  --target-ip 10.20.0.15 \
  --target-hostname app.internal \
  --confirmed
```

`--target-hostname` stores the hostname as operator context while the selected
literal `--target-ip` remains the only forwarding target sent to the agent. On
update, omit `--target-hostname` to retain the stored hostname, provide it to
replace that context, or use `--clear-target-hostname` to remove it.

Use `--preserve-source` only with a verified return route, or `--disabled` to
save a draft without host mutation. Every mutation uses the rule's current
`revision`; stale revisions are rejected rather than retargeted. CLI commands
are also available unchanged in the interactive VTY.

Use `--mode redirect --address-family both` without `--target-ip` for native
local redirection. Use `--mode custom_adapter --adapter-definition-id UUID`
for a reusable listener adapter; `--target-ip` is optional in that mode.
