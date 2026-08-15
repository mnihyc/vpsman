# Telemetry Metric Definitions

This document defines the numbers shown in Fleet > Monitor, Home, VPS detail,
and Observability. It separates live operational activity, authoritative traffic
accounting, general Ping, and bounded tunnel tests so a chart never implies a
measurement it does not contain.

## Time, Ranges, And Retention

Monitoring has one exact short-range source and one canonical, age-tiered
long-term source:

- Accepted high-resolution telemetry samples preserve the agent payload at its
  configured collection cadence. Transactionally derived scalar and counter
  facts provide an indexed projection of the same accepted sample. General Ping
  stores one deterministic winner per logical probe instead of repeating an
  unchanged cached result in every telemetry frame. Its internal logical key
  uses the original agent check timestamp, while charts and range filters keep
  using the API-rebased timestamp so a misconfigured VPS clock cannot move
  evidence across the retained timeline. Raw samples and their facts
  support realtime and short-range queries and default to seven days of
  retention.
- Resource, network-rate, general-Ping, and system history retains 1-minute
  buckets through 2 days, 5-minute through 8 days, 30-minute through 31 days,
  1-hour through 91 days, 3-hour through 181 days, 6-hour through 366 days, and
  1-day buckets through 3,650 days.
- Traffic counters remain exact minute endpoints through 32 days so every
  supported active monthly cycle remains exact. Older traffic transitions use
  1-hour buckets through 91 days, 3-hour through 181 days, 6-hour through 366
  days, and 1-day through 3,650 days. Counter resets and imported versus live
  provenance remain separate.
- vnStat imports replace only their imported traffic-ledger contribution.
  They retain live traffic contributions and never rewrite the independent
  live network-rate curve or its counter epochs.
- Promotion sums sufficient statistics, counts, extrema, reset evidence, and
  the actual latest observation. It never spreads a coarse bucket into
  fabricated fine points. Sparse intervals therefore remain sparse.
- Long-term rows are materialized before eligible exact evidence is pruned.
  Promotion and pruning run in bounded, leased, transactional batches.

The canonical VPS detail ranges are **15m**, **1h**, **8h**, **1d**, **7d**,
**30d**, **90d**, **180d**, **1y**, **All**, and **Custom**. **15m** is the
realtime view and uses the existing sample store. A range uses high-resolution
samples only when the entire requested interval is still retained; older
intervals use tiered retained history. All means all retained history, normally
up to ten years. Server-side chart downsampling is aligned to the coarsest
retained tier in the selected range. The API reports requested step, effective
source resolution, actual chart step, and effective point count. A narrow old
custom range may therefore contain only one or a few coarse points; the UI
labels that resolution and never interpolates missing detail.

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

Operators manage exact and long-term retention independently under Audit > Retention & export:
`telemetry_samples` is the high-resolution policy, while
`telemetry_rollups`, `telemetry_network_rates`, `telemetry_ping_rollups`, and
`traffic_counter_samples` are long-term policies.

Counter fact rows share the sample's retention lifecycle and are deleted with
it. Logical Ping evidence uses the same seven-day cutoff but is keyed by target
series and original checked time so cached repeats do not consume another row,
even when transport timing rebases repeated frames to adjacent server seconds.
The original time is identity-only and is not exposed as chart time. All facts
are written only after the authenticated ingest sequence is accepted and in the
same database transaction as the retained JSON payload and long-term rollups.
Duplicate or stale frames therefore cannot create history, and the JSON payload
remains the raw audit/export representation.

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
speed, tunnel bandwidth, shaping/rate limits, and duration-bounded speed-test
throughput remain capacity measurements in `bps`, `Kbps`, `Mbps`, or `Gbps`;
they must not use the live-transfer formatter.

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

If the default reference has no configured `traffic.selectors`, aggregate RX
and TX display as **-**. That is an intentional empty selection, not partial
telemetry. An explicit all or non-empty interface selection with missing
samples remains incomplete and is surfaced as such.

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
- `traffic.reset_day=-1` keeps accounting configured without a calendar reset.
  Its total is the sum of valid deltas from all retained counter evidence;
  cycle start and end are absent, and counter-reset intervals remain gaps. Days
  `1` through `31` retain the existing UTC monthly-cycle behavior.
