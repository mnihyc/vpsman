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
