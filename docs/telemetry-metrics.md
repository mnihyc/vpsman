# Telemetry Metric Definitions

This document defines the numbers shown in Fleet / Monitor, Home, and
Observability. It distinguishes retained interval averages from bounded network
tests so operators do not interpret a chart point as a random instantaneous
sample.

## Time And Aggregation

- The API accepts telemetry in authenticated process-incarnation and transport
  sequence order. Duplicate or older frames do not create another sample.
- Durable resource and interface-counter rollups use API receive time, not the
  agent wall clock. This prevents a misconfigured VPS clock from moving samples
  into the wrong chart interval.
- The base retained interval is 60 seconds. When a selected range needs a
  coarser chart step, the dashboard averages the retained interval values in
  that displayed step. Missing intervals remain gaps.
- "Current" means the latest accepted interval that satisfies the page's
  freshness and scope rules. It does not mean an instantaneous read performed
  when the page rendered.

## Resource Metrics

| Display metric | Base 60-second value | Coarser chart point | Peak/lowest value |
| --- | --- | --- | --- |
| CPU load | Sample-count-weighted arithmetic mean of Linux 1-minute load readings accepted in the interval. | Arithmetic mean of retained base-interval values. | Maximum accepted 1-minute load. Linux load is scheduler demand, not CPU utilization percent. |
| Memory used | `(max MemTotal - mean MemAvailable) / max MemTotal` for the interval. | Arithmetic mean of retained used ratios. | Uses the minimum accepted `MemAvailable` value. |
| Disk free | `mean available bytes / max total bytes` after summing reported filesystems for each sample. | Arithmetic mean of retained free ratios. | Uses the minimum accepted summed available bytes. |

The Linux disk collector ignores pseudo filesystems, deduplicates repeated
source/filesystem pairs, and sums the filesystems it reports. A custom metrics
provider can replace or augment the Linux snapshot under the configured agent
contract.

## Interface Rate And Traffic

Agents report cumulative RX and TX byte counters per interface. vpsman derives
an interval-average bit rate for each VPS/interface pair:

```text
rate_bps = max(current_counter_avg - previous_counter_avg, 0) * 8
           / elapsed_seconds_between_bucket_starts
```

The query includes one pre-window counter as a baseline when available. A
counter reset or wrap is clamped to zero instead of producing negative traffic.
The first retained point without a baseline is therefore zero.

Fleet chart points sum interface rates for each base interval, then average
those fleet totals when several base intervals share one displayed chart step.
The current fleet/VPS value sums only the latest coherent interface rows; rows
more than 180 seconds behind that VPS's newest interface sample are excluded.
Virtual, bridge, and tunnel interfaces can represent overlapping traffic, so a
sum across interfaces is operational activity rather than guaranteed unique
wire volume.

Traffic totals are different: they sum non-negative counter deltas over the
selected range and are displayed as bytes. Rate and traffic must not be treated
as interchangeable.

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