- Each selected source/interface counter stream is differenced independently.
  Diagnostic RX and TX are visible on cards and detail and by default in
  history, while the derived Total history series is selectable through the
  existing chart legend. These diagnostic cycle totals deduplicate a stream
  selected through separate `+rx` and `+tx` entries. A selector's billing
  direction affects the counted RX/TX/total used by quota progress and alerts;
  it does not hide either diagnostic direction.
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

### One-time vnStat history import

vnStat is not a selectable runtime accounting backend. Agent-managed tunnel
interfaces may not exist long enough for `vnstatd` to retain them, so tunnel
traffic telemetry always uses the managed interface's live kernel counters.

For a host interface whose agent started after the current traffic cycle, an
operator may dispatch `network_traffic_import_vnstat` once. The command accepts
an optional list of host interface names and a UTC-minute-aligned start. An
empty list asks the agent to read one all-interface vnStat JSON snapshot and
import every valid interface it reports; an explicit list remains exact and is
queried with vnStat's canonical `--iface` option. The API derives
the end independently for each interface from its first retained live agent
sample; an operator-supplied end could create a gap or overlap and is therefore
not part of the command.

The agent supports vnStat 2.0 and newer and reads the retained five-minute,
hourly, daily, monthly, and yearly JSON in one snapshot per interface. For
vnStat 2.0–2.9, whose JSON calendar fields do not include Unix timestamps, the
agent reconstructs each timestamp from the reported date and time. Those
versions report an interface's creation date without a time of day, so it is
conservatively treated as midnight in vnStat's configured calendar mode;
imported byte totals remain exact, but the first partial day can begin up to 24
hours earlier than the interface was actually created. The agent also reads
vnStat's effective calendar configuration once per import, so `MonthRotate`,
`MonthRotateAffectsYears`, local-versus-UTC storage, and sparse trafficless
periods are interpreted consistently with the source database. Monthly and
yearly rows use their natural calendar boundaries rather than the next retained
row. The agent merges the emitted bucket intervals and reports the start of the
latest continuous retained component for each interface. The API validates that
component through the first live sample and starts at the later of its start and
the operator's requested minute. This skips expired leading history and any
older component separated by a retention gap without inventing traffic. The
requested start remains present in the job operation and agent result for audit.
Old history therefore remains
usable after daily rows expire without stretching a sparse row across a missing
period. If rotated month periods do not also rotate year periods, the single
monthly row that crosses a year boundary is omitted when aligned year rows
supply the requested span; if yearly collection is disabled, the month remains
authoritative. This keeps the aggregate hierarchy reconcilable. The API
reconciles overlapping resolutions from finest to coarsest,
preserves each aggregate byte total, and distributes only the unresolved coarse
residual across uncovered minutes. It then inserts cumulative synthetic
host-interface samples before the first live sample. The synthetic to live
transition is an intentional counter epoch boundary: no bridge delta is counted
and it is not reported as a counter reset.

Collection and server-side backfill are asynchronous and durable. After the
agent's output is persisted, the target remains running while the API imports
it; an API restart discovers that output and resumes finalization. The import
requires continuous retained coverage through a live agent sample that
establishes its end. Rerunning it replaces only prior `vnstat_import:*` samples
for the selected interfaces. Normal agent collection continues unchanged
afterward; vnStat is not polled periodically by vpsman. There is no fixed import
lookback: a requested date may precede retained history, and each interface is
clamped independently to the start of its latest continuous retained coverage.
Long ranges are
expanded and inserted in bounded batches rather than allocating the complete
minute span in memory.

The optional display rules next to traffic do not alter accounting:

- `billing.price` accepts an amount, currency symbol or three-letter code, and
  `/m`, `/q`, `/h` or `/hy`, or `/y`. `-1` and an unset rule display billing as
  **-** on operator monitoring cards. A Shared view shows that placeholder only
  when Billing is included in its visibility.
- `billing.cycle` is an optional renewal anchor, independent of traffic reset:
  a day for monthly billing, or `MM-DD` for quarterly, half-yearly, and yearly
  billing. Full calendar anchors use the same `MM-DD` syntax in storage,
  search, API responses, and operator displays.
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

