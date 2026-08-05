# Telemetry Metric Definitions

This document defines the numbers shown in Fleet > Monitor, Home, VPS detail,
and Observability. It separates live operational activity, authoritative traffic
accounting, general Ping, and bounded tunnel tests so a chart never implies a
measurement it does not contain.

## Time, Ranges, And Retention

Monitoring has two retained tiers, not competing minute/hour/day histories:

- Accepted high-resolution telemetry samples preserve the agent payload at its
  configured collection cadence. They support realtime and short-range queries
  and default to 90 days of retention.
- Minute-derived resource, network, traffic-counter, and Ping history is the
  authoritative long-term source. It defaults to 3,650 days of retention.
- Resource, network, and Ping rows for adjacent, exact-equivalent logical minutes
  may be stored as one longer minute-aligned span. This is lossless compaction,
  not a coarser hourly value: queries preserve the constituent minute weighting,
  sample count, coverage, extrema, latest evidence, and gaps. Traffic-counter
  rows remain minute-derived counters and are not folded into these spans.
- Long-term rows are materialized before an eligible high-resolution sample is
  pruned. Retention and compaction run in bounded leased batches.

The canonical VPS detail ranges are **15m**, **1h**, **8h**, **1d**, **7d**,
**30d**, **90d**, **180d**, **1y**, **All**, and **Custom**. **15m** is the
realtime view and uses the existing sample store. A range through
90 days uses high-resolution samples only when the entire requested interval is
still retained; 180d, 1y, All, and older custom intervals use minute history.
All means all retained history, normally up to ten years. Server-side chart
downsampling changes presentation density, never the authoritative retained
tier.

- The API accepts telemetry in authenticated process-incarnation and transport
  sequence order. Duplicate or older frames do not create another sample.
- Durable resource and interface-counter history uses API receive time, not the
  agent wall clock. A misconfigured VPS clock therefore cannot move a sample
  into another retained minute.
- “Current” means the latest accepted evidence that satisfies the page's
  freshness and scope rules. It does not mean a new read performed when the page
  renders.
- Missing intervals remain gaps. Missing, stale, invalid, unsupported, and
  unconfigured values never become healthy zeroes.

Operators manage the two tiers independently under Audit > Retention & export:
`telemetry_samples` is the high-resolution policy, while
`telemetry_rollups`, `telemetry_network_rates`, `telemetry_ping_rollups`, and
`traffic_counter_samples` are long-term policies.

## Resource Metrics

| Display metric | Retained meaning | Important unavailable state |
| --- | --- | --- |
| CPU utilization | Busy CPU time divided by total CPU time between two valid aggregate `/proc/stat` reads. Minute history retains average, maximum, valid-sample count, and maximum reported core count. | The first read, a counter reset, a zero delta, or an invalid `/proc/stat` read has no utilization value. Load is never substituted. |
| Load 1/5/15 | Sample-count-weighted arithmetic mean of the corresponding Linux load-average readings. Load pressure is normalized by the reported core count only for its visual track. | Missing load evidence remains unavailable; it is not CPU utilization. |
| Memory used | `(MemTotal - MemAvailable) / MemTotal` per accepted snapshot; interval history retains the sample-weighted average and maximum of those ratios. Capacity maximum and availability average/minimum remain separate evidence. | A missing or invalid memory snapshot rejects the core Linux collection instead of synthesizing zero. |
| Swap used | `(SwapTotal - SwapAvailable) / SwapTotal` per complete, positive-capacity snapshot; interval history retains the swap-sample-weighted average and maximum. `swap_sample_count` counts only those positive-capacity utilization samples. A complete `(0, 0)` report is retained as explicit “no swap” current evidence but contributes no utilization point or weight. | Missing, one-sided, invalid, and zero-capacity swap evidence remain chart gaps. Swap has its own sample count instead of borrowing memory coverage or fabricating 0%. |
| Aggregate reported-filesystem disk used | `(summed total - summed available) / summed total` per accepted snapshot, then sample-weighted average and maximum across the interval. Capacity maximum and availability average/minimum remain separate evidence. | This is an aggregate of reported filesystems, not a root-volume or quota claim. |
| TCP/UDP sockets | Agent-observed entries in the Linux network namespace's available IPv4 and IPv6 kernel socket tables. TCP includes every state and listening socket; UDP counts every reported UDP entry. | If neither address family supplies a protocol table, or a present table is malformed, both socket counts remain unavailable for that sample. Missing evidence is a chart gap, never a healthy zero. |

The Linux disk collector ignores pseudo filesystems, deduplicates repeated
source/filesystem pairs, and sums the filesystems it reports. A custom metrics
provider can replace or augment the Linux snapshot under the configured agent
contract. A replacement must provide the required load, core, memory, disk, and
network fields; CPU utilization remains optional. A core procfs or custom-source
failure rejects that collection so previous evidence ages naturally. A failure
limited to optional `/proc/stat` utilization resets its delta baseline and leaves
that field unavailable without discarding otherwise valid telemetry.

