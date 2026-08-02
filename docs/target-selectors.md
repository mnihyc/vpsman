# Target Selectors

`vpsman` target selectors are expression strings used for target previews, bulk
resolution, tag mutation, configuration-preset assignment, Ping-target
assignment, monitoring-share scope, and expression webhook rules. Jobs,
schedules, Ping definitions, and monitoring shares store the concrete VPS IDs
resolved during API/CLI preview or the browser Review step. A saved selector is
audit and maintenance context, not live authority. The Rust parser/evaluator
lives in `vpsman-common`; the frontend parser mirrors the same grammar for local
previews and token tooltips.

## Fixed Target Workflow

Selectors are operator input and audit context. They are not a live binding for
job or schedule execution, configuration-preset assignment, Ping probing, or
public sharing. The browser Review step and CLI/API preview resolve the selector
to concrete VPS IDs. The confirmation and mutation use that fixed
`target_client_ids` list. Job `selector_expression` values may also be free-form
audit text; job creation validates only transport safety, while any record that
supports a later **Update targets** action keeps valid selector syntax.

Schedules follow the same rule. A schedule stores both the audit selector and a
fixed target snapshot. Due runs use the saved snapshot. If a saved target is
later hidden, deleted, revoked, never connected, missing from inventory, or
otherwise unavailable, that fixed ID is recorded as a skipped target result for
the run and the remaining available targets still materialize. If tags, display
names, or other selector inputs later drift, the operator can choose
**Update targets** from the schedule table Actions menu or row context menu.
The action supports one or many selected schedules. It resolves each saved
audit selector on the backend, includes only changed non-empty snapshots in the
privilege confirmation, and then saves those replacement snapshots.

This keeps human review as the authority for broad scope: changing tags never
silently changes a saved job, schedule, Ping assignment, or monitoring share.

Ping targets use the same fixed-list rule. Create resolves the expression and
stores the exact assignments. A normal edit preserves that frozen list while
the selector expression is unchanged; changing the expression resolves its new
matches for the edit review. If a saved expression later resolves differently
because fleet metadata changed, **Update targets** is the deliberate table
action for one or many selected Ping definitions. It previews exact
additions/removals and replaces assignments transactionally.

Monitoring shares resolve and freeze their exact VPS list at creation. Their
target and visibility scope are immutable evidence, so they intentionally have
no **Update targets** action. Create a replacement share when either scope must
change.

In the console, selector matches are previewed locally as the expression is
edited. That preview is for orientation only. Direct VPS choices and selector
matches contribute to one deduplicated union, and the nearby count/list must
show that complete local union. **Review** asks the backend to resolve the
request again and freezes the exact IDs used by the confirmation and mutation.
An invalid expression or empty union cannot be reviewed.

## Fixed Target Snapshots

Job submission, schedule creation, Ping-target assignment, and monitoring-share
creation are fixed-target workflows. Operators review a selector, then the API
receives the resolved `target_client_ids` alongside the audit selector. A later
tag or alias change never silently changes the VPSs affected by the saved
record.

For schedules, **Update targets** is a deliberate table action for records
whose saved audit selector currently resolves to a different non-empty VPS set.
The action asks the backend to resolve each selected record's selector, rejects
no-op records whose fixed target list already matches, and replaces only the
saved target snapshots after privilege confirmation. CLI
schedule creation follows the same rule: the previewed target set is the saved
target set.

## Grammar

Selectors support parentheses, unary NOT with `~` or `!`, explicit `&&`/`and`,
implicit AND, and explicit `||`/`or`.

Precedence is:

1. Parentheses
2. NOT
3. AND, including implicit AND
4. OR

Examples:

```text
*
status = "stale"
status in [stale]
vps.status = stale && tag:edge
(provider:alpha && country:US) || id:edge-01
interval.30sec && tag:edge && !(status = offline)
```

## Predicates

Comparisons:

```text
status = "stale"
vps.status != offline
last_seen < 2026-06-08T00:00:00Z
vps.internal_build_number > 10
```

Membership:

```text
status in [stale]
vps.tag in [edge, prod]
vps.tag not in [/^test-.*/]
```

Values may be quoted when they contain spaces or commas. List values are
comma-separated; quoted list values preserve commas, for example
`["abc, def"]`. Regex list values use slash delimiters and are case-sensitive.
Regex flags are not supported.

Literal matching is case-insensitive. Visible VPS display names are unique
case-insensitively, while hidden/deleted VPS records do not reserve names. Bare
text still searches VPS id and display name by contains for operator
convenience.
The bare wildcard `*` is supported as the concise all-VPS selector, equivalent
in practice to `id:*` for target selection.

Datetime ordering accepts RFC3339 timestamps and Unix seconds.

## Aliases

Canonical VPS fields use `vps.<path>`.

- `status:online`, `status = online`, and `vps.status = online` are equivalent.
- `tag:edge`, `vps.tag in [edge]`, and `vps.tags in [edge]` are equivalent.
- `provider:alpha` matches the tag `provider:alpha`.
- `country:US` and `region:US` match the tag `country:US`.
- Unknown namespaced shorthand like `role:edge` matches the exact tag
  `role:edge`; use `vps.role = edge` for future serialized VPS JSON fields.
