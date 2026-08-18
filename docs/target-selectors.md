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
audit selector on the backend, includes every changed exact snapshot in the
privilege confirmation (including a reviewed empty result), and then saves
those replacement snapshots.

This keeps human review as the authority for broad scope: changing tags never
silently changes a saved job, schedule, Ping assignment, or monitoring share.

Ping targets use the same fixed-list rule. Create resolves the expression and
stores the exact assignments. A normal edit preserves that frozen list while
the selector expression is unchanged; changing the expression resolves its new
matches for the edit review. If a saved expression later resolves differently
because fleet metadata changed, **Update targets** is the deliberate table
action for one or many selected Ping definitions. It previews exact
additions/removals and replaces assignments transactionally.

**System > Maintenance > Stale selectors** is the consolidated repair surface
for mutable saved snapshots. It compares every loaded schedule (including
backup-policy schedules), Ping definition, and active monitoring share with the
current visible-fleet resolution. Operators can review **Update targets** for
selected rows or **Update all** resolvable rows. Invalid selector text or an
invalid saved schedule operation remains **Repair required** and is never
silently skipped into a write. Schedule updates keep their per-schedule
privilege/audit boundary; Ping assignments and monitoring shares each use their
own transactional preview hash. An exact empty resolution is explicit and may
be frozen after review.

Monitoring shares resolve and freeze their exact VPS list at creation. Their
saved selector remains audit context; **Update targets** deliberately
re-resolves it for one or many active shares, previews exact additions and
removals, and transactionally replaces only the frozen target list. Existing
public target keys are preserved. Visibility remains immutable evidence.
Approval records are likewise immutable evidence.

In the console, selector matches are previewed locally as the expression is
edited. That preview is for orientation only. Direct VPS choices and selector
matches contribute to one deduplicated union, and the nearby count/list must
show that complete local union. **Review** asks the backend to resolve the
request again and freezes the exact IDs used by the confirmation and mutation.
An invalid expression cannot be reviewed. Creation workflows that require at
least one target also reject an empty union; explicit **Update targets**
maintenance may freeze an exact empty resolution after review.

## Fixed Target Snapshots

Job submission, schedule creation, Ping-target assignment, and monitoring-share
creation are fixed-target workflows. Operators review a selector, then the API
receives the resolved `target_client_ids` alongside the audit selector. A later
tag or alias change never silently changes the VPSs affected by the saved
record.

For schedules, **Update targets** is a deliberate table action for records
whose saved audit selector currently resolves to a different VPS set, including
an exact empty set.
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
- `country:US` matches the tag `country:US`; `region:IAD` independently
  matches `region:IAD`. Country and region never alias one another.
- Unknown namespaced shorthand like `role:edge` matches the exact tag
  `role:edge`; use `vps.role = edge` for future serialized VPS JSON fields.
- `untagged` is true only when VPS metadata exists and the tag list is empty.
- `last_seen` aliases `vps.last_seen_at`.
- `*` selects all VPSs; `id:*` remains the explicit ID-field spelling.

`client:<id>` is not an operator selector. Internal audit and command
records may still render concrete resolved targets as `client:<id>`.

`country:` is the fleet-card location signal. `region:` is optional finer
placement metadata. The personal **Country and Fleet location** preference
keeps the Fleet table's original single-line country view by default and can
place region directly below it; full identity tooltips and details retain both.
Cards reserve region for details and full identity tooltips. Structured country
and region values are not repeated as ordinary tag pills in those surfaces.

## VPS Rules

Operators with `config:read` can select VPSs by their directly configured VPS
rules through the single `vps.rules` field. Defaults and referenced effective
behavior are not synthesized: a presence expression means that the operator
actually stored that rule on the VPS.

Autocomplete keeps this scope compact: the ordinary selector menu shows one
**VPS rules…** entry, and reveals rule names, examples, and observed canonical
values only after `vps.rules:` is entered or selected.

```text
vps.rules:*                                      # any configured rule
vps.rules:traffic.reset_day                      # this rule is configured
!vps.rules:traffic.reset_day                     # this rule is absent
vps.rules:traffic.*                              # configured traffic rule
vps.rules in [/^traffic\.quota\./]               # key regex

vps.rules:traffic.reset_day >= 15
vps.rules:traffic.quota.total >= 1TB
vps.rules:network.port_speed >= 1Gbps
vps.rules:billing.price < "50 USD/m"
vps.rules:billing.price = "29.90 CNY/m"
vps.rules:network.port_speed = "*Gbps"
vps.rules:product.name = "Storage-Box 4"
vps.rules:product.name in [/^LN\./]
```