Cards pair exact CPU, RAM, disk, and load values with proportional tracks or
small histories. Neutral colors stay stable; warning and danger states come from
explicit backend alert or data state, not hidden browser thresholds.

## Session-Reported System Information

The agent reports relatively static host facts once in its authenticated session
hello rather than duplicating them in every telemetry sample: parsed OS name,
architecture, CPU model, kernel release, and evidenced virtualization kind. The
control plane stores their observation time with the VPS identity. Uptime remains
sampled telemetry and is joined from the latest accepted resource evidence.

These facts are descriptive evidence, not inventory discovery. Missing or
unreadable sources omit only the affected optional fact. The public projection
contains the normalized display fields only; it never forwards raw `os-release`,
hostname, IP addresses, capability payloads, build identity, process identity,
or interface data.

## Interface Rate

Agents report cumulative RX and TX byte counters per interface. vpsman derives
an interval-average bit rate for each VPS/interface stream:

```text
rate_bps = (current_counter - previous_counter) * 8
           / elapsed_seconds
```

Storage, API fields, and raw CSV values keep this `bps` bit-rate contract.
Operator-facing live RX/TX values and chart labels convert it by eight and use
decimal transfer-rate units (`B/s`, `KB/s`, `MB/s`, `GB/s`). Declared port
speed, tunnel bandwidth, shaping/rate limits, and bounded speed-test throughput
remain capacity measurements in `bps`, `Kbps`, `Mbps`, or `Gbps`; they must not
use the live-transfer formatter.

The query includes one pre-window counter as a baseline when available. A
counter reset or wrap invalidates that interval instead of inventing zero
activity; the reset point is retained as the next interval's baseline. The
first point without a baseline is likewise omitted, so both cases appear as an
explicit chart gap.

Config > Rules scopes aggregate live rate through `network.rate.interfaces`:

- An absent or unset value defaults to a live reference to the current per-VPS
  `traffic.selectors` rule, using its selected host interfaces.
- The explicit input syntax `[traffic.selectors]` stores that same typed
  reference object; it is not copied into a fixed selector list.
- Empty input or `[]` explicitly selects every reported interface.
- Any other value uses the existing interface/direction selector grammar. For
  example, `eth0,eth1+tx` selects `eth0` and `eth1`. Live-rate scope ignores
  `+rx` and `+tx` suffixes and retains both separately reported speed
  directions for every selected interface. Those suffixes remain meaningful
  only to authoritative traffic accounting.

This scope controls aggregate rate values on VPS cards, charts, the dashboard,
and public monitoring views. It does not filter agent collection or retained
per-interface evidence: raw interface diagnostics, APIs, and CSV remain
complete.

Fleet chart points sum both direction rates for the selected interfaces in each
logical interval, then average those fleet totals when several intervals share
one displayed chart step. Monitoring-card current values accept only selected
rows within 180 seconds of the card snapshot; historical curves retain older
rows and their gaps. When all interfaces are selected, virtual, bridge, and
tunnel interfaces can represent overlapping traffic, so the sum is operational
activity rather than guaranteed unique wire volume.

## Authoritative Traffic Accounting

Traffic is separate from live interface rate. Config > Rules defines the
authoritative interface/direction selectors, quota, reset day, and cycle. The
cards and VPS detail use only those saved accounting rules and retained cycle
state; they never silently sum arbitrary interfaces as billing traffic.

- If selectors or reset-cycle configuration are missing, vpsman displays
  **Traffic unconfigured**. Quotas are optional; traffic remains accounted and
  visible without quota progress. A quota value of `-1` means explicitly
  unlimited and remains distinct from an unset quota.
- When current accounting is unconfigured, retained counter history may still
  exist from a prior rule. Detail views label it as prior accounting history and
  never reuse those retained totals, cycle dates, or quotas as current evidence.
- RX, TX, and total bytes remain exact even when usage exceeds a quota. A visual
  progress track may fill completely, but the numeric percentage and totals may
  exceed 100%.
- Each selected source/interface counter stream is differenced independently.
  Diagnostic RX and TX are visible by default, while the derived Total series
  is selectable through the existing chart legend. A selector's billing
  direction affects quota accounting only; it does not hide either diagnostic
  direction from history.
- Every configured date-based cycle boundary starts a new RX and TX accounting
  cycle together, regardless of which direction contributes to the quota.
  Billing direction changes the limited total, not the lifetime or reset point
  of either diagnostic counter.
