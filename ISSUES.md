I inspected the 51 uploaded desktop screenshots and mapped them into the operator lifecycles. Overall: **the project has a strong functional foundation and many advanced concepts, but it currently feels like an engineer-facing control surface, not a production-grade cloud console.** The biggest problems are not visual polish first; they are **correctness, safety, lifecycle clarity, auditability, and discoverability**.

A useful comparison point: Cloudflare and Google Cloud both make scope and lifecycle explicit. Cloudflare separates user profile, account, and zone concepts so operators understand where settings apply; Google Cloud dashboards are tied to project context and support charts, indicators, incidents, logs, SLOs, text, traces, grouping, filters, and events for troubleshooting. Cloudflare notification setup also treats destinations/webhooks as a clear create-test-save lifecycle, not just raw delivery records.

## Highest-priority production blockers

### 1. Status correctness is visibly inconsistent

The most damaging issue is trust. Several screens show states that contradict each other.

Examples I saw:

- Fleet table: VPS rows marked **online** while **Last seen = never seen / until first gateway...**
- Runtime config bulk page: selector text exists, but the control still says **no selector**.
- VPS Rules dry-run: **Changed rows = 3**, but `before` and `after` values are identical.
- Audit log: **0 audit records**, while the UI contains privileged workflows, login/session data, update actions, config changes, file operations, terminal sessions, etc.
- Global header says **Jobs 3** on almost every page, while several job-specific pages show no jobs or no runs.
- Many hashes, IDs, payloads, dates, and object keys are truncated or shown as repeated fixture-looking values, which makes the console feel non-authoritative.

**Fix direction:** define one canonical lifecycle model for every major object and render it consistently. A VPS should not be “online” and “never seen” unless those are two clearly separate fields such as `inventory_registered_at`, `gateway_connected_at`, and `last_telemetry_at`. Every status card, table chip, and dashboard metric should come from the same state vocabulary. Add a “data freshness” timestamp per widget and per table.

**UI requirement:** every resource row must show: current state, last heartbeat, last telemetry sample, gateway connection, agent version, and update time. Any stale or missing field must say exactly what is missing and why.

------

### 2. Privileged and destructive workflows are not safe enough

The app has a global **Unlock** button, but many pages also contain local locked states. The operator cannot easily tell:

- what capability is currently unlocked,
- for how long,
- for which action,
- whether unlock applies globally or only to the next request,
- what will be executed after unlock.

This affects command dispatch, runtime config, file edit, tag bulk mutation, backup/restore, topology promotion, OSPF cost update, artifact cleanup, update activation, rollback, and key revocation.

**Fix direction:** replace generic global unlock behavior with a standard per-action review flow:

```
Prepare → Preview impact → Unlock privilege → Confirm exact payload → Execute → Track job → Audit
```

**UI requirement:** every privileged action must show a review panel with:

- target list and target count,
- excluded/unavailable targets with exact reasons,
- operation payload,
- diff or before/after changes,
- timeout, concurrency, retry, rollback behavior,
- requesting user,
- required permission scope,
- audit ID that will be created.

Keep the top **Unlock** as a status indicator only: “Locked”, “Unlocked for config:write, 8m remaining”, etc.

------

### 3. Audit is not first-class enough

The audit page showing no records is unacceptable for a VPS control plane. Cloudflare’s Access logs are positioned around tracking who accessed what, when, and whether access was allowed; this panel needs that same operational seriousness for every privileged action.

**Fix direction:** audit should be a cross-cutting system, not a separate empty table.

**UI requirement:** every confirmation screen must say “This will create audit event `...`”. Every resource detail drawer should have an **Audit** tab. Audit records must include actor, role, IP, session, privilege unlock state, target resources, old/new values, request payload hash, result, and linked job/session/artifact IDs.

------

### 4. The information architecture hides lifecycles instead of guiding them

The product has good primitives, but the workflows are scattered:

- Alert lifecycle is split across Alerts, Alert policies, Notifications, Webhooks, Deliveries, Maintenance.
- Runtime config lifecycle is split across Overview, Bulk patch, VPS config, VPS Rules, Templates.
- Job lifecycle is split across History, Dispatch, Files, Multi files, Update registry, Transfers, Terminal sessions, Processes, Server jobs, Schedule runs.
- Topology lifecycle is split across Graph, Tunnel plans, Tests, Promotion, Evidence, OSPF.
- Backup lifecycle is split across Requests, Policies, Artifacts, Restore, Migration.

The subpages exist, but operators are not guided through the process.

**Fix direction:** add lifecycle steppers and resource detail pages.

For example:

```
Topology: Observe → Plan → Test → Evidence → Promote → Apply → Monitor
Runtime config: Template/patch → Target preview → Diff → Apply → Verify → Rollback
Backup: Policy → Request → Artifact → Restore test → Restore/migrate → Verify
```

**UI requirement:** each workflow should have a visible stepper, a current state, next recommended action, and links to evidence/history.

------

### 5. Targeting is too raw for production operators

Selectors such as `tag:edge`, `provider:alpha && country:US`, `id:edge-* || ...`, and webhook expressions are powerful, but they are currently exposed as raw strings in too many places. Expert DSLs are fine as an advanced mode; they should not be the only usable path.

**Fix direction:** keep the DSL, but add a query builder and target preview.

**UI requirement:** every selector field should provide:

- syntax suggestions,
- validation errors,
- matching count before execution,
- preview table of matching VPSs,
- “why matched” explanation,
- saved selector chips,
- copyable DSL,
- an expert-mode toggle.

Cloud consoles often expose advanced query syntax, but they surround it with previews, filters, chips, validation, and examples. This panel exposes the power before the guardrails.

------

## Workflow and lifecycle issues by area

### Global shell, navigation, and page structure

**Issues**

- The top header is too heavy: resource selector, search, saved views, view name, icons, live plane, unlock, and six fleet health cards repeat across almost every screen.
- Page-specific content often starts too far down, especially on 900px-high screenshots.
- The same fleet status cards appear on Access, Audit, System, Backups, Jobs, and Topology pages even when not directly relevant.
- “Saved views” and “View name” are always visible, but the user is not clearly in a view-editing mode.
- The global search box does not visibly expose result categories, query syntax, recent searches, or command navigation.
- The black floating toolbar appears in the screenshots. If this is an artifact from the screenshot tool, ignore it. If it is part of the product UI, it is a serious obstruction and must be removed or docked.

**Fix direction**

- Collapse global fleet health into a thin status strip.
- Move saved view editing into a view menu.
- Add a clear environment/project/fleet selector, similar in role to Cloudflare account/zone scope or Google Cloud project scope.
- Add a command palette: “Go to VPS”, “Open terminal”, “Create backup policy”, “Review failed jobs”.
- Keep page headers compact: title, subtitle, primary action, refresh, and current scope.

------

## Screen-by-screen practical issue inventory

### 01 — Dashboard overview

**Issues**

- Good high-level cards, but the page is too long and the charts are basic.
- Charts lack hover detail, thresholds, incidents, annotations, event overlays, zoom, compare, timezone, and per-series drilldown.
- “Grouped Statistics” cards like `country:US`, `country:DE`, `provider:alpha`, `all VPS` feel like tag fragments, not operational insights.
- “Top VPS” cards under charts waste space and do not clearly link to root cause.
- Dashboard does not show RPO/RTO, backup coverage, alert trend, update posture, agent versions, or topology health in a consolidated way.

**Fix direction**

- Use a dashboard grid with configurable widgets, saved filters, event overlays, and collapsible groups. Google Cloud dashboards explicitly support multiple widget types, incidents, logs, SLOs, grouping, filters, and events; this product should move in that direction.
- Add “why unhealthy?” summary: offline VPS, critical alert, backup gap, failed job, topology degradation.

------

### 02 — Fleet instances

**Issues**

- “online” plus “never seen” is the most obvious correctness bug.
- Missing expected cloud inventory fields: public/private IP, region, OS, kernel, agent version, last heartbeat, last telemetry, uptime, CPU/memory/disk, update status.
- Row actions are hidden behind an unclear dropdown.
- Details are accessible through an expander, but the main table does not communicate what happens after expanding.

**Fix direction**

- Add a dedicated VPS detail drawer/page.
- Table default columns: Name, Status, Region/Country, Provider, Public IP, Agent version, Last heartbeat, Alerts, Jobs, CPU, Memory, Disk, Tags.
- Make row click open the resource details; keep expander only for quick preview.

------

### 02b — Fleet traffic/rules detail

**Issues**

- The expanded row becomes an entire dashboard inside a table. This does not scale to many VPSs.
- Tabs inside a table row are hard to navigate.
- “No rollup”, “No resource rollup history”, and “No CPU rollup history” take large empty space.
- Traffic rules, alert policies, recent alerts, and charts are mixed into one row.
- “Samples 3 rate”, “counter epochs seen”, “incomplete reasons”, and raw key names need explanation.
- Rule values and matched policies use low-level names rather than operator language.

**Fix direction**

- Move this to a VPS detail page with sticky tabs: Overview, Metrics, Traffic, Rules, Alerts, Jobs, Files, Network, Config, Audit.
- Add tooltips and “explain this metric” links.
- Add traffic-quota visualizations: quota used, reset date, projected exhaustion, threshold, policy firing status.

------

### 03 — Fleet alerts

**Issues**

- Alerts are a table, not an incident workflow.
- No owner, assignee, duration, runbook, silence, escalation, timeline, policy link, or affected resource link.
- Columns truncate alert names and target names.
- Categories use raw names like `source_readi...`.
- “operator state” and “alert state” are not clearly separated.

**Fix direction**

- Use an incident detail drawer: summary, severity, affected VPSs, evidence, policy, timeline, notifications, runbook, actions.
- Add filters: open, acknowledged, silenced, critical, warning, category, target, age.
- Add bulk acknowledge/silence with audit preview.

------

### 04 / 04b — Alert policies and editor

**Issues**

- Policy table is sparse, and the editor is too raw.
- Selector expression and condition expression require DSL knowledge.
- No threshold builder, missing-data behavior, evaluation interval, notification route, runbook, labels, or test result preview.
- “Evaluate policy” is not the same mental model as “enabled”.
- “Dry-run” and “Review create” are good, but the preview is not prominent enough.

**Fix direction**

- Add a visual policy builder:
  - target selector,
  - metric,
  - operator,
  - threshold,
  - duration/window,
  - missing-data behavior,
  - severity,
  - notification route,
  - runbook.
- Keep raw expression under “Advanced”.
- Policy creation should show matched VPSs and simulated current states before saving.

------

### 05 / 05b / 05c / 05d — Notifications, webhooks, deliveries, maintenance

**Issues**

- The notification lifecycle is not clear. The user sees channels, webhooks, deliveries, queues, review dispatch, and cleanup, but not a simple route from alert to destination.
- “Review matches”, “Review queue dispatch”, “Review queued deliveries”, and “Review delivery” sound similar but mean different things.
- Webhook setup lacks visible test/send result, secret status, retry policy, cooldown explanation, and failure handling.
- Delivery history is split into notification and webhook histories, but their relationship is not obvious.
- Maintenance cleanup is too bare for a retention/destructive operation.

**Fix direction**

- Model it as: **Destination → Route/channel → Match preview → Delivery attempts → Retries/DLQ → Cleanup**.
- Add “Send test notification” next to every destination. Cloudflare’s webhook setup explicitly ends with Save and Test, which is a good expectation here.
- Delivery rows should show attempt timeline, HTTP status, response body excerpt, next retry, and correlation ID.
- Cleanup must show count, age, status, size, and irreversible effect before confirmation.

------

### 06 — Runtime config overview

**Issues**

- Many cards repeat concepts without showing the real config lifecycle.
- “Patch generators”, “templates”, “runtime syncs”, “template checks needing review”, and “VPS Rules” are powerful but abstract.
- The page says changes push immediately after confirmation, but there is no prominent rollout/rollback model.

**Fix direction**

- Replace with a lifecycle dashboard:
  - Current drift,
  - pending template changes,
  - failed syncs,
  - last successful apply,
  - affected VPSs,
  - rollback availability.
- Add primary actions: Read config, Create patch, Assign template, Review drift, Roll back.

------

### 07 — Runtime config bulk patch

**Issues**

- Raw JSON patch generation is too exposed.
- Selector field contradicts itself by showing selector text and “no selector”.
- No rollout controls: batch size, canary, concurrency, failure threshold, rollback plan.
- “Max timeout seconds” is low-level and not enough for safe fleet change.
- Review buttons are disabled without enough explanation.

**Fix direction**

- Add a guided bulk patch wizard:
  1. Choose generator or custom patch.
  2. Select targets.
  3. Render patch.
  4. Show per-VPS diff.
  5. Select rollout policy.
  6. Unlock and apply.
  7. Track verification.

------

### 08 — VPS config

**Issues**

- Empty state does not guide the user.
- Reading config is locked, but the UI does not explain which scope is required.
- “Redacted runtime TOML” panel is blank and large.
- No compare-to-template, compare-to-last-known, or drift indicator.

**Fix direction**

- When no VPS is selected, show a searchable VPS picker with health/status.
- After selection, show: current config, rendered desired config, drift, last sync job, source templates, and audit history.
- Use syntax highlighting and copy/download.

------

### 08b — VPS Rules

**Issues**

- The rule editor is key-value text, not a safe operator form.
- Dry-run says rows changed when before/after are the same.
- “Preview hash” is unreadable.
- Set and unset operations are visually cramped and too low-level.
- Traffic quota values should be validated with units, reset day, and selectors.

**Fix direction**

- Use typed forms for known rule families:
  - traffic quota,
  - reset day/timezone,
  - interfaces/selectors,
  - per-interface rx/tx limits.
- Show computed result: current quota, projected use, alert thresholds affected.
- Treat raw key-value as advanced mode.

------

### 09 — Runtime config templates

**Issues**

- Too many workflows on one screen: registry, source status, create template, assign template, render config, lifecycle clone/test/diff/update.
- Raw JSON dominates.
- Template scopes such as `default`, `shared`, `vps_local`, and statuses like `selected_no_store` need explanation.
- Buttons are disabled without inline reasons.

**Fix direction**

- Split into:
  - Template registry,
  - Template detail,
  - Assignment workflow,
  - Source readiness,
  - Render/test/diff.
- Add a template detail drawer with definition, assigned VPSs, last check, affected config, source evidence, and audit.
- Add schema-aware editors and validation.

------

### 12 / 13 / 14 — Tags registry, assignments, bulk tags

**Issues**

- Tag model mixes namespaced tags like `country:US` with free tags like `bgp` and `bird2`.
- “Create tag” placeholder suggests comma-separated multiple tags, but the action says singular.
- “Fleet tag order” is not important enough for its current prominence.
- Bulk tag selector again uses raw DSL and shows “no selector”.
- No tag governance: reserved tags, ownership, source, allowed values, or conflict warnings.

**Fix direction**

- Define whether tags are free labels or key/value labels. For production, prefer key/value labels with optional free tags.
- Add tag schema: namespace, allowed values, description, owner, system/user-managed.
- Bulk tag mutation must show target preview, before/after tags, and audit confirmation.

------

### 15 — Job history

**Issues**

- Job rows lack duration, requester, target names, output status, retry/cancel state, error summary, and links to logs.
- Payloads are truncated and not safely inspectable.
- Job IDs look like fixture values.
- “privileged” is a chip, but the reason/scope is not shown.

**Fix direction**

- Standard job detail page:
  - summary,
  - command/template,
  - targets,
  - per-target status,
  - stdout/stderr/output artifacts,
  - timeline,
  - retries/cancellations,
  - audit,
  - rerun with same targets.

------

### 16 — Command dispatch

**Issues**

- Too many operation types are tabs in one form: argv, shell, terminal, file pull/push, resumable upload/download, manual update, check update, activate, rollback, backup, sessions, processes, supervisor.
- This is powerful but mentally heavy.
- Target impact shows “1 needs review” but the exact reason is not visible.
- Safety model is incomplete: no canary, concurrency, failure policy, rollback, or command risk classification.
- Default command `/usr/bin/uptime` is fine as a fixture, but the form needs stronger production guardrails.

**Fix direction**

- Use a command launcher with operation cards or a searchable command template library.
- Each operation type should have its own typed form.
- Show impact and risks before unlock.
- Add “Run on one canary first”, “limit concurrency”, “stop on first failure”, and “require approval for root/PTY/destructive commands”.

------

### 17 — VPS file browser

**Issues**

- Action icons are unlabeled and ambiguous.
- Root path and blank editor dominate the layout.
- Missing expected file metadata: owner, group, size, modified time, type, symlink target.
- No visible diff before save, binary warning, large-file warning, conflict handling, checksum, or backup-before-edit.
- “Follow symlinks” is important but under-explained.

**Fix direction**

- Use a three-pane file manager:
  - tree/list,
  - metadata,
  - editor/preview.
- Every dangerous file action must show a named button or tooltip.
- Save flow: edit → diff → privilege unlock → save → verify → audit.
- Add download/upload progress and conflict resolution.

------

### 18 — Multi-file actions

**Issues**

- [x] Dangerous path `/` is shown as a normal default. Resolved 2026-06-25: the multi-file panel now starts with an empty path unless launched from a selected file path, and `/` requires explicit root-path approval before review.
- [x] Bulk download/upload lacks size estimate, target compatibility, and deeper preflight compatibility checks. Resolved 2026-06-25: the execution summary now shows bounded size estimates, target status compatibility, privilege readiness, stale/unavailable risk, and explicit remote-size limitations before confirmation.
- [x] Path validation is too easy to miss before execution. Resolved 2026-06-25: review now blocks empty, relative, traversal, and unapproved root paths before generating a confirmation.
- [x] “Execution summary” is empty and not helpful before execution. Resolved 2026-06-25: the summary pane now shows path readiness, target preview status, and the review contract before a job runs.
- [x] Per-target progress and detailed retry/artifact retention states need richer post-execution handling. Resolved 2026-06-25: post-run handling now summarizes retry candidates, downloadable target counts, retained output behavior, and points operators to exact job details while preserving existing grouped failure reasons and progress stats.

**Fix direction**

- [x] Require explicit path selection.
- [x] Show matched files/estimated size before execution. Resolved as bounded preflight: selected upload/text payloads are known exactly; download remote file counts and exact bytes are explicitly deferred until agents read paths because no pre-execution manifest endpoint exists.
- [x] Add per-target progress, failures, retry, artifact retention, and download bundle status.

**Critical review**

The old right-pane summary behaved like an empty placeholder until execution, which is not acceptable for a Cloudflare/GCP-style operation review. Operators could review target scope and path safety, but the UI did not distinguish known estimates from unknown remote state, did not surface stale/unavailable compatibility risk in the main decision area, and did not explain how retry candidates or retained outputs would be handled after the job.

**Resolved fix**

The multi-file panel now treats the summary pane as a preflight evidence surface: path readiness, server/local target scope, bounded size estimates, privilege/compatibility risk, retry plan, and retention behavior are visible before confirmation. After execution, a compact post-run strip shows terminal counts, retry candidates, and artifact/download handling alongside the existing grouped per-target outcomes.

**Future enhancement**

A true matched-file count and exact remote-size estimate still needs backend/API support, such as a non-mutating multi-target file-stat/manifest preview. The frontend now states that limitation instead of implying certainty.

------

### 19 — Agent update registry

**Issues**

- The page explicitly says registered-update policy is **not enforced**. That is dangerous for production.
- GitHub update check, manual activation, rollback, artifact URL, hash, and size are present, but release trust is not clear.
- No staged rollout matrix, current agent version inventory, failure rollback plan, or release notes.
- “Activate staged” appears without showing what is staged.

**Fix direction**

- Treat updates as a release management workflow:
  - release source,
  - signature/checksum/SBOM,
  - current fleet versions,
  - staging state,
  - canary targets,
  - rollout policy,
  - health checks,
  - rollback.
- Make enforcement explicit: “Only registered/signed releases can be installed” for production mode.

------

### 20 — File transfer history

**Issues**

- [x] Source artifacts, handoffs, and transfer records were mixed on one screen. Resolved by separating the visible lifecycle into upload source artifacts, download handoffs, and transfer sessions while preserving the dense operator workflow.
- [x] “No handoff” appeared in completed upload rows while “2 handoff ready” appeared above. Resolved by labeling upload rows as `Upload session` and keeping handoff readiness scoped to completed downloads.
- [x] Rate values like `100000 kb...` and `unlimited` were not well formatted. Resolved with human rate caps such as `100 Mbps cap` and `No transfer cap`.
- [x] Checksum, security policy, resume state, and failure reason were not visible enough. Resolved with row/detail evidence for SHA-256, source/destination role, resume state, security policy, and failure reason where the session data provides it.
- [x] Retention expiry is still not exposed by the transfer API. Skipped 2026-06-25 as a non-deserved frontend fix: the UI now states this explicitly in details, and correct expiry display requires backend data.

**Fix direction**

- [x] Separate upload source artifacts from transfer sessions.
- [x] Transfer row should show direction, source/destination role, target, progress, speed, checksum, and action.
- [x] Add a session detail drawer with chunk history and retry. Skipped 2026-06-25 as a non-deserved frontend fix: the inline detail now exposes last chunk metadata and lifecycle evidence, while true retry/history controls require a larger transfer workflow/API.

------

### 21 — Terminal sessions

**Issues**

- [x] Terminal session table had many tiny icon actions with no labels. Resolved by changing row controls to compact labeled buttons for Follow, Replay, Attach, Poll, Input, Resize, and Close while preserving existing ARIA labels.
- [x] Output indicators like `1 -> 4` and `4 -> 5` were not understandable. Resolved with terminal sequence labels such as `Seq 1-3 retained, next 4` and `Seq 4 retained`.
- [x] Retained output summary existed, but terminal replay/follow states needed clearer semantics. Resolved with explicit replay range, input state, live-follow state, and durable replay sequence text.
- [x] Session metadata that exists in the current API was too hard to inspect. Resolved by surfacing working directory, shell/window, idle timeout, flow window, output retention, last input, close reason, last job, last sequence, and observed time in row/context/detail surfaces.
- [x] Opened-by, explicit host identity beyond VPS label, privilege scope, and retention expiry are still not exposed by the terminal API. Skipped 2026-06-25 as a non-deserved frontend fix: the UI now states missing API fields in details, and correct display requires backend fields.
- [x] The embedded dispatch command form below was useful for convenience but lacked stronger separation and context. Resolved for retained sessions by renaming the table to `Session inventory and controls` and adding active-session context before the emulator.

**Fix direction**

- [x] Terminal detail view should include replay, follow, input, close, audit/retention evidence where available.
- [x] Add tooltips and labels to actions.
- [x] Download transcript, copy, explicit audit link, kill-vs-close, and retention expiry controls still need additional workflow/API support. Skipped 2026-06-25 as a non-deserved frontend fix because these controls require transcript/export, audit-link, lifecycle, and retention contracts.
- [x] Separate “open new terminal” from “dispatch generic command” as a dedicated composer flow while keeping both accessible. Skipped 2026-06-25 as a non-deserved frontend fix: session reconnect controls are clearer, but new-terminal creation belongs to the broader command-dispatch redesign.

------

### 22 — Process supervisor

**Issues**

- [x] Sparse inventory: process name, status, PID, source, started, observed. Resolved by adding a health summary, health/resource/log columns, cgroup evidence, restart evidence, and expanded process details.
- [x] CPU weight, current memory, restart count, log paths, and last exit evidence were available but too compressed. Resolved with readable row and detail labels for CPU weight, observed cgroup memory, process/PID count, stdout/stderr logs, restart attempts, last exit, and last restart.
- [x] CPU percent, memory history, restart-count history, recent-exit timeline, last exit reason text, supervisor config, and direct restart/stop row actions are not exposed in the current inventory API or dispatch preset model. Skipped 2026-06-25 as a non-deserved frontend fix: the UI now states these gaps explicitly.
- [x] `source` looked like a raw ID. Resolved by showing operator-facing source labels such as `Status snapshot` with the raw source job as secondary evidence.

**Fix direction**

- [x] Add process detail view with health, logs, restart evidence, and resource usage available from the current API.
- [x] Add direct restart/stop/log actions once process supervisor dispatch presets or row action callbacks are wired. Skipped 2026-06-25 as a non-deserved frontend fix because row-safe supervisor callbacks/presets are not wired.
- [x] Link process state back to jobs and alerts. Skipped 2026-06-25 as a non-deserved frontend fix: source job ID is now visible, but alert links and first-class job navigation are not exposed by the current API.

------

### 22b — Server jobs / artifact cleanup

**Issues**

- [x] Cleanup used raw expression and domain checkboxes without enough operator framing. Resolved 2026-06-25: the panel now separates Filter expression from Authority domains, adds domain descriptions, and shows a selected-domain scope summary.
- [x] “Queue cleanup” was destructive but not clearly gated by a preview result. Resolved 2026-06-25: queueing is disabled until a fresh dry-run preview hash matches the current expression and domain set, and stale edits invalidate the preview.
- [x] Preview hash and matched count fields were empty before preview. Resolved 2026-06-25: both fields now show “Preview required before queueing” and the readiness summary reports the blocked state.
- [x] Size impact was not visible before confirmation. Resolved 2026-06-25: dry-run results now show matched artifact count and formal byte units in the panel and confirmation prompt.
- [x] Object list, oldest/newest age, and retention-rule impact are still unavailable because the current cleanup preview API only returns expression, domains, count, bytes, and preview hash. Skipped 2026-06-25 as a non-deserved frontend fix.

**Fix direction**

- [x] Cleanup must always be dry-run first before queueing from the UI.
- [x] Show total size, domain scope, and irreversible warning from the current preview response.
- [x] Show objects to delete, oldest/newest, and retention rule once the preview API exposes those fields. Skipped 2026-06-25 as a non-deserved frontend fix until the preview API exposes those fields.
- [x] Require typed confirmation or action-scoped privilege unlock for cleanup queueing. Resolved 2026-06-25: artifact cleanup confirmation now requires typing `DELETE` before the destructive queue button enables.

------

### 23 — Schedule runs

**Issues**

- [x] Empty state is clear but disconnected from schedule registry. Resolved 2026-06-25: the empty state now explains worker-created runs and offers direct registry and worker-check actions.
- [x] No link to create a schedule or inspect existing schedule. Resolved 2026-06-25: the empty state opens the Schedule registry for creation/inspection and keeps refresh available as Check worker.
- [x] Due-run rows still need richer lifecycle fields such as due time, dispatched time, schedule link, result, skipped targets, retry, and worker health detail. Resolved 2026-06-25: scheduled job rows now show command type, job/payload evidence, dispatched/completed lifecycle, target count, result status, worker authority, schedule-link availability, skipped-count availability, and retry/worker-health API limitations.

**Fix direction**

- [x] Empty state should offer: “Create schedule”, “View schedule registry”, “Check worker”.
- [x] Runs table should include schedule, due time, dispatched time, targets, job ID, result, skipped targets, and retry.

**Critical review**

The previous non-empty Schedule runs table looked like a generic job-history excerpt. For production operators, that is weak because due-run work needs to answer what automation created, when the worker dispatched it, what happened, what target scope was involved, and what follow-up is possible. The old row also failed to state which desired lifecycle fields were absent from the current jobs API.

**Resolved fix**

Schedule-run rows now use a lifecycle-specific grid: Schedule job, Lifecycle, Result, Worker evidence, and Actions. The row exposes existing `JobHistoryRecord` evidence and explicitly labels due time, schedule link, skipped count, retry policy, and worker health as not exposed instead of hiding those gaps. The header keeps direct access to Schedule registry and Refresh.

**Future enhancement**

First-class due time, schedule ID/name, skipped target counts, retry policy, worker-health detail, and a safe retry action need backend schedule-run fields or a dedicated schedule-run endpoint. The current UI is intentionally honest about those missing fields.

------

### 24 — Schedule registry

**Issues**

- [x] Cron expression is truncated and hard to understand. Resolved 2026-06-25: schedule rows now show a human-readable cadence such as "Hourly at minute 0" while retaining raw cron as secondary text.
- [x] Timezone is small and not central. Resolved 2026-06-25: cadence text includes UTC when meaningful and raw cron/timezone remains visible in the schedule column.
- [x] “one missed / retry 5m” is compact but not explanatory. Resolved 2026-06-25: policy cells now explain missed-run and retry behavior in sentence form.
- [x] Create schedule is collapsed even though the page’s likely primary action is scheduling. Non-deserved-fix for existing schedules: keeping the composer collapsed preserves console density once schedules exist; resolved for empty registry by opening the composer automatically when no schedules exist.
- [x] No next-runs preview. Resolved 2026-06-25: the registry table now shows up to five next runs plus last-run state, and the composer preview shows the interpreted cadence before save.

**Fix direction**

- Render human-readable schedule text: “Hourly at minute 0, UTC”.
- Show next 5 runs, last run, last result, missed-run behavior, target snapshot age, and target update action.
- Add pause/resume and manual run.

------

### 25 — Topology graph

Status: Resolved 2026-06-25

**Critical review**

- The graph had a useful SVG, but it still behaved like a diagram above a table rather than the primary topology inspection surface. Cloudflare/GCP-style topology views need visible layers, readable node identity, navigation controls, and an explanation of what the edge metrics mean.
- Node labels are now shown in full inside the graph with native hover details for nodes and tunnels.
- The graph now includes viewport controls for zoom, reset, and directional pan, plus a minimap for orientation.
- A graph legend now explains visible layers, health colors, OSPF cost, latency/loss, bandwidth evidence, and region grouping.
- OSPF cost is now shown as a readable `OSPF 22 (+8)` style value with a “Why OSPF cost changed” disclosure using the product’s cost model text.

**Resolved fix**

- [x] Made the graph a stronger first-read surface before the edge table with a visible legend, minimap, and viewport controls.
- [x] Added applied/planned/attention layer summary and health-color explanations.
- [x] Added native hover detail for nodes and tunnels.
- [x] Added legend coverage for state, OSPF cost, latency, loss, bandwidth, and regional grouping.
- [x] Added “why cost changed” explanation for the OSPF cost model.

**Future enhancement**

- A full drag-to-pan canvas and richer geographic clustering would be stronger for very large fleets. The current controls cover practical zoom/pan/minimap behavior without adding a separate graph library.

------

### 26 — Tunnel plans

Status: Resolved 2026-06-25

**Critical review**

- The tunnel planner had the right expert controls, but it exposed them as one long configuration sheet. That is weak for production topology work because an operator needs a Cloudflare/GCP-style lifecycle read: pick endpoints, choose type, allocate addresses, validate overlap, test, review generated config, then apply/promote.
- The page now leads with an inline tunnel plan wizard strip that shows seven operator stages without turning the workflow into a blocking modal: endpoints, type, addresses, validation, connectivity test, generated config review, and promote/apply.
- CIDR and endpoint validation is now visible in the composer. The console flags missing endpoints, duplicate draft addresses, reserved-address collisions, saved-plan exact endpoint reuse, and invalid point-to-point prefix ranges.
- The generated config preview no longer leads with raw text. Latest generated config now has review cards for plan, touched files, validation steps, conflicts, rollback notes, and runtime mutation semantics.
- Raw ifupdown/Bird snippets are preserved under an “Advanced / generated config” disclosure so expert operators can inspect payloads without forcing every user to parse config text first.

**Resolved fix**

- [x] Added a compact lifecycle wizard for common tunnel creation while keeping advanced fields inline.
- [x] Added visible address/CIDR collision validation against draft duplicates, reserved addresses, saved plan endpoint reuse, and point-to-point prefix mistakes.
- [x] Added direct handoff buttons to connectivity testing and promotion/apply flows without blocking concurrent operator work.
- [x] Reframed generated config as a reviewable impact strip before raw snippets.
- [x] Moved raw generated ifupdown/Bird config into an expandable “Advanced / generated config” panel.

**Future enhancement**

- A true live diff against currently applied runtime files would be stronger than the current generated-plan review. That needs backend/runtime diff data, because the frontend can only review generator output, touched files, validation steps, conflicts, rollback notes, and host-mutation metadata already returned with saved plans.

------

### 27 — Network tests

**Issues**

- [x] Test form was clear but still low-level. Resolved 2026-06-25: the Tests panel now leads with a review contract strip for privilege, expected baseline, speed caps, recent evidence, and last local run before the parameter groups.
- [x] Speed test fields needed unit labels, safety explanation, and expected bandwidth context. Resolved 2026-06-25: labels now include units, the panel summarizes duration/data/rate/port/timeout caps, and the confirmation prompt repeats baseline and safety-cap evidence.
- [x] Buttons were disabled due lock, but required privilege was not visible. Resolved 2026-06-25: the panel now shows required privilege as locked/unlocked and each disabled review button explains the unlock requirement.
- [x] Results/history were not visible on the same screen. Resolved 2026-06-25: the panel now shows persisted probe/speed evidence for the selected plan plus the last local execution result state on the Tests screen.
- [x] Dedicated latency/loss/speed charts inside the Tests panel and explicit “attach evidence to topology plan” controls are still not implemented; current trend evidence is visible and linked through existing topology evidence/job history flows. Resolved 2026-06-25: the Tests panel now shows dedicated Latency, Packet loss, and Throughput trend charts for the selected plan, plus an explicit disabled Attach evidence affordance that states backend attachment support is required.

**Fix direction**

- [x] Show recent test history and expected baseline.
- [x] Add safety text: max data, duration, rate, endpoint, TCP port, timeout.
- [x] Add dedicated latency/loss/speed charts to Tests and an explicit evidence-attachment affordance.

**Critical review**

The Tests panel already exposed strong preflight safety and recent evidence text, but operators still had to leave the workflow to visually compare latency, loss, and throughput trends. That weakened the Cloudflare/GCP-style expectation that operational tests should show recent evidence in chart form next to the controls that create new evidence.

**Resolved fix**

The panel now includes a Trend evidence section with dedicated Latency, Packet loss, and Throughput charts for the selected plan. The section also exposes an Attach evidence control as a disabled affordance with a backend requirement note, so the product intent is visible without pretending the endpoint exists.

**Future enhancement**

Attaching evidence to a topology plan still needs a backend evidence-attachment endpoint and audit model. The current UI correctly presents the affordance as unavailable until that support exists.

------

### 28 — Topology promotion

Status: Resolved 2026-06-25

**Critical review**

- The promotion screen used to read as two unrelated create/import forms. That is weak for production topology work because operators need to understand the observed source, current saved/applied state, proposed change, conflict status, risk, and review step before committing.
- The workflow now leads with a compact promotion diff strip: observed topology, current applied topology, proposed plan, conflicts, risk, and review/approve.
- “External observe” and “Custom adapter” now have explicit operator-facing descriptions. The custom adapter path is moved into an advanced drawer so low-level argv fields no longer dominate the first view.
- Confirmation prompts now include the same review evidence, so the final action is tied to observed/current/proposed/risk context instead of just field values.

**Resolved fix**

- [x] Converted promotion into a diff workflow:
  - observed topology,
  - current applied topology,
  - proposed plan,
  - conflicts,
  - risk,
  - review/approve.
- [x] Moved custom adapter to an advanced drawer while preserving the existing expert workflow.
- [x] Added Playwright coverage for the promotion diff strip, advanced custom adapter drawer, and existing promotion payload behavior.

**Future enhancement**

- A deeper file/config diff against live runtime state would still be stronger than the current saved-plan/generated-conflict comparison. That needs authoritative runtime diff data beyond the current promotion form model.

------

### 29 — Topology evidence

Status: Resolved 2026-06-25

**Critical review**

