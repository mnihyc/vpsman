# Target Selectors

`vpsman` target selectors are expression strings used by bulk jobs, schedules,
tag mutation, data-source assignment, previews, and expression webhook rules.
The Rust parser/evaluator lives in `vpsman-common`; the frontend parser mirrors
the same grammar for local previews and token tooltips.

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

Literal matching is case-insensitive. Bare text still searches VPS id and
display name by contains for operator convenience.

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

`client:<id>` is not an operator selector. Internal audit and signed command
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
  `alert.state:<state>`, `alert.open`.
- Telemetry: `telemetry.rollup`, `telemetry.network_rate`, `telemetry.tunnel`,
  plus `telemetry.<path>` comparisons.

The current worker materializes interval events for expression webhooks. Other
event predicates are parsed and evaluable for API dry-runs and future producers.

## Expression Webhooks

Expression webhook rules are separate from alert notification channels. A rule
has `name`, `enabled`, `expression`, `target`, `body_template`,
`cooldown_secs`, and `notes`.

Delivery is one aggregated webhook call per rule/event occurrence. The JSON
body includes rule metadata, event metadata, `matched_vps`, and rendered
`message`.

Template placeholders include `{vps.name}`, `{vps.display_name}`, `{vps.id}`,
`{vps.status}`, `{vps.tags}`, `{event.kind}`, `{event.id}`, `{rule.id}`, and
`{rule.name}`. When multiple VPSs match, values are joined with spaces.

Webhook targets must be HTTPS, except HTTP localhost targets are allowed for
local testing. Embedded credentials and redirects are rejected.

Examples:

```text
interval.30sec && status = stale
interval.1min && provider:alpha && vps.tag not in [/^test-.*/]
alert.open && alert.severity:critical && tag:edge
job.status.become_failed && job.type:shell && job.target.status:online
```
