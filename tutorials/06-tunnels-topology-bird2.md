# Tutorial 06: Tunnels, Topology, And Bird2

Tunnel work has three separate operator workflows:

- vpsman-managed tunnels: vpsman renders and applies the runtime tunnel and
  Bird2 intent.
- observed/imported tunnels: vpsman records a tunnel that another system owns.
- custom adapter tunnels: vpsman delegates runtime lifecycle commands to a
  custom adapter, then manages the saved topology and Bird2 intent.

Allocation pools are not enabled by default. Set them in operator preferences,
suite config `[network]`, or pass them on the request. Example private pools are
`10.255.0.0/16` and `fd80::/80`, but deployments must choose ranges that do not
overlap their real networks. Pools may be larger than `/31` or `/127`; endpoint
allocation returns the next non-conflicting IPv4 pair and/or IPv6 pair inside
the pool. Manual tunnel endpoints are CIDRs such as `10.255.0.2/31`; they are
not limited to `/31` or `/127` when the tunnel segment occupies a larger
subnet. Reserved addresses accept comma-separated values.

In the frontend, open Topology -> Plans or Topology -> Promotion. Enter IPv4
and/or IPv6 endpoint CIDRs directly, or click Allocate endpoints to use the
persistent pools from Preferences -> Tunnel allocation. The Allocation overrides
section is optional and only for one-off pool or reserved-address overrides.
Empty pools mean there is no default allocator. Clicking Allocate endpoints
again appends the currently displayed endpoint addresses to Reserved addresses
before asking for another suggestion.

`Preference` is a routing-cost bias, not job priority. The default is `1.0`.
Higher values lower the recommended OSPF cost relative to the same latency,
loss, and bandwidth evidence.

## Workflow 1: vpsman-Managed Tunnel

Generate endpoint suggestions first. This step has no planning side effects.

```sh
cargo run -p vpsctl -- tunnel-allocate \
  --ipv4-pool-cidr 10.255.0.0/16 \
  --ipv6-pool-cidr fd80::/80 \
  --reserved-address 10.255.0.0,fd80::
```

Render a local `plan.json`:

```sh
cargo run -p vpsctl -- tunnel-plan \
  --name edge-a-b \
  --interface-name tunab \
  --kind gre \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-underlay 203.0.113.10 \
  --right-underlay 203.0.113.20 \
  --address-pool-cidr 10.255.0.0/16 \
  --ipv6-address-pool-cidr fd80::/80 \
  --left-tunnel-ipv4-cidr 10.255.0.0/31 \
  --right-tunnel-ipv4-cidr 10.255.0.1/31 \
  --left-tunnel-ipv6-cidr fd80::/127 \
  --right-tunnel-ipv6-cidr fd80::1/127 \
  --bandwidth 100m \
  --latency-ms 20 \
  > ./plan.json
```

Save the plan to the API when it is ready for shared use:

```sh
cargo run -p vpsctl -- tunnel-plan \
  --name edge-a-b \
  --interface-name tunab \
  --kind gre \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-underlay 203.0.113.10 \
  --right-underlay 203.0.113.20 \
  --address-pool-cidr 10.255.0.0/16 \
  --left-tunnel-ipv4-cidr 10.255.0.0/31 \
  --right-tunnel-ipv4-cidr 10.255.0.1/31 \
  --bandwidth 100m \
  --latency-ms 20 \
  --save \
  --enabled \
  --confirmed
```

Re-export a saved plan any time:

```sh
cargo run -p vpsctl -- tunnel-plans
cargo run -p vpsctl -- tunnel-plan-export \
  --plan-id <saved_plan_uuid> \
  --output-file ./plan.json
```

In the frontend, use Topology -> Plans -> Export JSON for the selected saved
plan. The exported file is the inner runtime `TunnelPlan` object expected by
status, probe, and speed-test commands, not the full saved-plan database record.

Enable or update the saved plan to apply desired tunnel state. Disable or edit
the plan to roll it back. Status, probe, and speed-test commands remain
explicit inspection tools:

```sh
cargo run -p vpsctl -- tunnel-status --plan-file ./plan.json --side left
cargo run -p vpsctl -- tunnel-probe --plan-file ./plan.json --side left
cargo run -p vpsctl -- tunnel-speed-test --plan-file ./plan.json --confirmed
```