Exact equality normalizes its input with the same rule parser used by the VPS
Rules editor, so cosmetic whitespace does not change a match. Globs and
regexes intentionally match the canonical stored text. Ordered comparisons
interpret reset days as days, quotas as bytes, and port speeds as
bits per second, accepting the same units as the VPS Rules editor. Billing
prices are ordered only against a value with the same currency and billing
period; different units do not match. Billing cycles and interface-selector
rules do not have a meaningful order and reject `<`, `<=`, `>`, and `>=`.
Product names likewise support equality, globs, and regexes but have no ordered
comparison.
The `-1` continuous, unlimited, or disabled sentinels can be matched exactly
but are not ordered.

New and edited rules are canonicalized before preview and persistence. A
spacing-only edit therefore produces the same `value_raw`, parsed value, and
dropdown entry instead of creating a distinct configuration value.
Two-component billing renewal anchors use canonical `MM-DD`, for example
`billing.cycle=06-15`; `M-D` shorthand such as `6-15` is accepted and normalized
to the standard form before preview and persistence. Storage, search, API, and
display all use `MM-DD`. Monthly billing keeps a day-only recurring anchor such
as `billing.cycle=15`. `product.name` is optional free-form display text;
leading, trailing, and repeated whitespace is canonicalized, while case,
punctuation, and Unicode text are preserved. Its canonical UTF-8 value is
limited to 160 bytes.

A missing rule makes every direct value predicate false, including `!=`; use
`!vps.rules:<key>` to select absence. Rule-aware live resolution requires
`config:read` and fails explicitly if complete rule evidence is unavailable.
Reviewed jobs, schedules, Ping assignments, configuration assignments, and
monitoring shares still freeze exact VPS IDs. Changing a rule does not silently
retarget them; an explicit **Update targets** action re-evaluates the saved
expression. Dynamic alert and webhook expressions evaluate the current
committed rule values.

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
  generic `alert.triggered` and `alert.resolved`, compatibility `alert.open`,
  and policy aliases
  `alert.policy_triggered`, `alert.policy_reached`, and
  `alert.policy_resolved`.
- Policy alert payloads: `policy.<path>`, `rule.<path>` (also
  `policy_rule.<path>`), and `traffic.<path>` comparisons for triggered and
  resolved policy-alert events.
- Telemetry: `telemetry.rollup`, `telemetry.network_rate`, `telemetry.tunnel`,
  plus `telemetry.<path>` comparisons.

The worker materializes interval events for expression webhooks. Alert
lifecycle edges have the following contract:

| Alert producer | Trigger edge | Resolve edge |
| --- | --- | --- |
| Operational condition or occurrence | Event kind and predicate `alert.triggered`; compatibility predicate `alert.open`; event ID `fleet-alert:<episode UUID>:triggered` | Event kind and predicate `alert.resolved`; event ID `fleet-alert:<episode UUID>:resolved` |
| Policy condition | Compatibility event kind `alert.policy_reached`; predicates `alert.policy_triggered`, `alert.policy_reached`, and generic `alert.triggered` | Event kind and predicate `alert.policy_resolved`, plus generic `alert.resolved` |

Persisting and Unknown are state updates, not lifecycle edges, and emit no
webhook. Confirmed backfilled pre-lifecycle episodes start as Persisting
generation 1. Explicitly degraded legacy tunnel evidence whose current
runtime or client/session attribution cannot be proven is instead retained as
Unknown generation 1. Neither backfill synthesizes a Triggered webhook; an
explicit or evidence-driven resolution can therefore be its first delivered
edge.

The top-level webhook `alert` object identifies the episode and carries
`id`, `record_kind`, `category`, `severity`, `source_status`,
`lifecycle_state`, `trigger_generation`, `triggered_at`,
`last_confirmed_at`, `resolved_at`, `resolution_reason`, and
`resolution_note`, plus `resolution_actor_id` for an operator-resolved
occurrence. Triggered and Resolved are durable edges, so webhook-rule
cooldowns do not suppress them; the existing per-rule/event identity still
deduplicates each edge when it is queued. HTTP transport is retryable and
therefore at least once. A receiver must deduplicate by `event.id` and merge
using the episode UUID, generation, lifecycle state, and lifecycle timestamps;
it must not assume delivery order. Within one episode and generation, Resolved
is terminal even if a retried Triggered delivery arrives later.

