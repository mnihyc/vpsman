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

Monitoring shares resolve and freeze their exact VPS list at creation. A
reviewed **Edit** can change one active share's name, selector, frozen IDs, and
visibility, with exact target and disclosure deltas. **Update targets** is the
narrower one-or-many action that re-resolves each saved selector and replaces
only its frozen target list. Both paths preserve unchanged public target keys;
the bearer identity and expiry are unchanged. Every approval and revision is
retained as immutable audit evidence.

In the console, selector matches are previewed locally as the expression is
edited. That preview is for orientation only. Direct VPS choices and selector
matches contribute to one deduplicated union, and the nearby count/list must
show that complete local union. **Review** asks the backend to resolve the
request again and freezes the exact IDs used by the confirmation and mutation.
An invalid expression cannot be reviewed. Creation workflows that require at
least one target also reject an empty union; reviewed **Edit** or **Update
targets** maintenance may freeze an exact empty resolution after review.

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

## Alert Policy and automation playbook

This is the authoritative authoring reference for Alert Policy conditions,
Trigger and Resolve meta conditions, Alert-event Schedules, direct argv, and
their webhook handoff. Start with the policy: raw status, telemetry, job, and
backup facts are evidence, while the policy-owned alert episode is the stable
automation event. Reuse or edit an applicable enabled default policy instead
of creating an overlapping rule unless two independently owned alerts are
intentional.

### Lifecycle model

Every displayed alert is created by an Alert Policy. Agent state, access,
tunnel health, telemetry, jobs, backups, and capability results are typed
**evidence**; those raw facts do not directly create a displayed alert or an
Alert-event Schedule job. Expression webhooks may still observe their supported
raw event contexts when an operator deliberately chooses that independent
condition. The policy-owned lifecycle is the anti-flap boundary:

```text
typed evidence
  -> Trigger condition
  -> Trigger meta condition
  -> alert.triggered
  -> Resolve condition
  -> Resolve meta condition
  -> alert.resolved
```

`record_kind=condition` is a recoverable state or metric. `record_kind=event`
is an immutable occurrence. Triggered is the first lifecycle edge, Persisting
means the same generation remains confirmed, Unknown means a condition lacks
enough authoritative evidence, and Resolved is terminal for that generation.
Persisting and Unknown never create automation edges.

### Rule types and evidence adapters

The rule type and evidence source are stored separately and validated as one
typed pair:

| Rule type  | Evidence source                                  | Typical Trigger condition            | Enabled default Trigger / Resolve meta    |
| ---------- | ------------------------------------------------ | ------------------------------------ | ----------------------------------------- |
| State      | `agent.status` / never connected                 | `evidence.status = never`            | Sustained 10m / Sustained 60s             |
| State      | `agent.status` / disconnected, stale, or offline | exact status match                   | Sustained 120s / Sustained 60s            |
| State      | `agent.access`                                   | `evidence.status = revoked`          | Immediate / Immediate                     |
| State      | `tunnel.adapter`                                 | `evidence.adapter.success = false`   | Sustained 120s / Sustained 60s            |
| State      | `tunnel.traffic`                                 | `evidence.traffic.status != ok`      | Sustained 120s / Sustained 60s            |
| Occurrence | `job.terminal`                                   | configured terminal failure outcomes | Immediate / elapsed 7d                    |
| Occurrence | `backup.failure`                                 | `evidence.status = execution_failed` | Immediate / elapsed 7d                    |
| Occurrence | `job.capability`                                 | `evidence.status = skipped`          | Immediate / elapsed 7d                    |
| Metric     | `telemetry.combined`                             | resource or traffic expression       | Starter rules are disabled until reviewed |

Operational defaults are ordinary visible policy groups. Their stable `*`
selector does not disappear when a subject becomes offline. Tunnel rules are
scoped by the enabled current endpoint plan. A subjectless terminal-job rule
also requires selector `*`.

The source fixes the rule type, available condition fields, and correlation
choices. These are the complete supported pairs:

| Evidence source      | Rule type  | Condition fields                                                                                                        | Correlation constraints                               |
| -------------------- | ---------- | ----------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| `telemetry.combined` | Metric     | Numeric variables listed below                                                                                          | `natural_key`                                         |
| `agent.status`       | State      | `evidence.status`                                                                                                       | `natural_key`                                         |
| `agent.access`       | State      | `evidence.status`                                                                                                       | `natural_key`                                         |
| `tunnel.adapter`     | State      | `evidence.adapter.success`, `evidence.interface`, `evidence.reason`                                                     | `natural_key`                                         |
| `tunnel.traffic`     | State      | `evidence.traffic.status`, `evidence.interface`, `evidence.reason`                                                      | `natural_key`                                         |
| `job.terminal`       | Occurrence | `evidence.status`, `evidence.command_type`, `evidence.job_id`, `evidence.target_count`                                  | `natural_key`, or `global` for Count; never `subject` |
| `backup.failure`     | Occurrence | `evidence.status`, `evidence.backup_request_id`, `evidence.client_id`                                                   | `natural_key`, or `subject`/`global` for Count        |
| `job.capability`     | Occurrence | `evidence.status`, `evidence.reason`, `evidence.hint`, `evidence.job_id`, `evidence.command_type`, `evidence.client_id` | `natural_key`, or `subject`/`global` for Count        |

