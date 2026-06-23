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

Use `id:<client_id>` or explicit client ids for destructive one-off work, and
tags for operational groups, provider labels, countries, regions, or optional
`pool:<name>` labels.

## Daily Fleet Views

```sh
cargo run -p vpsctl -- summary
cargo run -p vpsctl -- agents
cargo run -p vpsctl -- fleet-alerts
cargo run -p vpsctl -- telemetry-rollups
cargo run -p vpsctl -- gateway-sessions
```

## Tune Fleet Alert Policy

Resource alerts use a startup policy instead of hardcoded thresholds. Set these
on the API process when the default operating tolerance is too noisy or too
late for your fleet:

```sh
export VPSMAN_ALERT_MEMORY_AVAILABLE_WARNING_RATIO=0.20
export VPSMAN_ALERT_MEMORY_AVAILABLE_CRITICAL_RATIO=0.10
export VPSMAN_ALERT_DISK_AVAILABLE_WARNING_RATIO=0.20
export VPSMAN_ALERT_DISK_AVAILABLE_CRITICAL_RATIO=0.10
export VPSMAN_ALERT_CPU_LOAD_WARNING=2.0
export VPSMAN_ALERT_CPU_LOAD_CRITICAL=4.0
```

Inspect filtered alerts from CLI or VTY:

```sh
cargo run -p vpsctl -- fleet-alerts --severity critical
cargo run -p vpsctl -- fleet-alerts --client-id edge-01 --limit 20
```

The evidence field includes the threshold that fired. Use this to adjust the
policy deliberately instead of suppressing useful warnings. In the panel, active
VPS alerts are shown in a dense Fleet alerts table with search, pagination,
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

For per-VPS traffic accounting, save VPS Rules first. These are low-level
server-side values keyed by VPS and rule key; the alert policy editor reads
them but does not modify them.

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

Then create a policy group. The selector chooses target VPSs using the same
selector expressions as dispatch previews (`tag:edge`, `provider:hetzner`,
`id:<client_id>`, boolean operators, and parentheses). Rule rows are full
condition expressions: comparisons, arithmetic, boolean operators, and
parentheses are evaluated by the backend expression parser from current VPS
rule/accounting values rather than treated as plain strings.

```sh
cargo run -p vpsctl -- alert-policy preview \
  --name edge-traffic \
  --selector 'tag:edge' \
  --rule 'traffic.cycle.total >= traffic.quota.total * 0.8' \
  --severity warning

cargo run -p vpsctl -- alert-policy upsert \
  --name edge-traffic \
  --selector 'tag:edge' \
  --rule 'traffic.cycle.total >= traffic.quota.total * 0.8' \
  --severity warning \
  --confirmed

cargo run -p vpsctl -- alert-policies list --selector 'tag:edge'
```

In the UI, Fleet > Instances keeps traffic columns hidden by default; enable
them through Fields when you need operational status in the main table. Expand a
VPS and open Traffic & Rules for counters, current cycle usage, incomplete
reasons, matched policies, and recent issued alerts. Use Config > VPS Rules for
bulk dry-run, preview-hash confirmation, and explicit unset actions. Use Fleet
> Alert Policies for policy-group editing, selector dry-runs, and rule previews.
Issued policy alerts appear in Fleet > Alerts and are delivered by the existing
notification/webhook channels as `alert.policy_reached` events with `alert`,
`vps`, `policy`, `rule`, and `traffic` payload roots.

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
- Do not dispatch destructive work from a fuzzy mental target set. Resolve and
  inspect first.
- Keep unprivileged targets visible. Degraded operations are useful signals,
  not errors to hide.
