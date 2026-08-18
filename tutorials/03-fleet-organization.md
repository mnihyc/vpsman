# Tutorial 03: Fleet Organization

Use tags for provider, location, ownership, and operational selection. This
keeps bulk actions predictable when managing 20+ VPSs.

## Create Tags

```sh
cargo run -p vpsctl -- tag-create --name provider:provider-a
cargo run -p vpsctl -- tag-create --name region:sfo
cargo run -p vpsctl -- tag-create --name edge
cargo run -p vpsctl -- tag-create --name bgp
cargo run -p vpsctl -- tag-create --name lab
```

Assign agents:

```sh
cargo run -p vpsctl -- agent-tag --client-id edge-01 --tag provider:provider-a
cargo run -p vpsctl -- agent-tag --client-id edge-01 --tag region:sfo
cargo run -p vpsctl -- agent-tag --client-id edge-01 --tag edge
cargo run -p vpsctl -- agent-tag --client-id edge-01 --tag bgp
```

## Resolve Before Bulk Operations

Bulk operations should resolve the exact target set before dispatch:

```sh
cargo run -p vpsctl -- bulk-resolve --tags edge
cargo run -p vpsctl -- bulk-resolve --tags provider:provider-a,region:sfo
cargo run -p vpsctl -- bulk-resolve --tags id:edge-01
cargo run -p vpsctl -- bulk-resolve --clients edge-01 --tags lab
```

These commands are the headless path. In the console, each operation keeps the
same target control in its normal workflow, previews matches automatically, and
uses **Review** to freeze the server-resolved list; there is no separate bulk
page.

Use `id:<client_id>` or explicit client ids for destructive one-off work, and
tags for operational groups, provider labels, countries, regions, or optional
`pool:<name>` labels.

## Daily Fleet Views

```sh
cargo run -p vpsctl -- summary
cargo run -p vpsctl -- agents
cargo run -p vpsctl -- fleet-alerts
cargo run -p vpsctl -- telemetry-rollups --latest
cargo run -p vpsctl -- telemetry-network-rates --latest
cargo run -p vpsctl -- gateway-sessions
```

### Scan the visual VPS grid

Home shows a bounded fleet preview. Open **Fleet > Monitor** for the complete
matching fleet; the grid defaults to all VPSs and progressively presents every
match rather than making a small first page the workflow.

Use search, status, tag, provider, and sort controls before changing density:

- **Comfortable** normally shows fewer, wider cards with identity context,
  fuller resource/network histories, traffic-cycle details, and primary Ping.
- **Compact** fits more cards per row with a tight CPU/RAM/disk/load matrix, one
  RX/TX activity row, one traffic row, and one primary-Ping strip. It is a
  different information hierarchy, not a smaller Comfortable card.

Every primary metric has a visual indicator and an exact value. A missing or
stale value remains visibly unavailable; card color does not invent an alert
threshold. **Traffic unconfigured** means the VPS lacks authoritative selectors
or a reset cycle under Config > Rules. A quota is optional; accounting remains
available without one. Card selection opens the canonical VPS detail, where
resource, network, traffic, and all assigned Ping evidence share the ranges
**15m**, **1h**, **8h**, **1d**, **7d**, **30d**, **90d**, **180d**, **1y**,
**All**, and **Custom**. 15m is the rolling realtime view. Traffic
history shows diagnostic RX and TX by default; use the existing chart legend to
show Total. A selector's billing direction affects quota accounting, not which
diagnostic direction remains available in the chart.

Grid search, filters, sort, density, and scroll position belong to the current
browser-history entry. Back/Forward restores them until a manual refresh starts
a fresh entry.

## Manage General Ping Targets

Use **Observability > Ping targets** for reusable ICMP or TCP measurements that
are independent from tunnel tests. A TCP target requires a port. Creating a
target resolves the default `*` or entered selector to a frozen VPS list.
Editing other fields preserves that list; changing the selector expression
resolves its current matches for that save.

When tags or other selector inputs later change, select one or more definitions
whose saved expression resolves differently and use **Update targets** from the
header **Actions** menu. Review the exact additions/removals, then confirm the
transactional replacement. The saved expression is never a live assignment
rule. Expand a target to inspect assigned VPSs; select VPS rows and use **Make
primary** when that target should appear on those cards. Every assigned target
still appears in VPS Ping detail.