Unprivileged agents report degraded mutation capability by default. Use
`--force-unprivileged` only for explicit best-effort adapter commands that are
safe as a normal user.

## Workflow 2: Observed Or Imported Tunnel

Use this when the tunnel process/config is owned outside vpsman, but operators
still want topology, evidence, and OSPF recommendation visibility.

```sh
cargo run -p vpsctl -- telemetry-tunnels --client-id edge-a
cargo run -p vpsctl -- tunnel-promote-external-observe \
  --client-id edge-a \
  --interface wg42 \
  --peer-client-id edge-b \
  --local-underlay 198.51.100.10 \
  --peer-underlay 203.0.113.20 \
  --address-pool-cidr 10.255.0.0/16 \
  --left-tunnel-ipv4-cidr 10.255.0.2/31 \
  --right-tunnel-ipv4-cidr 10.255.0.3/31 \
  --side left \
  --bandwidth 1000m \
  --latency-ms 8 \
  --enabled \
  --confirmed
```

Observed plans are observe-only. They do not let vpsman mutate ifupdown,
netplan, NetworkManager, or the tunnel process.

## Workflow 3: Custom Adapter Tunnel

Use this when another tunnel implementation should remain custom, but vpsman
should call bounded lifecycle commands and keep topology/Bird2 intent
consistent.

Start from an observed plan, then promote it to a custom adapter:

```sh
cargo run -p vpsctl -- tunnel-promote-custom-adapter \
  --plan-id <observed_plan_uuid> \
  --runtime-status-argv /usr/local/libexec/wg-adapter,status,{interface} \
  --runtime-startup-argv /usr/local/libexec/wg-adapter,start,{interface} \
  --runtime-stop-argv /usr/local/libexec/wg-adapter,stop,{interface} \
  --runtime-cleanup-argv /usr/local/libexec/wg-adapter,cleanup,{interface} \
  --confirmed
```

Custom adapters may provide startup, status, stop, cleanup, restart, and
traffic-limit commands for OpenVPN, WireGuard helper scripts, TUN programs,
provider scripts, or other custom adapter implementations. Status is required
for custom adapters. Mutating commands run only through privilege-gated
network jobs.

## Probe And Speed Test

```sh
cargo run -p vpsctl -- tunnel-probe \
  --plan-file ./plan.json \
  --side left \
  --count 3 \
  --interval-ms 500

cargo run -p vpsctl -- tunnel-speed-test \
  --plan-file ./plan.json \
  --server-side left \
  --duration-secs 3 \
  --max-bytes 16777216 \
  --rate-limit-kbps 100000 \
  --confirmed
```

Review persisted evidence:

```sh
cargo run -p vpsctl -- network-observations --limit 50
cargo run -p vpsctl -- network-trends --limit 50
```

Observation rows are retained as operational history, but topology health and
OSPF recommendations only use rows bound to the current saved plan ID and
endpoint identity hash. Reusing a plan name for different endpoints does not
carry old probe or speed-test evidence into the new topology edge.

## Bird2 OSPF Cost Updates

Generate recommendations from saved plans plus probe/speed trends:

```sh
cargo run -p vpsctl -- network-ospf-recommendations --limit 50
cargo run -p vpsctl -- network-ospf-update-plans --limit 50
```

Apply a reviewed cost delta:

```sh
cargo run -p vpsctl -- tunnel-ospf-cost-update \
  --plan-file ./plan.json \
  --side left \
  --current-ospf-cost 14 \
  --recommended-ospf-cost 22 \
  --confirmed
```

The cost model prefers higher bandwidth when latency is tolerable and
downgrades effective bandwidth when measured throughput falls below the
configured burst tier.

## Operator Rules

- Keep topology intent in saved tunnel plans.
- Configure allocation pools explicitly; empty pools mean allocation is manual.
- Use `tunnel-plan-export` or the frontend Export JSON action to obtain
  `plan.json` from saved plans.
- Keep source choices such as `ip`, `tc`, `ping`, `vnstat`, and Bird2 hooks in
  templates or server runtime config.
- Treat imported tunnels as first-class topology edges even when another
  program owns their process.
- Always inspect status/probe/speed evidence before changing OSPF cost.