- A counter-epoch change or counter decrease is reset evidence, not zero
  traffic. A bucket containing only reset intervals has `sample_count = 0`, a
  positive `reset_count`, and nullable RX, TX, and total values, so its chart
  remains a gap. A mixed bucket retains its valid deltas and its reset count as
  explicit incomplete evidence.
- Long-term traffic counters keep the latest accepted counter for each logical
  minute and default to ten years. Pruning preserves one pre-cutoff baseline per
  VPS/source/interface stream, and configured retention cannot be shorter than
  32 days so an active monthly cycle remains computable.

The optional display rules next to traffic do not alter accounting:

- `billing.price` accepts an amount, currency symbol or three-letter code, and
  `/m`, `/q`, `/h` or `/hy`, or `/y`. `-1` explicitly displays billing as
  **n/a**; an unset rule shows no billing fact.
- `billing.cycle` is an optional renewal anchor, independent of traffic reset:
  a day for monthly billing, or day-month for quarterly, half-yearly, and yearly
  billing.
- `network.port_speed` accepts an explicit bit-rate unit such as `400 Mbps` or
  `1.5 Gbps`. It is display-only on monitoring cards. A new tunnel-plan draft
  may use one endpoint's value, or the lower of two values, as a convenience;
  it never configures shaping or discovers interface capacity.

## General Ping Targets

Observability > Ping targets owns reusable, named ICMP or TCP probes. A TCP
target requires an explicit port. Each enabled target is distributed through
server-managed runtime config to its frozen VPS assignments; an agent accepts at
most 16 enabled targets and runs three bounded attempts every 60 seconds with
bounded concurrency.

Each result carries the target ID, target generation, checked time, average
successful-attempt latency, loss ratio, status, and a bounded failure reason.
Probe-affecting edits advance the generation. Current values and history accept
only the active assignment generation, so old and new target definitions cannot
be mixed.

All assigned targets appear in a VPS's Ping detail. Only the explicitly selected
primary target appears on its fleet card. Disabling or removing a primary leaves
an explicit disabled or unconfigured state; vpsman does not silently choose a
replacement. Missing probe intervals remain gaps, including complete loss where
latency is unavailable.

General Ping is independent from declared-tunnel tests. A Ping target does not
enable a tunnel, assess an OSPF plan, or become tunnel health evidence.

## Declared-Tunnel Tests

Only observations bound to a saved tunnel plan appear in Network Metrics.
Status records that do not contain the selected measurement remain evidence but
do not become empty chart points.

| Measurement | Point definition | Retained trend definition |
| --- | --- | --- |
| Latency | Mean RTT reported by one bounded ICMP probe run. | Arithmetic mean of retained run averages that contain latency. |
| Packet loss | Lost/transmitted packet ratio from one bounded ICMP probe run. | Arithmetic mean of retained run ratios that contain loss. |
| Throughput | `bytes transferred * 8 / actual elapsed seconds / 1,000,000`; average TCP throughput over one capped test. | Arithmetic mean of retained test averages that contain throughput; maximum is the highest retained test average. |

Throughput is not an interface line-speed sample. It is constrained by the
test's configured duration, byte cap, rate limit, TCP path, peer verification,
and current host/network conditions. Compare it with a plan's operator-entered
bandwidth as evidence, not as automatic discovery of link capacity.

## Reading Evidence

- Exact sample endpoints and retained span are shown separately from relative
  age so two old timestamps do not collapse into an ambiguous label.
- Sparse evidence renders as points. A line is shown only when the selected
  metric has enough retained measurements for a trend reading.
- Coverage counts use only series and timestamps for the selected metric. Gaps
  between those measurements remain visible.
- Last-known values stay available for diagnosis but carry stale state when
  they fall outside the current freshness window.
- CSV export contains only the visible series and selected retained range.

Shared monitoring views reuse these definitions but expose only the immutable
metric visibility groups selected when the share was created. Billing and
normalized system information are separate, opt-in groups; an unset billing
rule or absent system fact is omitted rather than rendered as a misleading
placeholder. They never expose real VPS IDs, network-address fields, raw host
files, internal configuration, actions, jobs, terminals, files, backups, audit
data, or operator identity. Operator-entered display and Ping target names
appear as entered.

## Fleet Alert Read-Model Bounds

Fleet alerts are a bounded operational read model, not an unbounded history
export. The API combines current agent and resource snapshots with at most 200
matching candidates from each durable event source. Client, category, severity,
and dashboard-time filters are applied by the source before that horizon. The
combined result is then merged with operator state, ordered, and limited to the
requested page.

When any source reaches its horizon, dashboard responses mark the affected
counts as truncated and the console renders them as lower bounds (`200+` or
`≥200`) instead of presenting an exact total. An `operator_state` filter applies
to this same bounded active-alert working set. Use the fleet-alert-state ledger
for each alert's current durable triage state, the audit log for triage
transitions, and the owning policy, job, backup, or network workflow for older
event history.