Probe-affecting edits start a new target generation, and runtime-sync evidence
shows whether the affected agents received it. An agent accepts at most 16
enabled targets and runs three bounded attempts every 60 seconds. Disabling or
removing a primary leaves an explicit unconfigured/disabled state; no target is
chosen automatically.

## Manage Shared Monitoring Views

Use **Observability > Shared views** as the durable management surface. The
shortcut in Fleet > Monitor carries the current grid filter/selection into a new
share draft; otherwise the default selector is `*`. Review freezes the exact VPS
list and these optional visible groups: identity context, resources, network,
traffic, Ping, and detail history. Display name and health are always visible.

The default expiry is 24 hours; accepted expiry is one minute through 365 days.
The public URL is shown after creation and can later be recovered with the
active row's **Copy URL** action; treat it as a bearer credential. Targets stay
frozen until **Update targets** re-resolves the saved selector through a reviewed
preview. Visibility remains immutable after creation.

The Active, Expired, and Revoked tables retain lifecycle and access evidence.
Select active rows to update targets or extend them, capped at 365 days from
extension time, or to revoke them immediately and irreversibly. Expired and revoked links cannot be
reactivated; create a replacement. Each distinct visitor creates one
`monitoring_share.visitor_opened` audit event with source-IP and bounded
User-Agent evidence. Later polling updates last-accessed evidence without
creating another event for that visitor.

Public pages reuse the monitoring grid/detail in read-only form. They expose
only the selected groups and opaque share-specific VPS keys, never internal VPS
IDs or network-address fields, internal configuration, actions, jobs, terminals,
files, backups, audit data, or operator identity. Operator-entered public labels
appear as entered.

## Configure Fleet Alert Policies

Resource threshold alerts are issued only by enabled policy groups in
Observability > Alerts. Starter resource policies are disabled by default, so
review their selectors, typed evidence source, Trigger condition, Resolve
condition, and meta conditions before enabling them.

Inspect filtered alerts from CLI or VTY:

```sh
cargo run -p vpsctl -- fleet-alerts --severity critical
cargo run -p vpsctl -- fleet-alerts --client-id edge-01 --limit 20
```

Policy-issued alert evidence includes the rule and threshold that fired. Use
this to adjust the policy deliberately instead of suppressing useful warnings.
In the panel, fleet alert records are shown in a dense table with search, pagination,
selection, expandable evidence, and bulk acknowledge, mute, escalate, or clear
actions for daily fleet triage.

Triage an alert without changing the detection policy:

```sh
alert_id="$(cargo run -p vpsctl -- fleet-alerts --severity warning --limit 1 | jq -r '.[0].id')"
cargo run -p vpsctl -- fleet-alert-state-update \
  --alert-id "$alert_id" \
  --action mute \
  --muted-for-secs 14400 \
  --reason maintenance \
  --confirmed
cargo run -p vpsctl -- fleet-alerts --operator-state muted --include-muted
cargo run -p vpsctl -- fleet-alert-export --include-muted --limit 200
```

Use `--action acknowledge`, `--action escalate`, or `--action clear` for the
same alert id when the operational state changes.

For per-VPS traffic accounting, save Config > Rules values first. These are
low-level server-side values keyed by VPS and rule key; the alert policy editor
reads them but does not modify them.

```sh
cargo run -p vpsctl -- vps-rules preview \
  --selector 'tag:edge' \
  --set traffic.reset_day=14 \
  --set traffic.quota.total=3TB \
  --set traffic.selectors=eth0+tx,ens3

cargo run -p vpsctl -- vps-rules upsert \
  --selector 'tag:edge' \
  --set traffic.reset_day=14 \
  --set traffic.quota.total=3TB \
  --set traffic.selectors=eth0+tx,ens3 \
  --confirmed
```

The same Config > Rules editor also accepts optional card-only facts:
`billing.price=29.90 CNY/m`, `billing.cycle=15`, and
`network.port_speed=1.5 Gbps`. Quarterly, half-yearly, and yearly prices use a
month-day renewal anchor such as `06-15`; the billing anchor is independent of
`traffic.reset_day`. Use `-1` for an explicitly unlimited traffic quota or
explicit billing **-**. Leaving a field blank means no rule, which is a
different operator choice. Port speed is display-only, although a new tunnel
plan can prefill its editable Mbps bandwidth from one endpoint or the lower of
both endpoints.