The current summary smooths packet loss over the 15 minutes ending at the
latest accepted result, independently of the history range selected in the UI.
It weights retained rows by `sample_count`; a target is degraded when that
rolling loss is at least 10%. The window uses whatever evidence is available
without a warm-up state or minimum sample count. A latest complete `down` or
`error` result takes effect immediately, while the displayed loss remains the
15-minute weighted value.

All assigned targets appear in a VPS's Ping detail. Only the explicitly selected
primary target appears on its fleet card. Disabling or removing a primary leaves
an explicit disabled or unconfigured state; vpsman does not silently choose a
replacement. Missing probe intervals remain gaps, including complete loss where
latency is unavailable.

General Ping is independent from declared-tunnel tests. A Ping target does not
enable a tunnel, assess an OSPF plan, or become tunnel health evidence.

## Declared-Tunnel Monitoring and Tests

Each enabled declared tunnel can run the existing bounded ICMP reachability
check from both endpoints at its configured latency-monitoring cadence. Every
completed run, including a failed run without latency, is retained in the same
`network_observations` timeline used by a manual Network probe. This is not a
second Ping-target system or a new telemetry store.

Automatic observations carry their endpoint side, address family, source,
topology identity, measured time, packet counts, latency distribution, loss,
result, and freshness window. Network Metrics, Network Evidence, Tunnel plans,
and the Topology graph distinguish automatic monitoring from manual tests and
show expired evidence as stale. A failed run remains an explicit chart gap; it
is never omitted or drawn as zero latency.

The topology identity includes the saved plan name, kind, endpoints, interface,
local and remote underlay bindings, tunnel addresses, and primary address
family. Changing one of those fields starts a new evidence generation. MTU,
bandwidth, OSPF policy, and runtime command changes do not detach otherwise
valid reachability history.

From **Network > Tunnel plans**, a reviewed **Clear evidence** action can remove
all retained automatic and manual observations for the selected plans when an
operator wants to discard evidence from earlier topology identities. The
action is audited and leaves declarations, runtime state, jobs, assessments,
and OSPF state unchanged. Monitoring remains enabled and records new evidence
normally.

Automatic OSPF control requires the newest paired endpoint reachability window
to be fresh and the configured number of preceding paired windows to be
contiguously healthy. Endpoint probes may be independently phased within their
declared cadence. Exact automatic reachability observations remain available
for two days. Older automatic reachability history follows the same 5-minute,
30-minute, 1-hour, 3-hour, 6-hour, and 1-day schedule as other retained
monitoring evidence. Latest endpoint evidence is retained separately, so
promotion never changes current topology or OSPF decisions. Manual probes,
speed tests, and status records remain exact rows under their existing policy.
Older windows in an automatic contiguous streak remain evidence even
after their individual current-state window expires; stale newest evidence
never authorizes an update. Reviewed OSPF and manual probes continue to use the
same retained evidence model.

Only observations bound to a saved tunnel plan appear in Network Metrics.
Status records that do not contain the selected measurement remain evidence but
do not become empty chart points.

| Measurement | Point definition | Retained trend definition |
| --- | --- | --- |
| Latency | Mean RTT reported by one bounded ICMP probe run. | Arithmetic mean of retained run averages that contain latency. |
| Packet loss | Lost/transmitted packet ratio from one bounded ICMP probe run. | Arithmetic mean of retained run ratios that contain loss. |
| Throughput | `bytes transferred * 8 / actual elapsed seconds / 1,000,000`; average TCP throughput over one duration-bounded test. | Arithmetic mean of retained test averages that contain throughput; maximum is the highest retained test average. |

Throughput is not an interface line-speed sample. The configured duration
always bounds its transfer phase, and the overall job timeout bounds the full
test workflow. Optional byte and rate limits add stricter bounds when
configured; the TCP path, peer verification, and current host/network
conditions also shape the result. Compare it with a plan's operator-entered
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
normalized system information are separate, opt-in groups. A group that was
not shared is absent. A shared current fact remains in its established place
and renders `-` when no evidence was reported; explicit states such as
`Unlimited`, `Unconfigured`, `Disabled`, or `None` are never collapsed into
that placeholder. Shared views never expose real VPS IDs, network-address
fields, raw host files, internal configuration, actions, jobs, terminals,
files, backups, audit data, or operator identity. Operator-entered display and
Ping target names appear as entered.

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