An immediate occurrence uses `natural_key`, preserving one episode per source
fact. A counted occurrence must instead use `subject` or `global` correlation;
because terminal jobs have no authoritative subject, `job.terminal` Count is
global only. Any enabled group containing `job.terminal` must use the stable
all-subject selector `*`.

Severity is exactly `info`, `warning`, or `critical`. Category is exactly one
of `agent_status`, `network`, `backup`, `agent_update`, `job`,
`capability_degraded`, `traffic`, or `resource`.

### Complete condition syntax

State and occurrence conditions use the Boolean grammar from this document,
but every leaf must be a comparison or membership predicate against a field in
the source table above. Supported forms are:

```text
field = value                 field == value
field != value                field < value
field <= value                field > value
field >= value
field in [value, "value with spaces", "*glob*"]
field not in [value, /^case-sensitive-regex$/]

!(predicate)                  predicate && predicate
predicate and predicate       predicate predicate     # implicit AND
predicate || predicate        predicate or predicate
(predicate)
```

`!`, `~`, and `not` are equivalent unary NOT spellings. Literal matching is
case-insensitive; `*` and `?` are globs, and slash-delimited list members are
case-sensitive regular expressions. A missing, null, stale, incomplete, or
non-scalar required field is Unknown, including under negation. Bare search,
`untagged`, event anchors, VPS fields, and fields belonging to another evidence
source are rejected in a policy condition. Ordered comparisons are meaningful
for numeric fields such as `evidence.target_count`; use equality, membership,
glob, or regex matching for textual fields.

Metric conditions use a separate numeric grammar. Every Boolean leaf is a
comparison of numeric expressions; comparison operators are `=`, `==`, `!=`,
`<`, `<=`, `>`, and `>=`. Numeric expressions support `+`, `-`, `*`, `/`,
unary `+`/`-`, and parentheses. Boolean composition supports `!`/`~`/`not`,
`&&`/`and`, and `||`/`or` with the normal NOT, AND, OR precedence. Division by
zero and non-finite results are invalid.

The complete metric variable set is:

| Variable                 | Meaning and unit                                                                |
| ------------------------ | ------------------------------------------------------------------------------- |
| `traffic.quota.total`    | Configured total quota in bytes                                                 |
| `traffic.quota.rx`       | Configured RX quota in bytes                                                    |
| `traffic.quota.tx`       | Configured TX quota in bytes                                                    |
| `traffic.cycle.total`    | Accounted total bytes in the current cycle                                      |
| `traffic.cycle.rx`       | Accounted RX bytes in the current cycle                                         |
| `traffic.cycle.tx`       | Accounted TX bytes in the current cycle                                         |
| `traffic.cycle_percent`  | Highest configured quota utilization, from `0` to `100` and allowed above `100` |
| `cpu.utilization_ratio`  | CPU utilization ratio, where `1` is 100%                                        |
| `cpu.load_1`             | One-minute load average                                                         |
| `cpu.load_saturation`    | One-minute load divided by CPU cores, where `1` is one runnable task per core   |
| `memory.available_ratio` | Available-memory ratio, where `1` is 100% available                             |
| `disk.available_ratio`   | Available-disk ratio, where `1` is 100% available                               |

Literals may be finite decimal numbers; traffic arithmetic also accepts byte
size literals such as `500GB` or `2TiB`. A Traffic selector override is valid
only when the Trigger expression references a `traffic.*` variable. Blank uses
each VPS's `traffic.selectors`; an override uses the same comma-separated
interface grammar, for example `eth0`, `eth0+rx`, `eth0+tx`,
`eth0+tx/rx`, or `tunnel:tun0`.

Policy title and detail templates are required literal text plus strict scalar
placeholders. All rules may use `policy.id`, `policy.name`, `policy_rule.id`,
`policy_rule.name`, `policy_rule.rule_version`, `policy_rule.rule_kind`, and
`policy_rule.trigger_condition_expression`. A non-global, non-terminal-job
rule may also use `subject.client_id`, `subject.display_name`, and
`subject.status`. Prefix the selected source field or metric variable with
`evidence.` for presentation, for example `{evidence.status}` or
`{evidence.cpu.utilization_ratio}`. A globally correlated rule cannot use
`evidence.client_id`. Unknown paths and control syntax are rejected; the title
is limited to 256 bytes and detail to 4,096 bytes.

### Trigger and Resolve conditions

Every rule has a required **Trigger condition**. A condition rule has an
optional **Resolve condition**:

- blank Resolve condition means the Trigger expression must be conclusively
  false;
- a separate Resolve expression supplies hysteresis, such as Trigger above
  90% and Resolve below 75%;
- an occurrence has no Resolve expression because a historical fact cannot
  become false. It always has an elapsed Resolve meta condition and may be
  resolved earlier by an authorized operator.

Each phase has one meta-condition slot. Empty means **Immediate** wherever the
slot is optional; an occurrence's Resolve slot must be Elapsed after Triggered.