- The evidence screen had the right raw material but read like adjacent logs instead of an operator decision path. A Cloudflare/GCP-style console should first answer “what happened, what evidence supports it, and what can I do next?” before exposing dense tables.
- Mixed OSPF plans, probe results, speed tests, runtime status, and command rows now sit behind a compact evidence timeline: observation, probe, speed test, status check, recommended cost, approval, and command output.
- Command output no longer collapses empty and unqueried states into “Output not loaded.” The UI now distinguishes pending output from “no retained output” after a fetch.
- Approval-required cost changes now expose the intended action path and a direct “Approve cost update” control that opens the OSPF review workflow.

**Resolved fix**

- [x] Converted the evidence summary to a timeline:
  - observation,
  - probe,
  - speed test,
  - status check,
  - recommended cost,
  - approval.
- [x] Added explicit “Load output”, “Compare to previous”, and “Approve cost update” actions.
- [x] Added Playwright coverage for the topology evidence timeline and the approval handoff.

**Future enhancement**

- A full row-level “compare to previous” diff drawer would be stronger than the current action, which jumps to trend-range comparisons. The current implementation still gives operators an explicit comparison path without adding an unsupported diff model.

------

### 30 — OSPF

**Issues**

- [x] OSPF update was compact, but the meaning of cost changes was not explained. Resolved 2026-06-25: the OSPF panel now leads with cost change, why, measurements, affected tunnel, traffic impact, rollback, and monitoring context.
- [x] “14 to 22” did not show why, confidence, measurement, expected path effect, and affected tunnel. Resolved 2026-06-25: the review strip and confirmation prompt now show confidence, persisted probe/speed evidence, effective bandwidth, expected path preference effect, endpoint scope, Bird2 file, and proposed snippets.
- [x] Rollback and monitoring-after-apply were not visible. Resolved 2026-06-25: the review now states the rollback cost and the post-apply probe/speed/evidence verification plan.
- [x] One-click rollback and automated post-apply monitoring status remain open; the current UI exposes the rollback/monitoring plan, but not a dedicated rollback job or verification status stream. Skipped 2026-06-25 as a non-deserved frontend fix because rollback jobs and verification status streams need OSPF workflow hooks.

**Fix direction**

- [x] Review cost update should show current cost, proposed cost, evidence, confidence, affected routes/tunnels, potential traffic impact, and rollback plan.
- [x] Add explicit post-apply monitoring state and rollback action once the OSPF update workflow exposes those lifecycle hooks. Skipped 2026-06-25 as a non-deserved frontend fix until those lifecycle hooks exist.

------

### 31–35 — Backups: requests, policies, artifacts, restore, migration

Status: Resolved 2026-06-25

**Critical review**

- The backup surface already had serious one-time request, policy, artifact handoff/upload, restore, rollback, and migration machinery, but the first read was table-first. That is not enough for a Cloudflare/GCP-style production backup console, where operators need posture before records: coverage, freshness, failures, storage, restore readiness, and migration gaps.
- The panel now leads with a backup posture overview on every Backups subpage. It summarizes protected VPSs, unprotected VPSs, last backup, next backup, failed requests, active artifact storage, restore test readiness, migration readiness, and retention/security state.
- The overview explicitly separates authoritative current records from API gaps. Encryption, immutability, storage backend, retention expiry, and full restore verification are not invented when the current artifact/restore APIs do not expose them.
- Restore and migration now read less like disconnected tables because the posture overview establishes the lifecycle before the operator opens the workflow drawer: protect -> store -> restore test -> verify.
- The existing executable restore flow still creates a metadata plan first, then dispatches an actual restore from staged agent-local archive metadata, with rollback review. The overview now names the missing restore-test posture rather than relying on the confusing table summary alone.

**Resolved fix**

- [x] Added a backup posture overview before subpage records.
- [x] Added coverage, unprotected, latest backup, next backup, failed request, artifact storage, restore-test, migration, and retention/security cards.
- [x] Exposed missing encryption/immutability evidence as an API gap rather than a false “secure” state.
- [x] Kept one-time backup, policy, artifact, restore, rollback, and migration workflows in the action drawer to preserve concurrent console work.
- [x] Added Playwright coverage for the posture overview on the restore workflow path.

**Future enhancement**

- Artifact rows still need storage backend, encryption state, immutability, retention expiry, size trend, and verification status once the API exposes those fields.
- Restore posture should become stronger when restore jobs expose explicit restore-test result state, file preview/diff, overwrite/merge policy, and post-restore verification streams.
- Migration can still use deeper identity/key mapping and cutover checklist data when the migration API exposes those lifecycle hooks.

------

### 36 / 37 — Audit events and retention

**Issues**

- [x] Audit table was empty without explaining that this is a production audit coverage gap. Resolved 2026-06-25: the events page now leads with an audit coverage overview, expected workflow contract, capture-health state, and an empty state that names missing privileged event evidence.
- [x] Audit events lacked first-class filters beyond grid search. Resolved 2026-06-25: added actor, action, resource, result, IP, session, privilege scope, and date filters backed by metadata-aware matching.
- [x] Retention page was too low-level for compliance. Resolved 2026-06-25: retention now starts with compliance cards for policy domains, current records, storage size, last export, next prune, metadata-only behavior, cleanup effect, and compliance warnings.
- [x] “Metadata only” for audit logs needed stronger explanation. Resolved 2026-06-25: audit-log retention now states that metadata-only cleanup prunes database ledger rows and does not imply external object blob deletion.
- [x] Cleanup review needed object count and effect. Resolved 2026-06-25: dry-run cleanup review now carries reviewed rows, object count, effect text, and nullable preview-hash handling into the confirmation prompt.
- [x] Export was present, but export scope and format were not obvious. Resolved 2026-06-25: added an export-scope strip showing export-enabled domains and JSON bundle format, plus export result text with domain count and limit.

**Fix direction**

**Critical review**

- The old audit surface looked like a generic empty database table, which is unacceptable for a VPS control plane with privileged unlocks, command dispatch, file operations, terminal input, key lifecycle, backups, topology, and suite configuration. A Cloudflare/GCP-style console must make missing security evidence loud, specific, and reviewable.
- The fix makes the audit page a coverage surface first and a table second. The table can still be empty when the backend returns no records, but the UI now identifies that as an API/capture gap, lists the expected workflow contract, and gives operators filters they would expect in access/audit logs.
- The retention page now separates compliance posture from low-level policy mutation. Operators can see enabled/exported domains, record/storage evidence limitations, last export state, next prune readiness, metadata-only semantics, and cleanup effect before touching retention fields.
- The implementation does not invent backend totals. Current audit rows are shown when available, while non-audit history record counts and storage size are explicitly marked “Not reported” until the API exposes compliance-grade metrics.

**Resolved fix**

- [x] Added audit coverage overview with expected event contract and capture-health state.
- [x] Added metadata-aware audit filters for actor, action, resource, result, time, IP, session, and privilege scope.
- [x] Added retention compliance overview and export-scope strip.
- [x] Added cleanup object count/effect to prune review and hardened nullable preview-hash handling.
- [x] Added focused Playwright coverage for events filters, retention compliance posture, dry-run effect, and cleanup confirmation.

**Future enhancement**

- Backend still must capture every login, unlock, config read/write, command dispatch, file edit, terminal input, key import/revoke, backup/restore, topology update, and system config change.
- Retention APIs should expose authoritative current row count, storage bytes, oldest/newest record, last export by domain, next scheduled prune, and object-deletion effect per domain.
- Every privileged confirmation should link to the audit event that was or will be created, and resource detail drawers should expose an Audit tab once audit records are available.

------

### 38–41 — Access overview, VPS keys, gateway, privilege unlock

**Issues**

- [x] Access had important concepts, but the wording was too internal: “No bearer session token in none”, “No privilege vault request-bound assertions only”, “no panel claim workflow”. Resolved 2026-06-25: the first read now uses operator-facing posture cards for authentication, RBAC, bearer session, privilege unlock, agent identities, and gateway operations.
- [x] Admin had TOTP off in system users and access screens, which is not production-safe. Resolved 2026-06-25: Access now flags `Admin MFA required` and shows an explicit admin-MFA warning in the TOTP panel.
- [x] Session TTL of 365 days was too long for admin but not visible in Access. Resolved 2026-06-25: Access now shows admin refresh TTL risk, current bearer expiry, refresh expiry, and a direct handoff to System sessions.
- [x] Super password + super salt hex was alarming as a UI concept. Resolved 2026-06-25: the shared privilege control now labels these as privilege secret and verifier/privilege salt while preserving the same local unlock contract.
- [x] Key import expected raw public key hex and client ID without enough lifecycle framing. Resolved 2026-06-25: VPS keys now starts with an agent identity lifecycle strip and the action drawer explains registration versus rotation, generated keypairs, and install-command dependency on gateway defaults.
- [x] Gateway page said agents connect only to gateways and no panel-side endpoint lookup, but this was not operationally actionable. Resolved 2026-06-25: Gateway now has a readiness panel for install defaults, live sessions, routing model, Preferences, and Suite config handoffs.
- [x] Role model was not visible enough. Resolved 2026-06-25: Access now summarizes visible roles/scopes and links directly to System users for full role management.

**Fix direction**

**Critical review**

- The previous Access panel was technically capable, but its first screen read like implementation status rather than an access-governance console. Mature cloud consoles make authentication posture, role scope, sessions, privileged authority, identities, and gateway connectivity separable at a glance.
- The new Access posture overview makes those six domains explicit and actionable. It does not hide expert concepts, but it names the operator consequence: admin MFA risk, long refresh TTL, local-only privilege material, request-bound assertions, identity lifecycle, and missing gateway install defaults.
- The privilege unlock wording is now safer and less alarming. It still accepts the same local secret/salt material, but the UI no longer presents “super password / super salt hex” as the product concept.
- The VPS key workflow is still expert-grade, but raw client ID/public-key entry is now framed by a lifecycle: register -> pending install -> connected -> rotate -> revoke -> blocked. This matches Cloudflare/GCP-style identity lifecycle thinking better than a bare import form.
- Gateway readiness is now actionable: operators can distinguish missing install defaults, absent live sessions, and suite gateway configuration instead of reading “no panel-side endpoint lookup” as an unexplained limitation.

**Resolved fix**

- [x] Added Access posture overview cards for operator authentication, RBAC roles/scopes, bearer sessions, privilege unlock, agent identities, and gateway operations.
- [x] Added admin MFA and long admin refresh-TTL warnings from current operator/session data.
- [x] Added direct handoffs to System users, System sessions, Preferences, and Suite config.
- [x] Reworded privilege unlock labels to privilege secret and verifier/privilege salt.
- [x] Added agent identity lifecycle strip and clearer registration/rotation helper copy.
- [x] Added gateway readiness panel with install-defaults, live-session, routing-model, and setup actions.
- [x] Added focused Playwright coverage for Access posture, MFA risk, lifecycle, gateway readiness, and existing import/rotation flows.

**Future enhancement**

- System should enforce admin MFA and shorter admin refresh TTLs server-side; the UI now flags risk but cannot enforce policy without backend rules.
- Access could expose a compact server-session revoke action for non-current sessions, while full session evidence is handled in Audit / Sessions in the release IA.
- Agent identity rows still need explicit pending/install/connected timestamps, audit links, installer command state, and rotation readiness once the key lifecycle API exposes them.
- Gateway readiness should include configured endpoint list, gateway server public-key fingerprint, rejected connection counts, and per-gateway health once those are exposed in the Access data model.

------

### 42 — System dashboard

**Issues**

- [x] Charts were basic and exposed raw counters without operator interpretation. Resolved 2026-06-25: System dashboard now starts with a control-plane posture overview, threshold status badges, attention queue, chart insights, and current metric sections that explain why each signal matters.
- [x] Legends were crowded and low-contrast. Resolved 2026-06-25: chart legends now use higher-contrast chip styling, and the Gateway Events chart now focuses on queue, age, drop, critical, rejected, and retry signals instead of mixing delivered totals into the same scale.
- [x] No visible thresholds. Resolved 2026-06-25: DB capacity, dispatch lifecycle, deadlines, gateway events, and cancellations now show threshold chips and semantic status tones.
- [x] “50-VPS capacity profile active” was useful but disconnected from planning. Resolved 2026-06-25: dashboard notes now feed a capacity forecast strip with expected max VPS count, configured dispatcher limits, and a recommendation summary.
- [x] Gateway and dispatch metrics were technical. Resolved 2026-06-25: posture cards and attention rows translate gateway drops/retries/rejections, deadline failures, queue depth, and DB pressure into operator-readable actions.

**Fix direction**

- [x] Added health/posture scoring and threshold badges.
- [x] Added “what needs attention” before raw charts.
- [x] Added explicit drilldown coverage note: rollup series are available, while raw incident overlays and per-series event/log links need backend endpoints.
- [x] Added capacity forecast with current configured dispatcher limits, recommendation text, and expected max from the active capacity profile.

**Future enhancement**

- Backend should expose incident overlays, anomalies, and raw event/log drilldowns for each System dashboard series; the UI now makes that gap visible instead of implying unavailable drilldowns exist.

------

### 43 — System users

**Issues**

- [x] Admin had TOTP off but no policy-level posture. Resolved 2026-06-25: System users now starts with an identity governance overview that flags admin MFA gaps and states the admin-MFA-required policy target.
- [x] Admin session TTL of 365d was risky but only visible as a raw table value. Resolved 2026-06-25: admin refresh TTL over 30d is now surfaced as governance risk in the overview, table, and selected-user evidence panel.
- [x] Role model felt too simple. Resolved 2026-06-25: the page now shows a compact RBAC role model, explicit scope override count, and admin-grant guardrail copy while preserving the existing role/scopes editor and reviewed mutation flow.
- [x] Last login, failed login count, disabled/deleted lifecycle state, and active session count were not visible in the Users workflow. Resolved 2026-06-25: user rows, expanded details, and the editor evidence panel now include active sessions, last login, failed login counts, lifecycle state, and direct non-current session revoke review.
- [x] Invited/locked state, password age, and API token status were not visible. Resolved 2026-06-25: these are now explicit backend evidence gaps in the governance overview and selected-user evidence instead of implied healthy states.

**Fix direction**

- [x] Added MFA-required policy posture and admin MFA risk count.
- [x] Formalized the existing role/scopes editor with RBAC role-model context, scope override evidence, and admin grant guardrail.
- [x] Added last login, failed login count, active sessions, TOTP state, lifecycle status, direct user-session revoke review, and explicit password-age/API-token evidence gaps.
- [x] Admin role grants and admin-targeting user actions now have visible pre-review guardrail copy plus the existing danger confirmation and privilege assertion flow.

**Future enhancement**

- Backend should expose enforceable MFA policy state, password age, invited/locked status, and operator API-token inventory so the UI can replace current evidence-gap labels with authoritative controls.

------

### 44 — System sessions

**Issues**

- [x] Dates were truncated. Resolved 2026-06-25: session rows and expanded details now use two-line timestamp cells with full title text for created, access expiry, and refresh expiry.
- [x] No user agent, device, browser, location/IP enrichment, revoke button, or suspicious login indicator. Resolved 2026-06-25: session rows now show user, role, IP/location evidence, browser/device, state, risk, and visible row-level revoke controls; the overview distinguishes available IP/device evidence from backend geo/impossible-travel gaps.
- [x] Authentication history was useful but needed filtering and clearer failure details. Resolved 2026-06-25: auth history now has All/Failures/Success/Suspicious filters, parsed browser/device evidence, clearer reason labels, and repeated-failure grouping.

**Fix direction**

- [x] Sessions table shows user, role, IP/location evidence, browser/device/user agent, created, access expiry, refresh expiry, state, risk, and revoke.
- [x] Auth history groups repeated failures and highlights suspicious activity available from current auth-event data.

**Critical review**

- The previous Sessions page had the right raw primitives but did not meet cloud-console expectations for security operations. An operator could see bearer sessions and auth events, but had to infer IP/device evidence, admin-session risk, revocation readiness, and repeated failures from separate rows.
- The new first read is a security posture strip: active sessions, admin sessions, IP/device evidence, location enrichment limits, suspicious auth, and revocation are visible before the tables. This matches Cloudflare/GCP-style operations surfaces where status and actionability precede raw logs.
- The Sessions table is now evidence-first rather than timestamp-first. It exposes identity, role, network, device, expiry, state, risk, and a visible revoke action, while preserving bulk revoke and the existing privilege-review contract.
- Auth history now supports the common investigation loop: filter to failures, separate success from suspicious events, inspect browser/device/IP details, and see repeated failure groups without losing the raw event table.

**Resolved fix**

- [x] Added session security overview cards for active sessions, admin sessions, IP/device enrichment, backend geo gap, auth failures, and revoke readiness.
- [x] Added two-line date cells, network/device/risk cells, expanded session evidence, and visible row-level revoke buttons.
- [x] Added auth-history filters and repeated-failure grouping with clearer failed-login reason display.
- [x] Added Playwright fixture coverage for multiple auth events, repeated failures, browser/IP evidence, filter behavior, and reviewed revoke.

**Future enhancement**

- Backend should expose geo lookup, impossible-travel analysis, device fingerprint stability, and authoritative session-login metadata so the UI can replace current "geo not exposed" gap language with real location/risk decisions.

------

### 45 — Suite config

**Issues**

- [x] Very powerful, but too much high-risk configuration on one page. Resolved 2026-06-25: Suite config now has a sticky section rail for API, Gateway, Worker, Capacity, Storage, Secrets, Timeouts, and Review, with the main page split into structured fields and review flow.
- [x] Many settings needed explanations: binds, gateway URL, pools, dispatch batch, in-flight, secrets, artifact limits. Resolved 2026-06-25: every structured field now exposes help text, validation rule, default value, current value, config path, and reload/restart impact.
- [x] Hot reload vs restart required was useful, but impact was not explained per field. Resolved 2026-06-25: each field gets a Hot reload / Restart required / Not reported badge derived from backend validation metadata, and the review panel groups changed keys by post-save impact.
- [x] Current redacted JSON was helpful for experts but too prominent for normal operation. Resolved 2026-06-25: redacted JSON diff now lives in a collapsed advanced disclosure, while the normal review path emphasizes changed keys and reload/restart plan.
- [x] Review/save was disabled until validation, but the next action was not strongly guided. Resolved 2026-06-25: review now has a save-flow stepper and next-action callout for validate, unlock privilege, review save, save, reload/restart, and audit evidence.

**Fix direction**

- [x] Split into sections with side navigation: API, Gateway, Worker, Capacity, Storage, Secrets, Timeouts, and Review.
- [x] Every structured field has help text, validation rule, default value, current value, and restart/hot-reload impact.
- [x] Save flow is explicit: edit -> validate -> unlock -> review -> save -> reload/restart/audit.
- [x] Raw TOML and redacted JSON remain available as advanced views.

**Critical review**

- The previous Suite config page exposed powerful capabilities, but it still asked operators to scan one long page and mentally join fields to validation output, restart notes, privilege state, and redacted JSON. That is risky for production configuration because the cost of editing the wrong field is high.
- The new structure follows mature cloud-console settings patterns: left section navigation, one visible field contract per setting, impact badges beside the field, a separate review stage, and raw data relegated to advanced disclosure.
- Field-level metadata now makes high-risk settings legible without hiding complexity. Binds, gateway URLs, pool sizing, dispatch capacity, secret refs, storage, artifact thresholds, and timeout values each carry help text, current/default value, validation rule, config path, and backend-reported impact.
- The save path is now an operational checklist instead of a disabled button. Operators can see why review is unavailable, validate, unlock privilege, review changed keys, see the restart/hot-reload plan, save, and then look for reload/restart and audit evidence.

**Resolved fix**

- [x] Added sticky Suite config side navigation and section anchors.
- [x] Added metadata-driven structured fields for API, Gateway, Worker, Capacity, Storage, Secrets, and Timeouts.
- [x] Added field-level help/default/current/validation/impact plus hover titles for compact metadata.
- [x] Added explicit save-flow stepper and next-action review guidance.
- [x] Collapsed raw redacted JSON diff into an advanced disclosure while preserving advanced TOML editing.
- [x] Added focused Playwright coverage for section navigation, per-field metadata, restart-impact review, next-action guidance, and advanced JSON disclosure.

**Future enhancement**

- Backend should expose authoritative per-field schema metadata, default source, min/max constraints, last applied reload result, restart-required service list, and audit event links so the UI can replace its local field catalog with server-owned policy.

------

### 46 — System preferences

**Issues**

- [x] Good preference coverage, but some settings were not merely personal preferences. Resolved 2026-06-25: Preferences now starts with scope overview cards and separates Personal display preferences, Local browser state, and Fleet/system defaults.
- [x] Gateway install defaults, tunnel allocation pools, and dashboard curve selectors affect operational behavior and should probably live in system config or project/fleet settings. Resolved 2026-06-25: these controls now live under a Fleet and system defaults section with explicit "operator-stored until shared backend scope exists" notices.
- [x] “Binary exact” comparison was expert-only and needed clearer context. Resolved 2026-06-25: the Bulk execution summaries card now explains that Binary exact compares bytes and is safest for security, checksums, generated files, and whitespace-sensitive command output.
- [x] Some sections were dense and visually similar. Resolved 2026-06-25: preferences now use scope bands, scope badges, overview tiles, operational warning blocks, and per-card reset controls.

**Fix direction**

- [x] Separated personal display preferences from fleet/system defaults and browser-local state.
- [x] Personal section includes timezone, language, sidebar behavior, name format, flags, tag visibility, and bulk output comparison.
- [x] System/fleet section contains gateway endpoints, tunnel allocation pools, and dashboard curve selectors with shared-scope caveats.
- [x] Added reset-to-default controls per server-stored preference card and kept local browser reset separate.

**Critical review**

- The previous Preferences page had strong coverage, but the IA was flat. A display setting, a browser-local cleanup action, and operational defaults for gateway install commands all looked equally personal. In a production console, that undermines trust because operators need to know which changes affect only their view and which can influence fleet workflows.
- The new first read is scope-based: Personal display, Browser state, and Fleet/system defaults. This matches mature cloud-console settings patterns where account preferences, browser-local state, and shared operational defaults are separate mental models.
- Fleet/system defaults are still stored through the current operator-preference API because that is the backend contract, but the UI no longer presents them as harmless personal preferences. It explicitly identifies gateway defaults, tunnel pools, and dashboard ranking as operational defaults awaiting shared backend scope.
- Expert comparison mode now has decision context. "Binary exact" is framed as byte comparison for security/checksum/generated-file correctness, with text normalization only for human log review.

**Resolved fix**

- [x] Added preferences scope overview cards for personal display, browser state, and fleet/system defaults.
- [x] Grouped cards into Personal display preferences, Local browser state, and Fleet and system defaults.
- [x] Added scope badges to every card and operational notices to gateway install defaults, tunnel allocation, and dashboard curves.
- [x] Added reset-to-default controls per server-stored preference card.
- [x] Expanded Binary exact copy with byte-level correctness context.
- [x] Added focused Playwright coverage for scope overview, section grouping, operational caveats, expert comparison copy, and reset affordances.

**Future enhancement**

- Backend should move gateway install defaults, tunnel allocation pools, and dashboard curve selectors into authoritative shared system/fleet settings when those scopes exist. Preferences should then keep only personal display, workflow presentation, and browser-local controls.

------

## Cross-cutting UI/UX requirements

### Tables

Current tables are consistent but not production-grade enough.

**Requirements**

- Every truncated value must have hover, copy, and detail view.
- Every raw ID/hash should be secondary text, not the primary label.
- Row actions must have labels or tooltips.
- Bulk actions must show selected count and target preview.
- Add column presets per role: operator, admin, network, backup, security.
- Add CSV/JSON export where appropriate.
- Add persistent filters, sorting, column layout, and saved views.

------

### Charts and visual impact

The visual style is clean but too plain. It feels like a well-organized internal admin app rather than a mature cloud console.

**Requirements**

- Add chart hover values, time range, timezone, legend toggles, threshold bands, incident markers, deploy/config event markers, and drilldown.
- Add status color semantics consistently:
  - green = healthy/success,
  - yellow = warning/review,
  - red = critical/failure/destructive,
  - blue = informational/active job.
- Avoid using large bordered status boxes everywhere; make the page hierarchy more deliberate.
- Add widget density controls: compact, comfortable, expanded.
- Add skeleton loading and explicit stale-data states.

------

### Tooltips and expert terminology

Many terms need explanation:

- OSPF, GRE, OPENVPN, Noise, TOTP, PTY, TTL, CIDR, underlay, endpoint allocation, preview hash, request-bound assertion, privilege vault, handoff, retained output, materialized run, selector snapshot, no rollup, missing samples.

**Requirements**

- Every advanced label gets an info icon.
- Every raw expression editor gets examples and autocomplete.
- Every disabled button explains why it is disabled and how to enable it.
- Every destructive button explains whether it is reversible.

------

### Empty states

Many empty states are visually clean but not actionable.

**Requirements**

- Empty state must answer:
  - what this table is for,
  - why it is empty,
  - what to do next,
  - whether this is healthy or a problem.
- Example: “No backup policies” should say “This fleet has no scheduled backups. Create a policy to protect selected VPSs.”

------

### Security and access

**Requirements**

- Mandatory MFA for admins.
- Shorter default admin session TTL.
- Session revocation visible everywhere.
- Granular RBAC/scopes surfaced in UI, not only docs.
- Privilege unlock scoped by action and time.
- Audit event created for every sensitive read/write.
- Key lifecycle with pending/current/revoked/blocked states.
- Installer commands should be generated from a safe wizard, not raw hex fields alone.

------

## Practical redesign direction

The product should not blindly copy Cloudflare or GCP, but it should adopt their durable console patterns:

1. **Scope first:** organization/project/fleet/environment selector, then resource context.
2. **Resource detail pages:** VPS, job, alert, backup artifact, topology plan, user/session, release.
3. **Lifecycle steppers:** for config, topology, backup, update, schedule, notification, and privileged jobs.
4. **Preview before mutation:** target preview, diff, risk, audit, confirmation.
5. **Evidence and timeline:** every alert, topology change, job, backup, and access event should have a timeline.
6. **Advanced mode without forcing it:** keep DSLs, TOML, JSON, hashes, and generated config, but wrap them in forms, validation, examples, and previews.
7. **Production observability:** dashboards with thresholds, incidents, logs, SLO-like health, events, filters, and drilldowns.

## Suggested build order

**Phase 1 — Trust and safety**
 Fix state contradictions, audit logging, unlock semantics, disabled-button reasons, raw action icons, and destructive confirmation flows.

**Phase 2 — Workflow clarity**
 Add VPS detail page, job detail page, alert detail page, backup posture page, topology lifecycle stepper, and config diff/apply workflow.

**Phase 3 — Expert usability**
 Add selector builder, expression autocomplete, schema-aware TOML/JSON editors, rollout/canary controls, and saved query/view management.

**Phase 4 — Polish**
 Improve charting, spacing, typography, tooltips, empty states, responsive behavior, dark mode, keyboard shortcuts, and command palette.

The project already has many advanced primitives. The key practical improvement is to stop exposing them as disconnected tables and raw forms, and instead turn them into guided, reviewable, auditable operator workflows.

------

## Release IA refactor plan — new intended shape, implementation map, and verification

Append marker, 2026-06-25: this block is intentionally appended after the old audit backlog. Treat it as the new release IA contract and migration checklist, not as another set of legacy visual issues or compatibility notes.

This section is a new release-shape execution plan. It is not part of the legacy issue audit above.

The plan intentionally does not preserve old page compatibility. The pre-release target is a clean VPS management console that combines:

- Cloudflare/GCP-style console structure: scope, product areas, resource pages, review, audit, search, and evidence.
- VPS-monitor density: Komari-style cards for quick server scanning where cards are genuinely useful.
- SSH/VNC/SCP replacement workflows: browser terminal, file browser, transfer retry, process logs/restart/stop, and reviewed network rollback.

### Release navigation contract

The release sidebar must contain exactly these top-level product areas:

1. Home
2. Fleet
3. Remote Operations
4. Jobs
5. Automation
6. Network
7. Backups
8. Config
9. Observability
10. Audit
11. Access
12. System

The old top-level areas `Dashboard`, `Tags`, `Schedules`, and `Topology` must not remain as release navigation entries. Their implementations are moved into the new product areas below.

### Global shell target

Existing implementation:

- `frontend/src/components/ConsoleShell.tsx`
- `frontend/src/constants.ts`
- `frontend/src/App.tsx`
- `frontend/src/styles/shell.css`

Target shape:

- Left navigation grouped by the release product areas above.
- Top bar includes fleet scope selector, global search/command palette, privilege lock state, live control-plane state, and session/account menu.
- Global search finds pages, VPSs, jobs, files, terminal sessions, schedules, backups, audit events, and saved views.
- Every page has a consistent breadcrumb, title, scope context, primary action area, content area, and optional right-side detail drawer.
- Saved views remain global but are scoped by resource type where needed.

Execution tasks:

- [x] Replace `ActiveView` with the release top-level areas.
- [x] Replace `viewSubpages` and `navSections` with the release page/subpage tree.
- [x] Replace hero-style page copy with operational page headers.
- [x] Add a command-palette model that indexes page routes and current entities.
- [x] Add global route helpers for `openVpsDetail`, `openTerminal`, `openFiles`, `openProcess`, `openJobEvidence`, `openAuditEvidence`, and `openNetworkEvidence`.
- [x] Remove legacy top-level navigation labels from production UI.

Verification:

- [x] Playwright verifies all 12 top-level navigation entries and every subpage are reachable on desktop and mobile.
- [x] Keyboard navigation reaches sidebar, global search, fleet scope, privilege lock, and page primary action.
- [x] Global search returns pages and at least one fixture entity type from VPS, job, terminal, transfer, backup, and audit fixtures.
- [x] No release screenshot contains old top-level labels as sidebar entries: `Dashboard`, `Tags`, `Schedules`, `Topology`.
- [x] `npm exec tsc -- --noEmit`, `npm exec vite -- build`, Impeccable detector, and `git diff --check` pass.

### Home

Target UI/UX:

- Fleet posture strip: online, stale, offline, warnings, running jobs, failed jobs.
- Komari-style VPS card strip for fast health scanning.
- Needs-attention queue: failed transfers, failed jobs, stale agents, backup failures, network degradation, access risks.
- Recent activity feed.
- Quick actions: Open terminal, Browse files, Dispatch command, Run backup, View network.
- Custom dashboard widget slots.

Existing implementation to map:

- Move from `DashboardPanel.tsx`: operational health, resource usage, network curves, label clusters, recent lists.
- Reuse fleet summary data from `useDashboardData`.
- Reuse existing dashboard preferences as Home widget preferences where appropriate.

Execution tasks:

- [x] Rename/rebuild `DashboardPanel` as `HomePanel`.
- [x] Add Komari-style fleet card strip using current `AgentView`, telemetry rollups, network rates, jobs, warnings, and backup status.
- [x] Add needs-attention queue composed from current job, transfer, backup, alert, network, access, and system health data.
- [x] Add quick actions that route to release destinations, not legacy pages.
- [x] Keep custom dashboard/widget preference hooks but rename them away from old dashboard language where visible.

Verification:

- [x] Home shows cards when agents exist and a useful empty state when no agents exist.
- [x] Every quick action routes to the correct new page.
- [x] Needs-attention queue links to the correct detail/evidence page.
- [x] Card text fits at desktop, tablet, and mobile widths.

### Fleet / Instances

Target UI/UX:

- Canonical VPS table.
- Columns: name, status, provider, region, tags, agent version, uptime, CPU/memory/disk summary, network, running jobs, backup state, alerts.
- Filters: status, provider, tag, region, agent version, warning.
- Bulk selection.
- Row actions: Open detail, Terminal, Files, Processes, Backup, Network.

Existing implementation to map:

- Move from `FleetWorkspace.tsx` subpage `instances`.
- Keep table primitives and current fleet actions.
- Move tag editing actions out to `Fleet / Groups`.
- Move alert policy authoring out to `Observability / Alerts`.

Execution tasks:

- [x] Extract `FleetInstancesPanel` from `FleetWorkspace`.
- [x] Add release row actions that route to `Fleet / Instance detail`, `Remote Operations`, `Backups`, and `Network`.
- [x] Add table/card view toggle only where appropriate; default stays table for Instances.
- [x] Remove inline workflows that belong to Remote Operations, Config, or Observability.

Verification:

- [x] Table supports filtering, sorting, column persistence, selection, and saved views.
- [x] Row actions route to release pages with selected VPS context.
- [x] No terminal/file/process tool is rendered inline inside the Instances table.

### Fleet / Monitor

Target UI/UX:

- Komari-style card grid for all visible VPSs.
- Each card shows VPS name, provider/region, tags, online/stale/offline, CPU, memory, disk, network, uptime, latency, warnings, running jobs.
- Card density: compact and comfortable.
- Sort: warning first, traffic, CPU, memory, region, provider.
- Click opens `Fleet / Instance detail`.
- Quick actions limited to Terminal, Files, Processes, More.

Existing implementation to map:

- Use telemetry and summary data currently visible in `FleetWorkspace.tsx` detail cards and `DashboardPanel.tsx`.
- Add new card component; do not reuse table rows as cards.

Execution tasks:

- [x] Create `FleetMonitorPanel`.
- [x] Create reusable `VpsMonitorCard`.
- [x] Add card density and sort controls.
- [x] Keep cards non-destructive: destructive/privileged quick actions open reviewed drawers elsewhere.

Verification:

- [x] Cards remain readable for 0, 3, 20, 50, and 100 VPS fixture counts.
- [x] Card states are distinguishable without relying on color alone.
- [x] Card quick actions route with selected VPS context.

### Fleet / Groups

Target UI/UX:

- Saved views and resource grouping.
- Tag registry, tag assignment, bulk tag mutation, provider/country/custom tag counts.
- Scope expression editor with preview.
- Counts by status/provider/tag.

Existing implementation to map:

- Move all of `TagsPanel.tsx` into `Fleet / Groups`.
- Move global saved fleet views from shell into Fleet group management where persistent group editing is needed, while keeping the top-bar selector as global scope control.

Execution tasks:

- [x] Replace top-level `Tags` with `Fleet / Groups`.
- [x] Rename `TagsPanel` to `FleetGroupsPanel` or split into `GroupsRegistry`, `GroupAssignments`, and `BulkGroupMutation`.
- [x] Keep tag mutation review prompts and schedule-impact warnings.

Verification:

- [x] No top-level Tags nav remains.
- [x] Tag registry, assignments, and bulk mutations are all reachable under Fleet / Groups.
- [x] Bulk mutation preview shows selected count, changed count, schedule impact, and review hash.

### Fleet / Alerts

Target UI/UX:

- Active fleet-resource alert queue.
- Alert severity, affected VPSs, first seen, last seen, source policy.
- Actions: acknowledge, mute, escalate, clear.
- Linked recent jobs/activity.

Existing implementation to map:

- Move active alert state and triage actions from `FleetWorkspace.tsx` subpage `alerts`.
- Move alert policy and notification channel authoring to `Observability / Alerts`; move webhook rule authoring to `Observability / Webhooks`.

Execution tasks:

- [x] Extract active alert queue into `FleetAlertsPanel`.
- [x] Keep triage review prompts for acknowledge/mute/escalate/clear.
- [x] Add links to affected VPS detail and alert policy context.

Verification:

- [x] Active alert triage works without opening alert policy editors.
- [x] Policy authoring controls do not render in Fleet / Alerts.

### Fleet / Instance detail

Target UI/UX:

- Full-page or right-drawer resource detail for one VPS.
- Tabs: Summary, Remote access, Files, Processes, Config, Backups, Network, Activity.
- Summary includes Komari-style single VPS card, health, tags, latest jobs, warnings.
- Remote access links to terminal/session shortcuts.
- Activity correlates jobs, alerts, audit, backups, network events.

Existing implementation to map:

- Move selected-agent detail surfaces from `FleetWorkspace.tsx`.
- Link existing jobs, config, backup, topology, and audit evidence rather than duplicating whole workflows inline.

Execution tasks:

- [x] Create `VpsDetailPanel` as a first-class route.
- [x] Add cross-page links from all VPS rows/cards to this detail route.
- [x] Remove oversized selected-VPS detail blocks from generic Fleet table pages once this exists.

Verification:

- [x] Opening a VPS detail from Home cards, Fleet table, alert rows, network graph, jobs, and backups shows the same canonical detail page.
- [x] Each tab has empty/loading/error states.

### Remote Operations / Terminal

Target UI/UX:

- New terminal composer first: target, shell/profile, working directory, privilege scope, timeout, review.
- Active sessions table.
- Retained sessions table.
- Terminal detail drawer: attach, follow, replay, input, resize, close, copy transcript, download transcript.
- Evidence: opened time, last input, sequence range, retention, related job/audit.

Existing implementation to map:

- Move `TerminalSessionsPanel.tsx` from `Jobs / Terminal sessions`.
- Move terminal-open convenience out of generic `JobDispatchPanel.tsx` into this page.
- Keep replay/follow/input/resize/close controls from current implementation.

Execution tasks:

- [x] Create top-level `Remote Operations` area.
- [x] Move terminal sessions into `Remote Operations / Terminal`.
- [x] Add dedicated new-terminal composer separate from generic command dispatch.
- [x] Add transcript copy/download when backend support exists; until then preserve visible unavailable reasons.
- [x] Route terminal audit evidence to `Audit / Sessions`.

Verification:

- [x] An operator can open or resume a terminal without visiting Jobs.
- [x] Generic shell dispatch remains in Jobs / Dispatch and is visibly distinct.
- [x] Transcript controls are present only when supported or disabled with explicit reasons.

### Remote Operations / Files

Target UI/UX:

- One-VPS file browser.
- VPS selector, path breadcrumb, file table, preview/editor pane.
- Actions: upload, download, edit, create folder, rename, delete, chmod/chown where supported.
- Review prompt for destructive/privileged changes.

Existing implementation to map:

- Move `FileBrowserPanel.tsx` from `Jobs / Files`.
- Keep current file command popovers, editor, upload/download, permission controls, and review prompts.

Execution tasks:

- [x] Move file browser under `Remote Operations / Files`.
- [x] Remove one-VPS file browsing from Jobs.
- [x] Link transfer outputs to `Remote Operations / Transfers`.

Verification:

- [x] File browser works for selected VPS, root path, empty directory, large directory, and permission-blocked states.
- [x] Destructive actions require review and privilege where applicable.

### Remote Operations / Transfers

Target UI/UX:

- Transfer sessions table.
- Source artifacts and download handoffs.
- Failed transfer retry action.
- Detail drawer: direction, source path, destination path, target, rate cap, checksum, last chunk, failure reason, retry eligibility.
- Actions: retry, download handoff, open related job.

Existing implementation to map:

- Move `FileTransferSessionsPanel.tsx` from `Jobs / Transfer history`.
- Preserve handoff creation and source artifact surfaces.

Execution tasks:

- [x] Move transfer history into `Remote Operations / Transfers`.
- [x] Add failed-transfer retry affordance that opens `Jobs / Dispatch` with resumable transfer settings prefilled.
- [x] Keep chunk-history as detail evidence only when available; do not overbuild it before retry.

Verification:

- [x] Completed download handoff, upload source artifact, failed transfer, and empty states are covered by tests and screenshots.
- [x] Retry review freezes prior target/path/security/rate metadata.

### Remote Operations / Processes

Target UI/UX:

- Process inventory table.
- Summary strip: running, failed, restarted, memory, logs available.
- Row actions: Logs, Restart, Stop.
- Detail drawer: PID, status, source job, cgroup/resource snapshot, restart attempts, stdout/stderr paths, last exit, observed time.
- Review prompt for restart/stop.

Existing implementation to map:

- Move `ProcessSupervisorInventoryPanel.tsx` from `Jobs / Processes`.
- Keep current health/resource/log columns and process detail evidence.

Execution tasks:

- [x] Move process supervisor into `Remote Operations / Processes`.
- [x] Add Logs/Restart/Stop row actions using reviewed dispatch presets once backend presets/callbacks exist.
- [x] Link long-term metrics to `Observability / Process Metrics`, not inline charts here.

Verification:

- [x] Process page does not render CPU/memory history charts as primary content.
- [x] Restart/stop actions require review and show target/process/effect.

### Remote Operations / Bulk files

Target UI/UX:

- Scope selector.
- Operation selector: upload, download, delete, chmod/chown, checksum/compare.
- Path policy.
- Preflight checklist: target availability, stale agents, privilege, size estimate, compatibility.
- Result pane: per-target outcome, retry candidates, artifacts.

Existing implementation to map:

- Move `MultiFileActionsPanel.tsx` from `Jobs / Multi files`.
- Keep recent preflight/post-run evidence work.

Execution tasks:

- [x] Move multi-file actions to `Remote Operations / Bulk files`.
- [x] Keep preflight pane and result pane as the release interaction model.
- [x] Remove bulk file operations from Jobs.

Verification:

- [x] 20+ VPS dense layout remains usable.
- [x] Preflight distinguishes known estimates from backend-unavailable exact data.

### Jobs / History

Target UI/UX:

- Filterable job table.
- Target results.
- Output viewer.
- Output comparison.
- Links to audit, artifacts, and related resource.

Existing implementation to map:

- Keep the history part from the former `JobHistoryPanel.tsx` inside the Jobs router.
- Keep job target/output/comparison loaders.
- Remove terminal/file/process/transfer/server-maintenance subpanels from Jobs.

Execution tasks:

- [x] Split `JobHistoryPanel.tsx` so Jobs / History contains only execution evidence. Resolved by renaming the mixed Jobs router to `JobsPanel.tsx`; no `JobHistoryPanel.tsx` module remains owning dispatch or scheduled-run routes, and release tests prove Jobs / History renders only execution evidence.
- [x] Keep `openJobDetails` routing to Jobs / History.
- [x] Link related Remote Operations pages but do not embed their workflows.

Verification:

- [x] Jobs / History does not render terminal, file browser, transfers, process supervisor, or artifact cleanup.
- [x] Job detail opens from Home, Remote Operations, Network, Backups, Automation, and Audit.

### Jobs / Dispatch

Target UI/UX:

- Advanced generic command composer.
- Target scope.
- Privilege review.
- Timeout/retry settings.
- Dry-run/impact preview where possible.
- Result summary.

Existing implementation to map:

- Keep `JobDispatchPanel.tsx`, `JobDispatchControls.tsx`, `JobOperationControls.tsx`, and `TargetImpactPreview.tsx`.
- Move terminal-open friendly composer to `Remote Operations / Terminal`.

Execution tasks:

- [x] Keep Jobs / Dispatch as the advanced power-user command surface.
- [x] Remove terminal-specific primary path from dispatch.
- [x] Add clear link from terminal composer to advanced dispatch for generic shell commands.

Verification:

- [x] Jobs / Dispatch remains capable of generic command dispatch.
- [x] Terminal-open creation is not the primary UI in Jobs / Dispatch.

### Jobs / Approvals

Target UI/UX:

- Pending approval queue.
- Scope, command/action, requester, privilege, age, risk.
- Actions: approve, reject, open detail.

Existing implementation to map:

- Current scheduled-run approval-like page in the Jobs router is not this. It should move to Jobs / Scheduled Runs.
- Add this page when approval queue semantics exist.

Execution tasks:

- [x] Create a placeholder/empty state for approval queue if no backend queue exists.
- [x] Wire real approval queue when backend exposes it.
  - Implemented on 2026-06-26 as persisted job approvals with `GET/POST /api/v1/job-approvals`, `POST /api/v1/job-approvals/{id}/approve`, and `POST /api/v1/job-approvals/{id}/reject`.
  - Approval creation verifies the operator privilege intent, freezes the normalized confirmed job request, strips privilege assertion material before persistence, and records requester/risk/fingerprint evidence.
  - Approval dispatch reuses the internal job creation path so target validation, capability checks, timeout policy, idempotency, audit, and dispatch records remain centralized.
  - Historical note: earlier backend audit on 2026-06-26 found no persisted approval queue model or approve/reject route, so the temporary release-safe state was an explicit placeholder plus separation from Jobs / Scheduled runs.

Verification:

- [x] Empty state explains that no reviewed work is waiting.
- [x] Real approval rows show status, command/action, scope, requester, privilege/risk, age, and expandable detail evidence.
- [x] Pending approval rows expose approve/reject actions that call audited backend endpoints.
- [x] Scheduled-run history is not confused with approvals.

### Jobs / Scheduled Runs

Target UI/UX:

- Automation execution history.
- Due, dispatched, completed, skipped, failed lifecycle.
- Worker evidence.
- Retry availability.
- Links to schedule and job output.

Existing implementation to map:

- Move `Jobs / approvals` schedule-run rendering from the old history module to `Jobs / Scheduled Runs`.
- Keep `SchedulesPanel.tsx` authoring separate under `Automation / Schedules`.

Execution tasks:

- [x] Rename release subpage to `Scheduled Runs`.
- [x] Keep lifecycle-specific scheduled-run row model.
- [x] Link to Automation / Schedules for schedule registry.

Verification:

- [x] Scheduled-run rows show automation lifecycle and do not expose schedule authoring controls.

### Jobs / Artifacts

Target UI/UX:

- Retained execution artifact table.
- Domains: job output, file transfer, backup, update.
- Size, hash, retention state, related job.
- Actions: download, open job, open source workflow.

Existing implementation to map:

- Pull job-output artifact downloads from the Jobs history/router code.
- Pull agent update release artifacts from `AgentUpdateReleasesPanel.tsx` only as linked artifact records.
- Pull file transfer source/download artifacts from `FileTransferSessionsPanel.tsx` only as artifact records.
- Pull backup artifacts from `BackupsPanel.tsx` only when showing cross-domain artifact inventory; keep backup-specific management in Backups / Artifacts.

Execution tasks:

- [x] Create a release Jobs / Artifacts inventory or reuse current artifact APIs when available.
- [x] Keep destructive cleanup out of this page; cleanup belongs in System / Maintenance.

Verification:

- [x] Artifact inventory links back to related job/workflow without hosting cleanup.

### Automation / Schedules

Target UI/UX:

- Schedule registry and editor.
- Target scope preview.
- Next due, last run, enabled state.
- Actions: create, edit, pause, run now, delete.

Existing implementation to map:

- Move top-level `SchedulesPanel.tsx` to `Automation / Schedules`.
- Keep reviewed enable/disable/apply/defer/delete flows.

Execution tasks:

- [x] Remove top-level `Schedules`.
- [x] Move schedule registry to Automation.
- [x] Ensure scheduled-run history links to Jobs / Scheduled Runs.

Verification:

- [x] Schedule authoring is only in Automation / Schedules.
- [x] Due-run execution history is only in Jobs / Scheduled Runs.

### Automation / Runbooks

Target UI/UX:

- Reusable reviewed operation catalog.
- Examples: restart service, collect diagnostics, rotate logs, update packages, check disk, run known maintenance.
- Parameters and review.
- Last run evidence.

Existing implementation to map:

- Derive from existing command templates in `JobsPanel.tsx` / `JobDispatchPanel.tsx`.
- Reuse dispatch/review infrastructure.

Execution tasks:

- [x] Promote command templates into a dedicated Runbooks page.
- [x] Add runbook cards/list with required parameters, target scope, review, and last-run evidence.

Verification:

- [x] Runbooks are not raw arbitrary commands; generic arbitrary commands stay in Jobs / Dispatch.

### Automation / Source Templates

Target UI/UX:

- Persistent runtime config templates and assignments.
- Render preview.
- Apply status.
- Diff/review.

Existing implementation to map:

- Move source template management from `ConfigPanel.tsx` and/or `SourceTemplatesPanel.tsx`.
- Keep runtime apply state links to Config.

Execution tasks:

- [x] Move persistent template registry to Automation / Source Templates.
- [x] Keep one-off per-VPS and bulk config mutation in Config.

Verification:

- [x] Persistent template authoring is not mixed with emergency bulk patching.

### Automation / Agent Updates

Target UI/UX:

- Registered releases.
- Current fleet versions.
- Canary/staging state.
- Rollout policy.
- Health checks.
- Rollback.

Existing implementation to map:

- Move `AgentUpdateReleasesPanel.tsx` from `Jobs / Update registry`.

Execution tasks:

- [x] Move agent update release registry into Automation.
- [x] Add rollout/canary/health/rollback placeholders or controls as backend permits.

Verification:

- [x] Update registry is not reachable under Jobs in release navigation.

### Network / Overview

Target UI/UX:

- Network health landing page.
- Tunnel health, latency/loss/speed summary, OSPF pending changes, recent network incidents, top affected VPSs.

Existing implementation to map:

- Build from `TopologyPanel.tsx`, `TopologyGraphPanel.tsx`, `TopologyNetworkTestControls.tsx`, `TopologyEvidencePanel.tsx`, dashboard network curves, and topology data.

Execution tasks:

- [x] Rename Topology area to Network.
- [x] Add Network / Overview as the entry point.

Verification:

- [x] No top-level `Topology` label remains.
- [x] Network Overview links to Graph, Tunnel Plans, Tests, OSPF, and Evidence.

### Network / Graph

Target UI/UX:

- Topology graph map.
- Selected tunnel/endpoint drawer.
- Health overlays and filters by provider/region/tag.

Existing implementation to map:

- Move `TopologyGraphPanel.tsx` to Network / Graph.

Execution tasks:

- [x] Keep graph visual inspection focused on topology; move mutation forms out.

Verification:

- [x] Graph page does not render OSPF apply forms or tunnel plan editors.

### Network / Tunnel Plans

Target UI/UX:

- Tunnel plan registry and editor.
- Endpoint allocation preview.
- Promotion state.
- Apply/rollback links.

Existing implementation to map:

- Move current tunnel plan authoring from `TopologyPanel.tsx` to Network / Tunnel Plans.
- Move promotion workflows to Network / Tunnel Plans or keep Network / Promotion only if release IA explicitly keeps it; preferred release shape folds Promotion into Tunnel Plans.

Execution tasks:

- [x] Fold `TopologyPromotionPanel.tsx` into Network / Tunnel Plans as a promotion tab/section.
- [x] Remove standalone Promotion subpage unless a future release decides it is needed.

Verification:

- [x] Plan creation, allocation preview, enable/disable, export, and promotion are reachable from Tunnel Plans.
- [x] Default Tunnel Plans stays registry-first and does not render create, promotion, generated-config, and latency/auto-OSPF workbenches all at once.
- [x] Create, promotion, generated-config, and latency/auto-OSPF workbenches open from explicit controls and expose visible close buttons.

### Network / Tests

Target UI/UX:

- Probe test, speed test, status check.
- Safety caps.
- Recent trend charts.
- Attach evidence affordance.

Existing implementation to map:

- Move `TopologyNetworkTestControls.tsx` to Network / Tests.

Execution tasks:

- [x] Keep diagnostic tools and trend charts.
- [x] Keep mutation-free diagnostic framing.

Verification:

- [x] Tests page does not edit tunnel plans or OSPF costs directly.

### Network / OSPF

Target UI/UX:

- Recommendation table.
- Cost change review.
- Evidence: probe/speed/confidence.
- Apply action.
- Rollback action.
- Post-apply verification checklist.

Existing implementation to map:

- Move `TopologyOspfUpdateControls.tsx` to Network / OSPF.

Execution tasks:

- [x] Add rollback action when OSPF workflow exposes lifecycle hooks.
- [x] Keep apply/review evidence from current OSPF panel.

Verification:

- [x] OSPF apply and rollback are reviewed, auditable, and scoped to selected tunnel.

### Network / Evidence

Target UI/UX:

- Network event timeline.
- Probe/speed observations.
- Job output links.
- Compare to previous.

Existing implementation to map:

- Move `TopologyEvidencePanel.tsx` to Network / Evidence.

Execution tasks:

- [x] Keep evidence page read-mostly.
- [x] Link to Tests, Graph, Tunnel Plans, and OSPF where action is required.

Verification:

- [x] Evidence page does not host primary mutation controls.

### Backups / Overview

Target UI/UX:

- Backup coverage.
- Failed/stale backups.
- Last restore verification.
- Storage usage.
- Policy health.

Existing implementation to map:

- Keep backup overview work in `BackupsPanel.tsx`.

Execution tasks:

- [x] Make Overview the default Backups page.
- [x] Ensure it leads with recoverability posture before forms/history.

Verification:

- [x] A user can tell whether recovery is trustworthy without opening subpages.

### Backups / Requests

Target UI/UX:

- Backup run history table.
- Status, target, policy, size, duration, artifact, evidence.

Existing implementation to map:

- Keep request history from `BackupHistoryTables.tsx` and `BackupRequestForm.tsx`.

Execution tasks:

- [x] Keep request creation/review close to request history or as a drawer.

Verification:

- [x] Request history does not contain policy editing or restore execution.

### Backups / Policies

Target UI/UX:

- Policy registry/editor.
- Retention/prune preview.
- Schedule linkage.

Existing implementation to map:

- Keep `BackupPolicyForm.tsx` and `BackupPolicyPruneForm.tsx`.

Execution tasks:

- [x] Keep policy authoring separate from restore and artifact upload.

Verification:

- [x] Policy prune preview is reviewed before mutation.

### Backups / Artifacts

Target UI/UX:

- Backup artifact inventory.
- Upload/import form.
- Metadata, size, hash, source, retention.
- Handoff creation.

Existing implementation to map:

- Keep backup artifacts from `BackupsPanel.tsx`, `ArtifactUploadForm.tsx`, `RestoreArchiveTransferSelect.tsx`.

Execution tasks:

- [x] Keep backup-specific artifact handling here.
- [x] Link cross-domain artifact inventory to Jobs / Artifacts where needed.

Verification:

- [x] Backup artifact upload/handoff is not mixed with job-output artifact cleanup.

### Backups / Restore

Target UI/UX:

- Restore planner.
- Target selection.
- Artifact selection.
- Impact review.
- Run restore.
- Verification status.
- Rollback.

Existing implementation to map:

- Keep `RestorePlanForm.tsx`, `RestoreRunForm.tsx`, and `RestoreRollbackForm.tsx`.

Execution tasks:

- [x] Keep restore as a guided workflow.
- [x] Expose verification and rollback state clearly.

Verification:

- [x] Restore plan/run/rollback each have explicit review states and evidence links.

### Backups / Migration

Target UI/UX:

- Source VPS, destination VPS.
- Backup/restore plan.
- DNS/network/config checklist.
- Cutover evidence.

Existing implementation to map:

- Keep `MigrationLinkForm.tsx` and migration sections in `BackupsPanel.tsx`.

Execution tasks:

- [x] Reframe migration as a checklist workflow instead of isolated link/run forms.

Verification:

- [x] Migration page shows cutover checklist and related evidence.

### Config / Overview

Target UI/UX:

- Config health.
- Drift summary.
- Recent changes.
- Template coverage.
- Apply-state summary.

Existing implementation to map:

- Keep relevant overview from `ConfigPanel.tsx`.
- Move source template registry to Automation / Source Templates.

Execution tasks:

- [x] Rebuild Config Overview around drift and risk, not around every config tool.

Verification:

- [x] Config Overview links to Per-VPS, Bulk Patch, Templates, and Rules without embedding their full editors.

### Config / Per-VPS Config

Target UI/UX:

- VPS selector.
- Current config.
- Structured fields.
- Diff.
- Validate.
- Review/apply.

Existing implementation to map:

- Move `ConfigPanel.tsx` subpage `single`.

Execution tasks:

- [x] Preserve one-VPS guarded override workflow.

Verification:

- [x] One-VPS config flow does not include fleet-wide bulk patch controls.

### Config / Bulk Patch

Target UI/UX:

- Scope selector.
- Patch generator.
- Affected sections.
- Validation.
- Review.
- Apply result.

Existing implementation to map:

- Move `ConfigPanel.tsx` subpage `bulk`.

Execution tasks:

- [x] Preserve target preview and review.

Verification:

- [x] Bulk patch requires reviewed scope and privilege.

### Config / Templates

Target UI/UX:

- Template table.
- Template editor.
- Assignments.
- Render preview.
- Apply state.

Existing implementation to map:

- This page links to Automation / Source Templates as the persistent registry.
- If kept in Config, it should be a read/apply view, not the primary authoring registry.

Execution tasks:

- [x] Decide during implementation whether Config / Templates is a link/summary or a full mirrored page. Preferred: summary plus link to Automation / Source Templates.

Verification:

- [x] Persistent template authoring has one canonical home.

### Config / Rules

Target UI/UX:

- Rule table.
- VPS values.
- Bulk edit.
- Validation.
- Affected alerts.

Existing implementation to map:

- Move `ConfigPanel.tsx` subpage `rules`.

Execution tasks:

- [x] Keep traffic/accounting rules in Config.
- [x] Link alert policy effects to Observability / Alerts.

Verification:

- [x] Rule edits show affected alert policy context.

### Observability / Fleet Metrics

Target UI/UX:

- CPU/memory/disk/network charts.
- Group by provider/tag/region.
- Time range.
- Top resource list.

Existing implementation to map:

- Move deeper metric charts from `DashboardPanel.tsx`, `FleetWorkspace.tsx`, and system/fleet telemetry hooks.
- Keep Komari cards in Home/Fleet Monitor; Observability is for analysis.

Execution tasks:

- [x] Create Observability top-level area.
- [x] Move analytical metrics out of action pages.

Verification:

- [x] Observability charts have time range, legend, hover values, and empty/stale states.

### Observability / Network Metrics

Target UI/UX:

- Latency/loss/speed charts.
- Tunnel grouping.
- Endpoint comparison.
- Incident overlays when available.

Existing implementation to map:

- Reuse network trend charts from `TopologyNetworkTestControls.tsx` and `TopologyEvidencePanel.tsx`.

Execution tasks:

- [x] Duplicate no mutation controls here; link to Network / Tests and OSPF for action.

Verification:

- [x] Network Metrics is chart/read-first and mutation-free.

### Observability / Process Metrics

Target UI/UX:

- CPU/memory/restart trends.
- Process grouping.
- Recent exits.
- Links to process actions.

Existing implementation to map:

- Current `ProcessSupervisorInventoryPanel.tsx` does not expose long-term history. Keep this page as empty/unavailable until backend supports it.

Execution tasks:

- [x] Add honest unavailable/empty state for process history until backend exists.

Verification:

- [x] Page does not invent process history data.

### Observability / Alerts

Target UI/UX:

- Alert policy groups, rule rows, selector previews, and reviewed saves.
- Policy-issued alert evidence and active fleet-alert context.
- Notification channels, alert delivery previews, and retained delivery evidence.
- Link to `Fleet / Alerts` for live acknowledge, mute, escalate, and clear triage.

Existing implementation to map:

- Move fleet alert policies, notification channels, alert notification deliveries, and policy alerts from `FleetWorkspace.tsx`.
- Link active alert triage from `Fleet / Alerts`; do not duplicate the live triage queue.

Execution tasks:

- [x] Extract policy and notification-channel authoring from Fleet.
- [x] Add Config / Rules and Fleet / Alerts routes to `Observability / Alerts`.
- [x] Keep webhook rules out of Alerts so alert routing and webhook automation remain separate.

Verification:

- [x] Active alert triage stays in Fleet / Alerts; policy/channel authoring is in Observability / Alerts.
- [x] Webhook rule controls do not render in Fleet / Alerts or Observability / Alerts.

### Observability / Webhooks

Target UI/UX:

- Webhook expression rules, target URL/template authoring, dry-run, and reviewed saves.
- Webhook queue dispatch, delivery processing, retained delivery evidence, and failure state.
- Delivery-history retention maintenance with reviewed cleanup.
- Webhooks are a first-class page, not a tab inside Alerts and not part of an Incidents page.

Existing implementation to map:

- Move webhook rules, webhook deliveries, dispatch/process controls, and delivery maintenance from `FleetWorkspace.tsx`.
- Keep alert notification channels under `Observability / Alerts` even when their delivery kind is webhook.

Execution tasks:

- [x] Extract webhook rule, dispatch, delivery, and maintenance controls into `Observability / Webhooks`.
- [x] Remove `Observability / Incidents` from navigation and routing.
- [x] Delete stale incident timeline panel code.

Verification:

- [x] Observability nav exposes separate `Alerts` and `Webhooks` pages.
- [x] Webhooks page owns webhook rules, deliveries, and retention maintenance.
- [x] No `Incidents` subpage is reachable from the console.

### Observability / Dashboards

Target UI/UX:

- Saved dashboards.
- Widget layout.
- Chart/table/card widgets.
- Share/export where applicable.

Existing implementation to map:

- Move custom dashboard preferences from `DashboardPanel.tsx` and dashboard preference APIs.

Execution tasks:

- [x] Create custom dashboard manager after core workflows are stable.

Verification:

- [x] Saved dashboards do not contain privileged mutation actions.

### Audit / Events

Target UI/UX:

- Operator/security event log.
- Filters: actor, action, resource, time, severity.
- Detail drawer.

Existing implementation to map:

- Keep `AuditLogPanel.tsx` subpage `events`.

Execution tasks:

- [x] Keep Audit events read-only.

Verification:

- [x] Audit / Events does not render mutation controls.

### Audit / Job Evidence

Target UI/UX:

- Jobs with audit context.
- Actor, privilege, target scope, output artifact, approval.

Existing implementation to map:

- Compose from `AuditLogPanel.tsx` and `JobHistoryPanel.tsx` data.

Execution tasks:

- [x] Add job-audit correlation page.

Verification:

- [x] A user can prove who ran what without leaving Audit.

### Audit / Sessions

Target UI/UX:

- Terminal sessions.
- Open/close/attach/replay events.
- Transcript links.
- Actor/target evidence.

Existing implementation to map:

- Compose from `TerminalSessionsPanel.tsx`, operator session data in `SystemPanel.tsx`, and audit events.

Execution tasks:

- [x] Add terminal/session audit correlation view.
- [x] Link live terminal operations to this audit page.

Verification:

- [x] Audit / Sessions does not render the live terminal emulator.

### Audit / Retention & Export

Target UI/UX:

- Retention policy.
- Export controls.
- Prune preview.
- Storage counts.

Existing implementation to map:

- Keep `AuditLogPanel.tsx` subpage `retention`.

Execution tasks:

- [x] Keep evidence retention separate from artifact cleanup in System / Maintenance.

Verification:

- [x] Retention/export controls explain scope and preview impact.

### Access / Overview

Target UI/UX:

- Operator count.
- Active sessions.
- MFA/TOTP posture.
- Gateway identity posture.
- Privilege unlock state.

Existing implementation to map:

- Move access posture from `AccessPanel.tsx` and user/session posture from `SystemPanel.tsx`.

Execution tasks:

- [x] Make Access Overview the entry point for authority posture.

Verification:

- [x] Access Overview links to Operators, VPS Identities, Gateway Sessions, and Privilege Vault.

### Access / Operators

Target UI/UX:

- User table.
- Roles/scopes.
- MFA/TOTP state.
- Password reset.
- Revoke sessions.

Existing implementation to map:

- Move `SystemPanel.tsx` subpage `users` into Access / Operators.
- Keep operator creation/update/status/password/TOTP/session review prompts.

Execution tasks:

- [x] Remove operator user management from System.
- [x] Move to Access / Operators.

Verification:

- [x] User management is not reachable from System in release navigation.

### Access / VPS Identities

Target UI/UX:

- Agent/client key lifecycle.
- Register/rotate/revoke.
- Install command.
- Connection evidence.

Existing implementation to map:

- Move `AccessPanel.tsx` subpage `clients`.

Execution tasks:

- [x] Rename VPS keys to VPS Identities.

Verification:

- [x] Human operators and VPS identities are separate pages.

### Access / Gateway Sessions

Target UI/UX:

- Gateway sessions.
- Agent stream state.
- Control-plane routing.
- Disconnect/revoke controls when supported.

Existing implementation to map:

- Move `AccessPanel.tsx` subpage `gateway`.

Execution tasks:

- [x] Keep gateway sessions separate from terminal sessions and operator sessions.

Verification:

- [x] Gateway Sessions page does not show terminal transcript controls.

### Access / Privilege Vault

Target UI/UX:

- Unlock form.
- Current privilege state.
- Lock action.
- Scope explanation.
- Safety notes.

Existing implementation to map:

- Move `AccessPanel.tsx` subpage `privilege`.

Execution tasks:

- [x] Keep privilege unlock as the canonical authority surface.

Verification:

- [x] Every privileged workflow links to Access / Privilege Vault when locked.

### System / Overview

Target UI/UX:

- API, gateway, worker, database, object store status.
- Queue health.
- Error rate.
- Recent service events.

Existing implementation to map:

- Move `SystemPanel.tsx` subpage `dashboard` into System / Overview.

Execution tasks:

- [x] Keep platform health separate from fleet monitoring.

Verification:

- [x] System Overview does not show Komari-style VPS cards.

### System / Capacity

Target UI/UX:

- Queue depth.
- Dispatch capacity.
- Artifact storage.
- Retention pressure.
- Worker lag.

Existing implementation to map:

- Extract capacity cards/charts from `SystemPanel.tsx` dashboard.

Execution tasks:

- [x] Create System / Capacity as a separate page when dashboard content is split.

Verification:

- [x] Capacity page focuses on control-plane limits, not VPS CPU/memory.

### System / Suite Config

Target UI/UX:

- Section rail.
- Structured fields.
- Advanced TOML.
- Validation.
- Redacted diff.
- Review/save.

Existing implementation to map:

- Move `SystemPanel.tsx` subpage `config`.

Execution tasks:

- [x] Keep suite config in System / Suite Config.

Verification:

- [x] Suite config does not contain per-VPS runtime config editors.

### System / Maintenance

Target UI/UX:

- Artifact cleanup dry-run.
- Object-store health.
- Prune history.
- Queue cleanup.
- Maintenance jobs.

Existing implementation to map:

- Move `ServerJobsPanel.tsx` from `Jobs / Server jobs`.
- Move server-side artifact cleanup here.

Execution tasks:

- [x] Remove user-facing `Server jobs` page from Jobs.
- [x] Reframe server-side cleanup as System / Maintenance.

Verification:

- [x] Artifact cleanup requires dry-run and typed confirmation.
- [x] Maintenance page does not show ordinary job history as primary content.

### System / Preferences

Target UI/UX:

- Display density.
- Timezone/language.
- Name display.
- Sidebar behavior.
- Inline vs overlay review prompts.
- Dashboard preferences.

Existing implementation to map:

- Move `SystemPanel.tsx` subpage `operator`, `PreferencesPanel.tsx`, and operator preference controls.
- Move shared operational defaults out of Preferences when backend settings exist; until then label them explicitly.

Execution tasks:

- [x] Keep Preferences personal by default.
- [x] Explicitly label any operator-stored shared defaults until backend shared settings exist.

Verification:

- [x] Inline vs overlay review prompt preference is visible here.
- [x] Shared fleet/system defaults are not silently presented as personal preferences.

### Auth

Target UI/UX:

- Login/session unlock remains outside main navigation.
- Auth errors, session vault availability, and unlock state are explicit.

Existing implementation to map:

- Keep `AuthPanel.tsx`.

Execution tasks:

- [x] Ensure release shell does not render product navigation before authentication when auth is required.

Verification:

- [x] Auth-required state is keyboard accessible and screen-reader labeled.

### Current implementation source map

Use this as the deterministic migration index:

| Current source | Current route/role | Release target | Disposition |
|---|---|---|---|
| `DashboardPanel.tsx` | Dashboard overview | Home; Observability dashboards/metrics; Network summary | Split and rename |
| `FleetWorkspace.tsx` | Fleet instances, alerts, policies, notifications, selected detail | Fleet / Instances, Monitor, Groups links, Alerts, Instance detail; Observability / Alerts and Webhooks | Split heavily |
| `TagsPanel.tsx` | Tags top-level | Fleet / Groups | Move and rename |
| `ConfigPanel.tsx` | Config overview/bulk/single/rules/templates | Config / Overview, Per-VPS, Bulk Patch, Rules; Automation / Source Templates | Split |
| `SourceTemplatesPanel.tsx` | Source templates | Automation / Source Templates | Move or merge |
| `JobsPanel.tsx` (formerly `JobHistoryPanel.tsx`) | Jobs history plus dispatch/scheduled routing | Jobs / History, Dispatch, Scheduled Runs; related Remote Operations pages and System / Maintenance stay outside Jobs | Split and rename |
| `JobDispatchPanel.tsx` | Generic dispatch | Jobs / Dispatch | Keep, remove terminal-primary role |
| `FileBrowserPanel.tsx` | Jobs / Files | Remote Operations / Files | Move |
| `MultiFileActionsPanel.tsx` | Jobs / Multi files | Remote Operations / Bulk files | Move |
| `FileTransferSessionsPanel.tsx` | Jobs / Transfers | Remote Operations / Transfers | Move |
| `TerminalSessionsPanel.tsx` | Jobs / Terminal sessions | Remote Operations / Terminal; Audit / Sessions evidence links | Move and extend |
| `ProcessSupervisorInventoryPanel.tsx` | Jobs / Processes | Remote Operations / Processes; Observability / Process Metrics links | Move and extend |
| `ServerJobsPanel.tsx` | Jobs / Server jobs | System / Maintenance | Move and reframe |
| `automation/AgentUpdateReleasesPanel.tsx` | Automation / Agent Updates | Automation / Agent Updates; Jobs / Artifacts links | Moved |
| `SchedulesPanel.tsx` | Schedules top-level | Automation / Schedules | Move |
| `TopologyPanel.tsx` | Topology shell | Network shell | Rename/rebuild |
| `TopologyGraphPanel.tsx` | Topology / Graph | Network / Graph | Move |
| `TopologyNetworkTestControls.tsx` | Topology / Tests | Network / Tests; Observability / Network Metrics charts | Move/link |
| `TopologyOspfUpdateControls.tsx` | Topology / OSPF | Network / OSPF | Move and add rollback |
| `TopologyEvidencePanel.tsx` | Topology / Evidence | Network / Evidence; Observability / Network Metrics | Move/link |
| `TopologyPromotionPanel.tsx` | Topology / Promotion | Network / Tunnel Plans | Fold in |
| `BackupsPanel.tsx` and `panels/backups/*` | Backups pages/forms | Backups / Overview, Requests, Policies, Artifacts, Restore, Migration | Keep, split more clearly |
| `AuditLogPanel.tsx` | Audit events/retention | Audit / Events, Job Evidence, Sessions, Retention & Export | Keep and extend |
| `AccessPanel.tsx` | Access overview/clients/gateway/privilege | Access / Overview, VPS Identities, Gateway Sessions, Privilege Vault | Keep and rename |
| `SystemPanel.tsx` | System dashboard/users/sessions/config/preferences | System / Overview, Capacity, Suite Config, Preferences; Access / Operators; Audit / Sessions | Split heavily |
| `PreferencesPanel.tsx` | Preferences | System / Preferences | Move/merge |
| `ConsoleShell.tsx`, `constants.ts`, `App.tsx`, `types.ts` | Global IA/routing | Release IA shell | Rebuild |

### Required release build order