For operational episodes, lifecycle-edge predicates and `alert.severity` /
`alert.category` retain the values from the Triggered edge for the entire
generation, so a severity-scoped receiver can receive the matching Resolved
edge even if the condition presentation changes. `alert.current_severity` and
`alert.current_category` expose the latest presentation values separately.

Webhook VPS, tag, status, and VPS-rule predicates are evaluated against the
current committed VPS state when the worker materializes a delivery. A
lifecycle integration that requires both edges should therefore scope on the
stable `policy.id`, `policy_rule.id`, or alert category rather than on a mutable
tag or status that may itself be the reason the policy resolved.
When an explicitly referenced VPS has already been hidden or deleted, its
durable lifecycle edge can still match event, alert, policy, policy-rule, and
stable `vps.id` predicates and carries retained identity in `matched_vps`.
Mutable VPS name, tag, status, VPS-rule, and untagged predicates fail closed for
that retained subject; subjectless interval events never enumerate tombstones.

### Alert lifecycle source matrix

`record_kind=condition` describes evidence that can recover or leave scope.
`record_kind=event` describes a terminal occurrence that remains current until
an operator explicitly resolves it. Triggered is the first edge, Persisting is
confirmation of the same generation, Unknown means current condition evidence
is unavailable or incomplete, and Resolved is the terminal edge for that
generation.

| Displayed category / source | Kind | Trigger and confirmation | Unknown | Resolution |
| --- | --- | --- | --- | --- |
| `agent_status` / connectivity | condition | `clients.status` equal to `never`, `disconnected`, `offline`, or `stale`; any repeated non-online connectivity status confirms the same episode even if displayed status or severity changes | No incomplete domain value | `online` gives `condition_recovered`; hidden or deleted source gives `source_scope_exited`; recurrence starts generation +1 |
| `agent_status` / access | condition | `clients.status=revoked`; repeated revoked evidence confirms the same access episode and is independent of connectivity | No incomplete domain value | restored access gives `condition_recovered`; hidden or deleted source gives `source_scope_exited`; recurrence starts generation +1 |
| `network` / tunnel adapter | condition | Only the enabled current custom-adapter endpoint is in scope; explicit `adapter_health.success=false` triggers or confirms and displays source status `tunnel_adapter_degraded` | Missing endpoint or health evidence | Explicit `success=true` gives `condition_recovered`; plan/client scope exit or ceasing to use the custom manager gives `source_scope_exited` |
| `network` / tunnel traffic | condition | Explicit non-`ok` `traffic_status` triggers or confirms and displays source status `tunnel_traffic_degraded` | Missing endpoint or traffic status | Exact `ok` gives `condition_recovered`; plan/client scope exit gives `source_scope_exited` |
| `resource` or `traffic` / alert policy | condition | An enabled policy rule becomes Triggered after its configured window and Persisting on subsequent true evaluations | Incomplete rule evidence after a confirmation | Valid false evidence gives `condition_recovered`; scope/edit transitions give `policy_scope_exited`, `policy_scope_changed`, `policy_disabled`, `policy_changed`, or `policy_deleted` |
| `backup`, `agent_update`, or `job` / terminal job | event | Exact terminal status `partial_success`, `canceled`, `rejected`, `failed`, `agent_timeout`, or `control_timeout` triggers once; retained source evidence confirms Persisting | Never | Only explicit incident resolution gives `operator_resolved` |
| `backup` / backup request | event | Exact status `execution_failed` triggers once; retained source evidence confirms Persisting | Never | Only explicit incident resolution gives `operator_resolved` |
| `capability_degraded` / capability job target | event | Exact target status `skipped` with reason and hint triggers once; the persisted reason remains the source status and retained evidence confirms Persisting | Never | Only explicit incident resolution gives `operator_resolved` |

Tunnel condition evidence is valid only when it retains the established
topology identity, matches the current runtime-affecting plan and credential
identity, and was accepted after both the client/session and plan-runtime
boundaries. A one-time migration backfill may preserve explicitly marked
legacy degradation as Unknown, with its prior identity and confirmation time,
without asserting that the degradation is still active or emitting a
Triggered edge. Healthy evidence with unprovable attribution creates no
incident. Subsequent evaluations require fresh matching telemetry accepted
after both boundaries.