| Meta condition          | API shape                                                 | Range                                                  | Valid use                                           |
| ----------------------- | --------------------------------------------------------- | ------------------------------------------------------ | --------------------------------------------------- |
| Immediate               | empty / `null` (canonical)                                | no window                                              | Metric/State Trigger or Resolve; Occurrence Trigger |
| Sustained               | `{"kind":"sustained","seconds":300}`                      | 1 to 2,592,000 seconds                                 | Metric/State Trigger or Resolve                     |
| Count                   | `{"kind":"count","confirmations":3,"within_seconds":300}` | 1 to 1,000 confirmations within 1 to 2,592,000 seconds | Metric/State Trigger or Resolve; Occurrence Trigger |
| Elapsed after Triggered | `{"kind":"elapsed_since_trigger","seconds":604800}`       | 1 to 31,536,000 seconds                                | Occurrence Resolve only                             |

**Sustained** requires a true result for an accumulated duration. Unknown
pauses the current segment; a conclusive opposite result resets it. **Count**
requires distinct authoritative evidence revisions or distinct occurrences
inside its `within_seconds` window. Evaluator ticks never count. Opposite
conclusive Metric or State evidence resets that phase's confirmations; Unknown
contributes no sample while old Count samples continue aging out. **Elapsed
after Triggered** is evaluated from the durable Triggered time and
automatically resolves an occurrence when due, emitting a real
`alert.resolved` edge without a human.

Conditions also automatically resolve after their Resolve expression and
Resolve meta condition are satisfied; an empty Resolve expression uses
conclusive Trigger-false recovery. Thus every supported rule type has an
automatic resolution path. Scope exit, policy disable/change/delete, or
source-scope closure can also resolve the owned episode with explicit
provenance.

Counted occurrences correlate per subject or globally. Immediate occurrences
retain their unique natural key. A global terminal-job source must use global
correlation when Count is selected.

#### Evidence cadence and meta-condition timing

Meta-condition seconds are a dwell, Count window, or expiry duration. They are
**not a polling interval**. API-owned evidence is normally evaluated in the
database transaction that accepts it. Worker-owned evidence is durably
appended first and consumed by the API receipt repairer. The same API timer
also revisits persisted Sustained and Elapsed deadlines; it does not poll the
underlying source.

| Evidence or timer                         | When it normally advances                                                                                                                                                                                                                                                                                                                                                     |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| CPU, RAM, disk, and traffic-cycle metrics | On each accepted, nonduplicate agent telemetry sample. Agent `telemetry_interval_secs` defaults to 15s and accepts 5-3,600s. Configured Ping targets or enabled tunnel status/latency plans can shorten the effective telemetry tick to meet their own interval. Collection, transport, reconnects, and database work add jitter.                                             |
| `tunnel.adapter` and `tunnel.traffic`     | The agent checks configured runtime tunnel plans when `network.runtime_status_telemetry_interval_secs` is due: default 60s, allowed 15-3,600s. The check runs during telemetry collection; its cached result is carried by intervening telemetry and reconciled on each accepted sample. Probe duration and telemetry timing add jitter.                                      |
| `agent.status` and `agent.access`         | Connect, disconnect, access, and other authoritative status transactions reconcile evidence directly; telemetry may reconcile online state, but an unchanged status boundary deduplicates. Silent-online offline detection uses `worker.agent_offline_timeout_secs` (bundled/default 300s; suite range 1-86,400s), then the next eligible worker pass and API receipt repair. |
| Job, backup, and capability occurrences   | Event-driven when the terminal job result, failed backup, or capability skip is accepted; never sampled on a telemetry interval. API-owned paths evaluate in that transaction. Worker-owned scheduled-job facts are durably appended and reach rules on an API receipt-repair pass.                                                                                           |
| Persisted policy deadlines and repair     | The API `--policy-evaluation-interval-secs` / `VPSMAN_POLICY_EVALUATION_INTERVAL_SECS` timer defaults to 30s and is clamped to 5-3,600s. Each delayed tick repairs up to 500 missing receipts, then handles up to 200 due transitions, so contention or a backlog can delay an edge beyond the nominal next tick.                                                             |

The same offline timeout is available as
`--agent-offline-timeout-secs` / `VPSMAN_AGENT_OFFLINE_TIMEOUT_SECS`. The
worker checks it no more often than once per 60s of its own runtime and only
when its loop gets a turn. `worker.tick_secs` /
`--tick-secs` / `VPSMAN_WORKER_TICK_SECS` defaults to 30s; suite configuration
accepts 1-3,600s. With that bundled tick, an otherwise idle worker normally
checks about every 60s; a longer tick, ongoing work, or scheduling delay makes
the next check later. Consequently, silent "offline for 5m" means: first
accept the offline state after its connectivity timeout and worker-pass delay,
then consume it in receipt repair and apply the policy's 5m Trigger dwell. It
is not five minutes from the last packet with an exact wall-clock edge. An
explicit gateway disconnect is event-driven and does not wait for this silent
offline scan.

Sustained keeps durable accumulated true segments. A conclusive false resets
the current phase; Unknown pauses it and clears its running segment, so time
spent without authoritative evidence does not count. A State Sustained
deadline can mature on the next API evaluator pass using the last accepted
authoritative state, provided no newer evidence or scope revision is waiting.
A Metric deadline cannot turn one old sample into a confirmation: after the
dwell boundary, a **fresh accepted metric revision** must still make that phase
true. CPU, RAM, disk, and traffic alerts therefore cross or recover no earlier
than the configured dwell and normally on a subsequent telemetry sample.