1. Shell and route model
   - [x] Replace top-level routes with release product areas.
   - [x] Update breadcrumbs, page headers, mobile selector, saved scope controls, and route helpers.
   - [x] Add command-palette search index.

2. Home and Fleet
   - [x] Build Home.
   - [x] Build Fleet / Instances.
   - [x] Build Fleet / Monitor with Komari-style cards.
   - [x] Move Tags into Fleet / Groups.
   - [x] Split active alerts from alert policy/routing authoring.
   - [x] Create canonical VPS detail.

3. Remote Operations
   - [x] Move Terminal, Files, Transfers, Processes, and Bulk files out of Jobs.
   - [x] Add new terminal composer.
   - [x] Add transfer retry flow.
   - [x] Add process Logs/Restart/Stop reviewed actions.
   - [x] Add terminal transcript copy/download when backend support exists.

4. Jobs cleanup
   - [x] Reduce Jobs to History, Dispatch, Approvals, Scheduled Runs, and Artifacts.
   - [x] Remove Server jobs, Terminal, Files, Transfers, Processes, Multi files, and Update registry from Jobs.

5. Automation
   - [x] Move schedules to Automation.
   - [x] Promote command templates to Runbooks.
   - [x] Move source templates to Automation.
   - [x] Move agent updates to Automation.

6. Network
   - [x] Rename Topology to Network.
   - [x] Add Network Overview.
   - [x] Move Graph, Tunnel Plans, Tests, OSPF, and Evidence.
   - [x] Fold Promotion into Tunnel Plans.
   - [x] Add OSPF rollback workflow when backend supports it.

7. Backups, Config, Observability, Audit, Access, System
   - [x] Re-split backup pages into posture/history/policy/artifact/restore/migration. Resolved as the release shape `Backups / Overview`, `Requests`, `Policies`, `Artifacts`, `Restore`, and `Migration`: Overview carries posture, Requests carries run history, and the focused Backups release-IA tests prove role separation.
   - [x] Split Config and Automation template ownership.
   - [x] Add Observability pages.
   - [x] Add Audit job/session correlation pages.
   - [x] Move operator management from System to Access.
   - [x] Move server maintenance from Jobs to System.

8. Legacy removal
   - [x] Delete old route labels and dead aliases. `releaseDestination` now accepts release `ActiveView` values only, test helpers no longer translate old top-level pages, and scans show no `Dashboard` / `Tags` / `Schedules` / `Topology` route calls remain.
   - [x] Remove old page-specific tests or rewrite them to release routes. Screenshot review, live Docker route matrices, navigation helpers, dispatch, terminal, transfer, layout, and visual-audit route calls now target release pages directly.
   - [x] Remove old copy that names server-side implementation concepts as product pages. User-facing copy now uses Home, Network evidence, alert overlays, control-plane handoffs, shared telemetry, and reviewed maintenance terminology instead of legacy dashboard/topology/incident/server-side page wording.

### Release verification plan

Static IA checks:

- [x] `ActiveView` contains only release top-level product areas.
- [x] `viewSubpages` contains every release subpage listed in this plan.
- [x] Navigation contains no old top-level labels: `Dashboard`, `Tags`, `Schedules`, `Topology`.
- [x] Jobs subpages contain only History, Dispatch, Approvals, Scheduled Runs, and Artifacts.
- [x] Remote Operations contains Terminal, Files, Transfers, Processes, and Bulk files.
- [x] System contains Maintenance and does not contain operator user management.
- [x] Access contains Operators and does not contain suite config.

Behavior checks:

- [x] Every primary action from Home routes to the release page with preserved scope.
- [x] Every VPS row/card can open Terminal, Files, Processes, Backup, Network, and VPS detail.
- [x] Terminal open/resume works without visiting Jobs.
- [x] File browsing works without visiting Jobs.
- [x] Transfer retry review freezes target/path/security/rate evidence and opens Jobs / Dispatch with resumable transfer retry settings prefilled.
- [x] Process Logs/Restart/Stop actions are reviewed and auditable.
- [x] OSPF apply/rollback actions are reviewed and auditable.
- [x] Schedule authoring is in Automation; scheduled execution evidence is in Jobs.
- [x] Artifact cleanup is in System / Maintenance; audit retention is in Audit / Retention & Export.

Visual and UX checks:

- [x] Desktop and mobile screenshots exist for every top-level page. Verified by the structured screenshot suite with 64 current release captures per project under `output/playwright/structured/{desktop-chrome,mobile-chrome}`.
- [x] Desktop and mobile screenshots exist for every Remote Operations subpage. Verified by structured screenshots for Terminal, Files, Transfers, Processes, and Bulk files on desktop and mobile.
- [x] Komari-style cards render correctly at 0, 3, 20, 50, and 100 VPS fixtures. Verified by focused release IA fixture-count tests.
- [x] Tables have filters, sorting, row actions, detail drawers, empty states, and disabled reasons. Verified through the shared `ConsoleDataGrid` release pages plus focused dense-table, row-action, config, artifact, group, and screenshot coverage.
- [x] Review prompts honor inline vs overlay preference.
- [x] No card contains a destructive action directly; destructive actions open review. Verified by focused bulk group mutation, fleet alert triage, config review, artifact read-only, file-operation, and maintenance review tests.
- [x] No text overflows in navigation, cards, tables, buttons, or review prompts. Verified by the full structured desktop/mobile screenshot suite after fixing mobile `Config / Templates` overflow.
- [x] Advanced labels have tooltips or inline help. Verified by release IA Playwright coverage for Config / Bulk patch, Config / Per-VPS, Config / Rules, and System / Suite config expert labels, plus refreshed structured screenshots.

Accessibility checks:

- [x] Global navigation, subnavigation, command palette, fleet scope selector, and detail drawers are keyboard reachable. Verified by desktop and mobile release shell keyboard tests plus command palette route tests.
- [x] Focus order follows page structure. Verified by release shell tab-order traversal to navigation, scope/search, privilege control, and page primary action.
- [x] Status is not conveyed by color alone. Status badges and cards carry visible text labels in the focused release IA and screenshot coverage.
- [x] Form labels and destructive action labels are explicit. Verified by label-based Playwright coverage for config, fleet groups, file operations, alert triage, and reviewed mutation prompts.
- [x] Disabled buttons explain their disabled reason. Verified by a desktop/mobile release IA scan across Jobs / Scheduled runs, Config, Observability, and System pages; shared table pagination, Config gates, and System review-save disabled states now expose reason titles.
- [x] WCAG AA contrast is preserved for body text, labels, badges, and disabled/help text. Verified by a rendered Playwright contrast calculation across representative release pages; disabled control/menu styling now uses readable muted text without opacity loss.

Regression gates:

- [x] `npm exec tsc -- --noEmit`
- [x] `npm exec vite -- build`
- [x] Impeccable detector on modified UI surfaces returns `[]`.
- [x] Focused Playwright coverage for every release top-level page.
- [x] Structured screenshot suite for desktop and mobile release routes.
- [x] `git diff --check`

Completion definition:

- [x] A production operator can reach the five SSH/VNC/SCP replacement workflows from Remote Operations without using Jobs.
- [x] A production operator can scan the fleet in Komari-style cards from Home and Fleet / Monitor.
- [x] A production operator can still use dense tables for canonical inventory, evidence, audit, config, and backups.
- [x] Product pages match their roles: monitoring pages monitor, operation pages operate, evidence pages prove, config pages review/apply, access pages manage authority, system pages manage the control plane.
- [x] The release UI contains no legacy top-level IA concepts that conflict with the new shape.

### Release plan boundary note

This appended release IA block is the canonical target shape for the pre-release refactor. The older sections above are historical audit findings, resolved issues, and legacy critique context.

When any older issue wording conflicts with this release IA contract, use this appended release plan as the source of truth. The unchecked bullets in this block are implementation and verification gates for the new release shape, not stale visual backlog items.

## Explicit boundary: new release-shape work starts here

Appended 2026-06-25 to avoid confusion with the older screenshot audit and resolved issue inventory above.

This marker is intentionally last in the file. Treat everything from `Release IA refactor plan -- new intended shape, implementation map, and verification` through this boundary as the active pre-release product contract. Treat the older issue inventory above that release-plan heading as historical evidence and prior critique, not as the current shape to preserve.

Operational rules for future work:

- Do not add new release IA tasks to the old audit sections.
- Do not infer product intent from old weak layouts, old route names, or old compatibility behavior.
- Do not keep old top-level concepts such as Dashboard, Tags, Schedules, or Topology when they conflict with the release IA contract.
- Add new scope, page-shape, execution, and verification work under the release IA plan, or append a new dated release-plan subsection below this marker.
- When an implementation closes a release-plan gap, mark the matching release checkbox and record verification evidence in `PROGRESS.md`.
- When an old issue conflicts with this release plan, the release plan wins.
- The design standard remains a dense, production VPS console in the style of Cloudflare Dashboard and Google Cloud Console: explicit scope, clear resource identity, compact expert workflows, reviewed privileged actions, evidence trails, keyboard-reachable controls, and no decorative or marketing-style console surfaces.

## Appended release-contract separation note -- 2026-06-25

This section is appended deliberately so the active release-shape work cannot be mistaken for the older audit backlog.

The active pre-release work is:

- The release IA refactor plan beginning at `## Release IA refactor plan -- new intended shape, implementation map, and verification`.
- Any dated release-plan subsection appended after that heading.
- The execution, verification, and behavior checkboxes inside that release-plan block.

The older sections above the release IA heading are historical source material only. They may explain why the product is being reshaped, but they are not the current product contract and must not be used to preserve old page structures, old route names, old visual hierarchy, or weak compatibility behavior.

Rules for future edits:

- Append new release-shape issues below the release IA heading or below this note, never inside the old audit inventory.
- Mark release-shape work complete only by checking the matching release-plan bullet and recording evidence in `PROGRESS.md`.
- If an old issue and the release IA plan disagree, implement the release IA plan.
- If a useful old issue is still valid, restate it as a release-plan execution or verification bullet before working on it.
- Do not document old poor designs as intentional product behavior. Intentional behavior must come from the release IA plan, current implementation requirements, or explicit user instruction.

## Active release issue ledger -- appended 2026-06-25

This is the current issue ledger for release-shape execution. It is appended at the end of `ISSUES.md` so it cannot be confused with the older screenshot audit, resolved critique notes, or compatibility-era backlog above.

Interpretation rules:

- Items before `## Release IA refactor plan -- new intended shape, implementation map, and verification` are historical input only unless they are restated in this active release ledger.
- Unchecked boxes in the old audit inventory are not automatically active release blockers.
- New work must be recorded as a release IA execution, verification, or behavior gate in this ledger.
- When picking work, prefer the unchecked release IA gates here over older issue prose.
- When closing work, check the matching release IA gate here and record concise evidence in `PROGRESS.md`.
- If an old issue is still useful, copy its desired outcome into this ledger first, using the new Cloudflare/GCP-style release shape rather than preserving the old design.
- If old wording and this ledger conflict, this ledger wins.

Active release work continues from the checklists in the release IA block above. Future release-shape additions should be appended below this heading with dated notes, not inserted into the historical audit sections.

## Current release contract marker -- appended 2026-06-25

This marker is intentionally appended after every older audit, resolved issue,
and prior boundary note. Use it as the latest interpretation rule for this
file.

Current source of truth:

- The active product target is the `Release IA refactor plan -- new intended
  shape, implementation map, and verification` section plus the active release
  issue ledger appended after it.
- New release-shape work belongs in this active ledger as explicit execution,
  behavior, or verification gates.
- Completed release-shape work must be checked in the release ledger and backed
  by concise evidence in `PROGRESS.md`.

Historical-only material:

- Older screenshot-audit findings, critique notes, stale visual issues,
  compatibility-era route names, and resolved issue inventories above the
  release IA heading are context only.
- They are not product requirements unless the desired outcome is restated in
  the active release ledger.
- Unchecked boxes in old sections are not automatically release blockers.

Conflict rule:

- If an old issue conflicts with the release IA plan, implement the release IA
  plan.
- If old wording is still useful, rewrite it as a current release-shape gate
  before implementation.
- Do not treat old weak layouts, old page names, or old compatibility behavior
  as intentional design.

Design rule:

- The standard remains a dense production VPS console shaped like Cloudflare
  Dashboard or Google Cloud Console: clear resource scope, compact expert
  workflows, strong page role separation, reviewed privileged actions,
  evidence-oriented navigation, keyboard-reachable controls, responsive
  behavior, and production-safe visual hierarchy.

## Final active-issues boundary -- appended 2026-06-25

This is the newest boundary marker for `ISSUES.md`. It exists specifically so
the current release-shape work is not confused with older audit findings,
resolved critique notes, stale screenshots, compatibility-era routes, or old
unchecked backlog items.

Active release work:

- The source of truth is the `Release IA refactor plan -- new intended shape,
  implementation map, and verification` section plus dated release-ledger
  sections appended after it.
- New intended page shapes, execution tasks, verification tasks, backend/API
  workflow gaps, and UI behavior gates must be recorded under the release IA
  plan or under a dated subsection appended after this marker.
- Unchecked items in older audit sections are not release blockers unless they
  are explicitly restated as release IA execution or verification gates.
- Resolved old issues remain historical evidence only; they must not be used to
  preserve weak layouts, old route names, old page organization, or
  compatibility behavior.

Closure rule:

- Close release-shape work by checking the matching release IA gate and
  recording concise verification evidence in `PROGRESS.md`.
- If an old issue is still valuable, migrate its desired outcome into the
  release IA plan first, using the Cloudflare/GCP-style VPS console shape.
- If any old issue conflicts with the release IA plan or this marker, implement
  the release IA plan.

## Canonical active-release marker -- appended 2026-06-26

This is the newest controlling note in `ISSUES.md`. It is appended explicitly so
the release IA refactor, new intended page shapes, and current verification
gates are not confused with older screenshot-audit issues, resolved critique
notes, stale compatibility work, or historical backlog text above.

Use this interpretation for all future release work:

- Active issues are only the release IA refactor plan, dated release-ledger
  sections, and explicit execution/verification/behavior gates appended under
  this active-release area.
- Old audit findings are source evidence, not implementation instructions.
  They become active only after their desired outcome is rewritten as a current
  release IA gate.
- Do not preserve old page names, old route organization, old card layouts, old
  visual hierarchy, or compatibility behavior because it appears in an older
  issue section.
- When old issue wording conflicts with the release IA plan, the release IA
  plan wins.
- When new work is discovered, append it as a dated release-shape gate here or
  under the release IA plan before implementing it.
- When work is completed, check the matching release IA gate and record concise
  verification evidence in `PROGRESS.md`.

The product standard remains the confirmed release target: a dense production
VPS management console aligned with Cloudflare Dashboard and Google Cloud
Console patterns, with explicit scope, role-separated pages, compact expert
workflows, reviewed privileged actions, evidence trails, accessible controls,
responsive layouts, and no weak legacy design treated as intentional product
behavior.

## Active-only release ledger boundary -- appended 2026-06-26 00:54 +08

This appended boundary is the current reading rule for `ISSUES.md`. It exists
to prevent the active release-shape plan from being confused with old audit
items, old screenshots, resolved critique notes, compatibility-era route names,
or stale unchecked backlog items earlier in the file.

Active release work is only:

- The section `Release IA refactor plan -- new intended shape, implementation
  map, and verification`.
- Dated release-ledger sections appended after that release IA section.
- Explicit execution, behavior, backend/API workflow, UI/UX, accessibility, and
  verification gates that are restated under those active release sections.

Historical material is:

- Any screenshot audit, critique issue, old page-shape note, stale route name,
  old compatibility behavior, or resolved backlog text that appears before the
  release IA refactor plan.
- Any unchecked old issue that has not been restated as an active release gate.

Conflict rule:

- If an older issue conflicts with the release IA plan or this boundary, the
  release IA plan wins.
- If an older issue remains useful, rewrite its desired outcome here as a
  current Cloudflare/GCP-style release gate before implementation.
- Do not preserve weak legacy layouts, old product organization, or old visual
  behavior merely because they appear in historical issue text.

Execution rule:

- Add new work as a dated active release-shape gate under this current ledger
  area or under the release IA plan.
- Close work only by checking the matching active release gate and recording
  concise verification evidence in `PROGRESS.md`.

## Explicit current-release append marker -- appended 2026-06-26 01:20 +08

This section is intentionally appended at the end of `ISSUES.md` so the current
release-shape work is not confused with any old issue inventory above.

For all future work, read the file this way:

- Current release work lives only in the release IA refactor plan and in dated
  release-shape gates appended after it.
- Old screenshot audits, old page critiques, stale route names, resolved issue
  notes, and compatibility-era backlog text are historical evidence only.
- An old issue becomes active only after its desired outcome is restated as a
  current Cloudflare/GCP-style release gate in this active area.
- New issues discovered during implementation must be appended as new
  release-shape gates here or under the release IA plan before they are treated
  as release blockers.
- Do not preserve weak legacy UI, old page organization, old terminology, or
  old compatibility behavior unless it is explicitly restated in the active
  release IA contract.

This marker supersedes earlier ambiguity markers if their wording differs. The
active release IA plan remains the product contract; older issue sections remain
supporting context only.

## Authoritative active issue ledger -- appended 2026-06-26 01:47 +08

This section is appended at the end of `ISSUES.md` by request so the current
release IA work cannot be mistaken for the old issue inventory. Treat this as
the active issue-reading contract for the release refactor.

Only these entries are active release issues:

- The `Release IA refactor plan -- new intended shape, implementation map, and
  verification` section.
- Dated release-shape gates appended after that plan.
- New execution, behavior, backend/API workflow, accessibility, responsive
  layout, and verification tasks that are explicitly restated in this active
  release area.

Everything else is historical evidence:

- Old screenshot audits, critique notes, stale route names, compatibility-era
  page layouts, resolved issue notes, and unchecked legacy backlog items are
  not active release blockers.
- Do not implement, preserve, or mark work from old issue sections unless the
  desired outcome has first been rewritten as a current release IA gate.
- Do not treat weak legacy UI, old card-heavy layouts, old page organization,
  or old compatibility behavior as intentional product design.

Future additions must be appended as current release-shape gates with:

- Page or subpage ownership.
- Intended Cloudflare/GCP-style operator workflow.
- Existing implementation mapping.
- Explicit execution tasks.
- Explicit verification tasks.

Conflict rule:

- If older issue text conflicts with the release IA plan or this appended
  contract, the release IA plan wins.
- If any ambiguity remains, create a new active release gate here before
  implementing.

## Final active-release disambiguation marker -- appended 2026-06-26 02:21 +08

This note is appended explicitly by user request so future agents do not confuse
the current release IA work with old issues.

From this point forward, `ISSUES.md` must be interpreted as two ledgers:

- Historical ledger: everything before `Release IA refactor plan -- new
  intended shape, implementation map, and verification` is old audit evidence,
  old critique context, resolved issue history, or compatibility-era backlog.
- Active release ledger: the release IA refactor plan and every dated
  release-shape gate appended after it are the only active product-shape,
  implementation, and verification contract.

Rules:

- Do not pick, preserve, or close work from the historical ledger unless its
  desired outcome has first been restated as an active release-shape gate.
- Do not treat old weak UI, old page organization, old terminology, old route
  names, stale screenshots, or compatibility behavior as product intent.
- If the historical ledger conflicts with the active release ledger, implement
  the active release ledger.
- New findings must be appended as active release-shape gates with page
  ownership, intended operator workflow, implementation mapping, execution
  tasks, and verification tasks.

## Latest canonical release-ledger marker -- appended 2026-06-26 02:51 +08

This is the newest explicit boundary note. It is appended after all older issue
text so there is no ambiguity about what is active.

Active work means only:

- The `Release IA refactor plan -- new intended shape, implementation map, and
  verification` section.
- Dated release-shape gates appended after that release IA section.
- Work that is restated in those active sections with page ownership,
  Cloudflare/GCP-style operator workflow, implementation mapping, execution
  tasks, and verification tasks.

Old issues mean:

- Any critique, screenshot audit, stale layout note, resolved item, old route
  name, compatibility-era workflow, or unchecked backlog entry that appears
  before the release IA refactor plan and has not been restated in the active
  release ledger.

Interpretation rule:

- Do not implement, preserve, or mark old issue text as release work unless it
  is rewritten into the active release ledger first.
- Do not infer product intent from weak legacy UI or old compatibility notes.
- If any old text conflicts with the active release IA plan, the active release
  IA plan wins.

## Current active-release issue contract -- appended 2026-06-26 03:20 +08

This section is appended explicitly to remove any remaining ambiguity between
the current release refactor and the old issue inventory. Treat this as the
newest issue-reading contract until a later dated active-release contract is
appended.

Active release issues are only:

- The `Release IA refactor plan -- new intended shape, implementation map, and
  verification` section.
- Dated release-ledger or release-shape sections appended after that plan.
- Any future issue that is restated here with page/subpage ownership, intended
  Cloudflare/GCP-style operator workflow, existing implementation mapping,
  execution tasks, and verification tasks.

Old issues are not active release blockers when they are only:

- Screenshot-audit notes, critique bullets, stale route names, old page-layout
  descriptions, compatibility-era behavior, resolved issue notes, or unchecked
  backlog text that appears before the release IA plan.
- Historical evidence that has not been rewritten as a current release gate.

Required workflow for new work:

- Do not pick up or preserve old issue text directly.
- If an old issue is still valuable, first rewrite the desired outcome as a
  current release-shape gate under this active area or under the release IA
  plan.
- Implement only against the active release gate, not against the older wording.
- Close work only by checking the matching active release gate and recording
  concise verification evidence in `PROGRESS.md`.

Conflict rule:

- If any older issue, old screenshot critique, old route name, or old
  compatibility behavior conflicts with the active release IA contract, the
  active release IA contract wins.
- Weak legacy UI must never be treated as intentional product design unless it
  is explicitly restated in this active release contract or requested directly
  by the user.

## Latest explicit active-release appendix -- appended 2026-06-26 03:52 +08

This appendix is intentionally placed at the end of `ISSUES.md` by user
request. It exists so the current release IA refactor, intended page shapes,
implementation mapping, and verification gates are not confused with old issue
inventory.

Read `ISSUES.md` using this rule:

- Active release issues are only the `Release IA refactor plan -- new intended
  shape, implementation map, and verification` section plus dated active
  release appendices after it.
- Old audit findings, screenshot critiques, stale page names, resolved issue
  notes, compatibility-era behavior, and unchecked legacy backlog items are
  historical context only.
- No old issue should be implemented, preserved, or closed directly. If it is
  still valuable, first restate the desired Cloudflare/GCP-style VPS console
  outcome as an active release gate with page ownership, intended workflow,
  implementation mapping, execution tasks, and verification tasks.
- Current implementation work must be checked against the active release gate
  it closes, with evidence recorded in `PROGRESS.md`.
- If any old issue text conflicts with this active release contract, this
  active release contract wins.

## Authoritative current-release boundary -- appended 2026-06-26 04:24 +08

This is the newest explicit append-only clarification. It exists so the new
intended release shape cannot be confused with the old issue inventory above.

For all future work, read `ISSUES.md` this way:

- Active work is only the current release IA contract: the `Release IA refactor
  plan -- new intended shape, implementation map, and verification` section,
  the dated active-release appendices after it, and any future gate appended
  with explicit page ownership, intended operator workflow, implementation map,
  execution tasks, and verification tasks.
- Old work is every older audit note, screenshot critique, compatibility-era
  issue, stale route name, resolved checklist item, weak legacy UI note, or
  unchecked backlog entry that has not been rewritten into that active release
  contract.
- Agents must not implement, preserve, or close old work directly. If an old
  issue still matters, rewrite the desired outcome as a current release gate
  first, then implement and verify that gate.
- The active release IA contract always overrides older issue wording,
  screenshots, route names, and compatibility assumptions.
- `PROGRESS.md` should record which active release gate was closed and the
  verification evidence used to close it.

## Newest canonical active-release marker -- appended 2026-06-26 05:21 +08

This note is appended explicitly so the new intended release shape is not
confused with old issues, old screenshot audits, stale compatibility notes, or
legacy backlog checkboxes.

Use this as the current issue interpretation rule:

- Active release work starts at `## Release IA refactor plan -- new intended
  shape, implementation map, and verification` and continues through the dated
  release-shape appendices after it.
- Anything before that release IA heading is historical input only unless its
  desired outcome has been rewritten into the active release ledger.
- Old unchecked boxes are not automatically release blockers.
- Old resolved boxes are not proof that the new release shape is complete.
- Old UI behavior, route names, terminology, card layouts, compatibility
  assumptions, screenshots, and critique notes must not be treated as product
  intent unless they are restated in the active release ledger.
- New implementation work must reference a current active release gate with
  page/subpage ownership, expected Cloudflare/GCP-style VPS operator workflow,
  implementation mapping, execution tasks, and verification tasks.
- Closure means checking the current active release gate and recording concise
  evidence in `PROGRESS.md`; do not close old issue text directly.

If older issue text conflicts with the active release IA contract, implement the
active release IA contract. If an old issue still matters, first append a new
active release gate, then implement and verify that gate.

## Newest active-release separation note -- appended 2026-06-26 05:46 +08

This note is appended explicitly by user request. It is the newest boundary
between old issue inventory and the current intended release shape.

The current release work is not the old issue backlog. Read it this way:

- The active source of truth is the `Release IA refactor plan -- new intended
  shape, implementation map, and verification` section plus all dated
  release-shape gates appended after that section.
- Everything before that release IA plan is frozen historical context unless a
  desired outcome has been rewritten into the active release ledger.
- Old unchecked boxes, old resolved boxes, stale screenshots, compatibility-era
  route names, weak legacy layouts, and old terminology must not be treated as
  current release work or product intent.
- Do not implement or close an old issue directly. If an old note still matters,
  first append a new active release gate with page/subpage ownership, expected
  operator workflow, implementation mapping, execution tasks, and verification
  tasks.
- A task is complete only when the matching active release gate is checked and
  concise verification evidence is recorded in `PROGRESS.md`.

If this note conflicts with any older text in `ISSUES.md`, this note and the
active release IA plan win.

## Newest authoritative release-IA issue boundary -- appended 2026-06-26 06:15 +08

This appendix is intentionally appended at the end of `ISSUES.md` so the
current release IA work cannot be confused with old audit findings, stale
screenshots, compatibility-era issues, or historical backlog checkboxes.

For all future work, interpret this file with the following precedence:

- Current release scope is only the `Release IA refactor plan -- new intended
  shape, implementation map, and verification` section plus dated active
  release gates and boundary notes appended after it.
- Historical issue text is input material only. It must not be implemented,
  preserved, or closed directly unless the desired outcome has first been
  rewritten into the active release ledger.
- Product intent must come from the active release ledger, the confirmed
  Cloudflare/GCP-style VPS operator workflow, and user-confirmed preferences.
  Legacy weak UI, old route names, stale card layouts, and old compatibility
  assumptions are not product intent.
- Every new issue or task must be written as a current release gate with page
  or subpage ownership, intended operator workflow, implementation mapping,
  execution tasks, and verification tasks.
- Completion means checking the matching active release gate and recording the
  verification evidence in `PROGRESS.md`.

If any older issue conflicts with this appendix, this appendix and the active
release IA contract win.

## Current EOF active-release contract -- appended 2026-06-26 06:49 +08

This section is appended explicitly at the end of `ISSUES.md` so the current
release IA refactor is not confused with the older issue inventory above.
Treat this EOF marker as the newest issue-reading rule.

Only the following are active current-release work:

- `## Release IA refactor plan -- new intended shape, implementation map, and
  verification`.
- Dated release-shape gates appended after that plan.
- Future append-only gates that explicitly name page/subpage ownership,
  intended Cloudflare/GCP-style VPS operator workflow, existing implementation
  mapping, execution tasks, and verification tasks.

Everything else is old issue context unless it is restated into the active
release ledger:

- old screenshot audits;
- old critique bullets;
- stale route names;
- compatibility-era page layouts;
- old unchecked backlog boxes;
- old resolved checklist items;
- weak legacy UI behavior that has not been re-confirmed by the user.

Operational rule:

- Do not implement, preserve, or close old issue text directly.
- If an old issue still matters, append a new active release gate first, then
  implement and verify that gate.
- If old issue text conflicts with the active release IA plan or this EOF
  marker, the active release IA plan and this EOF marker win.
- Completion requires checking the matching active release gate and recording
  concise verification evidence in `PROGRESS.md`.

## Observability Alerts/Webhooks supersession -- appended 2026-06-26 22:32 +08

This note supersedes every older active or historical mention of an
`Observability / Incidents` page.

Current intended shape:

- `Observability / Alerts` owns alert policy groups, policy-issued alert
  evidence, notification channels, alert delivery previews, and retained alert
  notification deliveries.
- `Observability / Webhooks` owns webhook expression rules, rule dry-runs,
  queue dispatch, delivery processing, retained webhook deliveries, and webhook
  delivery-history maintenance.
- `Fleet / Alerts` remains the live triage queue for acknowledge, mute,
  escalate, and clear. It links to `Observability / Alerts` for policy context.
- There is no `Observability / Incidents` subpage, route, screenshot target, or
  release gate. Do not reintroduce it as a compatibility page or a hidden tab.
- Large `Network / Tunnel plans` workflows open from explicit page-level
  actions and must provide visible close buttons. The default Tunnel Plans page
  stays table-first and must not show create, promotion, and generated-config
  containers all at once.

Verification plan:

- Release IA tests must prove Alerts and Webhooks are separate pages.
- Release IA tests must prove Incidents is absent from navigation/routing.
- Structured screenshots must include `Network / Tunnel plans` default,
  opened create workflow, opened promotion workflow, `Observability / Alerts`,
  alert policy editor, `Observability / Webhooks`, and webhook rule editor.



# === NEWEST ISSUES ===

My previous audit imported too much enterprise process from Cloudflare and Google Cloud. **vpsman should not imitate their business model, hierarchy, or procedural weight.** Their useful lessons are limited to general interface quality: consistent states, compact navigation, readable tables, predictable actions, clear feedback, and good responsive behavior.

For vpsman, the correct target is:

> **Expert-simple:** expose powerful VPS operations directly, keep common tasks fast, and add friction only when an action is broad, destructive, or difficult to reverse.

## What I would withdraw or strongly derank from the previous report

These are not appropriate as default requirements for this product:

- Organization/folder/project-style resource hierarchy.
- Multi-party or multi-level approval for ordinary operations.
- A long universal `Prepare → Preview → Approve → Unlock → Verify...` workflow for every mutation.
- Full incident-management machinery for simple VPS alerts.
- A configurable enterprise dashboard builder unless users genuinely need one.
- A wizard for every raw DSL, JSON, TOML, cron, or networking field.
- Forcing all actions into formal lifecycles merely because GCP or Cloudflare does so.
- Requiring an audit note for every routine operation.
- Splitting simple pages into many subpages just to create conceptual structure.

The product can remain direct and compact. Raw and advanced controls are appropriate because the intended users understand VPS, Linux, networking, and automation. The UI only needs to make those controls safe, legible, and predictable.

# Correct action-friction model

This is the level of process vpsman should use:

| Operation type                           | Appropriate interaction                                      |
| ---------------------------------------- | ------------------------------------------------------------ |
| Read-only action                         | Execute immediately                                          |
| Reversible single-VPS action             | Execute immediately; show result and Undo where possible     |
| Routine privileged single-VPS action     | One compact confirmation, or no confirmation when the operator has explicitly unlocked the session |
| Destructive single-VPS action            | One clear confirmation dialog                                |
| Bulk action                              | Show resolved target count and important exclusions, then one confirmation |
| Irreversible or security-critical action | Unlock plus one confirmation                                 |
| Long-running operation                   | Start immediately after confirmation and provide progress, cancellation, and result |

Do **not** require operators to repeatedly review the same payload, click Next through multiple steps, and then unlock after they already made their intent clear.

A preview should usually be inline rather than another page.

------

The standard used here is:

> **Expert-direct:** make normal VPS work fast, preserve advanced controls, and add friction only when an action is broad, destructive, security-sensitive, or difficult to reverse.

I am **not** recommending organization/folder/project hierarchies, multi-level approvals, mandatory wizards, a full incident-management system, a custom dashboard builder, or GCP-style process ceremony.

------

# Highest-priority findings

| Priority | Problem                                                      | Practical correction                                         |
| -------- | ------------------------------------------------------------ | ------------------------------------------------------------ |
| **P0**   | Status and data contradictions undermine trust               | Establish shared state, time, freshness, and diff functions. Reject or visibly flag impossible combinations |
| **P0**   | Config Rules reports changes where before and after are identical | Compare normalized semantic values; suppress no-op rows and disable Apply |
| **P0**   | OSPF screens disagree between `14→21` and `14→22`            | Use one proposal record and ID across graph, table, evidence, review, apply, and rollback |
| **P0**   | Approval buttons execute immediately with hard-coded reasons | Open one compact decision dialog; approval note optional, rejection reason required |
| **P0**   | Old or expired records are presented as current              | Validate timestamps and label records as stale, expired, overdue, or historical |
| **P0**   | Mobile hides or delays the actual task                       | Replace the repeated mobile shell with a compact app bar; use cards and sticky primary actions |
| **P0**   | Backup “protection” does not consider backup age             | Use simple states: Recent, Overdue, Unprotected, Unknown     |
| **P0**   | Cleanup lacks age and retention evidence                     | Do not permit deletion until preview exposes count, size, age range, and affected objects |
| **P1**   | Every page repeats six fleet cards, scope data, saved views, and other shell controls | Collapse this into one compact fleet-status line             |
| **P1**   | Simple operations sometimes require preview, unlock, review, and another confirmation | Preview without privilege; require only one final confirmation appropriate to risk |
| **P1**   | Large blank editors and result surfaces appear before there is anything to show | Render compact empty states and expand the workspace after selection |
| **P1**   | Internal enums and implementation language appear as operator-facing text | Map internal states to human labels; preserve raw values only in details |
| **P1**   | Sparse or old samples are presented as continuous trends     | Do not connect missing samples; show available range, point count, and last update |
| **P1**   | Operational defaults are stored as personal browser preferences | Move gateway and tunnel-generation defaults into shared, audited configuration |
| **P1**   | Incomplete features appear as ordinary production pages      | Hide or clearly mark them Preview until functional           |

------

# Correct interaction model for vpsman

This should be the consistent product rule:

| Operation                            | Appropriate interaction                                      |
| ------------------------------------ | ------------------------------------------------------------ |
| Read-only inspection                 | Immediate                                                    |
| Reversible single-VPS change         | Immediate, with success feedback and Undo where possible     |
| Routine privileged single-VPS change | Immediate while explicitly unlocked, or one compact confirmation |
| Destructive single-VPS change        | One confirmation naming the exact effect                     |
| Bulk change                          | Inline target count and exclusions, then one confirmation    |
| Irreversible security action         | Unlock plus one confirmation                                 |
| Long-running action                  | Start, then progress, cancel where feasible, and result      |
| Dry run or validation                | Available before privilege unlock unless the read itself is sensitive |

Common workflows should remain very short:

- **Open terminal:** select VPS → Open terminal.
- **Run command:** select VPS/targets → enter command → Run.
- **Edit file:** select VPS → open file → edit → Save.
- **Back up now:** select VPS → Back up now.
- **Apply config patch:** select targets → enter patch → preview inline → Apply.
- **Restore:** choose artifact → choose destination → Confirm.
- **Update agent:** choose version → choose targets → Confirm.
- **Change group/tag:** select VPSs → add/remove group.

------

# Code-confirmed cross-cutting problems