Condition recovery is evidence-driven; missing evidence never acts as an
implicit recovery. Event resolution uses
`POST /api/v1/fleet-alerts/<alert-id>/resolve` with an explicit confirmation
and a required nonblank reason. It is valid only for a current Triggered or
Persisting event. Conditions, Unknown rows, malformed records, and unknown or
non-event IDs fail closed. Repeating Resolve for an already Resolved event ID
is idempotent: it returns the existing terminal episode and emits no second
Resolved edge or resolution audit record.

Operator triage is an orthogonal state machine:
`open`, `acknowledged`, `muted`, or `escalated`. The existing triage `clear`
API action means **Reset triage to Open**; it never resolves an episode,
increments a generation, or emits a lifecycle edge. Conversely, resolving an
event does not reuse or silently change its triage state.

The bounded current snapshot is not the only resolution surface. Operators can
page unresolved occurrences explicitly through
`GET /api/v1/fleet-alert-events?limit=<1..200>&cursor=<opaque>`. The response is
`{items,next_cursor,has_more}` and contains only unresolved `record_kind=event`
records, ordered by Triggered time and episode UUID descending. The cursor is
exclusive; filters are applied before pagination. The console loads this feed
only on request, deduplicates by public alert ID, exposes source failure versus
the terminal cursor, and offers a manual refresh from page one after reaching
the end. Mutation controls require Operator or Admin role with `fleet:read`,
`backups:read`, and `integrations:write` (or `*`).

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
Template comments put `{#` and `#}` on their own lines around a block. Comment
contents are not parsed or rendered, so one block can hold complete alternative
templates without leaving blank lines in the delivered message. Copy an example
outside the block to activate it. For example:

```text
{#
Alert: [{alert.severity}] {alert.title} on {vps.display_name} ({event.id})
Traffic threshold: {vps.display_name} used {traffic.cycle_percent}% in {policy.name}; source rule {policy_rule.name}
Resource threshold: [{alert.severity}] {alert.title} on {vps.display_name}; condition {policy_rule.condition_expression}
VPS status event: [{event.kind}] {vps.display_name} is {vps.status}
Interval fleet summary: [{event.kind}] {matched_vps.length} VPSs: {matched_vps.map(vps.name).join(", ")}
#}
[{event.kind}] {rule.name}: {vps.display_name} ({vps.id}) is {vps.status}
```

The `alert` root is populated for every alert lifecycle edge. `policy`,
`policy_rule`, and `traffic` are additionally populated for policy-alert
deliveries. Other event types may omit roots that do not apply; missing values
render as empty text.

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
alert.triggered && alert.category:agent_status
alert.resolved && alert.category:backup
alert.policy_triggered && alert.category:traffic && traffic.cycle_percent >= 80
alert.policy_resolved && alert.category:traffic
job.status.become_failed && job.type:shell && job.target.status:online
```

## Alert Policy Rule Expressions

Observability > Alerts uses selector expressions only to choose target VPSs.
Each rule then evaluates a full boolean condition expression against that VPS.
Condition expressions support numeric literals, variables, comparisons,
`+`, `-`, `*`, `/`, parentheses, unary signs, `&&`, `||`, and `!` through the
backend stack/RPN parser.

Resource metric values come from the latest telemetry rollup:

- `cpu.utilization_ratio` is the maximum reported CPU busy-time ratio (`0` to
  `1`) in that rollup.
- `cpu.load_1` is the maximum raw one-minute system load in that rollup.
- `cpu.load_saturation` is that raw one-minute load divided by the reported
  logical CPU count. It can exceed `1` and is missing when the CPU count is not
  positive.
- `memory.available_ratio` and `disk.available_ratio` are the lowest available
  fractions in that rollup.

Traffic quota and cycle variables come from the VPS's current traffic
accounting cycle.

Examples:

```text
traffic.cycle.total >= traffic.quota.total * 0.8
traffic.cycle.tx >= (traffic.quota.tx + 10GB) / 2
cpu.utilization_ratio >= 0.75
(traffic.cycle_percent >= 80 && memory.available_ratio <= 0.2) || cpu.load_saturation > 1
```

Missing variables, missing traffic selectors, unknown runtime interfaces, and
missing quota/reset-day values make the rule state incomplete. They do not
evaluate as zero and do not fire alerts.