- `untagged` is true only when VPS metadata exists and the tag list is empty.
- `last_seen` aliases `vps.last_seen_at`.
- `*` selects all VPSs; `id:*` remains the explicit ID-field spelling.

`client:<id>` is not an operator selector. Internal audit and command
records may still render concrete resolved targets as `client:<id>`.

## Event Contexts

Webhook rules evaluate expressions against an event context. A context may
contain server, VPS, job, schedule, alert, and telemetry metadata. Missing
metadata evaluates false for direct predicates, including `field = value`,
`field in [...]`, and `field not in [...]`; boolean NOT can invert that result.

Supported event predicate names include:

- Timing: `interval.30sec`, `interval.1min`, `interval.5min`, `interval.1h`.
- Server: `server.on_start`.
- VPS: `vps.status.<state>`, `vps.status.become_<state>`, `vps.tag:<tag>`,
  plus `vps.<path>` comparisons.
- Job: `job.created`, `job.status:<status>`, `job.status.become_<status>`,
  `job.type:<type>`, `job.target.status:<status>`.
- Schedule: `schedule.due`, `schedule.dispatched`, `schedule.failed`,
  `schedule.id:<id>`, `schedule.name:<name>`.
- Alert: `alert.severity:<level>`, `alert.category:<category>`,
  `alert.state:<state>`, `alert.open`, `alert.policy_reached`.
- Policy alert payloads: `policy.<path>`, `rule.<path>` (also
  `policy_rule.<path>`), and `traffic.<path>` comparisons for issued
  `alert.policy_reached` events.
- Telemetry: `telemetry.rollup`, `telemetry.network_rate`, `telemetry.tunnel`,
  plus `telemetry.<path>` comparisons.

The worker materializes interval events for expression webhooks, and the API
policy evaluator emits `alert.policy_reached` events. Other event predicates are
parsed and evaluable for API dry-runs and future producers.

## Expression Webhooks

Expression webhook rules are separate from alert notification channels. A rule
has `name`, `enabled`, `expression`, `target`, `body_template`,
`cooldown_secs`, and `notes`.

Delivery is one aggregated webhook call per rule/event occurrence. The JSON
body includes webhook-rule metadata as `rule`, event metadata, `matched_vps`,
and rendered `message`. Policy-alert deliveries additionally include `policy`,
`traffic`, and their source alert rule as `policy_rule`; this avoids replacing
the webhook rule used by existing `{rule.*}` templates.

A confirmed manual dispatch queues only the candidate set represented by its
review hash and limit. It does not log a broad event for the worker to
re-evaluate against a later rule or fleet state. Pre-upgrade broad manual
events without a durable reviewed candidate set are skipped fail-closed and
recorded as `webhook.legacy_manual_dispatch_events_skipped` audit entries.

Template placeholders include `{vps.name}`, `{vps.display_name}`, `{vps.id}`,
`{vps.status}`, `{vps.tags}`, `{event.kind}`, `{event.id}`, `{rule.id}`, and
`{rule.name}`. When multiple VPSs match, values are joined with spaces.

Webhook targets use HTTPS by default. At delivery time, every DNS answer must
be a public unicast address; private, loopback, link-local, multicast,
unspecified, documentation, and reserved addresses are rejected. The validated
answers are pinned for the request, redirects and proxies are disabled, and
embedded URL credentials are rejected.

Local development can explicitly set
`VPSMAN_DEV_ALLOW_LOOPBACK_WEBHOOKS=1` in both the API and worker environments
to permit only HTTP targets on `localhost`, `127.0.0.0/8`, or `::1`. This switch
does not permit other private-network targets or HTTPS loopback targets and
must remain unset in production.

Examples:

```text
interval.30sec && status = stale
interval.1min && provider:alpha && vps.tag not in [/^test-.*/]
alert.open && alert.severity:critical && tag:edge
alert.policy_reached && alert.category:traffic && traffic.cycle_percent >= 80
job.status.become_failed && job.type:shell && job.target.status:online
```

## Alert Policy Rule Expressions

Observability > Alerts uses selector expressions only to choose target VPSs.
Each rule then evaluates a full boolean condition expression against that VPS.
Condition expressions support numeric literals, variables, comparisons,
`+`, `-`, `*`, `/`, parentheses, unary signs, `&&`, `||`, and `!` through the
backend stack/RPN parser.

Examples:

```text
traffic.cycle.total >= traffic.quota.total * 0.8
traffic.cycle.tx >= (traffic.quota.tx + 10GB) / 2
cpu.load_1 >= 0.25 + 0.75
(traffic.cycle_percent >= 80 && memory.available_ratio <= 0.2) || cpu.load_1 > 4
```

Missing variables, missing traffic selectors, unknown runtime interfaces, and
missing quota/reset-day values make the rule state incomplete. They do not
evaluate as zero and do not fire alerts.