Count also requires fresh evidence: one distinct accepted evidence ID is at
most one confirmation, and timer passes add none. Metric/State Count
confirmations reset on conclusive opposite evidence; Unknown neither counts nor
resets them, although retained samples continue aging. For an Occurrence Count,
nonmatching or Unknown occurrences do not erase matching occurrences from the
current cohort. Its `within_seconds` window uses each fact's database
`accepted_at` wall-clock time, not a producer-supplied historical timestamp;
the Nth accepted matching occurrence triggers immediately if the retained
window contains enough confirmations. No separate timer fires or prunes a
Count gate: its cutoff is applied on the next Count evaluation. Tunnel Count
counts accepted policy evidence revisions, which can carry a cached probe
result, so it must not be interpreted as "N adapter probes."

Elapsed Resolve stores `triggered_at + seconds` as the occurrence deadline and
needs no fresh source fact or operator. The next successful API evaluator pass
at or after that deadline emits Resolved. All cadence figures above are nominal
configuration intervals: worker polling, probe execution, delivery, database
contention, delayed-tick behavior, and bounded repair/due batches mean they are
not exact delivery-time guarantees.

Condition evaluation uses three-valued logic. For example, `true OR Unknown`
is true, `false OR Unknown` is Unknown, `false AND Unknown` is false, and
`true AND Unknown` is Unknown. Missing or stale facts never masquerade as zero,
false, or recovery.

Metric expressions support numeric literals, typed variables, comparisons,
`+`, `-`, `*`, `/`, parentheses, unary signs, `&&`, `||`, and `!`. Examples:

```text
traffic.cycle.total >= traffic.quota.total * 0.8
cpu.utilization_ratio >= 0.9
cpu.load_saturation > 1
memory.available_ratio <= 0.2 || disk.available_ratio <= 0.1
```

Use a lower independent Resolve condition for hysteresis:

```text
Trigger: cpu.utilization_ratio >= 0.9
Resolve: cpu.utilization_ratio < 0.75
Trigger meta: Sustained 5m
Resolve meta: Sustained 2m
```

Editing, enabling, disabling, or deleting a policy moves its evidence arm
boundary. State and metric rules may take the latest authoritative current fact
as their baseline, but confirmation dwell starts no earlier than the newly
persisted database arm. Immutable occurrence rules are strictly prospective:
older jobs, backups, and capability facts never replay. Policy/source scope
closure can resolve either record kind; normal condition recovery and elapsed
occurrence expiry remain type-specific. Operator triage (`open`,
`acknowledged`, `muted`, `escalated`) is independent from lifecycle.

Fleet > Alerts bulk triage has one canonical write:
`POST /api/v1/fleet-alert-states/bulk`. A request carries one action and 1–1000
unique `{ alert_id, expected_revision }` items. Each `expected_revision` must
match the alert's displayed `state_revision`; revision `0` means that no triage
state row exists yet. The backend locks in stable order and commits every state
and audit change or none, returning all updated states and revisions. A stale
item rejects the whole batch with HTTP 409. The console applies a successful
response locally without reloading the Fleet snapshot; after a conflict or
transport error it makes one best-effort recovery read because a lost response
can follow a committed transaction and the client must not infer the outcome.

### Why one rule cannot duplicate an episode edge

Deduplication is part of the durable model, not a timing assumption:

1. A source fact is unique by evidence source plus source event ID. The same
   fact cannot become two evidence revisions.
2. A rule version records one durable evaluation receipt per evidence sequence.
   Repair and repeated evaluator passes find that receipt instead of consuming
   the evidence again.
3. Count confirmations are unique by rule version, correlation bucket, phase,
   and evidence ID. Only a distinct accepted evidence revision or occurrence
   can advance Count; periodic timer ticks cannot manufacture confirmations.
4. An episode is unique by rule, rule version, natural key, and Trigger
   generation, with at most one confirmed unresolved episode for that rule and
   natural key.
5. The lifecycle outbox has a unique key for episode, generation, and edge
   kind. Retrying the transition therefore still stores at most one
   `alert.triggered` and one `alert.resolved` edge. Persisting and Unknown are
   states of that same generation and emit no edge.
6. An Alert-event Schedule also stores a unique receipt for schedule, event
   kind, and event ID, and one job ID per receipt. Worker retries cannot create
   another job for the same accepted edge.

The guarantee is **one Triggered edge and one Resolved edge per rule-owned
episode generation**, not one alert for the entire rule forever. Different
subjects/natural keys and a later re-trigger after resolution are legitimate
new episodes or generations. Two separate rules that match the same evidence
also intentionally produce two separately owned alerts; avoid overlapping
rules when that is not the desired ownership model. Webhook HTTP transport is
at least once, so a remote receiver must still deduplicate delivery retries by
`event.id`.

### Alert lifecycle event contract

The durable outbox contains only two public alert event kinds:

- `alert.triggered`
- `alert.resolved`

The `alert` payload includes the episode ID, record kind, title, detail,
category, severity, source status, lifecycle state, generation, causal
timestamps, target identity, and resolution reason/provenance. `policy` and
`policy_rule` identify the exact rule version and its Trigger/Resolve meta
configuration. A causation ID and bounded schedule lineage follow direct
automation chains; lineage overflow fails closed instead of forgetting an old
schedule ID.

