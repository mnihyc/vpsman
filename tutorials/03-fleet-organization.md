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

For provider, tag, or client-specific thresholds, save a scoped policy:

```sh
cargo run -p vpsctl -- fleet-alert-policy-upsert \
  --name edge-resource-alerts \
  --scope-kind tag \
  --scope-value edge \
  --memory-available-warning-ratio 0.35 \
  --memory-available-critical-ratio 0.15 \
  --cpu-load-warning 1.5 \
  --cpu-load-critical 3.0 \
  --priority 25 \
  --confirmed
```

List policy records:

```sh
cargo run -p vpsctl -- fleet-alert-policies --scope-kind tag --scope-value edge
```

Scoped policies cascade from global to provider, tag, and client matches.
Higher-priority matching records override earlier values within their scope. In
the panel these are managed through the Alert policy CRUD table, so daily edits
are searchable, paginated, selectable, and reversible through explicit row
actions rather than scattered cards.

Route alert notifications through scoped channel presets:

```sh
cargo run -p vpsctl -- fleet-alert-notification-channel-upsert \
  --name edge-webhook \
  --scope-kind tag \
  --scope-value edge \
  --min-severity warning \
  --categories agent_status,network \
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