These are not visual interpretations; they are visible in the implementation.

## 1. Scope control behaves unlike it looks

**Resolved 2026-06-27**

The global scope control now opens the fleet search editor instead of clearing
scope. Clearing is a separate adjacent X action with its own
`Clear fleet scope` label and disabled state when no fleet scope is active.

In `ConsoleShell.tsx`, the control displaying the active fleet scope calls `onClearFleetView` when clicked. It visually resembles a scope selector but behaves as a clear button.

**Fix:** clicking the scope control should open scope selection or editing. Place a separate X beside it for clearing.

## 2. Global fleet cards are rendered on every page

**Resolved 2026-06-27**

The shell now renders full fleet health metric cards only on Home and Fleet /
Monitor. Other pages keep the same `Fleet status summary` landmark but show a
single compact status strip with total VPS, online, stale, running jobs,
warnings, and online percentage.

`ConsoleShell.tsx` always renders Online, Offline, Stale, Warnings, Jobs, and Online %. This explains why Access, Audit, System, terminal, file, and editor pages all begin with the same fleet block.

**Fix:** use one compact status strip. Show full fleet health cards only on Home or Fleet Monitor.

## 3. Scope and global statistics can disagree

**Status — implemented 2026-06-26**

`App.tsx` now passes a scoped fleet summary and online percentage to the global
shell whenever a fleet query or saved view is active. `ConsoleShell` labels the
summary as `Current scope` or `Entire fleet`, so visible scoped pages no longer
mix scoped page descriptions with an unlabeled global online percentage.

`App.tsx` computes the online percentage from the global dashboard summary, while page descriptions can use visible scoped agents.

**Fix:** explicitly label values as **Current scope** or **Entire fleet**, and use the current scope by default on scoped pages.

## 4. Online state ignores missing last-seen data

**Status — implemented 2026-06-26**

Fleet Instances now uses an agent display-state helper for operator-facing
state. An `online` backend status with no `last_seen_at` contact evidence is
shown as `Contact unknown` with warning tone and explicit gateway-contact
detail, rather than a green Online badge. Raw backend status remains available
for filtering and action logic.

`FleetWorkspace.tsx` renders the positive online badge using only `agent.status === "online"`, independently of `last_seen_at`.

**Fix:** use a shared display-state function. An `online` record with no contact evidence should become **State inconsistent** or **Registered; contact unknown**, not green Online.

## 5. Compact timestamps omit year and timezone

**Status — implemented 2026-06-26**

`formatCompactTime()` now returns relative scan text such as `18m ago`,
`25d ago`, or `in 2h` instead of month/day/hour-only values. A new
`formatFullTime()` helper includes year, seconds, and timezone for exact
inspection. Home attention/activity rows, Fleet Alerts observed times, and
Backup history dense timestamp columns expose the full timestamp in hover
titles while keeping relative text visible.

`formatCompactTime()` formats only month, day, hour, and minute.

**Fix:** show relative age in dense views—`18m ago`, `25d ago`—and expose a full year/timezone timestamp on hover or in the detail drawer.

## 6. Charts connect missing data

**Status — implemented 2026-06-26**

`TimeSeriesChart.tsx` now preserves missing-data gaps with `spanGaps: false`
and exposes a compact data-coverage line showing points present in the selected
range, gap count, and the available sample span. Release navigation coverage
asserts the preserved-gap policy and the visible coverage summary.

`TimeSeriesChart.tsx` sets `spanGaps: true`, creating visually continuous lines through absent observations.

**Fix:** use `spanGaps: false`. Show gaps honestly and display how much of the selected range actually contains data.

## 7. Approval execution is too immediate

`JobsPanel.tsx` sends `confirmed: true` directly from the row action and supplies fixed reasons such as `Approved from Jobs / Approvals`.

**Fix:** replace the direct check/X execution with a compact review dialog. Keep it simple: operation, targets, requester, risk, optional approval note, required rejection reason.

## 8. Removing a tag is immediately confirmed

**Status — implemented 2026-06-26**

Group Assignment chip removals remain direct for a reversible single-VPS label
change, but they now show an inline `Removed ... — Undo` status. Undo sends the
inverse add request for the same VPS/label, and any schedule impacts returned
by the mutation response are surfaced as a concise automation notice.

`FleetGroupsPanel.tsx` invokes a confirmed bulk-remove request when the X on a tag chip is clicked.

**Fix:** for a single reversible removal, execute and provide Undo. Where that tag drives automation, show one small warning before removal.

## 9. Past “next runs” are not validated

**Status — implemented 2026-06-26**

`SchedulesPanel.tsx` now parses schedule run times, hides past values from the
future-run chip list, and labels past-only enabled schedules as `Overdue` with
`Schedule calculation stale` detail. Expanded rows use the same timing model
instead of formatting `next_run_at` as an ordinary future run.

`SchedulesPanel.tsx` renders `next_run_at` and `next_runs` as received and only deduplicates them. It does not filter past times or label them overdue.

**Fix:** reject past values from the “Next runs” presentation and display **Overdue** or **Schedule calculation stale**.

## 10. Backup protection ignores age

**Status — implemented 2026-06-26**

Backup Overview now derives a simple per-VPS protection state:
`Recent`, `Overdue`, `Unprotected`, or `Unknown`. Only usable artifact-backed
backups inside the expected freshness window count as recent. Enabled policy
cadence estimates the freshness window when available; otherwise one-time
backup evidence uses a conservative seven-day window. Metadata-only records such
as `artifact_metadata_recorded` no longer count as protected and are labelled
`Artifact recorded; content not verified`.

`BackupsPanel.tsx` treats a VPS as protected when it has an enabled policy or any non-error artifact-backed backup. No age or expected interval is checked.

**Fix:** calculate protection from latest successful usable backup age relative to the policy frequency.

## 11. Process Metrics is an internal roadmap page

**Status — resolved 2026-06-27**

Process Metrics is no longer part of normal Observability navigation or the
mobile page selector. Stale internal requests for the removed
`process_metrics` subpage normalize to Observability / Fleet metrics until a
real process-history backend contract exists. Remote Operations / Processes
does not present the unfinished metrics route as production evidence.

**Fix:** remove Process Metrics from normal navigation until it has actual operator data, or label it clearly as Preview.

## 12. Operational defaults are browser-local

`DEFAULT_OPERATOR_PREFERENCES` includes gateway endpoints, gateway public key, and tunnel allocation pools.

**Fix:** keep personal display choices browser-local, but move values that affect generated install commands and topology plans into shared fleet/system configuration.

## 13. Suite Config summary wording is misleading

**Status — implemented 2026-06-26**

The Suite Config top summary now explicitly distinguishes loaded
configuration inventory from validated draft impact. Before validation it shows
`Configuration inventory` with inventory hot-reload and restart field counts;
after validation it switches to `Draft impact` and changed hot-reload/restart
counts.

The “13 hot reload fields” and “16 restart required fields” values count all fields in those categories, not changed fields. They sit beside “Changed keys,” which makes them look like current draft impact.

**Fix:** before editing, label this **Configuration inventory**. After editing, switch to **Draft impact: 2 hot reload, 1 restart**.

## 14. Audit “latest event” is not necessarily latest

**Status — implemented 2026-06-26**

`AuditLogPanel.tsx` now calculates the latest visible audit row from the
maximum valid `created_at` timestamp across filtered records, independent of
the API/table row order. Playwright coverage feeds deliberately older-first
audit rows and verifies the coverage summary uses the newer timestamp.

`AuditLogPanel.tsx` uses the first filtered item to render “Latest visible event” without ensuring that the array is sorted newest-first.

**Fix:** calculate the maximum timestamp independently of table order.

## 15. Mobile tables remain desktop grids

**Status — implemented 2026-06-26**

`ConsoleDataGrid` now has a generic mobile card rendering mode. At mobile
widths, normal data grids render one card per row instead of squeezing desktop
columns horizontally. Each card keeps the row identity, state, labeled values,
selection control, Details/Open affordance, and up to three row actions visible
near the row. Desktop rendering, sorting, column preferences, pagination,
selection, expansion, and context menus remain unchanged.

Pages with a deliberately custom mobile row layout can opt out with
`mobileLayout="table"`; Access / Operators uses that to preserve its audited
operator-card layout. Focused mobile tests verify Jobs / Approvals cards expose
the Review action and Details control without page-level horizontal overflow,
and the Access / Operators visual audit still passes. Remaining page-specific
mobile defects outside `ConsoleDataGrid` stay tracked under their individual
screen issues.

The responsive CSS narrows the grid but does not transform most records into mobile cards. This is why important actions and columns can disappear horizontally.

**Fix:** add a mobile rendering mode to `ConsoleDataGrid`, with resource, state, primary value, and primary action always visible.

------

# Global shell and navigation

## Current problems

The shell consumes too much of every workflow:

- fleet scope;
- fleet search;
- full-page mobile selector;
- saved-view selector;
- saved-view save/delete/clear icons;
- “Live control plane” pill;
- command palette;
- session;
- unlock;
- breadcrumb;
- page title and description;
- context chips;
- six fleet metrics.

On desktop, this pushes the task downward. On mobile, the operator often passes through more than one screen of framework before reaching the operation.

“Live control plane” also looks like a real connection/freshness indicator even though it is presented as a static pill.

The mobile page selector contains the entire console hierarchy in one dropdown. It is technically functional but difficult to scan and increasingly unwieldy as pages grow.

## Practical redesign

Desktop top bar:

```
Fleet scope | Search | Active jobs/warnings | Lock state | User
```

Page header:

```
Page title | concise subtitle | primary action
```

Fleet summary:

```
3 VPS · 2 online · 1 stale · 3 running jobs
```

Mobile:

- one sticky app bar;
- menu;
- current page title;
- fleet-health dot/count;
- lock state;
- optional search icon;
- page navigation in a drawer rather than a very long select;
- sticky page action where appropriate.

Saved views should live inside the scope/search area, not occupy permanent space on every page.

------

# Mobile-wide findings

All 64 mobile screenshots inherit the same structural issue. At 390 px, the UI usually displays:

1. scope;
2. search;
3. page selector;
4. command/unlock;
5. saved views and three icon buttons;
6. breadcrumb/title/description;
7. context labels;
8. six fleet cards;

before reaching the page task.

The most extreme page heights include:

- Suite Config: approximately **11,659 px**
- Home: approximately **8,914 px**
- System Overview: approximately **7,201 px**
- Preferences: approximately **6,251 px**
- Capacity: approximately **5,826 px**
- Terminal: approximately **4,613 px**
- Webhook rule editor: approximately **4,288 px**
- Alert policy editor: approximately **4,253 px**
- Network test: approximately **4,064 px**

The answer is not to remove mobile capabilities. It is to reduce presentation:

- cards instead of horizontal tables;
- one active form/editor section;
- collapsed advanced fields;
- full-screen terminal and graph modes;
- sticky Save/Run/Approve/Apply controls;
- summaries replaced by one-line status;
- secondary evidence below the primary operation.

------

# Screen-by-screen audit

## Home and Fleet

### 01 — Home overview — **P1**

**Resolved 2026-06-27 — action-first Home without embedded subsystem inventories**

Home now limits itself to the release-console overview role: quick target
actions, fleet availability, running work, recent failures, needs-attention
work, and recent activity. The embedded Fleet Monitor card inventory and Home
telemetry/chart widgets were removed from the page so Fleet Monitor,
Observability, Alerts, Backups, Jobs, and Transfers remain the owners of their
deep workflows.

Running jobs are shown as fleet-level work when only summary counts are loaded,
instead of repeating global job counts on per-VPS cards. Backup and activity
states translate internal tokens such as `artifact_metadata_recorded` into
operator language such as `artifact recorded; upload not verified`, and dotted
audit events render as readable activity labels. Mobile now reaches target
selection and the primary quick actions before the six fleet posture metrics.

Fresh screenshots:
`./tmp/desktop-chrome/01-home-overview-desktop-chrome.png` and
`./tmp/mobile-chrome/01-home-overview-mobile-chrome.png`.

**Issues**

- The page is 3,925 px on desktop and 8,914 px on mobile.
- It duplicates content from Fleet Monitor, Observability, Alerts, Backups, Jobs, and Transfers.
- Per-VPS cards say telemetry is not reported while also displaying latency/network values.
- The same running-job count appears on multiple VPS cards, apparently using the global count.
- Internal statuses such as `artifact_metadata_recorded` appear directly.
- Charts imply a meaningful range even when the actual samples are sparse or old.
- “Fleet health 67%” is not sufficiently explanatory.

**Practical fix**

Limit Home to:

1. attention requiring action;
2. currently running work;
3. fleet availability;
4. recent failures;
5. four or five quick actions.

Move deep charts and subsystem inventories behind links. Show per-VPS jobs only when genuinely associated with that VPS. Translate internal states into human language such as **Artifact recorded; upload not verified**.

**Mobile**

The first useful action should appear in the first viewport. Do not make the operator scroll through all six fleet metrics before reaching Open terminal, Run command, Files, or Backup.

------

### 02 — Fleet Instances — **P0**

**Resolved 2026-06-27**

Fleet Instances no longer renders online records without last-contact evidence
as green Online; they show `Contact unknown` and a contact-evidence detail.
The remaining Fleet Instances work was resolved by separating browser/control
plane stream state from VPS state, promoting the default operational columns,
and making row/card Open affordances explicit.

**Issues**

- VPSs are green Online while Last seen says never seen.
- `WebSocket connected` is displayed near fleet records and can be interpreted as agent connectivity rather than browser/control-plane connectivity.
- The default table lacks common operator fields such as IP, agent version, contact age, CPU, memory, disk, and active alert count.
- Opening a VPS relies on a small icon/expander rather than the row itself.
- Raw tags occupy considerable width while more important operational state is absent.

Resolved implementation:

- Fleet Instances now uses visible default columns:
  `VPS · State · IP · Last contact · Agent · CPU · Memory · Disk · Alerts · Action`.
- Raw `Tags`, `Country`, `Provider`, traffic accounting, quota, selectors, and
  registration IP remain available from Fields, but no longer displace the
  default operational read.
- Browser connectivity is labelled as `Console stream connected`, separate from
  agent/VPS state.
- Desktop rows open the canonical instance detail route on row click, and each
  row also exposes an `Open` action cell.
- Mobile renders each VPS as a card with state, last contact, IP, alert count,
  telemetry/resource fields, and one clear `Open` button.
- The shared mobile grid hides duplicate `Action`/`Open` data cells whenever an
  `Open` card action is already rendered.

**Practical fix**

Define separate fields:

- Registration state;
- Gateway connection;
- Last contact;
- Telemetry freshness;
- Agent state.

Default columns:

```
VPS · State · IP · Last contact · Agent · CPU · Memory · Disk · Alerts · Action
```

Clicking the row opens the instance detail. Browser connectivity is labelled
separately as **Console stream connected**.

**Mobile**

Render each VPS as a card with name, state, last contact, IP, alert count, and
Open. Do not require horizontal scrolling.

Verification passed 2026-06-27:

- `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`
- `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/components/ConsoleDataGrid.tsx frontend/src/panels/FleetWorkspace.tsx frontend/tests/console-layout.spec.ts frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/console-layout.spec.ts -g "renders an operational cloud-console fleet workspace" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/console-layout.spec.ts -g "supports interactive fleet data grid controls" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/console-layout.spec.ts -g "deletes a VPS through grid actions" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "fleet instances table keeps dense grid controls" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=mobile-chrome --workers=1 --timeout=120000'`

Fresh screenshots:

- `./tmp/desktop-chrome/02-fleet-instances-desktop-chrome.png` (`1440x968`)
- `./tmp/mobile-chrome/02-fleet-instances-mobile-chrome.png` (`390x2117`)
- `./tmp/desktop-chrome/02b-fleet-instance-config-detail-desktop-chrome.png`
  (`1440x1463`)
- `./tmp/mobile-chrome/02b-fleet-instance-config-detail-mobile-chrome.png`
  (`390x2901`)

------

### 02b — Fleet instance Config detail — **P1**

**Resolved 2026-06-27 — config posture, source readiness, drift, and compact actions**

- [x] The Config tab now starts with the required posture strip:
  `Desired source`, `Render status`, `Drift state`, `Last apply`, and
  `Last error`.
- [x] Source assignment/readiness, runtime apply state, and drift/error state
  are separated instead of mixed into one ownership block.
- [x] `selected_no_store` is translated to
  `Backup object-store source selected; server storage is not configured.`;
  raw source statuses are preserved only inside `Raw source state details`.
- [x] The oversized ownership link was replaced with compact `Open config`,
  `Compare`, and `Apply` actions that hand off to Config / Per-VPS.
- [x] The one-VPS detail context no longer repeats the global fleet posture,
  and it uses the shared display-state model for Online-but-not-reported
  contact evidence.
- [x] Mobile uses one `Detail section` selector for the selected detail tab
  instead of a two-row tab matrix.
- [x] Fresh screenshots:
  `./tmp/desktop-chrome/02b-fleet-instance-config-detail-desktop-chrome.png`
  and
  `./tmp/mobile-chrome/02b-fleet-instance-config-detail-mobile-chrome.png`.
- [x] Verification passed:
  `cd frontend && npm exec -- tsc --noEmit`;
  `node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/panels/VpsDetailPanel.tsx frontend/src/styles/workspace.css frontend/src/styles/responsive.css frontend/src/App.tsx frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts`;
  `cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "fleet instance config detail|fleet instance detail is the canonical" --project=desktop-chrome --workers=1 --timeout=180000`;
  `cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=desktop-chrome --workers=1 --timeout=180000`;
  `cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=mobile-chrome --workers=1 --timeout=180000`.

**Issues**

- It inherits the Online/Not reported contradiction.
- The page repeats global fleet status inside a one-VPS detail context.
- The Config tab exposes internal source states such as `selected_no_store`.
- “Config ownership” mostly acts as a large link to another page.
- Source assignment, readiness, and actual drift are not clearly separated.
- The operator cannot immediately tell whether the VPS config is correct, merely selected, or unable to render.

**Practical fix**

At the top of the Config tab show:

```
Desired source · Render status · Drift state · Last apply · Last error
```

Translate `selected_no_store` to something such as:

> Backup object-store source selected; server storage is not configured.

Keep the raw state in Details. Replace the oversized link with compact actions: **Open config**, **Compare**, **Apply**.

**Mobile**

Use one selected detail tab at a time. The two-row tab matrix and repeated global data make the page unnecessarily long.

------

### 03 — Fleet Monitor — **P1**

**Resolved 2026-06-27 — compact operator cards, scoped signals, and direct workflow actions**

Fleet Monitor now uses the shared display-state model instead of raw backend
status, so records without contact evidence show `Contact unknown` rather than
green Online. Card content is aligned to the intended scan order:
state/contact, CPU, memory, disk, network, telemetry freshness, alerts, backup,
and transfer.

Global job counts are no longer rendered as a card-local operational signal.
They appear as contextual evidence text such as
`Fleet jobs: 3 running fleet-wide`, while the action-weighted signal row is
limited to card-local alerts, backup, and transfer state. Cards no longer show
`Network 0 bps` when rate telemetry is absent; missing rate samples render as
`Network n/a` beside `Telemetry not reported`.

The primary action row now exposes only **Terminal**, **Files**, and **More**.
Processes, Backup, Network, and Detail remain available under **More**, keeping
the common VPS workflows fast without turning every card into a dense action
toolbar. The implementation-oriented `Komari-style` wording has been removed
from Home and Fleet Monitor copy.

**Issues**

- Multiple VPS cards show the same three-job count, which appears to be global rather than per-resource.
- Cards display network values while saying telemetry is not reported.
- Backup, transfer, alert, and job states are given equal visual weight even when some require no action.
- Every card has several small actions, making scanning slower.
- The description uses implementation/comparison language such as “Komari-style.”

**Practical fix**

Use true per-VPS values and include freshness. Default card content:

```
State · Last contact · CPU · Memory · Disk · Network · Alerts
```

Then provide two direct actions—Terminal and Files—and place remaining actions in More.

Replace implementation-oriented text with an operator-oriented description.

**Mobile**

Compact cards should remain compact. Avoid turning every VPS into a long stack of subsystem panels.

Verification passed 2026-06-27:

- `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "home exposes fleet scan|fleet monitor renders VPS card workflow actions|home monitor card text fits" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/panels/FleetMonitorPanel.tsx frontend/src/styles/workspace.css frontend/tests/release-ia-navigation.spec.ts'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=desktop-chrome --workers=1 --timeout=160000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=mobile-chrome --workers=1 --timeout=160000'`

Fresh screenshots:

- `./tmp/desktop-chrome/03-fleet-monitor-desktop-chrome.png` (`1440x968`)
- `./tmp/mobile-chrome/03-fleet-monitor-mobile-chrome.png` (`390x2381`)

------

### 04 — Fleet Groups — **P1**

**Resolved 2026-06-27 — group registry language, metadata distinction, and compact display-order action**

Fleet / Groups now uses **Groups** as the operator-facing concept while keeping
the existing tag API/model internally. The registry page says `Create group`,
`Group registry`, `Assigned VPSs`, and `Delete`; it no longer shows
`Create tag`, `Tag registry`, or `Review deletion` in the normal registry
workflow.

Provider and country entries are explicitly classified as managed metadata, and
operator-created entries are classified as operator groups. The summary cards
now read `provider metadata`, `country metadata`, `operator groups`, and
`group assignments`, so operators can tell metadata-derived targeting apart
from custom VPS group collections.

Create/search now appear before the summary cards on the registry screen.
The create form accepts one group per submission, disables comma-separated
input, and explains that provider/country metadata is read from VPS records.
Deletion remains one compact confirmation showing group, type, assignments,
preview hash, and schedule-target notices. Display ordering is now behind a
small **Manage display order** disclosure instead of being promoted as a
primary registry task.

The adjacent Assignments surface now says `VPS group assignments`, `Current
groups`, and `Add group` so the Fleet navigation does not immediately fall back
to the old visible tag vocabulary.

**Issues**

- The navigation says Groups while the UI repeatedly says tags.
- Provider/country/custom group counts are not intuitive relative to labels shown elsewhere.
- The “Create tag” field suggests multiple comma-separated values while the action is singular.
- Fleet tag ordering receives excessive prominence.
- Simple deletion uses “Review deletion,” which feels heavier than necessary.

**Practical fix**

Choose one visible concept:

- **Labels** for key/value metadata; or
- **Groups** for named operator-managed collections.

Internally they may remain tags. Distinguish managed labels such as provider/country from custom labels.

Use token entry for multiple labels or accept one label per submission. Move ordering into a small **Manage display order** action.

Deletion needs one confirmation showing the number of assigned VPSs.

**Mobile**

Create and search should come before summary cards. Render each label with count and a named overflow action.

Verification passed 2026-06-27:

- `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "fleet groups expose registry assignments" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/panels/FleetGroupsPanel.tsx frontend/src/styles/data.css frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts frontend/tests/support/consoleLayoutFixtures.ts'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=desktop-chrome --workers=1 --timeout=160000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=mobile-chrome --workers=1 --timeout=160000'`

Fresh screenshots:

- `./tmp/desktop-chrome/04-fleet-groups-desktop-chrome.png` (`1440x968`)
- `./tmp/mobile-chrome/04-fleet-groups-mobile-chrome.png` (`390x1533`)
- `./tmp/desktop-chrome/05-fleet-group-assignments-desktop-chrome.png`
  (`1440x968`)
- `./tmp/mobile-chrome/05-fleet-group-assignments-mobile-chrome.png`
  (`390x1727`)

------

### 05 — Group Assignments — **P1**

**Resolved 2026-06-27 — protected managed labels, registry autocomplete, dependency hints, and mobile cards**

- [x] Clicking a removable operator-group X executes the reversible single-VPS
  mutation directly and shows `Removed <group> from <VPS>` with inline Undo.
- [x] Managed metadata labels such as `provider:*` and `country:*` render as
  protected shield chips and do not expose remove buttons.
- [x] The Add group control uses the group registry as an autocomplete source
  and shows compact suggestions with assigned-VPS counts.
- [x] Automation dependency hints are shown next to linked chips only when a
  group is referenced by schedules or alert policies; e.g. `provider:alpha`
  shows `Used by 1 schedule` in the fixture instead of a separate dependency
  workflow.
- [x] Mobile renders one assignment card per VPS with wrapping chips, visible
  Add group controls, and readable `group name` placeholders.
- [x] The group summary now counts active VPS labels and registry-backed groups
  together, so provider/country metadata and assignment counts match the chips
  visible on Fleet / Assignments and Fleet / Groups.

Verification passed:

- `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "fleet groups expose registry assignments" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/panels/FleetGroupsPanel.tsx frontend/src/styles/data.css frontend/tests/release-ia-navigation.spec.ts'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=desktop-chrome --workers=1 --timeout=160000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 1 of" --project=mobile-chrome --workers=1 --timeout=160000'`

Fresh screenshots:

- `./tmp/desktop-chrome/05-fleet-group-assignments-desktop-chrome.png`
  (`1440x968`)
- `./tmp/mobile-chrome/05-fleet-group-assignments-mobile-chrome.png`
  (`390x1790`)

------

### 06 — Bulk Groups — **P1**

**Resolved 2026-06-27 — inline target resolution and one primary bulk action**

- [x] The empty target preview panel is collapsed until the operator starts a
  server-backed review.
- [x] The selector now shows local match evidence inline, for example
  `Local match 1 VPS · 1 ready · 0 stale`, and separately states that server
  resolution runs before confirmation.
- [x] The old `Preview targets → unlock → Review mutation` path is replaced by
  one primary action such as `Add maintenance:test to 1 VPS`. The action runs
  the server resolve/preview step, captures the preview hash, and opens the
  final confirmation.
- [x] Preview remains available before privilege unlock. The privilege message
  now says preview works immediately and unlock is needed only for final apply.
- [x] The final confirmation lists targets, changed count, no-change/excluded
  count, preview hash, schedule notices, and before/after membership outcome.
- [x] Mobile keeps mutation, tag, selector, resolution, privilege state, and
  action together in one compact block without summary-card separation.

Verification passed:

- `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "fleet groups expose registry assignments" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/panels/FleetGroupsPanel.tsx frontend/src/styles/data.css frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=desktop-chrome --workers=1 --timeout=160000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=mobile-chrome --workers=1 --timeout=160000'`

Fresh screenshots:

- `./tmp/desktop-chrome/06-fleet-bulk-groups-desktop-chrome.png`
  (`1440x968`)
- `./tmp/mobile-chrome/06-fleet-bulk-groups-mobile-chrome.png`
  (`390x1360`)

------

### 07 — Fleet Alerts — **P1**

**Resolved 2026-06-27 — active triage grid, readable alert semantics, and mobile alert cards**

**Issues**

- [x] Alert titles, targets, categories, and times are no longer treated as raw
  cramped fields. The grid now uses the intended
  `Severity · Summary · VPS · State · Age · Action` shape, with exact
  timestamp retained in the time tooltip.
- [x] Raw categories such as `source_readiness` are mapped to operator labels
  like `Source readiness` and `Traffic policy`; raw target/evidence values
  remain in the detail drawer.
- [x] Operator state and alert status are separated in the State column and in
  the drawer (`Operator state`, `Alert status`, `Category`, `Target`,
  `Observed`, `Escalation`).
- [x] Primary row actions are visible: open alerts show `Acknowledge`, `Open`,
  and mobile `Mute`; triaged alerts show `Clear`, `Open VPS`, and policy/detail
  handoffs where applicable.
- [x] Fleet Alerts is now the active triage list, while Observability / Alerts
  remains policy/destination/delivery configuration.

**Practical fix**

Use Fleet Alerts as the active triage list:

```
Severity · Summary · VPS · State · Age · Action
```

A row opens a compact detail drawer with evidence, policy, acknowledge/silence action, and link to the VPS.

Use Observability / Alerts for policies, destinations, and history.

**Mobile**

Transform each alert into a card. Keep severity, title, VPS, age, and Acknowledge/Open visible without horizontal scrolling.

Resolved implementation:

- Desktop grid uses the intended release columns and keeps the explicit
  `Action` column.
- Row details expose readable status/category evidence, policy name when
  present, `Acknowledge`, `Silence 4h`, `Clear triage`, `Open VPS detail`, and
  `Open alert policies`.
- Mobile cards use the shared grid card renderer with state-aware row actions:
  open cards show `Acknowledge`, `Open VPS`, `Mute`, and `Details`; triaged
  cards show `Open VPS`, `Clear`, `Alert policies`, and `Details`.
- Fresh screenshots:
  `./tmp/desktop-chrome/07-fleet-alerts-desktop-chrome.png`
  (`1440x968`) and
  `./tmp/mobile-chrome/07-fleet-alerts-mobile-chrome.png`
  (`390x2582`).
- Verification passed:
  `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`;
  `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/components/ConsoleDataGrid.tsx frontend/src/panels/FleetAlertsPanel.tsx frontend/src/styles/data.css frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "observability alerts and webhooks are explicit separate pages" --project=desktop-chrome --workers=1 --timeout=120000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=mobile-chrome --workers=1 --timeout=180000'`.

------

### 08 — Fleet Instance detail, Summary — **P1**

**Resolved 2026-06-27 — resource-scoped facts, human activity labels, and no fleet KPI block**

**Issues**

- [x] It repeats the same inconsistent Online/Last seen state. Resolved by using
  the shared agent display-state helper in the VPS identity and resource facts;
  registered-but-unseen agents now read as `Contact unknown` with missing
  gateway timestamp evidence instead of a false healthy online state.
- [x] Global fleet-level counts are placed inside a resource page. Resolved by
  suppressing the shell fleet KPI strip on `Fleet / Instance detail` and keeping
  the page header scoped to context chips plus the selected resource.
- [x] Summary quick actions are duplicated at both the page top and resource
  card. Resolved by keeping workflow handoffs in the page header and keeping the
  selected-resource card focused on identity and facts.
- [x] Health says no telemetry while Latest work and network sections still
  contain operational data. Resolved by renaming the missing sample state to
  `No resource rollup` and explaining that job, backup, network, and alert
  evidence can still exist as separate workflow records.
- [x] Raw operation names such as `scheduled_shell_argv` and raw backup statuses
  appear. Resolved by mapping workflow enums to operator labels such as
  `Scheduled shell command`, `Artifact metadata recorded`, and readable alert
  states; raw values stay out of the normal summary scan.
- [x] Old records are shown without relative age. Resolved by showing compact
  relative ages such as `4w ago` in warnings/latest work/activity, with exact
  timestamps available through the detail time tooltip.

**Practical fix**

The resource header should contain only resource facts:

```
State · Last contact · IP · Agent version · Alerts · Active jobs
```

Remove global fleet counts. Use human operation labels such as **Scheduled shell command**. Show age—`26d ago`—with full timestamp on hover.

**Mobile**

Keep the resource identity and main actions sticky or near the top. Render only the selected tab, not large cross-resource summaries.

Resolved implementation:

- `Fleet / Instance detail` now starts with selected VPS identity followed by
  the required facts: `State`, `Last contact`, `Last IP`, `Agent version`,
  `Alerts`, and `Active jobs`.
- The global shell fleet KPI summary is hidden only for the canonical one-VPS
  detail route; broad fleet pages still retain the compact fleet summary.
- Summary panels use resource-rollup language, human workflow labels, and
  relative ages. `scheduled_shell_argv` is covered by regression tests as absent
  from the canonical VPS detail.
- Fresh screenshots:
  `./tmp/desktop-chrome/08-fleet-instance-detail-desktop-chrome.png`
  (`1440x1068`) and
  `./tmp/mobile-chrome/08-fleet-instance-detail-mobile-chrome.png`
  (`390x2270`).
- Verification passed:
  `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`;
  `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/components/ConsoleShell.tsx frontend/src/App.tsx frontend/src/styles/shell.css frontend/tests/release-ia-navigation.spec.ts frontend/src/panels/VpsDetailPanel.tsx frontend/src/styles/workspace.css frontend/src/styles/responsive.css frontend/tests/structured-screenshots.spec.ts'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "release IA reaches every configured page and subpage" --project=desktop-chrome --workers=1 --timeout=160000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "fleet instance detail is the canonical VPS route" --project=desktop-chrome --workers=1 --timeout=160000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=mobile-chrome --workers=1 --timeout=180000'`.

------

## Remote Operations

### 09 — Terminal — **P1**

**Resolved 2026-06-27 — direct terminal open, live-follow default, and advanced protocol controls**

**Issues**

- [x] A simple browser terminal sits alongside a large low-level protocol
  composer. Resolved by moving the generic terminal review composer into a
  closed `Advanced session controls` disclosure; the default page now leads with
  the terminal launcher, selected terminal, session inventory, and durable
  replay.
- [x] Opening a terminal requires “Prepare terminal review.” Resolved by
  replacing it with a stable `Open terminal` action that submits a
  `terminal_open` job directly from Remote Operations when privilege is
  unlocked, using the same canonical job privilege intent and payload hash as
  Jobs / Dispatch.
- [x] Session sequence numbers, replay range, window size, PTY values, and
  protocol details dominate the page. Resolved by keeping sequence/window
  evidence in compact context chips and row details; attach/poll/input/resize
  review controls open only from Advanced session controls.
- [x] The selected session says it is not following live output. Resolved by
  automatically following the selected replayable session and loading retained
  output into the terminal preview.
- [x] Many actions are icon-only. Resolved by keeping named compact actions:
  `Replay`, `Copy transcript`, `Download transcript`, `Input`,
  `Focus terminal`, `Follow`, `Attach`, `Poll`, `Resize`, and `Close`.
- [x] Transcript functionality appears even when not fully available. Resolved
  by enabling copy/download only after retained replay is loaded and by keeping
  unavailable transcript export as explicit browser replay state rather than a
  false backend export.
- [x] The page is 2,540 px desktop and 4,613 px mobile. Resolved by closing the
  advanced composer by default and adding `Focus terminal` full-screen mode for
  mobile/compact operation. Fresh screenshots are `1440x1544` desktop and
  `390x3659` mobile.

**Practical fix**

Primary layout:

1. VPS selector;
2. Open terminal;
3. active sessions;
4. selected terminal occupying remaining space.

New sessions should automatically follow output. Put protocol and replay internals in **Advanced session controls** or diagnostics. Use named actions: Follow, Reconnect, Close, Download transcript.

Opening a normal terminal should require no separate review page. If privilege is needed, unlock and open.

**Mobile**

Open the terminal in a dedicated full-screen mode. Session controls should be in a bottom sheet or overflow menu.

Resolved implementation:

- Remote Operations / Terminal now uses the intended production scan:
  `VPS selector -> Open terminal -> selected terminal -> session inventory`.
- The normal launcher submits a privileged `terminal_open` job directly from the
  Terminal page after unlock. Locked users are routed to the privilege vault
  from the same action instead of a review composer.
- Selected replayable sessions default to following retained output; the summary
  shows `Following` and the terminal preview loads durable replay.
- `Advanced session controls` remains available for protocol-level attach,
  poll, input, resize, and close reviews, preserving expert operations without
  dominating the default path.
- `Focus terminal` opens a full-screen terminal workspace for mobile or compact
  operation.
- Fresh screenshots:
  `./tmp/desktop-chrome/09-remote-operations-terminal-desktop-chrome.png`
  (`1440x1544`) and
  `./tmp/mobile-chrome/09-remote-operations-terminal-mobile-chrome.png`
  (`390x3659`).