This guard is exact for provenance that the system can carry directly through
scheduled jobs, backup requests, capability outcomes, alert episodes, and
lifecycle edges. A shell command can also change later agent, tunnel, or
telemetry state without a causation token in that source protocol. Such an
indirect effect cannot be attributed to the originating schedule, so policy
confirmation and recovery suppress flapping but do not prove that this wider
feedback loop is acyclic. Design shell automations to be idempotent and use
specific policy/rule filters when they can affect their own evidence source.

Webhook transport is retryable and therefore at least once. Deduplicate HTTP
deliveries by `event.id`, then merge by alert episode ID, generation, edge, and
causal timestamps; do not assume arrival order. Alert lifecycle edges bypass
rule cooldown, while exact rule/event identity still deduplicates queueing.

When a referenced VPS is hidden or deleted, stable alert, policy, rule, event,
and `vps.id` predicates remain usable. Mutable name, tag, status, VPS-rule, and
untagged predicates fail closed. Scope lifecycle integrations on stable policy
or rule IDs, or on alert category—not on a mutable status that may be the
reason the condition resolved.

### Alert-event schedules

Automation > Schedules offers two clearly separate trigger modes:

- **Time · cron** uses UTC cron, catch-up, and retry controls.
- **Alert event** consumes policy-confirmed lifecycle edges. It never listens
  directly to status, job, telemetry, server, or interval events.

Every OR branch of an alert-event expression must contain a positive
`alert.triggered` or `alert.resolved` anchor. Immutable metadata may narrow it:

```text
alert.triggered && alert.category:traffic
alert.resolved && alert.category:traffic
alert.triggered && alert.severity:critical
alert.resolved && policy_rule.id = 6fddf19d-0000-4000-8000-000000000001
alert.triggered && alert.record_kind = event
(alert.triggered && alert.category:network) || (alert.resolved && alert.category:network)
```

The complete event-expression vocabulary is:

- Positive anchors: `alert.triggered`, `alert.resolved`.
- Classification shorthands: `alert.category:<category>` for the eight policy
  categories listed above, and `alert.severity:info`,
  `alert.severity:warning`, or `alert.severity:critical`.
- Comparison or membership fields:
  `event.id`, `event.kind`, `event.occurred_at`, `event.recorded_at`;
  `alert.id`, `alert.public_id`, `alert.episode_id`, `alert.record_kind`,
  `alert.category`, `alert.severity`, `alert.lifecycle_state`,
  `alert.trigger_generation`, `alert.source_status`,
  `alert.resolution_reason`, `alert.title`, `alert.detail`, `alert.client_id`,
  `alert.target_kind`, `alert.target_id`; `policy.id`, `policy.name`;
  `policy_rule.id`, `policy_rule.name`, `policy_rule.rule_version`,
  `policy_rule.rule_kind`, `policy_rule.evidence_source`,
  `policy_rule.system_seed_key`,
  `policy_rule.trigger_meta_condition.kind`,
  `policy_rule.trigger_meta_condition.window_seconds`,
  `policy_rule.resolve_meta_condition.kind`, and
  `policy_rule.resolve_meta_condition.window_seconds`.

Comparison/membership values and Boolean operators use the shared expression
grammar above. A field comparison does not replace the positive lifecycle
anchor: every side of `||` must still contain a non-negated
`alert.triggered` or `alert.resolved`. A null or missing field, such as
`alert.resolution_reason` on Triggered or `alert.client_id` on a global alert,
does not satisfy a direct comparison or membership predicate. Constrain the
edge shape before relying on an edge-specific field.

Raw predicates such as `vps.status.become_offline`, `job.status:failed`,
`telemetry.*`, tags, live VPS rules, or interval anchors are rejected. Put that
logic in an Alert Policy so Sustained, Count, hysteresis, Unknown, and automatic
resolution are applied once and consistently.

The event expression gates a source edge; it never retargets the job. The
schedule always dispatches its separately reviewed fixed target snapshot. One
or many matched evidence subjects still create one job for that schedule and
edge. Subjectless alerts evaluate once in alert/event context.

Event schedules are prospective. Create, edit, enable, target refresh, and
defer establish a new arm fence; older and deferred-window edges do not replay.
Once an edge is accepted under a locked definition, later edits do not rewrite
its captured targets, template, actor, or revision. Each accepted Triggered and
Resolved edge produces at most one job. If both match before dispatch, the
Resolved job waits for the same schedule's Triggered job to finish. It never
waits on another schedule. A Resolve-only definition can run without a matching
Triggered receipt. If that same definition accepted Triggered but its job could
not be materialized, the corresponding Resolve receipt fails closed instead of
running a recovery action for mitigation that never ran. Deterministic
template or authorization failures are terminal and visible; transient storage
failures retry the same receipt without duplicating a job.

#### One alert condition, schedule and webhook

An Alert-event Schedule can own the lifecycle condition while a webhook only
observes that saved schedule. For example, save this authoritative Schedule
event expression once:

```text
alert.triggered && alert.category:traffic && alert.severity:critical
```

Then bind a webhook to the job materialized by that schedule, without copying
the alert condition:

```text
schedule.due && schedule.id:<saved-schedule-id>
```

Optionally observe the terminal outcome as a separate rule:

```text
schedule.job_finished && schedule.id:<saved-schedule-id>
```

`schedule.due` carries the saved schedule ID and name plus the materialized job
payload. `schedule.job_finished` carries the same identity plus the terminal
job payload. Prefer the stable schedule ID so renaming the schedule does not
retarget the webhook.

A webhook may instead match `alert.triggered` or `alert.resolved` directly and
own its notification condition independently. The direct form is useful when
delivery should not depend on a schedule; the schedule-event form keeps one
authoritative alert condition and follows the resulting job lifecycle. Both
forms use the same canonical lifecycle event names and immutable field
meanings. Webhooks additionally support their documented raw event contexts;
Schedule source expressions remain limited to policy-confirmed alert lifecycle
edges.
Direct alert lifecycle deliveries bypass webhook cooldown; schedule events use
the rule's ordinary cooldown, so set it to match the desired delivery cadence.

#### Event argv templates

`event_argv_template` is an optional JSON string array. Omitted or null uses
the fixed no-op argv `["/bin/true"]`; a custom array containing
`"/bin/true"` is equivalent:

```json
["/bin/true"]
```

The HTTP API and stored definition use that JSON-array shape. The console's
Cron schedule, Alert-event schedule, and Job Dispatch **Argv** text boxes are a
compact authoring form for the same array: unquoted whitespace separates
elements; single or double quotes group one element and are removed; and `''`
or `""` creates an empty element. Outside quotes, a backslash quotes the next
character and a backslash-newline continues the same element. Inside double
quotes, backslash is removed only before `$`, backtick, `"`, `\`, or a newline;
before any other character the backslash is preserved. Single-quoted
text is literal until its closing quote. An unmatched quote or trailing
backslash is rejected. There is no variable, glob, command, or shell expansion,
and the parsed card shows the exact JSON array before review. For example,
console text

```text
/usr/bin/printf '%s %s\n' 'hello world' tail
```

becomes

```json
["/usr/bin/printf", "%s %s\\n", "hello world", "tail"]
```

Each saved element remains exactly one argv element. Literal text and direct
allowlisted scalar placeholders are rendered independently; rendered spaces,
quotes, and metacharacters are never split or reparsed. The rendered array is
passed directly as argv, with a literal executable in `argv[0]`; vpsman does
not insert, identify, or specially parse a shell. Conditional blocks, loops,
collection helpers, unknown paths, missing/null/non-scalar values, NUL bytes,
and size violations fail closed.

A custom array must contain 1 to 128 non-empty string elements. Each stored and
rendered element is limited to 16 KiB, and the complete stored and rendered
array to 64 KiB. `argv[0]` must be nonblank literal text and cannot contain a
placeholder. Later elements may combine literal text with these exact scalar
paths:

```text
event.id                              event.kind
event.occurred_at                     event.recorded_at
alert.id                              alert.public_id
alert.episode_id                      alert.title
alert.detail                          alert.category
alert.severity                        alert.record_kind
alert.lifecycle_state                alert.trigger_generation
alert.source_status                   alert.resolution_reason
alert.target_kind                     alert.target_id
policy.id                             policy.name
policy_rule.id                        policy_rule.name
policy_rule.rule_version              policy_rule.rule_kind
policy_rule.evidence_source           policy_rule.trigger_meta_condition.kind
policy_rule.trigger_meta_condition.window_seconds
policy_rule.resolve_meta_condition.kind
policy_rule.resolve_meta_condition.window_seconds
schedule.id                           schedule.name
schedule.definition_revision          schedule.fixed_target_count
schedule.matched_subject_count
```

Shape-specific paths must exist on every edge the Schedule expression accepts.
For example, `alert.resolution_reason` belongs in a Resolve-only argv template,
not a template that also accepts Triggered. Use `alert.target_id` when an
external controller needs the evidence subject; the job itself still runs only
on the Schedule's separately reviewed fixed target snapshot.

Direct executable example:

```json
[
  "/usr/bin/printf",
  "%s\\n",
  "[{event.kind}] {alert.title} · {alert.category}/{alert.severity} · episode {alert.id} generation {alert.trigger_generation}"
]
```

If the executable is `sh`, `bash`, or another shell, its flags, command-string
grammar, expansion, and quoting belong to that shell and to the user who chose
it. Prefer positional arguments so rendered alert data is not shell source:

```json
["/bin/sh", "-c", "printf '%s\\n' \"$1\"", "--", "{alert.title}"]
```

The final review calls the server validator and shows the exact saved array,
canonical rendered sample, per-index result, immutable sample context, and
template/render hashes before save. Event schedules cannot be run manually
because no real lifecycle context exists. Saving or dispatching requires
`fleet:read`, `backups:read`, `jobs:write`, and `schedules:write`.

### Practical end-to-end recipes

These recipes keep the evidence condition in exactly one Alert Policy rule.
Schedules select the resulting rule-owned lifecycle edge by stable rule ID;
they do not repeat raw status or telemetry logic.

#### Agent offline for a while, then online

Use or edit the enabled **Agent offline** operational rule instead of adding an
overlapping offline rule:

```text
Rule type / source:  State / agent.status
Correlation:         natural_key
Trigger condition:   evidence.status = offline
Trigger meta:        Sustained 5m
Resolve condition:   evidence.status = online
Resolve meta:        Sustained 60s
Severity / category: critical / agent_status
```

The five-minute dwell absorbs short disconnects. A fresh authoritative online
state starts the one-minute Resolve dwell; if that remains the latest state,
the durable State timer can complete it on the next evaluator pass and the
same episode emits `alert.resolved`. Missing evidence is Unknown and cannot
pretend the agent recovered. Page or remediate from a stable rule-ID expression
such as
`alert.triggered && policy_rule.id = 6fddf19d-0000-4000-8000-000000000002`,
and clear external state from the corresponding `alert.resolved` edge.

#### Traffic near quota, apply a limiter, then remove it after cycle reset

Create one reviewed metric rule (or edit the intended traffic starter) with a
positive-quota guard:

```text
Rule type / source:  Metric / telemetry.combined
Correlation:         natural_key
Trigger condition:
  traffic.quota.total > 0 &&
  traffic.cycle.total >= traffic.quota.total * 0.8
