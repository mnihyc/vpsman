# Port Forwarding

Port forwarding is an explicit per-VPS desired-state workflow under **Network >
Port forwards**. It is intended for direct TCP/UDP requests addressed to the
VPS itself. It does not discover, import, or manage Docker, system-firewall,
iptables, or third-party nftables rules.

## Host Contract

An enabled rule requires all of the following on the selected VPS:

- the `nft` executable is already installed;
- the agent runs as root or has `CAP_NET_ADMIN` in the host network namespace;
- nftables and the kernel accept the required `inet` NAT, local-destination,
  port-map, counter, connection-tracking, and masquerade expressions;
- IP forwarding is enabled by the operator when the target is not local.

The agent probes this exact capability and reports a reason when it is not
available. `vpsman` does not install nftables, write firewall configuration,
change sysctls, or select a distribution-specific persistence service. A rule
may be saved disabled on an unsupported VPS, but it cannot be enabled or
reapplied until the capability is reported as supported.

The agent owns one table only:

```text
table inet vpsman_port_forward
```

The table carries a human-readable `vpsman-owned desired=...` comment and a
structural `vpsman_ownership_v1` marker set. The structural marker is used for
ownership checks because older supported nftables JSON output omits table
comments. If a same-name table lacks the exact marker, the agent reports an
ownership conflict and leaves it unchanged. Every apply atomically checks and
replaces the complete marked table. The agent never flushes the ruleset and
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
2. Select TCP, UDP, or Both.
3. Enter the incoming and target port expressions.
4. Enter a literal target IP, or resolve a hostname and explicitly select one
   returned literal address.
5. Choose **Masquerade** or **Preserve source**.
6. Review the frozen VPS, mapping, target, and return-path snapshot, then apply.

Rules apply to every current local address in the target IP's family. IPv4
targets create IPv4 DNAT rules and IPv6 targets create IPv6 DNAT rules; NAT46
and NAT64 are not inferred. Hostname answers are never stored as desired state
and are never refreshed automatically. Use Resolve again, select the intended
literal address, and save a new revision when DNS changes.

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
incoming range. Port 0, reversed ranges, overlapping claims for the same VPS,
family, and protocol, and desired states that exceed the bounded nftables
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
Telemetry only inspects the owned table; it never repairs drift in the
background.

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

Delete keeps a tombstone until the agent reports the exact current table (or no
owned table when no rules remain). An admin may **Forget** a tombstone only for
a permanently unreachable or decommissioned VPS and must provide a reason.
Forgetting clears that VPS's cached forwarding snapshot, so any other active
rules show Pending until fresh telemetry arrives. It does not remove any
nftables state from that host. Agent deletion is
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
  --client-id 12 \
  --name public-web \
  --protocol both \
  --incoming 80,443 \
  --target 8080,8443 \
  --target-ip 10.20.0.15 \
  --confirmed
```

Use `--preserve-source` only with a verified return route, or `--disabled` to
save a draft without host mutation. Every mutation uses the rule's current
`revision`; stale revisions are rejected rather than retargeted. CLI commands
are also available unchanged in the interactive VTY.