Then create a policy group under Observability > Alerts. The selector chooses
target VPSs using the same expressions as dispatch previews (`tag:edge`,
`provider:hetzner`, `id:<client_id>`, boolean operators, and parentheses).
Choose **Metric condition** with `telemetry.combined`, then enter:

```text
Trigger condition: traffic.cycle.total >= traffic.quota.total * 0.8
Trigger meta condition: Sustained 5m
Resolve condition: traffic.cycle.total < traffic.quota.total * 0.7
Resolve meta condition: Sustained 2m
```

The lower Resolve condition supplies hysteresis. If it is blank, recovery
requires the Trigger condition to evaluate conclusively false. Leaving either
meta condition empty means Immediate. Missing or stale quota/counter evidence
is Unknown: it pauses Sustained confirmation and never masquerades as recovery.

In the UI, Fleet > Instances keeps traffic columns hidden by default; enable
them through Fields when you need operational status in the main table. Expand a
VPS and open Traffic & Rules for counters, current cycle usage, incomplete
reasons, current policy lifecycle, and recent policy alert history. Use
Config > Rules for bulk dry-run, preview-hash confirmation, and explicit unset
actions. Use Observability > Alerts for policy-group editing, selector dry-runs,
notification channels, and rule previews.

Unresolved policy episodes appear in Fleet > Alerts. A confirmed new generation
emits only `alert.triggered`; confirmed recovery emits only `alert.resolved`.
Persisting and Unknown evaluations emit no automation edge. Delivery payloads
use `alert`, `policy`, and `policy_rule` for the persisted lifecycle snapshot,
`matched_vps` for matched VPSs, and `rule` for the webhook rule. Resolved rows
remain in alert history.

Lifecycle edges bypass webhook-rule cooldown because the policy generation
already suppresses repeated true evaluations. Delivery remains retryable, so
consumers should deduplicate by `event.id` and use the alert ID, trigger
generation, lifecycle state, and timestamps rather than assuming HTTP arrival
order.

The predefined traffic starters are ordinary disabled policies. Once an
operator reviews and enables one, a rule that passes its Trigger condition and
meta condition enters **Triggered** and issues one edge for that generation.
Repeated valid confirmation moves it to **Persisting**, updates its last
confirmation time, and reuses the generation. The Resolve condition and meta
condition mark the generation **Resolved**. Missing, stale, or incomplete
evidence is **Unknown**; it neither resolves nor re-arms the rule. A later
recovery-to-trigger recurrence starts a new generation.

Resolution reasons are explicit and stable: `condition_recovered`,
`recovery_expression_matched`, `policy_scope_exited`, `policy_scope_changed`,
`policy_disabled`, `policy_changed`, and `policy_deleted`. Automation should
branch on those values; missing evidence is Unknown, never an implicit
recovery.

Fleet > Alerts is the unified policy-owned lifecycle view for agent, tunnel,
job, backup, capability, resource, and traffic evidence. Resolved alerts leave
the current queue but remain in history, where Triggered, Persisting, Unknown,
and Resolved states include their available confirmation or resolution
evidence. Active summaries count only Triggered and Persisting rows. Unknown is
current but inactive and never treated as an implicit zero or recovery.

### Dispatch a job from a confirmed alert

Open Automation > Schedules, create a schedule, and choose **Alert event**
instead of **Time · cron**. The two modes are intentionally separate: Cron owns
time cadence and catch-up, while Alert event owns prospective policy lifecycle
edges. For a traffic control workflow, create two schedules over the same
reviewed target snapshot:

```text
Mitigation expression: alert.triggered && alert.category:traffic
Recovery expression:   alert.resolved && alert.category:traffic
```

Narrow by stable policy or rule ID when only one policy should control the
command. Do not listen to raw `vps.status`, job, or telemetry events; put that
logic in the Alert Policy so Trigger/Resolve conditions and meta conditions
remove flapping once for webhooks and schedules alike.

An omitted argv template is an explicit no-op, `["/bin/true"]`, which is useful
while validating the expression. A direct shell-free mitigation can render
immutable alert fields without changing argv boundaries:

```json
[
  "/usr/local/sbin/apply-traffic-limit",
  "--episode",
  "{alert.episode_id}",
  "--severity",
  "{alert.severity}"
]
```

The review step shows the exact saved JSON array, a server-rendered sample,
each indexed argv result, and both hashes. `argv[0]` is always literal;
rendered values are never split into more arguments. Event schedules retain
their separately reviewed fixed VPS targets—the alert subject filters the edge
but never silently retargets the job. A durable receipt creates at most one job
per matching schedule and edge. Deferred-window edges are skipped, and a
matching recovery job waits for that same schedule's mitigation job to finish.

Direct schedule/job/backup/capability ancestry prevents known automation
cycles. A shell command's indirect effect on later agent or telemetry state
cannot currently carry that causation token, so make such commands idempotent
and filter them to a specific policy or rule.

Route alert notifications through scoped channel presets:

```sh
cargo run -p vpsctl -- fleet-alert-notification-channel-upsert \
  --name edge-webhook \
  --scope-kind tag \
  --scope-value edge \
  --min-severity warning \
  --categories agent_status,network,traffic \
  --operator-states open,escalated \
  --delivery-kind webhook \
  --target https://hooks.example/vpsman \
  --cooldown-secs 3600 \
  --confirmed
cargo run -p vpsctl -- fleet-alert-notification-dispatch --dry-run --include-muted
cargo run -p vpsctl -- fleet-alert-notification-dispatch --confirmed --include-muted
cargo run -p vpsctl -- fleet-alert-notification-process --status queued --delivery-kind webhook --dry-run
cargo run -p vpsctl -- fleet-alert-notification-process --status queued --delivery-kind webhook --confirmed
cargo run -p vpsctl -- fleet-alert-notifications --status failed
```

Create additional webhook channels when different alert scopes need different
receivers:

```sh
cargo run -p vpsctl -- fleet-alert-notification-channel-upsert \
  --name core-webhook \
  --scope-kind tag \
  --scope-value core \
  --min-severity warning \
  --categories agent_status,network \
  --operator-states open,escalated \
  --delivery-kind webhook \
  --target https://hooks.example/vpsman/core \
  --cooldown-secs 3600 \
  --confirmed
cargo run -p vpsctl -- fleet-alert-notification-dispatch --dry-run --include-muted
cargo run -p vpsctl -- fleet-alert-notification-dispatch --confirmed --include-muted
cargo run -p vpsctl -- fleet-alert-notification-process --status failed --delivery-kind webhook --dry-run
cargo run -p vpsctl -- fleet-alert-notification-process --status failed --delivery-kind webhook --confirmed
cargo run -p vpsctl -- fleet-alert-notifications --status failed
```

For unattended processing, run the worker in one-shot mode during validation or
as the normal background service in production:

```sh
VPSMAN_POSTGRES_URL=postgres://vpsman:vpsman@127.0.0.1:5432/vpsman \
  target/debug/vpsman-worker --once \
  --notification-delivery-limit 25 \
  --notification-retention-days 90 \
  --notification-retention-prune-limit 1000 \
  --notification-webhook-timeout-secs 5
```

The worker uses the same queued webhook outbox. Notification targets must use
HTTPS, except localhost HTTP for lab receivers. Failed rows keep attempt counts,
error details, and the next retry timestamp until they are delivered or become
permanently failed. Normal audit records remain the durable evidence trail for
channel changes, dispatches, and processing.

The panel uses CRUD tables for notification channels, expression webhook rules,
and delivery histories so operators can search, select, edit, delete, dispatch,
dry-run, and rotate retained records from one dense workflow.

In the panel, use the left navigation for fleet, tags, jobs, topology, backups,
and updates. The UI is meant for repeated operations: filter first, inspect
exact targets, then dispatch.

## Operator Rules

- Treat tags as operational intent: `edge`, `bgp`, `lab`, `backup-critical`.
- Treat namespaced tags as infrastructure ownership: `provider:provider-a`,
  `country:US`, `region:sfo`, `pool:legacy`, or reseller/account labels.
  Country and region are independent tags: country drives compact fleet-card
  location, while region adds finer placement in Fleet details and tooltips.
- Do not dispatch destructive work from a fuzzy mental target set. Resolve and
  inspect first.
- Keep unprivileged targets visible. Degraded operations are useful signals,
  not errors to hide.