Trigger meta:        Sustained 2m
Resolve condition:
  traffic.quota.total <= 0 ||
  traffic.cycle.total < traffic.quota.total * 0.2
Resolve meta:        Sustained 2m
Severity / category: critical / traffic
```

The lower Resolve threshold provides hysteresis. After the configured traffic
cycle resets, the first accepted post-reset telemetry sample makes
`traffic.cycle.total` naturally fall below 20% and starts the Resolve dwell. A
fresh accepted metric sample at or after the accumulated two-minute boundary
can produce Resolved. Incomplete accounting is Unknown, so it neither triggers
nor fakes a reset.

For example, let rule ID
`6fddf19d-0000-4000-8000-000000000001` identify only this limiter policy.
Create one Schedule whose fixed target snapshot is a reviewed controller VPS
and whose two OR branches select both edges of only that rule:

```text
Event expression:
(
  alert.triggered &&
  policy_rule.id = 6fddf19d-0000-4000-8000-000000000001
) || (
  alert.resolved &&
  policy_rule.id = 6fddf19d-0000-4000-8000-000000000001
)

Event argv:
["/usr/local/sbin/vpsman-traffic-limit", "{event.kind}", "{alert.target_id}", "10mbit"]
```

The example assumes that idempotent helper is installed on the controller;
vpsman does not supply it. The helper maps `alert.triggered` to apply and
`alert.resolved` to remove. Because one saved Schedule accepts the paired
edges, its Resolve job waits for its Trigger job to finish. If mitigation runs
on each affected VPS instead, use a separate fixed-target Schedule per VPS and
also compare `alert.target_id` to that VPS ID in both OR branches. A Schedule
never dynamically replaces its reviewed targets with the alert subject.

#### Job failure occurrence with elapsed auto-resolution

Use or edit the enabled general/specialized job-failure default rather than
copying its condition:

```text
Policy selector:     *
Rule type / source:  Occurrence / job.terminal
Correlation:         natural_key
Trigger condition:
  evidence.status in [canceled, rejected, failed, agent_timeout, control_timeout] &&
  !(evidence.command_type in ["*backup*", "*restore*", "*agent_update*"])