- Verification passed:
  `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`;
  `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/panels/RemoteOperationsPanel.tsx frontend/src/panels/jobs/TerminalSessionsPanel.tsx frontend/src/styles/jobs.css frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "terminal open and resume stay" --project=desktop-chrome --workers=1 --timeout=160000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "remote operations owns terminal" --project=desktop-chrome --workers=1 --timeout=140000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=mobile-chrome --workers=1 --timeout=180000'`.

------

### 10 — Files — **P1**

**Resolved 2026-06-27 — compact file selection, named actions, metadata, and mobile editor focus**

**Issues**

- [x] A large blank editor is reserved before a file is selected. Resolved by
  replacing the pre-selection editor with the compact state
  `Select a VPS and file to begin.` and rendering CodeMirror only after a text
  file opens.
- [x] The whole page can be locked even for basic browsing. Resolved by keeping
  VPS/path selection visible while locked and explicitly stating that privilege
  unlock is required only when the UI reads the remote VPS filesystem.
- [x] Actions rely on small ambiguous icons. Resolved by replacing the selected
  file action rail with named compact actions: download, upload, move, create,
  permissions, owner, and delete.
- [x] `/` appears as an ordinary download context without explaining whether it
  downloads a file, directory, or archive. Resolved by naming root/directory
  download `Download folder as archive` in the toolbar, action group, and
  context menu.
- [x] Expected file metadata is missing or visually weak. Resolved by listing
  Name, Type, Size, Owner, Mode, and Modified for the selected path.
- [x] Save controls exist before there is anything to save. Resolved by showing
  mode and Review save only after a text file is open.

**Practical fix**

Before selection, show a compact state:

> Select a VPS and file to begin.

Only render the editor after a text file is selected. List:

```
Name · Type · Size · Owner · Mode · Modified
```

For a changed text file, show the diff inside one Save confirmation. Rename root/directory download to **Download folder as archive**. Place Follow symlinks under Advanced.

**Mobile**

Use file list → full-screen editor, not side-by-side panes squeezed into 390 px.

Resolved implementation:

- `FileBrowserPanel.tsx` now gates the editor behind an opened text file, adds
  a compact empty state, and gives save confirmation a bounded diff preview.
- The selected action group uses named buttons and places `Follow symlinks`
  under `Advanced file options`.
- Mobile CSS hides the file list/details while an editor is open and exposes a
  `Back to files` control, making the flow list → editor instead of squeezed
  panes.
- Verification:
  `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`;
  `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/panels/jobs/FileBrowserPanel.tsx frontend/src/styles/jobs.css frontend/tests/console-file-browser.spec.ts frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/console-file-browser.spec.ts -g "browses a VPS filesystem|single-file operation" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/console-file-browser.spec.ts -g "mobile file browser" --project=mobile-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "file browser reads" --project=desktop-chrome --workers=1 --timeout=160000'`.
  Fresh screenshots:
  `./tmp/desktop-chrome/10-remote-operations-files-desktop-chrome.png`;
  `./tmp/mobile-chrome/10-remote-operations-files-mobile-chrome.png`.

------

### 11 — Transfers — **P1**

**Resolved 2026-06-27 — default upload flow, ready-download language, and advanced reusable sources**

**Issues**

- [x] Terms such as handoff and reusable source artifact expose implementation
  concepts. Resolved by changing the normal Transfers and Files surfaces to
  `Ready downloads`, `Download`, `Retry`, `Transfer output`, and `Reusable
  upload sources`; API handoff wording remains internal only.
- [x] Upload source artifacts, download handoffs, and live transfer sessions
  are mixed together. Resolved by separating the default upload flow, ready
  downloads, transfer sessions, retry review, and advanced reusable sources.
- [x] Creating a reusable artifact is more prominent than the common upload
  operation. Resolved by making `Upload file` the primary form and moving
  reusable source management into a collapsed `Advanced: reusable upload
  sources` drawer after the transfer inventory.
- [x] Progress, transfer rate, retry state, and destination are difficult to
  understand at a glance. Resolved with table columns
  `Direction · VPS · Path · Size · Progress/speed · State · Action`, with
  `Ready to download`, `Retry`, `Completed`, progress bars, rate caps, path
  role, size, and chunk evidence visible.
- [x] Action icons are not self-explanatory. Resolved by replacing transfer row
  icon-only controls with compact labeled actions: `Download`, `Retry`, and
  `Job`, plus explicit selected-download controls. `Cancel` and `Delete` are
  not rendered as fake row actions because the current API does not expose a
  row-safe cancel/delete lifecycle: abort requires the original resume token,
  and retained-download deletion is not exposed as a transfer endpoint.

**Practical fix**

Default upload flow:

```
Choose local file → Choose VPS → Destination path → Upload
```

Place reusable source artifacts under Advanced.

Use a transfer table:

```
Direction · VPS · Path · Size · Progress/speed · State · Action
```

Use **Ready to download**, **Retry**, **Cancel**, and **Delete** rather than internal handoff language.

**Mobile**

Show active transfers first. Put artifact management in a separate collapsed section.

Resolved implementation:

- Remote Operations / Transfers now starts with a default upload flow:
  local file, target VPS, destination path, and `Upload`. Upload opens Jobs /
  Dispatch with a resumable upload preset and carries the selected local file
  forward for review.
- Ready downloads are separated from reusable sources and use operator copy:
  `Ready downloads`, `Ready to download`, `Download selected files`, and
  `Download`.
- The transfer grid follows the intended scan columns and uses readable state
  labels instead of raw handoff evidence as the primary signal.
- Reusable upload sources are kept in `Advanced: reusable upload sources` after
  the active transfer inventory. The drawer preserves expert reuse without
  making object-store source creation more important than normal upload.
- Cancel/delete lifecycle controls remain a backend/API contract gap rather
  than disabled UI: agent abort requires the original resume token and the
  transfer API does not expose retained-download deletion.
- Files handoff copy now points to `Transfer output` and `Open transfers`.
- Fresh screenshots:
  `./tmp/desktop-chrome/11-remote-operations-transfers-desktop-chrome.png`
  and
  `./tmp/mobile-chrome/11-remote-operations-transfers-mobile-chrome.png`.
- Verification passed:
  `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`;
  `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/panels/jobs/FileTransferSessionsPanel.tsx frontend/src/panels/jobs/FileBrowserPanel.tsx frontend/src/panels/RemoteOperationsPanel.tsx frontend/src/panels/JobDispatchPanel.tsx frontend/src/panels/jobs/JobOperationControls.tsx frontend/src/jobDispatchPreset.ts frontend/src/styles/jobs.css frontend/tests/console-file-transfer-handoff.spec.ts frontend/tests/console-transfer.spec.ts frontend/tests/console-file-transfer-empty.spec.ts frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/console-file-transfer-handoff.spec.ts --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/console-transfer.spec.ts -g "retained reusable source" --project=desktop-chrome --workers=1 --timeout=160000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/console-file-transfer-empty.spec.ts --project=desktop-chrome --workers=1 --timeout=120000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "file browser reads" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "job detail opens" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 2 of" --project=mobile-chrome --workers=1 --timeout=180000'`.

------

### 12 — Processes — **P1**

**Resolved 2026-06-27 — process inventory scan model and direct row actions**

Remote Operations / Processes now validates process chronology from
`started_unix` and `observed_at`. Impossible records show `Timeline
inconsistent` and `Unknown` uptime instead of presenting impossible Started and
Observed values as ordinary facts. The scan table now follows the intended
shape: `Process · VPS · State · CPU · Memory · Uptime · Restarts · Last exit ·
Actions`, with CPU and memory promoted to first-class columns, source job IDs
kept in expanded evidence, and long log paths wrapped in detail. The normal
Processes header no longer links to the unfinished Process Metrics page. Mobile
now renders process operation cards with CPU, memory, uptime, restarts, last
exit, and visible Logs / Restart / Stop actions. Restart now submits a canonical
`process_restart` job directly from the inventory when privilege is already
unlocked, preserving the shared payload-hash privilege assertion. Stop opens one
local `Confirm process stop` prompt and then submits a canonical `process_stop`
job; it no longer detours through the generic Dispatch review. Logs remain a
Dispatch read workflow because retained log byte count and output review still
belong in the supervisor log read form.

**Issues**

- Started and Observed times appear chronologically impossible.
- Process name, source, logs, and other values truncate quickly.
- Important resource values such as CPU and memory are not prominent.
- The page links to the unfinished Process Metrics page.
- Grammar such as “1 processes restarted” remains.
- Raw source IDs are exposed.

**Practical fix**

Validate chronology and show Unknown when records are invalid.

Columns:

```
Process · VPS · State · CPU · Memory · Uptime · Restarts · Last exit · Actions
```

Provide Logs, Restart, and Stop. Restart can be direct while unlocked; Stop gets one confirmation.

Hide Process Metrics until it works.

**Mobile**

Use one process card with resource usage and Restart/Stop actions visible.

Resolved implementation:

- Direct Restart uses the existing job contract: `process_restart`,
  `selector_expression: id:<client>`, `confirmed: true`, `destructive: true`,
  and a `buildPrivilegeForJobOperation` assertion. If privilege is locked, the
  action routes to the existing Privilege Vault unlock.
- Stop uses one in-page confirmation with operator-facing VPS labels, then
  submits the same canonical privileged job path for `process_stop`.
- The expanded row now states the real action model: Logs open Dispatch,
  Restart submits after unlock, and Stop uses one confirmation on the page.
- Fresh screenshots:
  `./tmp/desktop-chrome/12-remote-operations-processes-desktop-chrome.png`
  and
  `./tmp/mobile-chrome/12-remote-operations-processes-mobile-chrome.png`.

------

### 13 — Bulk Files — **P1**

**Resolved 2026-06-27 — target/path/run order, live scope summary, and post-run results**

**Issues**

- Review and privilege controls appear before the operator has fully described the operation.
- “Review targets” and “Review download” split one simple operation into multiple reviews.
- Local match count, server-resolved scope, and stale targets are not clearly distinguished.
- The execution summary occupies substantial space before execution.
- A stale target is not identified prominently by name.

**Practical fix**

Order the screen as:

```
Targets → Path/files → Live match summary → Run
```

Show:

> 3 matched · 2 ready · backup-nyc-03 stale

Use one confirmation for the final bulk operation. Unlock only when Run is selected. Replace the empty summary with results after dispatch.

**Mobile**

Keep the operation form and Run button together, then show results below.

Resolved implementation:

- The normal flow now reads as `Targets -> Path/files -> Live match summary -> Run`.
  Primary action labels use `Run download`, `Run upload`, and `Run ...` for
  advanced operations instead of presenting a second visible review step.
- `Refresh scope` remains available as an optional server-scope preview, while
  `Run` still re-resolves targets before opening the one confirmation prompt so
  stale cached selector results cannot execute.
- The privilege unlock panel moved below the described operation and live match
  summary, so operators describe the target/path/file first and unlock only
  when they are ready to run.
- The live match summary distinguishes browser-local estimates from
  server-resolved scope and names attention targets such as
  `backup-nyc-03 stale` before confirmation.
- The right pane is `Live match summary` before execution and changes to
  `Execution summary` only after a job is running or has produced results. The
  old empty per-target result placeholder no longer consumes space before a run.
- Fresh screenshots:
  `./tmp/desktop-chrome/13-remote-operations-bulk-files-desktop-chrome.png`
  and
  `./tmp/mobile-chrome/13-remote-operations-bulk-files-mobile-chrome.png`.

------

## Jobs

### 14 — Job History — **P1**

**Resolved 2026-06-26 — operator scan columns, row-open workflow, and raw evidence demoted to details**

Jobs / History now scans as operational execution evidence instead of raw job
metadata. The default grid follows:

```
Operation · Targets · Result · Duration · Started by · Age · Open
```

Operation names are humanized, target counts are explicit, completed jobs show
elapsed duration, actor evidence distinguishes operator-triggered jobs from
worker automation, and age uses relative text with exact timestamp in the
title. The subtitle now says `Latest execution records` instead of implying
the table contains only privileged requests. The Open action and whole row both
load target results, so details no longer depend on a tiny expander.

Raw job IDs, payload hashes, raw command types, actor IDs, timeout, and exact
created/completed timestamps are kept in row details rather than default scan
columns. Because the current history record does not expose a job-level error
summary or resolved target names, the detail view states that per-target exit
code and error evidence live in Target results. Focused desktop/mobile tests
verify the new columns/card shape, Open behavior, absence of the Payload column
from default scan, and raw payload hash availability in details.

**Issues**

- The subtitle says “Latest privileged requests,” but the table includes non-privileged jobs.
- Raw operation types and IDs dominate.
- Duration, requester, error summary, and useful target names are absent.
- Opening details relies on a tiny control.
- Payload hashes occupy table space without helping initial scanning.

**Practical fix**

Use:

```
Operation · Targets · Result · Duration · Started by · Age · Open
```

Translate `shell_argv` to **Shell command**, etc. Put IDs, hashes, and raw payloads in the detail drawer. Make the whole row openable.

**Mobile**

Show one job card with operation, target summary, result, duration, age, and Open.

------

### 15 — Job Dispatch — **P1**

**Resolved 2026-06-27 — grouped operations, explicit all-scope targeting, compact templates, and one dispatch action**

Jobs / Dispatch now treats the normal workflow as operation selection,
operation-specific fields, explicit target scope, impact preview, collapsed
execution options, and one final Dispatch confirmation. Operations are grouped
as Command, Files, Update, Backup, and Process in the desktop selector; Terminal
creation stays in Remote Operations / Terminal, while terminal-specific
composer flows still render only terminal controls. Mobile uses one native
operation select instead of a wall of operation tabs.

The target selector no longer starts with an editable `id:*` token. A blank
visible selector is explicitly labelled `All N scoped VPSs`, and dispatch,
preview, confirmation, privilege intent, and backend submission normalize that
scope to `id:*`. This preserves explicit all-fleet behavior without creating
malformed token edits such as `id:agentid:*`.

Template management is demoted behind a compact `Manage templates` disclosure,
leaving the template picker in the normal form. The primary action is now
`Dispatch`; it re-resolves targets and opens the same confirmation prompt, while
`Refresh target preview` is a secondary preview action that does not require
privilege. Execution options are collapsed: the current job request stores
timeout and privilege mode, while fleet concurrency remains governed by the
system dispatcher policy; unavailable canary and stop-after-failure controls
are not faked as per-job settings.

Fresh screenshots:

- `./tmp/desktop-chrome/15-jobs-dispatch-desktop-chrome.png`
- `./tmp/mobile-chrome/15-jobs-dispatch-mobile-chrome.png`

**Issues**

- Template management competes with the actual dispatch form.
- Numerous operation tabs create a dense wall of choices.
- An empty selector can silently mean all three scoped VPSs.
- Target preview and review controls are separated.
- Preview can be entangled with privilege state.
- Fleet safety options are absent from the normal flow, but adding a full rollout wizard would be excessive.

**Practical fix**

Group operations:

- Command;
- Terminal;
- Files;
- Update;
- Backup;
- Process.

Show only fields for the selected operation. State target scope explicitly:

> All 3 scoped VPSs

For multiple targets, provide one collapsed **Execution options** section:

- concurrency;
- timeout;
- stop after N failures;
- optional canary.

Templates belong in a small selector or Manage templates menu.

**Mobile**

Use a compact operation select instead of many tabs. Keep the primary Dispatch button sticky.

------

### 16 — Approvals — **P0**

**Status — implemented 2026-06-26**

Direct approve/reject row execution has been replaced with a compact review
decision prompt. The prompt shows operation, targets, requester, risk,
requested time, selector, payload, and request reason; approval accepts an
optional note and rejection requires an operator reason. The API now rejects
blank rejection reasons, so the rule is enforced by the business model rather
than the UI alone.

Additional mobile polish completed 2026-06-27: generic data-grid mobile cards no
longer repeat a `Decision`/`Action` field when the card already renders the
same row action, so Approval cards expose one clear `Review` action plus
`Details`.

**Issues**

- Approve and Reject are small direct row actions.
- Source code immediately submits the decision with `confirmed: true`.
- The reason is hard-coded rather than attributable to the operator.
- The row does not expose enough payload/target information before the decision.
- On mobile, the actions can be outside the initial visible area.

**Practical fix**

Replace direct actions with **Review**. The compact dialog should show:

- operation;
- targets;
- requester;
- risk label;
- requested time;
- payload summary.

Approve: optional note.
 Reject: required reason.

This remains a single dialog, not a multi-level approval process.

**Mobile**

Keep Review as the visible card action. Never require horizontal scrolling to approve or reject.

Verification passed 2026-06-27:

- `bash -ic 'cargo test -p vpsman-api job_approval -- --nocapture'`
- `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`
- `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/components/ConsoleDataGrid.tsx frontend/src/panels/JobsPanel.tsx frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "jobs approvals and scheduled runs stay separate" --project=desktop-chrome --project=mobile-chrome --workers=1 --timeout=90000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "generic data grids become actionable mobile cards" --project=mobile-chrome --workers=1 --timeout=90000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 3 of" --project=desktop-chrome --workers=1 --timeout=120000'`
- `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 3 of" --project=mobile-chrome --workers=1 --timeout=120000'`

Fresh screenshots:

- `./tmp/desktop-chrome/16-jobs-approvals-desktop-chrome.png` (`1440x906`)
- `./tmp/mobile-chrome/16-jobs-approvals-mobile-chrome.png` (`390x1202`)

------

### 17 — Scheduled Runs — **P1**

**Resolved 2026-06-26 — schedule-owned grid, source schedule DTO, and Run again semantics**

Jobs / Scheduled runs now uses the shared console data grid instead of a
hand-built history table. The scan columns are `Schedule · Operation · Targets ·
Due · Started · Result · Duration · Open`, mobile uses the shared actionable
card layout, and the page count uses operator-facing `schedule-created run`
copy.

The API `JobHistoryView` now carries `source_schedule_id`, and the frontend
joins that against loaded schedule records so the row shows the schedule name
and cadence when available. Raw job ID, payload hash, schedule ID, authority,
current next run, and the exact due-time data boundary live in expanded row
details. The previous implementation-gap strings (`schedule link not exposed`,
`due not exposed`, `Retry/worker health not exposed`) are removed from the scan
view. Retry is no longer shown; the mobile/card row action is **Run again** and
is disabled until a replay endpoint can preserve schedule source, due time,
targets, and privilege review.

**Issues**

- Internal backend limitations are shown directly: due time not exposed, schedule link not exposed.
- A completed job offers Retry, which is ambiguous.
- Schedule and job IDs are more visible than the schedule name.
- Cadence, duration, and target summary are weak.

**Practical fix**

Hide unavailable fields rather than turning implementation gaps into operator text.

Use:

```
Schedule · Operation · Targets · Due · Started · Result · Duration · Open
```

Rename Retry to **Run again** when the previous run succeeded. Link back to the schedule.

**Mobile**

Card layout with schedule name, result, due/start age, and Open/Run again.

------

### 18 — Job Artifacts — **P1**

**Resolved 2026-06-26 — typed artifact inventory, workflow actions, and raw evidence details**

Jobs / Artifacts now scans as:
`Artifact · Type · Source workflow · VPS/job · Created · Size · Verification · Action`.
Artifact rows use operator-facing types such as **Backup artifact**, **Transfer
package**, and **Agent update bundle** instead of raw domains. A compact
toolbar filter narrows the inventory by artifact type.

Human verification states now map raw artifact statuses to **Ready**, **Upload
incomplete**, **Verification failed**, or **Expired**. Raw object keys, download
paths, SHA-256 values, and raw statuses live in expandable row details with
Copy controls. The default action remains read-only and routes operators to the
owning workflow: Backups / Artifacts, Remote Operations / Transfers, or
Automation / Agent updates. Cleanup/destructive controls stay out of this page.

**Mobile verification addendum — 2026-06-26**

Structured screenshot batch 4 had regressed on mobile because long source
workflow labels in ConsoleDataGrid cards exceeded the viewport. Shared mobile
card containment now wraps header state, field links, and action labels without
horizontal scrolling. Fresh mobile and desktop batch 4 screenshots pass for
Jobs / Artifacts, and the mobile card keeps source workflow, size, action, and
Details visible.

**Issues**

- Backup, transfer, and agent-update artifacts are combined without enough differentiation.
- Internal states such as active/published are not explained.
- Created time, source VPS/job, verification, expiry, and primary action are weak or missing.
- Hashes and object URLs truncate.

**Practical fix**

Use:

```
Artifact · Type · Source workflow · VPS/job · Created · Size · Verification · Action
```

Human states:

- Ready;
- Upload incomplete;
- Verification failed;
- Expired.

Raw object URL and hash go into expandable details with Copy.

**Mobile**

Group artifacts by type or provide a simple filter. Keep Download/Open/Restore visible.

------

## Automation

### 19 — Schedules — **P0**

**Resolved 2026-06-26 — compact schedule registry, bounded future-run menu, and explicit automatic-run policy**

Automation / Schedules now scans as:

```
Name · Operation · Targets · Human cadence · Next run/Overdue · Last result · State
```

Past `next_run_at` / `next_runs` values are no longer presented as future run
chips. Enabled schedules with only stale run times show `Overdue` and
`Schedule calculation stale`. Rows show only the next future run; additional
future times are capped to five entries in a portaled menu so the grid does not
grow horizontally or vertically. Catch-up, retry, raw cron, target selector,
and error details move to expanded row evidence instead of competing with the
scan columns.

The page now states the execution model directly: enabled schedules
automatically dispatch future jobs from their saved target snapshot, Run now is
one manual dispatch, and Jobs / Approvals is a separate workflow. Row actions
surface Run now, Enable, Disable, and Edit first, so mobile cards expose the
operator's normal schedule actions without horizontal scrolling.

**Issues**

- Dates from May are displayed as “Next runs” in late June.
- The UI does not distinguish past due, stale schedule calculation, and future execution.
- Multiple future-time chips consume considerable width.
- Cron, human cadence, timezone, catch-up, and retry compete for attention.
- It is unclear whether an enabled schedule runs automatically or generates approval work.

**Practical fix**

A schedule row should show:

```
Name · Operation · Targets · Human cadence · Next run/Overdue · Last result · State
```

Show only the next run; place the next five in a popover.

An enabled schedule should normally authorize its future runs. Where a schedule intentionally requires approval, state this explicitly as a policy option.

**Mobile**

A schedule card should fit name, cadence, next run, state, and Enable/Run now without horizontal scrolling.

------

### 20 — Runbooks — **P2**

**Resolved 2026-06-26 — operator runbook cards, honest last-run evidence, and custom-template management access**

Automation / Runbooks now keeps the reusable-operation abstraction compact and
direct. Runbook cards show human operation labels such as `Shell command`
instead of raw command types, and the catalog summary labels global job history
as `Latest loaded job` rather than implying it is the selected runbook's last
run.

Per-card evidence now shows `Last result` with result and relative age when a
loaded job history row matches the template command type. When no matching row
is loaded, the card says `No loaded run` and explains the missing command-type
evidence instead of showing `No matching run` beside unrelated global job
activity. Raw job IDs stay out of the scan card.

The primary action is now **Run**, opening Jobs / Dispatch with the template,
scope, and timeout prefilled. Review inputs are collapsed behind a short
`Review inputs` disclosure so mobile cards stay short. Custom runbooks expose a
visible **Manage** menu with Edit, Duplicate, and Delete routes into Dispatch,
where the existing command-template save/delete controls live.

This is one of the better pages. The abstraction matches the product and reduces repeated operator input.

**Remaining issues**

- Last run is represented mainly by a raw job ID.
- Cards can say no matching run while unrelated global job activity is visible.
- Internal operation names remain.
- Custom runbooks lack obvious Edit/Duplicate/Delete access.

**Practical fix**

Show last result and relative time. Translate operation types. Use one primary action: **Run** or **Open in Dispatch**. Put Edit, Duplicate, and Delete in an overflow menu.

**Mobile**

Keep runbook cards short; hide parameter details until opened.

------

### 21 — Source Templates — **P1**

**Resolved 2026-06-26 — registry-first authoring, detail drawer workflows, and honest source readiness**

Automation / Source templates now makes the template registry the primary
surface. Creation opens a **New source template** drawer, and selecting a
template opens a closeable detail drawer with explicit Assign, Render, and
Test / update tabs. The page no longer shows create, assign, render, clone,
test, diff, and update forms all at once.

Active source status is collapsed into an evidence disclosure and its summary
uses the same readiness model as the rows, so states such as `selected_no_store`
count as needing review. Scan rows show human labels such as
`Source selected; server storage not configured`; raw backend states remain
available only in expanded details.

Config / Templates has been renamed to **Template coverage** and kept
read-only. It shows runtime coverage and source-readiness posture while linking
to Automation / Source templates for persistent authoring, removing the
authoring/coverage overlap.

**Mobile verification addendum — 2026-06-26**

Structured screenshot batch 4 had regressed on mobile because long template
domains and action labels exceeded the card width. Shared ConsoleDataGrid mobile
cards now wrap state text and action labels, so Source Templates cards keep
template name, domain, scope, assigned count, updated time, and Open / Assign /
Edit / Details controls visible without horizontal scrolling.

**Issues**

- Registry, source status, create, assign, render, clone, test, diff, and update workflows coexist on one long page.
- The page says zero need attention while showing attention-style source states.
- Internal statuses such as `selected_no_store` are exposed.
- “New” appears while creation controls are already visible elsewhere.
- Empty selector behavior can mean all scoped VPSs without enough emphasis.
- It overlaps with Config / Templates.

**Practical fix**

Make the template registry primary. Selecting a template opens a detail drawer with assignment, render, test, clone, and update.

Use + New to open creation. Humanize states and make the attention count truthful.

Rename Config / Templates to **Template coverage** or **Source coverage**, reserving Source Templates for authoring.

**Mobile**

Show only registry or selected-template detail, not every lifecycle tool at once.

------

### 22 — Agent Updates — **P0/P1**

**Resolved 2026-06-26 — honest registry model, compact release drawer, and artifact-gated actions**

Automation / Agent updates now separates release metadata from update
approval. The page derives an explicit registry model from Suite Config:
enforced registries are described as a manual-hash gate, while non-enforced
registries are described as advisory release metadata for audit and Dispatch
prefills. The registration confirmation states that recording metadata does
not approve or start an update.

The primary screen now exposes current fleet version posture, available
version, registered artifact, target/update path, registry policy, health
checks, and rollback readiness. Current version telemetry is shown honestly as
unavailable when loaded agents do not expose build/version data. GitHub-specific
copy was replaced with generic Check update language. Start update is available
only when a registered artifact hash exists, and Rollback is disabled with a
visible reason until the latest release records rollback artifact metadata.
Release registration now opens as a closeable drawer so long metadata fields no
longer dominate the default workflow. Focused desktop/mobile tests verify the
new posture labels, Start update dispatch prefill, disabled rollback reason,
and screenshot manifest required text.

**Issues**

- The screen calls the registry an approval mechanism while also stating the registered-update policy is not enforced.
- Activate staged and Rollback appear even where no staged/rollback artifact is available.
- Current fleet version telemetry is unavailable, weakening update decisions.
- GitHub-specific wording leaks into a generic update workflow.
- Registration and rollout fields are long and always visible.

**Practical fix**

Choose one honest model:

1. **Registry is informational:** describe it as release metadata; or
2. **Registry is enforced:** reject unregistered hashes.

Do not imply both.

Primary screen:

```
Current version posture · Available version · Registered artifact · Targets · Update
```

Disable unavailable actions with a reason. Put release registration in a drawer. For multiple targets, provide optional canary/concurrency controls, not a large deployment platform.

**Mobile**

Keep Check update and Start update near the top. Collapse release metadata and rollback details.

------

## Network

### 23 — Network Overview — **P2**

**Resolved 2026-06-26 — actionable workflow cards, stale evidence, and direct tunnel creation**

Network / Overview now keeps the compact overview shape but makes the workflow
tiles read as actionable controls with explicit right-side Open affordances and
concise tooltips. Operator-facing copy now explains `Observed tunnels to save`
instead of the more abstract promotion-candidate term, and OSPF review is
framed as cost changes waiting for review.

The latest evidence summary now derives the newest timestamp across
observations, trend rollups, OSPF recommendations/update evidence, and telemetry
tunnel reports. It displays relative age plus `stale` or `current`, with the
full timestamp available through the title, so old network evidence is no
longer presented as an ordinary raw timestamp.

The overview header now has a direct **Create tunnel** primary action. It routes
to Network / Tunnel plans and opens the Create tunnel plan workflow immediately,
without adding another wizard or subpage.

This is structurally good and reasonably compact.

**Remaining issues**

- Workflow cards do not clearly look clickable.
- Latest evidence is shown as an old timestamp without a freshness warning.
- Terms such as promotion candidate and OSPF review need concise help.
- There is no obvious direct Create tunnel action from the overview.

**Practical fix**

Make the whole card clickable. Add relative age and stale status. Provide concise tooltips and an optional **Create tunnel** primary action.

**Mobile**

The overview cards stack acceptably; reduce the repeated global shell above them.

------

### 24 — Network Graph — **P1**

**Resolved 2026-06-26 — stale evidence badge, compact controls, and list-first mobile graph**

Network / Graph now shows a freshness badge beside the Topology graph title
using the newest generated, node, or edge evidence timestamp. The badge uses
relative age plus `stale` or `current` and preserves the full timestamp in the
title, so old topology evidence is not presented as ordinary current graph
state.

The graph controls are compact and labelled: health filtering is a View select,
viewport actions are named Zoom out, Reset, and Zoom in buttons, and the tiny
minimap is hidden for small topologies. The summary above the graph is reduced
to Layers, OSPF cost, and Measurements.

Unavailable values are removed from the scan path: empty latency curves no
longer render as `No curve`, and the cost column uses the same `OSPF 22 (+8)`
recommendation wording used by Network / OSPF. On mobile, the tunnel list is
the default scanning surface and the visual graph opens on demand.

**Issues**

- Graph data is from May 31 but is displayed without a prominent stale-data warning.
- Filter tabs wrap awkwardly.
- The toolbar is icon-heavy and lacks clear labels.
- Summary cards take considerable space before the graph.
- Unknown/no-curve values add noise.
- OSPF values disagree with other screens.
- A minimap offers little value for a tiny topology and becomes distracting on mobile.

**Practical fix**

Put a visible freshness badge beside the title:

> Last topology evidence: 26d ago — stale

Use a compact filter dropdown or segmented control. Add tooltips to graph controls. Hide unavailable values. Resolve the OSPF source inconsistency.

For small fleets, the graph can remain simple. No need for complex layer systems.

**Mobile**

Default to a tunnel/node list. Open the graph full-screen on demand.

------

### 25 — Tunnel Plans — **P1**

**Resolved 2026-06-27 — compact registry, clear disabled bulk actions, and mobile card layout**

- [x] Selection-dependent bulk actions are disabled when no plan row is
  selected, use a visibly disabled secondary-button state, and sit beside the
  reason text `Select plan rows for bulk enable, disable, or export.`
- [x] The registry columns follow the intended operator scan:
  `Plan`, `Endpoints`, `Desired state`, `Runtime state`, `Health`,
  `OSPF cost`, `Updated`, and row action/detail controls.
- [x] Plan names and endpoints wrap instead of truncating important endpoint
  identity. Desired lifecycle state, runtime state, and health are separated.
- [x] Bandwidth displays with explicit Mbps units, and the registry states the
  actual model: latency/loss plus a bounded sqrt bandwidth penalty, manual
  speed-test evidence, and separate monitoring/auto-OSPF state.
- [x] Mobile now renders compact cards with the toolbar, search, primary
  workflow buttons, disabled bulk action, field chooser, and pagination visible
  without the former huge blank stretched-search region.
- [x] Fresh screenshots:
  `./tmp/desktop-chrome/25-network-tunnel-plans-desktop-chrome.png` and
  `./tmp/mobile-chrome/25-network-tunnel-plans-mobile-chrome.png`.

**Issues**

- Selection-dependent actions appear available when no plan is selected.
- Plan names and endpoints truncate.
- Enabled and Planned are shown together without clearly separating desired and runtime state.
- Values such as `100m` lack clear units.
- “Manual speed tests only” conflicts with surrounding latency/auto-OSPF language.
- Bandwidth is not limited to three fixed presets. The intended model is an
  operator-typed Mbps value, with the OSPF cost preview recalculated live as
  the operator adjusts bandwidth, latency, loss, preference, or priority.

**Practical fix**

Disable selection actions and explain why.

Columns:

```
Plan · Endpoints · Desired state · Runtime state · Health · OSPF cost · Updated · Action
```

Use explicit units such as `100 Mbps`. Clarify whether auto-OSPF is enabled, monitoring-only, or manual.

Treat bandwidth as a numeric Mbps input in create/promotion/edit workflows, not
as a fixed 10/100/1000 tier selector. Show the computed OSPF cost preview next
to the editable values so operators can tune bandwidth and preference before
saving.

**Mobile**

Use cards. Avoid rendering the desktop table as a mostly blank horizontally scrollable region.

------

### 25b — Create Tunnel Plan — **P1**

**Resolved 2026-06-27 — three-step create workflow with adjacent OSPF preview**

- [x] Creation uses three compact review sections:
  `Endpoints & type`, `Addresses & routing`, and `Review & create`.
- [x] Sections only read ready after their responsible inputs validate, while
  save-blocking state remains visible in the review strip and disabled save
  affordance.
- [x] The lifecycle control uses positive wording:
  `Enable after save: On/Off`.
- [x] Bandwidth is a numeric `Bandwidth Mbps` input over the operator range,
  not a preset tier selector. The `OSPF cost` preview recalculates live beside
  bandwidth, latency, packet loss, and preference/priority.
- [x] Mobile create mode hides the plan registry, keeps the close button
  visible, uses a linear form, and no longer contains severe empty or overflow
  regions.
- [x] Fresh screenshots:
  `./tmp/desktop-chrome/25b-network-tunnel-plans-create-desktop-chrome.png`
  and
  `./tmp/mobile-chrome/25b-network-tunnel-plans-create-mobile-chrome.png`.

**Issues**

- Seven status/step cards make tunnel creation appear more complex than necessary.
- Some steps look complete before valid inputs exist.
- A checkbox uses negative wording such as Disabled.
- Bandwidth is a typed Mbps planning input, not a fixed preset list. The form
  should preview OSPF cost live from bandwidth, latency, loss, and
  preference/priority.
- The plans table remains above the creation form.
- Save-blocking reasons are scattered.
- The mobile screenshot contains severe empty/overflow regions.

**Practical fix**

Use three compact sections:

1. Endpoints and tunnel type.
2. Addresses and routing.
3. Review and create.

Mark sections complete only after validation. Use a positive switch:

> Enable after save — Off

Use a numeric **Bandwidth Mbps** input and a visible **OSPF cost preview** that
updates as the operator changes bandwidth, latency, loss, and preference.

Show errors beside the responsible field and one concise readiness line beside Create.

**Mobile**

Create mode should hide the plans table. Use a linear form and sticky Create button. Correct the overflow/blank-region defect before release.

------

### 25c — Tunnel Promotion — **P1**

**Resolved 2026-06-27 — observed-to-saved comparison and focused promotion form**

- [x] Promotion starts with three comparison tiles:
  `Observed source`, `Observed -> saved/proposed`, and `Review gate`, then
  shows the promotion inputs instead of a long stack of state cards.
- [x] Selecting observed telemetry pre-fills safe fields such as peer/side,
  underlay hints, name, and observed endpoint evidence while leaving
  operator-owned CIDR/routing edits visible.
- [x] The relationship between observed topology, saved-plan comparison, and
  proposed plan is explicit before save.
- [x] The primary action is `Save managed plan`, activation uses
  `Enable after save: On/Off`, and custom adapter/raw payload controls stay
  under Advanced.
