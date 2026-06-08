# Tutorial 06: Tunnels, Topology, And Bird2

The preferred model is runtime-owned tunnel management by the agent. Saved
plans describe intent; privileged apply/rollback changes only the managed
pieces. Imported or externally managed tunnels can be represented without
forcing ifupdown, NetworkManager, or netplan ownership.

## Plan A Built-In Tunnel

Create a non-mutating plan first:

```sh
cargo run -p vpsctl -- tunnel-plan \
  --name edge-a-b \
  --interface-name tunab \
  --kind gre \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-underlay 203.0.113.10 \
  --right-underlay 203.0.113.20 \
  --address-pool-cidr 10.255.0.0/30 \
  --bandwidth 100m \
  --latency-ms 20 \
  --save
```

Inspect saved plans:

```sh
cargo run -p vpsctl -- tunnel-plans
cargo run -p vpsctl -- topology-graph --limit 50
```

## Import Or Adapt External Tunnels

Promote observed telemetry into a saved observe-only plan:

```sh
cargo run -p vpsctl -- telemetry-tunnels --client-id edge-a
cargo run -p vpsctl -- tunnel-promote-telemetry \
  --client-id edge-a \
  --interface wg42 \
  --peer-client-id edge-b \
  --local-underlay 198.51.100.10 \
  --peer-underlay 203.0.113.20 \
  --address-pool-cidr 10.255.0.0/30 \
  --side left \
  --bandwidth 1000m \
  --latency-ms 8
```

Promote an observed/saved plan into an external adapter contract:

```sh
cargo run -p vpsctl -- tunnel-promote-adapter \
  --plan-id <saved_plan_uuid> \
  --runtime-status-argv /usr/local/libexec/wg-adapter,status,{interface} \
  --runtime-startup-argv /usr/local/libexec/wg-adapter,start,{interface} \
  --confirmed
```

Adapters can provide startup, status, stop, cleanup, and traffic-limit commands
for OpenVPN, WireGuard helper scripts, TUN programs, provider scripts, or other
custom tunnel implementations.

## Apply, Inspect, Roll Back

Apply one endpoint side at a time:

```sh
cargo run -p vpsctl -- tunnel-apply --plan-file ./plan.json --side left --confirmed
cargo run -p vpsctl -- tunnel-status --plan-file ./plan.json --side left
cargo run -p vpsctl -- tunnel-rollback --plan-file ./plan.json --side left --confirmed
```

Unprivileged agents report degraded mutation capability by default. Use
`--force-unprivileged` only for explicit best-effort adapter commands that are
safe as a normal user.

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
  --rate-limit-kbps 100000
```

Review persisted evidence:

```sh
cargo run -p vpsctl -- network-observations --limit 50
cargo run -p vpsctl -- network-trends --limit 50
```

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
- Keep source choices such as `ip`, `tc`, `ping`, `vnstat`, and Bird2 hooks in
  presets or agent config.
- Treat imported tunnels as first-class topology edges even when another
  program owns their process.
- Always inspect status/probe/speed evidence before changing OSPF cost.