Trigger meta:        Immediate
Resolve condition:   none (not valid for an occurrence)
Resolve meta:        Elapsed after Triggered 7d
Severity / category: critical / job
```

Each terminal job fact owns one prospective episode. Triggered is immediate;
at or after seven days, the next successful due-evaluator pass automatically
writes Resolved even with no operator action. An operator may resolve the
occurrence earlier with a confirmed reason. Narrow `evidence.command_type` in
this one rule when backup, restore, or agent-update outcomes require different
ownership. The shown general-job exclusion is what prevents it from
overlapping the enabled specialized defaults; if one all-job rule should
replace them, disable those specialized rules deliberately.

#### Sustained CPU, RAM, and disk pressure with recovery hysteresis

Review and enable or edit the disabled resource starters. Prefer one rule per
resource so the title, recovery, and automation ownership remain unambiguous:

| Rule                            | Trigger condition                | Resolve condition               |
| ------------------------------- | -------------------------------- | ------------------------------- |
| CPU utilization                 | `cpu.utilization_ratio >= 0.90`  | `cpu.utilization_ratio < 0.75`  |
| CPU load saturation alternative | `cpu.load_saturation >= 1`       | `cpu.load_saturation < 0.80`    |
| RAM availability                | `memory.available_ratio <= 0.10` | `memory.available_ratio > 0.20` |
| Disk availability               | `disk.available_ratio <= 0.10`   | `disk.available_ratio > 0.20`   |

Use `telemetry.combined`, `natural_key`, Sustained 5m Trigger, Sustained 2m
Resolve, and category `resource` for each. This requires real saturation for a
while, avoids boundary flap through separate recovery thresholds, and resolves
automatically only after stable recovery is confirmed by a fresh metric sample.
Use either the CPU-utilization or load-saturation rule when they represent the
same operational concern; enable both only when two independently actionable
alerts are intentional.

For any recipe, a webhook may match `alert.triggered`/`alert.resolved` and the
stable rule ID directly. Alternatively, let the Alert-event Schedule own that
expression and have the webhook match `schedule.due` and optionally
`schedule.job_finished` for the saved Schedule ID. The latter keeps the
condition in one place; the former keeps notification independent from job
materialization. Both are supported operator choices.

### Expression webhooks

Webhooks remain an additional delivery path. They use the shared expression
AST and the same generic alert lifecycle vocabulary. Other raw event contexts
remain available for observational integrations, but alert predicates are the
recommended choice for mitigation, paging, and recovery automation because
the policy has already removed ambiguous or transient facts.

Webhooks may match alert lifecycle edges directly, or match the resulting
`schedule.due` and `schedule.job_finished` events as shown above when a saved
Alert-event Schedule should own the condition. These are complementary choices;
the latter avoids duplicating the schedule's alert expression in the webhook.

Recommended alert rules:

```text
alert.triggered && alert.category:agent_status
alert.triggered && alert.category:traffic && alert.severity:critical
alert.resolved && alert.category:traffic
alert.resolved && alert.category:backup
alert.triggered && policy_rule.id = 6fddf19d-0000-4000-8000-000000000001
```

The starter body is executable documentation rather than commented
alternatives. Alert lifecycle branches come first because policy-confirmed
alerts are the recommended automation surface. The later raw-event branches
remain useful for observational integrations. This is the same starter shown
in the web console:

```text
[if alert.triggered]
🚨 ALERT TRIGGERED
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Episode: {alert.id} · generation {alert.trigger_generation}
Record: {alert.record_kind} · lifecycle {alert.lifecycle_state}
Classification: {alert.category} · {alert.severity}
Title: {alert.title}
Detail: {alert.detail}
Policy: {policy.name} ({policy.id})
Rule: {policy_rule.name} ({policy_rule.id})
Target: {alert.target_kind}:{alert.target_id}
[if alert.client_id]Client: {alert.client_id}
[endif]Source status: {alert.source_status}
Triggered at: {alert.triggered_at}
Observed subjects: {matched_vps.map(vps.display_name).join(", ")}
[elseif alert.resolved]
✅ ALERT RESOLVED
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Episode: {alert.id} · generation {alert.trigger_generation}
Record: {alert.record_kind} · lifecycle {alert.lifecycle_state}
Classification: {alert.category} · {alert.severity}
Title: {alert.title}
Detail: {alert.detail}
Policy: {policy.name} ({policy.id})
Rule: {policy_rule.name} ({policy_rule.id})
Target: {alert.target_kind}:{alert.target_id}
[if alert.client_id]Client: {alert.client_id}
[endif]Source status: {alert.source_status}
Triggered at: {alert.triggered_at}
Resolved at: {alert.resolved_at}
Resolution: {alert.resolution_reason}
[if alert.resolution_note]Resolution note: {alert.resolution_note}
[endif]Observed subjects: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "schedule.due"]
⏰ SCHEDULE DUE
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Schedule: {schedule.name} ({schedule.id})
Trigger: {schedule.trigger_kind} · definition revision {schedule.definition_revision}
Command: {schedule.command_type}
Selector snapshot: {schedule.selector_expression}
Catch-up run: {schedule.catch_up_run_index}/{schedule.catch_up_run_count} · {schedule.catch_up_policy}
Job: {job.id} · {job.type} · {job.status}
Target count: {job.target_count}
Target IDs: {schedule.target_ids.join(", ")}
Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "schedule.job_finished"]
🏁 SCHEDULE JOB FINISHED
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Schedule: {schedule.name} ({schedule.id})
Job: {job.id} · {job.type} · {job.status}
Target count: {job.target_count}
Target IDs: {job.target_ids.join(", ")}
[if schedule.last_job_error]Error: {schedule.last_job_error}
[endif]Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "job.status"]
🛠️ JOB STATUS
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Job: {job.id} · {job.type} · {job.status}
Target count: {job.target_count}
Target IDs: {job.target_ids.join(", ")}
Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "vps.status_changed"]
🖥️ VPS STATUS CHANGED
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
[if event.from_status]Transition: {event.from_status} → {event.to_status}
[else]Status: {event.to_status}
[endif]Reason: {event.reason}
Affected VPSs: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "telemetry.rollup"]
📊 TELEMETRY ROLLUP
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Telemetry subject: {telemetry.client_id} via {telemetry.gateway_id}
Host: {telemetry.hostname}
Observed at (unix): {telemetry.observed_unix}
Uptime seconds: {telemetry.uptime_secs}
Networks: {telemetry.network_count} · tunnels: {telemetry.tunnel_count}
Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[else]
ℹ️ EVENT
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Webhook rule: {rule.name}
Expression: {rule.expression}
Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[endif]
```

The JSON request also carries webhook-rule metadata, event metadata,
`matched_vps`, and the rendered `message`. Missing roots in non-alert contexts
remain absent. HTTPS targets are required by default; DNS answers are pinned
and private, loopback, link-local, multicast, unspecified, documentation, and
reserved addresses are rejected. Redirects, proxies, embedded credentials,
and DNS rebinding are not accepted.

Local development may set `VPSMAN_DEV_ALLOW_LOOPBACK_WEBHOOKS=1` in both API
and worker to permit HTTP only on `localhost`, `127.0.0.0/8`, or `::1`. Keep it
unset in production.