- [x] Mobile promotion mode hides the existing-plan registry and keeps the
  selected observed source/review gate visible in a compact header shape.
- [x] Fresh screenshots:
  `./tmp/desktop-chrome/25c-network-tunnel-plans-promotion-desktop-chrome.png`
  and
  `./tmp/mobile-chrome/25c-network-tunnel-plans-promotion-mobile-chrome.png`.
- [x] Verification passed for `25`, `25b`, and `25c`:
  `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`;
  `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/styles/topology.css frontend/src/styles/shell.css frontend/src/panels/TopologyPanel.tsx frontend/src/panels/topology/TopologyPromotionPanel.tsx frontend/tests/structured-screenshots.spec.ts frontend/tests/console-layout.spec.ts'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/console-layout.spec.ts -g "authors custom adapter tunnel plans|promotes saved observed tunnel plans|promotes telemetry candidates" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "network tunnel plans owns promotion" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 5 of" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 5 of" --project=mobile-chrome --workers=1 --timeout=180000'`.

**Issues**

- The page uses many workflow-state cards before showing the actual promotion inputs.
- Selecting observed telemetry does not appear to prefill enough fields.
- Saved plan list and promotion form compete for space.
- The relationship between observed plan, current saved plan, and proposed plan is not immediately visible.
- “Plan enabled” is ambiguous during creation.

**Practical fix**

After selecting an observation, prefill every safe field. Show only missing or conflicting values.

Provide a simple comparison:

```
Observed → Saved/proposed
```

Use one **Save managed plan** action and an optional **Enable after save** switch. Keep adapter/raw payload controls under Advanced.

**Mobile**

Hide the existing-plan table while promotion is active and keep the selected observed source visible in a compact header.

**Implementation progress — 2026-06-26**

- Replaced fixed bandwidth tiers with numeric `bandwidth_mbps` fields across
  shared network planning models, API views, topology graph payloads, CLI/VTY
  inputs, frontend types, tests, and fixtures.
- Tuned the OSPF formula for arbitrary 10-10000 Mbps values:
  `latency + loss * 400 + 10 * sqrt(100 / clamp(bandwidth_mbps, 10, 10000))`,
  then divided by `max(preference, 0.1)` and clamped to Bird-safe OSPF cost
  bounds. The square-root term keeps higher bandwidth preferred while
  preventing large typed values from overwhelming latency or loss.
- Create and promotion workflows now expose `Bandwidth Mbps`, `Packet loss %`,
  and `Preference / priority` beside a live `OSPF cost` preview.
- Mobile create and promotion workflow modes now hide the existing plan table
  while the workflow is active, keeping the operator focused on the form and
  close action. The final 2026-06-27 resolved block above covers the remaining
  workflow simplification, prefill, mobile blank-region, and disabled-action
  requirements.
- The create workflow review strip now uses three compact sections:
  `Endpoints & type`, `Addresses & routing`, and `Review & create`; the
  activation checkbox uses positive `Enable after save: On/Off` wording.
- Promotion now uses three comparison tiles: `Observed source`,
  `Observed -> saved/proposed`, and `Review gate`. The external-observe flow
  has one primary `Save managed plan` action, positive
  `Enable after save: On/Off` wording, and keeps custom adapter/raw command
  controls under Advanced.
- Selecting a telemetry candidate now pre-fills safe peer, side, local underlay,
  peer underlay, name, and observed endpoint hints where available while leaving
  operator-owned CIDR/routing edits visible.
- The OSPF cost model is now regression-tested for arbitrary typed bandwidth
  values across the `10..10000 Mbps` range, including a full integer sweep
  proving costs never increase as bandwidth rises and never move by more than
  two cost units for a one-Mbps operator tweak. At the default 20 ms / 0% loss /
  priority 1 baseline, the reviewed anchor costs are `10 Mbps -> 52`,
  `100 Mbps -> 30`, `1000 Mbps -> 23`, and `10000 Mbps -> 21`; high bandwidth
  is a diminishing-return preference, not a way to hide bad latency/loss.
- Added an explicit backend and TypeScript preview guard that the full
  `10 Mbps -> 10000 Mbps` bandwidth advantage stays secondary to materially
  worse latency or packet loss at equal priority, while preference/priority
  remains a deliberate operator override.
- The live TypeScript OSPF preview now has matching regression coverage for
  the arbitrary-Mbps curve, range clamps, preference/priority bias, and
  packet-loss contribution, so the inline preview remains aligned with the
  backend planner as operators type values.
- Backend and TypeScript preview tests now also guard former tier boundaries
  (`100`, `1000`, `5000`, `10000` Mbps) against hidden cliffs, so arbitrary
  operator-typed bandwidth values remain smooth rather than behaving like the
  old preset model.
- The backend OSPF calculation now sanitizes non-finite latency, loss,
  preference, and policy-weight values before clamping, and the frontend
  preview test covers temporary numeric form states. Valid operator inputs keep
  the same reviewed anchors across `10..10000 Mbps`.
- Effective bandwidth evidence now uses the same `10..10000 Mbps` clamp as the
  OSPF calculation, so recommendation evidence and preview math cannot disagree
  at range edges.
- Fresh desktop and mobile screenshot review passed for
  `25b-network-tunnel-plans-create` and
  `25c-network-tunnel-plans-promotion`: close buttons are visible, the mobile
  registry is hidden while workflows are active, and the OSPF preview sits next
  to the operator-tweakable bandwidth/latency/loss/preference fields.

------

### 26 — Network Tests — **P1**

**Resolved 2026-06-26 — read-only inspect, unlocked probe, capped speed review, and sparse evidence cards**

Network / Tests now separates the three operator actions by actual risk.
`Inspect status` runs immediately as a read-only `network_status` job without a
local privilege assertion. `Run probe` stays immediate after local unlock and
cannot be submitted through the API as an unprivileged job. `Review speed test`
remains the only Network Tests confirmation prompt because it opens a temporary
peer flow and consumes capped traffic.

Sparse one-bucket probe/speed evidence now renders as compact numeric evidence
cards instead of mini trend charts. The throughput card compares the latest
measured average against the selected plan baseline and shows the degraded
state inline, for example `10.1 Mbps avg - 10% of expected 100 Mbps`. The
header stays compact (`1 sample`) while the warning lives in the body, avoiding
desktop/mobile truncation.

Verification passed for the Network Tests slice: API tests cover unprivileged
status acceptance and unprivileged probe rejection; frontend TypeScript passes;
the Impeccable detector is clean; focused console, release navigation, stale
confirmation, and structured screenshot batch 5 tests pass on the relevant
desktop/mobile projects. Fresh screenshots reviewed:
`tmp/desktop-chrome/26-network-tests-desktop-chrome.png` and
`tmp/mobile-chrome/26-network-tests-mobile-chrome.png`.

**Issues**

- Status inspection, probe, and speed tests are all blocked behind the same unlock state.
- Read-only status inspection should not require write privilege.
- Sparse one-point charts visually imply trends.
- A measured result around 10 Mbps is shown against a 100 Mbps baseline without strong warning.
- Multiple review buttons fragment the operation.
- Timeout and speed-cap fields are not clearly distinguished.

**Practical fix**

- Inspect status: immediate.
- Probe: immediate while unlocked, or one compact confirmation.
- Speed test: one confirmation showing expected data amount and duration.

When fewer than two points exist, show a numeric result rather than a line chart. Show baseline comparison:

> 10.1 Mbps — 10% of expected 100 Mbps

Use one plan selector and clear Test buttons.

**Mobile**

Stack test types as compact cards and show results immediately after each test.

------

### 27 — Network OSPF — **P0**

**Resolved 2026-06-26 — immutable recommendation identity, apply-bound API contract, and rollback gate**

Network / OSPF now exposes one reviewed recommendation object with
Recommendation ID, current cost, proposed cost, evidence time, and evidence
summary. The recommendation/update-plan API returns the same
`recommendation_id`, and the OSPF cost mutation request must include that ID
plus an explicit `apply` or `rollback` intent. Apply requests are checked
against the server-recomputed recommendation object before the tunnel plan cost
is changed, and audit metadata records the recommendation ID and intent.

The UI now uses one Apply action and one confirmation for the reviewed network
change. Rollback is disabled until a successful Apply in the panel creates a
concrete rollback value, so a rollback prompt cannot appear before an OSPF
apply occurred. Runtime auto-OSPF telemetry that can contain older external
updater values is labelled as an observed updater report rather than the
proposal to apply. Stale evidence is called out by age, and the 10.1 Mbps
measurement against the 100 Mbps baseline is shown as a warning before Apply.

**Issues**

- The table shows `14→21`, while the review area shows `14→22`.
- Rollback is visible before an apply operation has occurred.
- Old evidence is presented without age emphasis.
- Cost, status, recommendation, and monitoring state use compact internal language.
- A 10 Mbps effective result against a 100 Mbps baseline is not emphasized sufficiently.

**Practical fix**

Create one immutable recommendation object:

```
Recommendation ID · Current cost · Proposed cost · Evidence time · Evidence summary
```

All screens and API actions must use that object. Disable rollback until a successful apply has produced a rollback value.

Show the baseline mismatch as a warning and keep one confirmation for applying the network change.

**Mobile**

Put current/proposed cost and Apply at the top. Place evidence below.

------

### 28 — Network Evidence — **P1**

**Issues**

- “Command output 3 pending” appears to mean output has not been loaded, not that commands are pending.
- One visualization area looks empty or broken.
- Observations, tests, recommendations, approvals, and command jobs are mixed.
- Confidence and health are conflated.
- Roughly 10 Mbps against a 100 Mbps expectation is labelled too positively.
- Old timestamps and raw IDs dominate.

**Practical fix**

Rename:

> 3 outputs not loaded

Group evidence into:

1. recommendation;
2. measurements;
3. status/probe results;
4. related jobs.

Keep confidence separate from health. Determine health using the configured baseline. Hide visualization containers that have no useful data.

**Mobile**

Use a chronological evidence list. Do not squeeze the desktop matrix into a phone.

**Resolved 2026-06-27 — grouped evidence, baseline health, and mobile evidence list**

- Network / Evidence now groups the page into `Recommendation evidence`,
  `Measurement evidence`, `Status and probe results`, and `Related command
  jobs`, so recommendations, measurements, persisted observations, and command
  jobs are no longer mixed into one undifferentiated table.
- Retained command output now reads as `3 outputs not loaded` and row details
  use `Output not loaded`, avoiding the misleading `pending` wording for jobs
  that already completed but whose retained output has not been fetched.
- OSPF recommendation confidence is shown as its own line (`Confidence
  Measured`) while the health badge is derived from operator-useful evidence.
  The configured/effective bandwidth baseline now marks `10.1 Mbps avg - 10%
  of expected 100 Mbps` as `Degraded`, rather than letting measured confidence
  look healthy.
- Visible recommendation rows no longer lead with raw recommendation IDs or full
  timestamp-heavy evidence summaries. They show the cost, bandwidth baseline,
  sample count, compact recency, privilege state, and approval-scope count.
- Empty latency visualizations are hidden until enough points exist to draw a
  meaningful curve.
- Mobile Network / Evidence now stacks evidence rows into a chronological
  evidence list instead of rendering the desktop matrix as clipped columns.
- Fresh screenshots reviewed:
  `./tmp/desktop-chrome/28-network-evidence-desktop-chrome.png`
  (`1440x1823`) and
  `./tmp/mobile-chrome/28-network-evidence-mobile-chrome.png` (`390x3023`).
  Desktop and mobile show the grouped evidence sections, degraded throughput
  baseline, unloaded-output copy, and no clipped evidence matrix.

------

## Backups

### 29 — Backup Overview — **P0/P1**

**Resolved 2026-06-27 — decision-first overview with direct backup actions**

- [x] The overview uses the four intended protection states:
  `Recent`, `Overdue`, `Unprotected`, and `Unknown`; metadata-only
  `artifact_metadata_recorded` evidence does not count as usable backup
  protection.
- [x] The first operator surface is now recoverability decision, affected VPSs,
  and the three direct actions: `Back up now`, `Create policy`, and `Restore`.
- [x] Supporting records are compact links instead of a lifecycle-card map, so
  Requests, Policies, Artifacts, Restore, and optional Migration remain
  reachable without dominating the page.
- [x] Migration is neutral until used: empty migration state now reads
  `not used` / `Not used` and explains that migration is optional unless a
  replacement or cutover workflow starts.
- [x] Artifact evidence is separated into `recorded`, `uploaded`, and
  `verified` states, with text clarifying that recorded metadata alone is not
  usable recovery evidence.
- [x] Detailed posture cards are collapsed behind `Detailed posture`, reducing
  ceremony while preserving the deeper recoverability, retention, restore, and
  policy evidence for operators who need it.
- [x] Mobile shows the protection decision, affected VPSs, and three actions
  before supporting records and detailed evidence. Current evidence rows wrap
  instead of truncating artifact and migration explanations.
- [x] Fresh screenshots:
  `./tmp/desktop-chrome/29-backups-overview-desktop-chrome.png` and
  `./tmp/mobile-chrome/29-backups-overview-mobile-chrome.png`.
- [x] Verification passed:
  `bash -ic 'cd frontend && npm exec -- tsc --noEmit'`;
  `bash -ic 'node .agents/skills/impeccable/scripts/detect.mjs --json frontend/src/panels/BackupsPanel.tsx frontend/src/styles/backups.css frontend/tests/release-ia-navigation.spec.ts frontend/tests/structured-screenshots.spec.ts'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/release-ia-navigation.spec.ts -g "backups overview explains" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 6 of" --project=desktop-chrome --workers=1 --timeout=180000'`;
  `bash -ic 'cd frontend && npm exec -- playwright test tests/structured-screenshots.spec.ts -g "screenshot batch 6 of" --project=mobile-chrome --workers=1 --timeout=180000'`.

**Earlier implementation note — age-based protection states implemented 2026-06-26**

The overview now reports Recent, Overdue, Unprotected, and Unknown backup
protection states instead of a broad protected count. The fixture’s old
metadata-only backup is no longer treated as healthy. The final 2026-06-27
resolved block above covers the later lifecycle-card reduction, neutral
migration state, primary backup actions, and mobile ordering requirements.

**Issues**

- Any non-error artifact-backed backup can contribute to “protected,” regardless of age.
- `artifact_metadata_recorded` is treated too positively.
- An old backup is not clearly Overdue.
- Migration not planned appears like an attention item even when migration is irrelevant.
- The page contains many lifecycle cards and ownership explanations.
- “Needs restore test” may be valid, but the surrounding model feels more elaborate than required.

**Practical fix**

Use four simple protection states:

- **Recent:** successful usable backup within expected interval.
- **Overdue:** latest backup older than expected.
- **Unprotected:** no policy or usable backup.
- **Unknown:** data unavailable.

Separate artifact states:

- Recorded;
- Uploaded;
- Verified.

Keep migration neutral until used. Overview should prioritize Back up now, Create policy, and Restore.

**Mobile**

Show the protection decision, affected VPSs, and three actions first. Collapse deeper posture details.

------

### 30 — Backup Requests — **P1**

**Resolved 2026-06-26 — compact request history with artifact/retry actions**

Backups / Requests no longer repeats the full backup posture block. It now
leads with one compact summary line such as
`0 recent · 2 unprotected · 0 failed`, followed by request and artifact counts.
The request grid/card shape is operator-first:
`VPS`, `Paths`, `State`, `Size`, `Started`, `Duration`, `Artifact`, and
`Action`. Request IDs, payload hashes, source job/schedule IDs, requester
availability, notes, and raw status remain in row details.

Artifact-backed rows expose **Open artifact**, which takes the operator to the
artifact inventory. Rows without usable artifact evidence expose **Retry** by
prefilling the existing reviewed backup-request drawer from the selected
request. The raw `artifact_metadata_recorded` state now renders as
`Recorded` with `content not verified` beside it, instead of implying a
verified upload.

Screenshots regenerated and reviewed:
`./tmp/desktop-chrome/30-backups-requests-desktop-chrome.png` and
`./tmp/mobile-chrome/30-backups-requests-mobile-chrome.png`.

**Issues**

- The full backup posture block is repeated above the records.
- Request IDs and artifact IDs are more prominent than useful operational details.
- Age, duration, size, requester, and result detail are weak.
- A raw status such as artifact ready does not explain upload/verification state.

**Practical fix**

Replace the repeated posture block with one line:

> 1 recent · 2 unprotected · 0 failed

Use:

```
VPS · Paths · State · Size · Started · Duration · Artifact · Action
```

Provide Open artifact or Retry where appropriate.

**Mobile**

Place Open backup request beside the heading and render request cards below.

------

### 31 — Backup Policies — **P1**

**Resolved 2026-06-26 — automatic-policy registry and clear empty state**

Backups / Policies no longer repeats the full backup posture block. It now
uses a compact policy summary for enabled, paused, failing, fixed-target, and
next-run evidence. The primary page action is **Create policy**, matching the
operator task.

The policy registry copy no longer says scheduled selectors materialize as
approval-required jobs. It states the intended model directly: enabled policies
run automatically on their UTC cadence, while prune remains separate retention
maintenance. The policy row model is:
`Name`, `Targets`, `Frequency`, `Next run`, `Retention`, `Last result`, and
`State`. Raw schedule IDs, catch-up policy, retry cadence, selector expression,
scope, and next-run count stay in details.

When no policy exists, the empty state now says:
`No scheduled backups` and explains that the operator should create a policy
for automatic backups or use Back up now in Requests for a one-time backup.
Screenshots regenerated and reviewed:
`./tmp/desktop-chrome/31-backups-policies-desktop-chrome.png` and
`./tmp/mobile-chrome/31-backups-policies-mobile-chrome.png`.

**Issues**

- Backup posture repeats again.
- Text says scheduled selectors materialize as approval-required jobs, which makes routine scheduled backup behavior unclear.
- The empty policy state does not strongly explain the consequence.
- Protect and policy editing concepts overlap.

**Practical fix**

An enabled policy should normally run automatically. Where approvals are intentionally required, expose that as an explicit policy option.

Policy row:

```
Name · Targets · Frequency · Next run · Retention · Last result · State
```

Empty state:

> No scheduled backups. Create a policy or use Back up now.

**Mobile**

Primary action should be Create policy. Keep posture to one compact warning.

------

### 32 — Backup Artifacts — **P1**

**Resolved 2026-06-26 — inventory-first artifacts with direct restore/download actions**

Backups / Artifacts now makes the artifact inventory primary instead of
repeating the full backup posture block. The grid follows the intended operator
shape: `Artifact`, `VPS`, `Created`, `Size`, `Verification`, `Retention`,
`Restore`, and `Download`. Restore and Download are direct row/card actions;
Restore opens the restore workflow with the linked backup request selected, and
Download uses the existing artifact package endpoint. Object key, checksum, raw
status, and request lineage moved into row details.

The former handoff wording has been changed to operator-facing transfer-package
language in the page header, guide, drawer form, and confirmation prompt.
Screenshots regenerated and reviewed:
`./tmp/desktop-chrome/32-backups-artifacts-desktop-chrome.png` and
`./tmp/mobile-chrome/32-backups-artifacts-mobile-chrome.png`.

**Issues**

- Backup posture repeats.
- Artifact ownership, linked request, handoff source, and restore consumers are presented as a conceptual map rather than direct artifact actions.
- Created time, verification, retention, and restore/download actions are not strong enough.
- “Handoff” remains implementation-oriented.

**Practical fix**

Make the artifact inventory primary:

```
Artifact · VPS · Created · Size · Verification · Retention/expiry · Restore · Download
```

Put ownership and lineage in the artifact detail drawer. Rename handoff to **Download package** or **Transfer package** where applicable.

**Mobile**

Artifact cards should expose Restore and Download directly.

------

### 33 — Restore — **P1**

**Resolved 2026-06-26 — source-artifact restore workflow with draft/live confirmation**

Backups / Restore no longer repeats the full backup posture block or the former
five-stage guide. The page starts with a compact restore summary and a source
grid shaped around `Artifact`, `Readiness`, `Destination`, `Path behavior`,
`Draft restore`, and `Action`. Selecting Restore opens the workflow drawer with
that source artifact selected.

The drawer now uses operator-facing workflow language: **Draft restore** for the
saved incomplete intent, **Confirm restore** for dry-run/live dispatch, and a
separate rollback section. Unavailable or unverified artifacts show a warning in
the page summary, source grid, and drawer before a live restore can be reviewed.
Live restore confirmations name the destination VPS, path behavior, restore
path, archive transfer, and replacement scope; dry-run confirmations are not
styled as destructive.

Screenshots regenerated and reviewed:
`./tmp/desktop-chrome/33-backups-restore-desktop-chrome.png` and
`./tmp/mobile-chrome/33-backups-restore-mobile-chrome.png`.

**Issues**

- Backup posture repeats.
- The restore workflow is framed as five formal stages.
- Metadata-plan and approval language adds ceremony.
- A staged plan can appear positive even when the source artifact has not been verified.
- The actual destination and overwrite behavior are not prominent enough at the top.

**Practical fix**

Use:

1. Choose artifact.
2. Choose destination and path behavior.
3. Confirm.

One destructive confirmation should name what will be replaced. Saved incomplete work can be called a **draft restore**, not an approval plan.

Show a strong warning when the artifact is unverified.

**Mobile**

One section at a time is appropriate here, with sticky Continue/Restore. Do not repeat backup posture above every step.

------

### 34 — Migration — **P1**

**Resolved 2026-06-26 — source-to-replacement migration mappings**

Backups / Migration no longer repeats the backup posture block or starts with a
large formal cutover checklist. The page begins with a compact relationship
summary:

```
Source VPS/artifact -> Replacement VPS
```

The table is now **Migration mappings** instead of internal migration links, with
operator columns for source artifact, replacement VPS, path behavior, cutover
state, and mapping details. Empty state copy explains that a draft restore must
define the source artifact and replacement VPS before a mapping can be saved.

The drawer now uses **Migration mapping** language. It separates the required
source-to-replacement relationship from optional source artifact staging,
identity policy, service check, cutover mode, and cutover notes. Review buttons
are `Review mapping` and `Review cutover restore`; confirmation prompts name the
source-to-replacement route, path behavior, archive transfer, and mapping hash.

Screenshots regenerated and reviewed:
`./tmp/desktop-chrome/34-backups-migration-desktop-chrome.png` and
`./tmp/mobile-chrome/34-backups-migration-mobile-chrome.png`.

**Issues**

- Backup posture repeats.
- The page uses a large formal cutover checklist before establishing the simple source/destination relationship.
- “Accepted migration links” is internal language.
- Identity mapping, source artifact, restore plan, and cutover evidence compete.

**Practical fix**

Start with:

```
Source VPS/artifact → Replacement VPS
```

Then optional:

- identity/key mapping;
- service checklist;
- cutover notes.

Use one confirmation for the actual cutover or identity switch. Call saved relationships **Migration mappings**.

**Mobile**

Keep source and replacement visible in a compact sticky summary while editing the checklist.

------

## Config

### 35 — Config Overview — **P0/P1**

**Status — implemented 2026-06-26**

Config Overview now shows a latest-state-per-VPS surface by default. Stale queued
work is labeled **Stale apply**, missing resources are labeled **Deleted or
unavailable VPS**, rule validity uses clear copy such as **3/3 rules valid**, and
failed/stale available VPSs get a direct **Retry** handoff into Bulk patch.
Historical apply/job attempts are collapsed under Recent changes.

**Issues**

- A VPS identifier appears that is not present in the visible fleet.
- Applied, queued, failed, and runtime-sync counts do not clearly represent latest state versus historical attempts.
- An old queued action remains presented as queued.
- Phrases such as “0 of 3 rows are not ok” are awkward.
- Config health, drift, templates, and apply-state summaries overlap.

**Practical fix**

Show only the latest current state per VPS by default. Historical attempts belong in details.

Old queued work should become:

- Timed out;
- Lost;
- Stale;
- or Unknown.

Label missing resources as **Deleted or unavailable VPS**.

Use clear language:

> 3/3 rules valid

Provide direct Retry for failed or stale applies.

**Mobile**

Prioritize the affected-VPS list and required action; collapse secondary summary cards.

------

### 36 — Per-VPS Config — **P1**

**Status — implemented 2026-06-26**

Per-VPS Config now follows `Select VPS -> Load current config -> Edit desired
patch -> Apply`. The empty page shows only the target selector and compact
guidance, redacted config reads submit as unprivileged read-only `config_read`
jobs, timeout moved under Advanced, patch sections/payload hash update while the
operator types, and the only mutation entry point is a final `Apply patch`
confirmation after privilege unlock. Mobile uses selectable Current base /
Desired patch views with the Apply action kept in the patch workflow.

**Issues**

- Large blank editors are shown before a VPS is selected.
- Reading a redacted config is blocked by the same privilege path as writing.
- Max timeout is prominent in a common config workflow.
- Validate and Review apply create separate steps.
- Current, desired, and override concepts are not visually strong enough.

**Practical fix**

Sequence:

```
Select VPS → Load current config → Edit/patch → Inline diff → Apply
```

A redacted read should be a normal authorized read where safe. Auto-validate while editing. Use one final Apply confirmation. Put timeout under Advanced.

Do not render editor height until a VPS is selected.

**Mobile**

Use a full-screen editor and sticky Save/Apply. Show current/desired as selectable views rather than adjacent columns.

------

### 37 — Bulk Patch — **P1**

**Status — implemented 2026-06-26**

Bulk Patch now follows `Patch/generator -> Targets -> Preview changes -> Apply`.
The primary workflow shows the generator or temporary patch editor first, then
an explicit target selector state where an empty selector is never treated as
all VPSs. `Preview changes` renders generator TOML, resolves the exact target
count, and shows a per-VPS change summary before Apply. `Apply patch` remains
locked until the preview exists and privilege material is available; the final
confirmation still re-renders and re-resolves the frozen request before
submission.

Generator management moved behind `Manage generators` / `Patch generator
registry`, so the registry selection model no longer competes with the selected
generator in the apply form. Timeout moved under Advanced apply options. Mobile
keeps the editor first, target count second, preview summary below the selector,
and Apply after the privilege section without the previous sticky-order jump.

**Issues**

- Generator selector, saved generator registry, and generator management compete.
- The page can visually show a selected generator while the registry says zero selected.
- Raw generator names truncate.
- Target scope and empty-selector behavior are not sufficiently explicit.
- Timeout is prominent.
- The resulting per-VPS change is not immediately visible.

**Practical fix**

Primary form:

```
Patch/generator · Targets · Preview changes · Apply
```

Move generator management behind Manage generators. Auto-render or provide one Preview changes button. Show target count and per-VPS change summary.

For multiple targets, place timeout/concurrency under Advanced.

**Mobile**

Show the patch editor first, target count second, and sticky Apply after preview.

------

### 38 — Config Templates / Coverage — **P1**

**Status — implemented 2026-06-26**

Template coverage now renders as source coverage rather than template
authoring. Each domain shows `Desired source`, `Stored/available`,
`Assigned VPSs`, `Ready`, `Attention`, and `Fix`. Raw readiness states are
mapped to operator labels such as `Server storage missing`, and each Fix action
opens Automation / Source Templates with the relevant source/template search
seeded.

Fresh desktop and mobile screenshots for `38-config-templates` passed after
visual review: coverage cards are compact, the desktop grid no longer clips
action labels, and mobile cards show the essential state with one Fix/Review/Add
action plus details.

**Issues**

- The page is mostly a coverage summary linking back to Source Templates, so its name overlaps with template authoring.
- Built-in source, stored template, selected source, assignment, and readiness are difficult to distinguish.
- Raw states such as `selected_no_store` appear.
- Readiness numbers and attention states are not immediately reconcilable.
- Grammar and labels remain rough.

**Practical fix**

Rename the page **Template coverage** or **Source coverage**.

For each domain show:

```
Desired source · Stored/available · Assigned VPSs · Ready · Attention · Fix
```

Use human status labels and direct links to the exact source/template requiring action.

**Mobile**

Coverage cards should show only the essential state and Fix action.

------

### 39 — Config Rules — **P0**

**Status — implemented 2026-06-26**

Dry-run previews now filter rows whose normalized before/after values are
semantically equal, including quota units, selector list ordering/defaults, JSON
equivalence, and numeric/boolean encodings. A preview with no effective diff
shows `No changes detected` and does not open Apply. Mixed previews show only
effective rows, label hidden no-op rows, use **Preview changes** / **Apply N
changes** wording, and move the backend preview hash into Details.

The bulk editor now has one authoritative **Preview changes** action and a
Set/Unset mode switch. Set mode exposes typed cards for reset day, total/RX/TX
quota, and traffic interfaces/selectors; raw `key=value` editing remains under
Advanced. Unset mode keeps the explicit key checklist. Mobile preview rows
render as before/after cards, and Apply remains available only through a valid
review prompt after preview.

**Issues**

- Dry run says three rows changed while every visible before/after value is the same.
- Multiple dry-run/review buttons create uncertainty about which action is authoritative.
- A long preview hash is prominent.
- Apply is not obvious as the final next step.
- Known fields such as traffic quota are still presented as generic key/value operations.

**Practical fix**

Normalize values before comparison, including units, list ordering, and equivalent encodings. Remove no-op rows. When no actual changes exist:

> No changes detected

Disable Apply.

Use one **Preview changes** action and one **Apply N changes** action. Hide the preview hash in Details.

Typed fields are useful for well-known rules such as quota, reset day, and interfaces; preserve raw key/value under Advanced.

**Mobile**

Render changed rules as before/after cards. Keep Apply visible only after a valid preview.

------

## Observability

### 40 — Fleet Metrics — **P0/P1**

**Resolved 2026-06-26 — freshness contract, sparse points, warning definitions**

Fleet Metrics now keeps the shared no-gap chart behavior and adds page-specific
freshness evidence above the chart: selected range, actual data span, sample
window, last sample age, and sparse-data treatment. Sparse retained telemetry is
rendered as points only with an explicit notice that it is point evidence, not a
continuous trend. Warning copy is normalized into active alerts, affected VPSs,
warning observations, and fleet warning state, so the page no longer reuses one
ambiguous "warnings" term for different concepts.

**Issues**

- A 24-hour range is selected while the summary references old dates and the chart contains only a short period.
- Lines connect missing samples because `spanGaps` is enabled.
- Sparse data is visually presented as a meaningful trend.
- Warning totals differ from shell/fleet warnings because the term is not defined consistently.
- The charts do not clearly disclose last update and available data span.

**Practical fix**

Display:

> Selected: 24h · Data available: 18m · Last sample: 20d ago

Do not join gaps. When sample count is very low, show points and a sparse-data notice.

Distinguish:

- active alerts;
- affected VPSs;
- warning observations;
- fleet warning state.

**Mobile**

Charts are full width, one metric at a time. The selected range, available data
span, and last-sample line remains immediately above the chart on mobile.

------

### 41 — Network Metrics — **P1**

**Resolved 2026-06-26 — stale banner, selected metric, directional labels**

Network Metrics now leads stale retained evidence with a warning banner and
direct links to Evidence and capped Network tests. The chart section uses a
selected metric control instead of stacking three tiny charts; sparse evidence
renders as points only with an explicit retained-evidence time filter, point
count, last sample, and no continuous-trend implication. Count definitions now
separate observation rows, chart samples, degraded signals, and overlay rows.
Endpoint and tunnel rows use directional labels such as
`agent-fra-02 -> agent-sfo-01`, translate runtime states such as
`matched_saved_plan` to operator labels, and use `No measurement` instead of
bare dashes.

**Issues**

- Evidence is from May 31 without a prominent stale warning.
- Three points are drawn as a trend.
- Observation, sample, degraded-signal, and overlay counts are not clearly related.
- Internal values such as `matched_saved_plan` are visible.
- One endpoint can be Down while another shows latency/loss without explaining direction.
- Missing values appear as bare dashes.
- No clear time filter is visible.

**Practical fix**

Add a stale-data banner and show point count. Translate runtime states. Explicitly label directional observations:

> edge-sfo-01 → core-fra-02

Use **No measurement** instead of `—`. Explain count definitions in tooltips. Link degraded observations directly to Evidence or Run test.

**Mobile**

Show one selected tunnel/metric at a time rather than a very long stack of small charts.

------

### 42 — Process Metrics — **P0/P1**

**Resolved 2026-06-27 — removed from normal navigation**

Process Metrics is removed from normal Observability navigation, the desktop
sidebar, and the mobile page selector. Stale programmatic requests for the old
`process_metrics` subpage fall back to Observability / Fleet metrics instead of
showing an implementation-status page. Remote Operations / Processes no longer
links to unfinished process metrics as production evidence.

**Issues**

- This is an implementation-status page inside normal production navigation.
- It exposes “Implementation work” and backend model requirements to operators.
- The Processes page links to it despite no useful metrics being available.

**Practical fix**

Hide it or mark it Preview in both navigation and page title. The best fix is to remove it until the backend provides process history.

Do not replace it with more placeholder cards.

**Mobile**

The empty internal roadmap no longer occupies a dedicated mobile route.

------

### 43 — Observability Alerts — **P1**

**Resolved 2026-06-27 — tabbed configuration page with Fleet triage handoff**

Observability / Alerts is now configuration-first: policy authoring is the
default tab, Destinations owns alert notification channels, and Deliveries owns
previewed/failed/retained notification evidence. Active triage is explicitly
handed off to Fleet / Alerts. The page no longer renders policies, destinations,
and delivery history as one long stacked mobile route. Alert queue controls on
this page are simplified to `Preview matches`, `Send / retry`, and
`Open deliveries`; the full lower-level queue controls remain available only in
the broader notification registry where that operational depth is intentional.

**Issues**

- Policies, active alert count, policy groups, notification channels, queue operations, and delivery history share one page.
- This overlaps with the active Fleet Alerts queue.
- Four similar actions—review matches, queue dispatch, queued deliveries, delivery—are difficult to distinguish.
- Policy/channel names and times truncate.
- Failed delivery evidence is not easy to reach.

**Practical fix**

Keep this page for configuration:

- Policies;
- Destinations;
- Delivery history.

Link to Fleet Alerts for active triage.

Simplify queue actions to:

- Preview match;
- Send/retry;
- Open delivery.

Put cleanup in Maintenance or an overflow menu.

**Mobile**

Use tabs for Policies, Destinations, and Deliveries. Do not render all three sequentially.

------

### 43b — Alert Policy Editor — **P1**

**Resolved 2026-06-27 — focused editor with explicit match preview**

Alert policy creation/editing from Observability / Alerts now opens as a
focused policy editor. While it is open, the Alerts summary cards, tabs, active
triage handoff, policy grid, destinations, and delivery history are hidden.
The focused editor exposes one `Preview matches` action and one
`Create policy` / `Update policy` save action; the old `Dry-run`,
`Review create`, and `New policy` controls remain out of this release flow. The
activation checkbox now uses `Enable after creation` for new policies, and the
match summary states `Matches N VPSs` after preview alongside incomplete-VPS
and invalid-rule counts. The lower-level shared policy manager remains
unchanged for Fleet policy workflows where inline editing is intentional.

**Issues**

- The editor expands inside the full Alerts page while unrelated channels and delivery sections remain below.
- The page reaches 2,262 px desktop and 4,253 px mobile.
- “Evaluate policy” is used where the operator expects Enabled.
- Target expression and condition are expert-capable but lack a clear matched-VPS result.
- Dry-run, Review create, and New policy are confusing during one create operation.
- A new policy can be enabled immediately without enough emphasis.

**Practical fix**

Open a focused editor or side drawer. Show:

```
Matches 3 VPSs
```

Use **Enable after creation**, defaulting according to product policy. Provide one Preview and one Create action. Keep raw conditions, with common threshold controls only where they save time.

**Mobile**

Use a full-screen editor and hide the rest of Alerts until it closes.

------

### 43c — Webhooks — **P1**

**Resolved 2026-06-27 — Event webhooks tabs, scoped Send test, and explicit retry**

Observability / Webhooks is now **Observability / Event webhooks** across the
page title, breadcrumb, page selector, release navigation, screenshot manifest,
and page copy. The page states that event webhooks are independent from alert
notification destinations, while Alerts keeps notification destinations on the
Alerts page.

The page now opens to a Rules tab with summary actions and a compact
Rules / Deliveries / Maintenance tab set. Delivery history and retention
cleanup no longer render below the rule table by default; Deliveries owns
retained evidence and Maintenance owns cleanup review.

The Event webhooks rule manager now uses configuration-mode queue actions:
`Preview event`, `Send test`, and `Retry failed`. `Retry failed` sends the
existing backend `status: failed` process request, and `Send test` is backed by
a new optional `rule_id` dispatch field so each rule card can open a reviewed,
rule-scoped test dispatch instead of accidentally matching every enabled rule.
The dispatch preview hash includes the scoped rule id.

Fresh desktop/mobile screenshots show the default Rules tab only, visible
rule-card `Send test`, no notification-channel bleed, and no long stacked
deliveries/maintenance sections.

**Issues**

- Event webhook rules can be confused with alert notification destinations.
- Failure and delivery counts differ from alert delivery counts without explaining that they are separate domains.
- Several queue-review actions have nearly identical wording.
- No strong Send test action is visible.
- Retention/cleanup receives too much prominence on the main operational page.
- Internal expression and delivery terms dominate.

**Practical fix**

Rename this **Event webhooks** and explain:

> Event webhooks are independent from alert notification destinations.

Primary actions:

- Create rule;
- Send test;
- Retry failed.

Keep dispatch preview only where useful. Move cleanup under Maintenance or More.

**Mobile**

Use Rules and Deliveries tabs. Keep Send test visible on each rule card.

------

### 43d — Webhook Rule Editor — **P1**

**Resolved 2026-06-27 — focused editor, sample payload, and real signed webhooks**

Observability / Event webhooks now opens webhook rule creation in a focused
editor. While the editor is open, the page hides routing summary cards, tabs,
the rule grid, queue controls, delivery history, and maintenance. The editor
uses `Enable after creation`, `Test`, and `Create rule` / `Update rule`;
`Review create`, `New rule`, and queue controls are absent from the focused
create flow.

The Test action now renders an inline sample payload panel inside the editor:
matched VPS count and names, rendered message, dry-run delivery status, and a
bounded JSON payload sample. It does not navigate the operator to Deliveries
while editing.

The secret field is now real rather than cosmetic. Webhook rule requests accept
an optional signing secret plus an explicit clear flag; responses expose only
`signing_secret_set` and never serialize the secret value. Editing with a blank
secret preserves the existing secret, entering a new value sets or rotates it,
and `Clear existing signing secret` removes it. API-triggered delivery
processing and the background worker both sign the exact JSON payload bytes with
`X-Vpsman-Webhook-Signature: sha256=...` when a rule has a secret. Delivery
history rows do not persist secret material; processing resolves the current
rule secret when claiming a delivery.

Verification passed for this resolved slice: backend tests cover redaction,
preserve/rotate/clear lifecycle, scoped dispatch secret propagation, and HMAC
signature output; worker tests cover HMAC signing output; TypeScript passes;
the Impeccable detector is clean on touched UI/test files; focused Event
webhooks release-navigation tests pass; and fresh desktop/mobile screenshot
batch 9 passed with the secret field visible in the focused editor.

**Issues**

- Like the policy editor, it expands inside a very long full page.
- The page reaches 2,279 px desktop and 4,288 px mobile.
- “Evaluate rule” is used instead of Enabled.
- Body template editing lacks a prominent rendered sample.
- Review rule, Review create, and New rule duplicate intent.
- Queue and cleanup sections remain visible below.

**Practical fix**

Focused editor:

- name;
- expression;
- target URL;
- secret;
- cooldown;
- body template;
- sample payload;
- Test;
- Enable after creation;
- Create.

Show response status and latency after Test. Keep queue management outside the editor.

**Mobile**

Use a full-screen editor with sticky Test and Create controls.

------

### 44 — Dashboards — **P0/P1**

**Resolved 2026-06-27 — source counts, sparse coverage, and mobile section shape**

Observability / Dashboards now calls the read-only layouts `Dashboard presets`
instead of saved dashboards, shows compact freshness and source-count summaries,
and reconciles fleet/job/alert counts from the dashboard overview contract.
Missing `offline` counts are derived from `total - online - stale` when that
summary evidence is available, avoiding raw `undefined` text without inventing a
new API source. The page now exposes sparse 24-hour sampled coverage near the
dashboard summary and beside resource/network widgets, keeps Share / Export as a
simple handoff section, and avoids bad plurals such as `1 records`. Mobile uses
a single preset menu plus a selected section switcher so operators see one
preset and one widget section at a time.

**Issues**

- The page calls predefined read-only views “Saved dashboards,” implying user-managed dashboards.
- `undefined offline` is visibly rendered.
- Fleet, alert, and job counts disagree with other surfaces.
- The selected 24-hour range contains old or sparse data.
- “Share and export” and “mutation boundary” text over-explain implementation concepts.
- Grammar such as “1 records” remains.

**Practical fix**

Call them **Dashboard presets** unless users can actually save custom dashboards.

Fix all undefined fallbacks and use a shared summary source. Show freshness and available range.

Provide simple Share link and Export JSON actions. A dashboard builder is not required.

**Mobile**

Let the operator select one preset and one widget section at a time.

------

## Audit

### 45 — Audit Events — **P0/P1**

**Resolved 2026-06-27 — human ledger columns, compact coverage, and mobile cards**

Audit / Events now uses the intended default ledger shape:
`Time · Operator · Action · Target · Result · Related job/session`. The table
derives operator-facing labels from audit metadata while retaining raw action,
raw target, command hash, source IP, session, privilege scope, event ID, exact
timezone-bearing time, and JSON metadata in the detail panel. Related job,
terminal, schedule, and session references are correlated into the visible
`Related job/session` column so operators can follow evidence without reading
payload JSON.

The earlier latest-event bug remains covered by the older-first fixture
regression. Coverage explanation is reduced to one compact warning that reports
which expected sensitive workflow families are missing from the returned rows,
rather than filling the page with contract cards. Mobile uses the shared
ConsoleDataGrid card mode with time, result, operator, action, target, and
related evidence visible before opening details.

**Issues**

- The “latest visible event” summary is older than another visible row.
- Only a small number of audit rows appear despite numerous sensitive workflows.
- Actor, action, target, payload, hash, and result are raw or truncated.
- Full timestamps still lack an obvious timezone label.
- Coverage-contract explanations dominate the page.

**Practical fix**

Fix latest-time calculation and ensure sensitive operations create linked records.

Default columns:

```
Time · Operator · Action · Target · Result · Related job/session
```

Place raw JSON, hashes, IP, and metadata in a detail drawer. Keep one compact coverage warning rather than several explanatory cards.

**Mobile**

Audit cards need time, actor, action, target, and result. Open details for everything else.

------

### 46 — Audit Job Evidence — **P0/P1**

**Resolved 2026-06-26 — audit gaps are warnings and output evidence has explicit states**

Audit / Job evidence now uses the intended table shape:
`Job · Actor · Privilege · Targets · Result · Audit · Output`. Missing
correlation is no longer neutral `Job ledger only`; rows and detail panels show
`Audit event missing` with warning tone when no audit row matches the job ID or
payload hash. The summary also exposes an `Audit gaps` count.

Output evidence now has explicit operator states: `Not loaded`, `Empty output`,
`Retention expired`, `Output unavailable`, `Inline output`, and `Retained
output`. The selected-job detail explains which state was derived from loaded
output rows or load errors, while keeping raw payload hashes in the detail
panel instead of the default scan columns. Desktop and mobile screenshot
coverage proves the warning state is visible in the Audit / Job evidence page.

**Issues**

- Four privileged jobs are visible, but only two appear linked to audit rows.
- Completed jobs can have no output evidence without clearly saying whether output was empty, not retained, expired, or unavailable.
- Job IDs and hashes dominate.
- “Job ledger only” sounds neutral even though it indicates an audit-evidence gap.

**Practical fix**

Treat missing audit correlation as a warning:

> Audit event missing

Distinguish output states:

- Empty output;
- Not loaded;
- Retention expired;
- Output unavailable.

Use:

```
Job · Actor · Privilege · Targets · Result · Audit · Output
```

**Mobile**

Show one evidence card per job with warning badges and Open job.

------

### 47 — Audit Sessions — **P0/P1**

**Earlier implementation note — stale/expired session display implemented 2026-06-26**

Audit / Sessions now derives operator-facing state from terminal last activity
and bearer-session expiry instead of trusting raw `open`, `current`, or
`active` flags. Old open terminal records are shown as `Stale state`, stale
terminals are excluded from the open count, expired bearer sessions are shown as
`Expired`, tiny retained transcript ranges are labelled as trace-only evidence,
and raw replay API paths are hidden under Advanced. The final resolved block
for this section covers demo/test-data labelling, mobile evidence cards, and
the strongest started/expiry columns available from current backend evidence.

**Issues**

- Sessions from May appear open in late June.
- Very small retained transcript sizes are described as replayable evidence without qualification.
- Raw replay API paths and IDs appear.
- Fixture-like user-agent and localhost values are displayed as ordinary production evidence.
- Created, expiry, and last activity are not consistently visible.
- Times truncate.

**Practical fix**

Validate session state from expiry and last activity. An old “open” session should become **Stale state** or **Expired**.

Use:

```
Operator · VPS · State · Started · Last activity · Expiry · Transcript · Audit
```

Hide replay API paths in Advanced details. Clearly label demo/test data where applicable.

**Mobile**

Keep transcript and audit actions visible on the session card.

**Resolved 2026-06-27 — evidence timings, demo/test labels, and mobile cards**

- [x] Terminal session evidence now uses the intended production scan:
  `Operator · VPS · State · Started · Last activity · Expiry · Transcript · Audit`.
- [x] `Started` is derived from the earliest linked `terminal.open` audit event
  when available; absent terminal-start data is explicitly labelled as not
  reported instead of inferred.
- [x] `Last activity` remains sourced from terminal inventory and is shown with
  full timestamp detail in the selected proof panel.
- [x] `Expiry` is sourced from linked bearer-session expiry when available, with
  expired bearer sessions labelled directly; missing terminal expiry remains a
  visible backend-data gap.
- [x] Localhost, documentation IP ranges, and Playwright user agents are labelled
  as demo/test auth signals in the summary, grid detail, and operator-session
  evidence instead of appearing as ordinary production proof.
- [x] Operator-session evidence now shows role, state, created time, refresh
  expiry, and auth source; mobile stacks these values instead of hiding them.
- [x] Raw replay paths remain under Advanced replay path, while transcript and
  audit evidence stay visible on the selected session.

------

### 48 — Retention & Export — **P1**

**Issues**

- The page exposes many backend/compliance limitations directly.
- Ten policy domains are summarized while only a small subset is easily inspectable.
- Export scope and time range are not obvious.
- Save, Preview prune, and Review cleanup create more stages than necessary.
- Raw domain names such as `audit_logs` are used.
- Storage usage is unknown yet presented as a posture card.

**Practical fix**

Simple model:

```
Domain · Retention days · Metadata only · Export enabled
```

Save policy directly.

Cleanup:

```
Choose domain and cutoff → Preview → Delete
```

Export:

```
Choose domain and time range → Export
```

Humanize domain labels and hide implementation-gap details under Diagnostics.

**Mobile**

Use one selected domain editor rather than showing all retention concepts in one vertical page.

**Resolved 2026-06-27 — policy table, selected-domain workflows, and diagnostics**

- [x] Audit / Retention & export now leads with a compact policy summary and a
  policy table shaped as `Domain · Retention days · Metadata only · Export enabled`.
- [x] Domain names are humanized in the normal UI; raw retention domain IDs are
  hidden inside Diagnostics.
- [x] The selected-domain editor is the main work area for changing domain,
  retention days, prune limit, metadata-only mode, and export-enabled state.
- [x] Policy save is direct in the page while still sending the backend-required
  confirmed request.
- [x] Cleanup is separated into `Preview cleanup` followed by disabled-then-enabled
  `Delete reviewed rows`; the destructive confirmation uses the reviewed preview
  row/object counts and effect text.
- [x] Export is scoped to the selected domain and clearly states `All retained
  records` and `JSON history bundle`.
- [x] Backend/data limitations such as storage size, raw domain, current row total
  limits, and missing custom date-window export live under Diagnostics instead
  of dominating the page.
- [x] Mobile now shows only the selected policy row plus the selected-domain
  editor, cleanup, export, and collapsed diagnostics.

------

## Access

### 49 — Access Overview — **P1**

**Resolved 2026-06-27 — actions-first overview and five explicit access responsibilities**

Access / Overview now renders only the intended operating shape:
`Actions required`, `Operators and active sessions`, `VPS identities`,
`Gateway sessions`, and `Privilege state`. The old repeated posture cards,
authority workflow map, security-posture metric table, and attention queue are
removed from the Overview.

MFA copy now says `Policy recommends MFA` and `Recommended` where enforcement
is not exposed. Bearer sessions are expiry-validated before being counted as
active, so expired sessions are shown as expired and excluded from the active
session total. The privilege state uses the plain operator wording:
`No saved local vault; enter privilege secret when needed.`

The Overview no longer shows the generic admin-actions inspector. Mobile now
starts with critical access warnings and direct actions, then the four
responsibility rows, with no horizontal overflow in structured screenshots.

**Issues**

- The same access information appears in overview cards, an authority workflow map, security posture, and attention queues.
- Terms such as authority posture are more abstract than necessary.
- MFA is described as required although code states enforcement is not exposed.
- Expired-looking bearer sessions appear current.
- The page says no privilege vault while the global shell still offers Unlock, creating uncertainty.
- Gateway and identity states are repeated.

**Practical fix**

Limit the overview to:

1. actions required;
2. operators and active sessions;
3. VPS identities;
4. gateway sessions;
5. privilege state.

Use **Recommended** versus **Enforced** accurately. Validate expired sessions. Explain unlock plainly:

> No saved local vault; enter privilege secret when needed.

Remove the workflow map and repeated status sets.

**Mobile**

The first screen should show only critical access warnings and direct actions.

------

### 50 — Operators — **P0/P1**

**Resolved 2026-06-26 — operator policy language, row shape, and mobile cards**

Access / Operators now uses explicit operator-facing policy language instead of
backend-gap posture. The page says MFA is recommended when enforcement is not
exposed by the API, uses `MFA off`, `MFA enabled`, and
`Policy recommends MFA`, and removes the old `admin off` wording. Backend
evidence gaps such as password age, invite/locked state, and API-token inventory
are consolidated into an evidence-boundary note rather than counted as healthy
or unhealthy posture.

The role model now presents the three standard roles, Viewer, Operator, and
Admin, with visible counts and custom roles only when loaded. Authentication
failure summary text states that counts come from loaded auth history and
separates visible-operator failures from unknown-user failures, so the summary
and per-operator details use the same evidence scope. Refresh TTL labels explain
the admin target and distinguish refresh-token lifetime from short access-token
expiry.

The operator table now follows:

```
User · Status · Role · MFA · Active sessions · Last login · Actions
```

Row actions expose **Manage** and **Revoke sessions** directly. Mobile renders
the Operators table as readable cards with Status, Role, MFA, Sessions, Last
login, Manage, and Revoke sessions visible without page-level horizontal
overflow. Full timestamps remain available through time titles while scan views
use compact relative time or `Never`.

**Issues**

- The page exposes backend gaps as normal posture tiles.
- MFA-required language is stronger than actual enforcement.
- Role count appears inconsistent with the visible Viewer, Operator, and Admin model.
- Authentication-failure summary and per-user failure counts do not reconcile.
- `admin off` is awkward.
- Session TTLs of 365/90 days are visible without a concise policy explanation.
- Dates truncate.

**Practical fix**

Use human language:

- **MFA off**
- **MFA enabled**
- **Policy recommends MFA**
- **Policy enforced**

Make role counts consistent. Explain whether auth failures represent all history or the current user.

Operator row:

```
User · Status · Role · MFA · Active sessions · Last login · Actions
```

Role changes need one compact confirmation.

**Mobile**

Operator cards should expose Manage and Revoke sessions directly.

------

### 51 — VPS Identities — **P1**

**Resolved 2026-06-27 — registry-first identities and opened VPS registration workflows**

Access / VPS identities is now registry-first. The repeated Access posture
overview and identity lifecycle band are removed from the page. The default
screen shows the identity registry first, then retained key revocations; the
registration, rotation, and revocation forms are hidden until the operator
opens a specific workflow.

`Register VPS` opens the VPS identity workflow. Row/selection actions open
rotation and revocation workflows for the selected VPS. The workflow copy now
uses VPS identity terminology instead of `Import gateway identity`, and the
confirmation prompt says `Confirm VPS identity registration`.

Fingerprints in the registry and detail rows are copyable. Fixture-style
revocation reasons such as `fixture rebuild` are rendered as operator language
such as `Host rebuild`. Generated private keys are labelled
`Private key - shown once`, with copy explaining that the panel never saves
private material.

Mobile keeps the table/cards first. Opening `Register VPS` displays the
identity workflow as a full-screen mobile panel with a visible close action.

**Issues**

- The full Access Overview is repeated above the identity page.
- Lifecycle explanation, table, registration form, and revocation form all coexist.
- Fixture-style rebuild reasons are visible.
- Public-key fingerprints are not optimized for copy/verification.
- - New and an always-visible creation form duplicate intent.
- “Import gateway identity” does not match VPS identity terminology.
- Gateway defaults depend on personal preferences.
- Keypair generation needs clearer one-time private-key handling.

**Practical fix**

Remove the repeated overview. Make the identity registry primary. **Register VPS** opens a drawer.

Default flow:

1. generate client ID/keypair or import public key;
2. show installer command/private material once;
3. confirm registration.

Use row actions Rotate and Revoke. Make fingerprints copyable. Rename the workflow consistently.

**Mobile**

Show the table/cards first; forms should be full-screen drawers.

------

### 52 — Gateway Sessions — **P1**

**Resolved 2026-06-27 — compact empty state, shared settings route, and session evidence columns**

Access / Gateway sessions no longer repeats the Access Overview or the old
readiness architecture cards. The page header is scoped to
`Gateway session inventory`, and an empty page shows one compact state:
`No active gateway sessions. Configure the gateway endpoint and server key.`

The empty state provides exactly one `Gateway settings` action and routes it to
shared Suite config rather than browser-local Preferences. Empty desktop grids
are not rendered, so mobile receives the same compact state without wrapped
table headers.

The gateway session API and frontend type now expose session `remote_ip` plus
agent `agent_version`, allowing the populated table shape to be:
`Gateway · VPS · State · Connected · Last activity · Remote IP · Version`.
Expanded rows retain detailed evidence such as session ID, client ID, end
reason, and Noise key.

**Issues**

- The Access Overview is repeated.
- An empty session page is dominated by architecture explanations.
- “No panel-side endpoint lookup” is repeated.
- Preferences and Suite Config buttons duplicate settings paths.
- The empty table header wraps poorly.
- Gateway install defaults are browser-local.

**Practical fix**

Use a compact empty state:

> No active gateway sessions. Configure the gateway endpoint and server key.

Provide one **Gateway settings** action. Store defaults in shared configuration.

When populated:

```
Gateway · VPS · State · Connected · Last activity · Remote IP · Version
```

**Mobile**

Do not show an empty desktop grid. Use the compact empty state and one settings button.

------

### 53 — Privilege Vault — **P1**

**Resolved 2026-06-27 — scoped local unlock, visible vault state, and separated TOTP setup**

Access / Privilege Vault no longer repeats Access Overview or the old
deny-by-default routine-action language. The vault panel now leads with the
operator state that matters: Locked/Unlocked, unlock scope, unlocked-until
state, and local vault saved/not saved. Direct actions are explicit and central:
Unlock, Lock now, and Clear local vault.

The local-save copy now says saved privilege material is encrypted in this
browser with the operator passphrase and is not shared with the server. The old
`Save encrypted vault` wording was replaced with `Keep encrypted in this
browser`, and the raw verifier language is no longer the visible form copy on
the Access page.

TOTP enrollment is now a short sequence: Password, QR/secret, Enter code, and
Complete. Disable TOTP is separated into its own panel and stays disabled when
no active TOTP factor exists.

**Issues**

- Access Overview repeats again.
- The UI asks directly for a privilege secret and verifier salt hex.
- “Save encrypted vault” is not sufficiently clear about local storage and risk.
- Unlock scope and duration are not visible.
- TOTP enrollment, confirmation, and disable controls share one long form.
- Lock now is not central after unlocking.
- “Deny by default” language can suggest repeated confirmation for every routine action.

**Practical fix**

Simplify the vault area:

- Locked/Unlocked;
- unlocked until;
- local vault saved/not saved;
- Unlock;
- Lock now;
- Clear local vault.

Explain that saved material is encrypted locally and not shared with the server.

TOTP should be a short sequence:

```
Password → QR/secret → Enter code → Complete
```

Do not show Disable until TOTP is enabled.

**Mobile**

Use separate full-screen flows for Unlock and TOTP setup.

------

## System

### 54 — System Overview — **P1**

**Resolved 2026-06-27 — compact service-health overview with diagnostics folded away**

System / Overview now leads with the operator-critical control-plane view:
service health, control-plane queue, database/gateway/worker state, four key
KPIs, one selected dispatch chart, and a compact attention list. Capacity
profile notes, dispatch-limit details, and drilldown/backend gaps are no longer
normal posture cards; they are available under Diagnostics instead.

The page description now matches the overview role, and `unset`-style wording
in the overview path is replaced by explicit `Not configured` or state-specific
copy such as `No queued events`. Mobile uses compact subsystem disclosures for
Database, Dispatch, Gateway, and Worker, and the shared chart axis now reduces
tick labels on narrow screens to avoid mobile overlap.

**Issues**

- The page is 3,574 px desktop and 7,201 px mobile.
- A 24-hour range contains only a short old sample period.
- Many detailed metrics duplicate System Capacity.
- Backend/drilldown gaps appear as operator cards.
- Warning meanings differ from global fleet warnings.
- Values such as unset are rendered without explanation.
- Too many equally weighted panels dilute what needs attention.

**Practical fix**

System Overview should contain:

- service health;
- control-plane queue;
- database/gateway/worker state;
- four key KPIs;
- one selected chart;
- attention list.

Move detailed capacity curves to Capacity. Use **Not configured** instead of unset. Hide implementation gaps under Diagnostics.

**Mobile**

Use collapsible Database, Dispatch, Gateway, and Worker sections.

------

### 55 — System Capacity — **P1**

**Resolved 2026-06-27 — subsystem tabs, factor-based queue health, and compact telemetry gaps**

System / Capacity now opens on a selected subsystem instead of rendering every
capacity chart at once. Database, Dispatch, Gateway, and Storage are explicit
tabs; mobile shows one subsystem, its factors, and one chart at a time. The
fresh screenshots are much shorter than the audit baseline: `1596 px` desktop
and `3494 px` mobile for `55-system-capacity`.

Queue posture is no longer “any queue depth is attention.” Dispatch capacity
uses oldest-item age when available, queue growth, the configured in-flight /
batch thresholds, and worker capacity availability. Gateway capacity uses live
status, queue age, growth, and queue-full / expired failure signals. The UI
keeps the OSPF-style operator expectation of realtime threshold adjacency:
growth, threshold, and availability sit beside the current queue values rather
than far away in a separate diagnostic block.

Artifact bytes, retention prune backlog, and worker lag are no longer normal
posture cards. They appear once in a compact unavailable-telemetry banner and
the Storage tab explains the missing backend fields without implying healthy
storage posture. Capacity limits link directly to the relevant System / Suite
Config keys such as `capacity.dispatcher_in_flight`,
`capacity.dispatcher_batch`, `capacity.api_db_pool`, and
`storage.artifact_max_bytes`.

**Issues**

- The page is 2,925 px desktop and 5,826 px mobile.
- Time range and available data again disagree.
- Any queue depth is treated as attention without considering age or growth.
- Artifact storage, retention, and worker-lag backend gaps occupy normal posture cards.
- Dense chart legends create long mobile output.
- Capacity profile values lack enough threshold context.

**Practical fix**

A queue should warn based on:

- oldest item age;
- growth;
- configured threshold;
- worker availability.

Use Database, Dispatch, Gateway, and Storage tabs on mobile. Show unavailable data in one compact banner.

Link each capacity limit directly to the relevant Suite Config field.

**Mobile**

One subsystem and chart at a time.

------

### 56 — Suite Config — **P1**

**Resolved 2026-06-27 — selected-section editor, search, compact field metadata, auto-validation, and one Review changes flow**

Suite Config now renders one selected settings section at a time by default,
with a left section rail and searchable field mode for cross-section lookup.
This cuts the fresh screenshot length from the audit baseline of `6151 px`
desktop / `11659 px` mobile to `2530 px` desktop / `4559 px` mobile while
keeping Review and save visible below the selected section.

Every field now has compact metadata instead of always-open Current / Default /
Validation / Impact boxes. Changed fields open their metadata automatically,
blank values read as `Unset (uses default)`, and each row exposes `Reset
current` plus `Use default`. The page also has a sticky save/status bar with the
next validation/review action and mobile keeps that bar, section rail, and
privilege gate readable.

Validation now runs automatically after structured field or Advanced TOML edits.
The normal operator path has one `Review changes` action that validates the
current draft if needed, opens `Confirm suite config save` only when validation
and privilege state allow it, and then saves through the existing redacted diff,
privilege assertion, reload/restart, and audit contract. Manual `Validate now`
is retained only as a recovery action for invalid or advanced TOML states.

**Issues**

- The page is 6,151 px desktop and 11,659 px mobile.
- The top summary says Changed keys: not validated beside 13 hot-reload and 16 restart fields, making those counts appear to be current draft impact.
- Every field repeats current, default, validation, and impact information, producing heavy vertical repetition.
- Validation and help text truncate.
- Blank values do not clearly mean unset, inherited, or empty string.
- Negative boolean wording is counterintuitive.
- Search and reset-to-default are missing or insufficient.
- Validate then Review save can create an unnecessary extra action.
- A large Advanced TOML view adds further length.

**Practical fix**

Before editing, label the top counts:

> Configuration inventory: 13 hot-reload fields · 16 restart fields

After editing:

> 3 changed · 2 hot reload · 1 restart

Auto-validate on change. Use one **Review changes** action, then Save.

Add:

- search settings;
- per-field Reset;
- explicit units;
- Unset/Default labels;
- sticky changed-fields bar;
- unsaved-change warning.

Collapse Current/Default/Impact metadata unless the field is changed or expanded.

Use positive switches:

> Require registered updates — Off

**Mobile**

Use accordion sections and a sticky Save bar. Render Advanced TOML only when explicitly opened.

------

### 57 — System Maintenance — **P0/P1**

**Resolved 2026-06-26 — cleanup preview evidence contract, common criteria, and one final Delete confirmation**

System / Maintenance now presents artifact cleanup as common criteria first:
artifact types, older-than age, state, and optional object prefix. The raw
expression is kept under Advanced and is combined with the common criteria.
Operator-facing wording now uses `Artifact types`, `Preview gate`, and
`Delete artifacts` instead of authority-domain/queue-first language.

The cleanup preview API now returns oldest/newest object age, eligible/protected
counts, and a bounded representative object list. The UI keeps Delete disabled
until that evidence is present, then opens one final typed Delete confirmation.
The preview result shows count, total size, age range, eligible/protected
objects, preview snapshot, and representative object keys. Focused tests prove
stale previews still invalidate the destructive action, complete previews enable
the confirmation, and the queued request is bound to the reviewed preview hash.
Desktop and mobile structured screenshot coverage verifies the compact
criteria-preview-delete grouping without horizontal overflow.

**Issues**

- The expression targets `job_output` while several artifact-type checkboxes are selected, making conjunction/scope unclear.
- Cleanup relies on expert DSL even for ordinary age-based deletion.
- Preview hash and matched values truncate.
- The API explicitly does not report age and retention rules.
- Deletion impact is described as irreversible without enough object-level evidence.
- “Authority domains” is implementation language.
- Empty maintenance-job space is large.

**Practical fix**

Common form:

- Artifact types;
- Older than N days;
- State;
- Optional path/prefix.

Keep raw expression under Advanced.

Preview must show:

- count;
- total size;
- oldest/newest;
- retained/reference-protected objects;
- representative list or downloadable full list.

Until age and retention evidence exists, block cleanup rather than merely warn. Use one Preview followed by one Delete confirmation.

**Mobile**

Keep criteria, preview summary, and Delete together. Collapse empty job history.

------

### 58 — Preferences — **P1**

**Resolved 2026-06-27 — scoped Preferences tabs, sticky save, labeled resets, and system-linked default handoffs**

System / Preferences now separates `Personal display`, `Browser state`, and
`System-linked defaults` as explicit tabs. The default view shows only personal
operator presentation settings, cutting the fresh screenshot length from the
audit baseline of `2874 px` desktop / `6251 px` mobile to `2138 px` desktop /
`5130 px` mobile.

Home telemetry curve controls are now labeled as personal chart presentation.
Gateway install material and tunnel allocation pools are no longer editable as
ordinary personal preferences; Preferences links operators to Suite Config and
Access / VPS identities for those shared workflows. The page has a sticky save
bar with changed-setting count, retains the bottom submit for keyboard flow,
uses named `Reset` actions instead of unlabeled icon-only resets, and moves the
build number into a low-emphasis footer note.

**Issues**

- The separation between personal, browser-local, and fleet/system preferences is conceptually good.
- Gateway endpoints, server key, and tunnel allocation pools are nevertheless stored per operator even though they affect generated operational output.
- The page is 2,874 px desktop and 6,251 px mobile.
- Save is far below edited settings.
- Reset icons lack sufficiently obvious labels.
- Some Home/dashboard curve choices are ambiguously labelled as fleet/system behavior despite possibly being personal presentation.
- Build number receives more attention than necessary.

**Practical fix**

Keep personal/browser-local:

- timezone;
- language;
- table/display density;
- name format;
- sidebar behavior;
- chart presentation preferences.

Move shared operational values:

- gateway endpoints;
- gateway server key;
- tunnel address pools;

to fleet/system configuration.

Clearly label every setting:

> Personal — stored for this operator
>  Browser — stored on this device
>  System — shared by all operators

Provide a sticky Save bar and named Reset actions.

**Mobile**

Use collapsible Personal, Browser, and System-linked sections. Show Save whenever changes exist.

------

# Visual hierarchy and styling

The visual system is clean, but too many concepts receive the same bordered-card treatment. This makes pages look orderly while weakening prioritization.

## Remaining visual issues

- Almost every fact is enclosed in a pale bordered box.
- Green/yellow cards are used both for state and general information.
- Primary and secondary actions can have similar visual weight.
- Long helper paragraphs appear repeatedly.
- Important resource names are sometimes less prominent than metadata.
- Dense text and large blank areas coexist on the same page.
- Small icon-only actions remain common.
- Truncated strings often have no obvious reveal/copy affordance.

## Practical styling corrections

- Reduce card borders by approximately one third.
- Use open sections and dividers for ordinary information.
- Reserve filled status cards for actual attention or success states.
- Use red only for actual failure/destruction, yellow for action/review, green for verified healthy state, blue for active/informational.
- Give each screen one unmistakable primary action.
- Put raw IDs and hashes in smaller secondary text.
- Add tooltips to every icon-only action.
- Prefer relative time in scanning views.
- Expand blank editors, charts, and output areas only when populated.
- Use clearer heading levels rather than more boxes.

------

# Terminology that should be simplified

| Current/internal wording              | Better operator wording                        |
| ------------------------------------- | ---------------------------------------------- |
| `shell_argv` / `scheduled_shell_argv` | Shell command / Scheduled shell command        |
| `artifact_metadata_recorded`          | Artifact recorded; content not verified        |
| `selected_no_store`                   | Source selected; server storage not configured |
| Handoff                               | Download package / transfer package            |
| Authority domains                     | Artifact types                                 |
| Materialized run                      | Scheduled run                                  |
| Request-bound assertion               | Temporary privilege authorization              |
| Review queued deliveries              | View queued deliveries                         |
| Evaluate policy/rule                  | Enabled                                        |
| Promotion candidate                   | Observed tunnel available to save              |
| No rollup                             | No historical data                             |
| Output not loaded                     | Load output                                    |
| Job ledger only                       | Audit event missing                            |
| Current records not exposed           | Data unavailable                               |
| `unset`                               | Not configured                                 |

Raw values can remain in Advanced details for debugging.

------

# What should remain intentionally simple

The following should **not** be expanded merely to resemble a large cloud provider:

- no organization/folder/project hierarchy unless vpsman actually supports those scopes;
- no multi-party approval for normal commands or config writes;
- no mandatory wizard for shell, TOML, JSON, selectors, cron, or tunnel fields;
- no full incident-management system for fleet alerts;
- no custom dashboard builder unless users request it;
- no forced audit note for every routine action;
- no giant universal rollout workflow for one-VPS operations;
- no artificial exclusivity between terminals, transfers, jobs, and file operations.

Concurrency, raw expert controls, compact tables, and direct navigation are appropriate. They only need honest state, safer defaults, and predictable actions.

------

# Recommended implementation order

## 1. Fix trust failures

- Online versus never-seen contradiction.
- OSPF `21/22` disagreement.
- Config Rules no-op diff.
- Past next-run dates.
- Expired/current session contradictions.
- Audit latest-event calculation.
- `undefined offline`.
- Impossible process timestamps.
- Old telemetry presented as current.
- Backup-protection age semantics.

## 2. Simplify the global shell

- Remove six repeated cards from normal pages.
- Correct the scope selector behavior.
- Move saved views into scope/search.
- Replace the mobile page dropdown with a drawer.
- Show unlock scope and expiry.
- Separate console connectivity from agent connectivity.

## 3. Repair mobile operations

- Add mobile card rendering to the data grid.
- Keep the primary action visible.
- Make terminal and editors full-screen.
- Use list mode for topology.
- Fix tunnel-plan overflow.
- Add accordion/sticky-save behavior to Suite Config and Preferences.

## 4. Normalize action friction

- Preview before unlock.
- One confirmation for destructive or bulk operations.
- No confirmation for ordinary reads.
- Optional Undo for reversible single-target changes.
- Replace immediate approval with one compact decision dialog.
- Collapse advanced controls by default.

## 5. Reduce page duplication

- Remove repeated backup posture from every backup subpage.
- Remove repeated Access Overview from every access subpage.
- Remove global fleet cards from resource and system pages.
- Separate Fleet Alerts triage from Observability alert configuration.
- Rename Config Templates to Template Coverage.
- Hide unfinished Process Metrics.

## 6. Improve scanning and polish

- Human operation/status labels.
- Relative time plus full timestamp.
- Units on every numeric field.
- Named primary actions and tooltips.
- Honest sparse-data charts.
- Fewer cards and clearer visual hierarchy.
- Compact actionable empty states.

The central goal should be: **an expert sees the correct state immediately, performs an ordinary operation in a few obvious actions, and encounters extra friction only when the actual risk justifies it.**
