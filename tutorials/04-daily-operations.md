# Tutorial 04: Daily Operations

This tutorial covers the workflows operators use most often: commands,
terminal sessions, file transfers, process supervision, schedules, and job
output.

## Run Commands

Privileged command execution first resolves the selector into a fixed VPS
target list, builds a request-bound privilege assertion locally, and sends both
the audit selector and concrete `target_client_ids` to the API. Retries reuse the
client-generated job ID and cannot silently change targets:

```sh
export VPSMAN_SUPER_PASSWORD=<local_super_password>
source ./path/to/secrets/operator-privilege.env

cargo run -p vpsctl -- job-create --command uptime --tags edge
cargo run -p vpsctl -- job-create --command /bin/sh --argv '/bin/sh,-lc,uname -a' --clients edge-01
```

For PTY-backed noninteractive output:

```sh
cargo run -p vpsctl -- job-create --command /bin/sh --argv '/bin/sh,-lc,tty && id' --pty --tags edge
```

Command execution behavior is selected through the `command_execution`
configuration behavior. Use its effective preset to choose shell argv prefix, default working
directory, inherited/clean/minimal environment handling, explicit env values,
PTY enabled/disabled policy, and process-group or direct-child cleanup for a
VPS. Use one point-in-time target selection to set explicit overrides on
multiple VPSs. Explicit argv jobs remain the preferred frequent-use path.

### Target confirmation is the execution boundary

Selectors are resolved before submission. In the browser, the confirmation modal
shows the concrete VPS list and that list is sent as `target_client_ids`. In the
CLI, preview/confirmation performs the same freeze. The API stores the selector
for audit but dispatches only the fixed target list it receives.

Schedules also store a fixed target snapshot. Tag changes may show schedules
that involve the edited VPSs, but this is a maintenance notification, not a
warning that schedule targets changed automatically. Use the Schedules table
**Update targets** table action when the saved snapshot should be replaced by
the selector's current resolution.

For a fleet-wide check, open **System > Maintenance > Stale selectors**. The
table compares current visible-fleet resolution with frozen Schedule (including
backup-policy), Ping-target, and active shared-view snapshots. Select records
and use **Actions > Update targets**, or use **Update all** to review every
resolvable stale row. Rows marked **Repair required** have invalid selector or
schedule data and must be corrected in their owning workflow. Each resource
keeps its own reviewed write and audit boundary; approval records remain
immutable evidence.

## Inspect Jobs And Output

```sh
cargo run -p vpsctl -- jobs --limit 20
cargo run -p vpsctl -- job-targets --job-id <job_uuid>
cargo run -p vpsctl -- job-target-status-download \
  --job-id <job_uuid> \
  --output-file ./job-status.tar
cargo run -p vpsctl -- job-outputs --job-id <job_uuid>
cargo run -p vpsctl -- job-follow --job-id <job_uuid> --interval-ms 1000 --max-polls 120
```

If a large output chunk was externalized to local object storage:

```sh
cargo run -p vpsctl -- job-output-download \
  --job-id <job_uuid> \
  --client-id edge-01 \
  --seq <output_seq> \
  --output-file ./stdout.bin
```

Explicit output and status downloads use the CLI binary streaming path and
write through a temporary file before rename. They are not subject to the JSON
API response cap; the server still enforces the configured
`api.artifact_max_bytes` / `VPSMAN_ARTIFACT_MAX_BYTES` envelope.

In the browser Job History detail panel, bulk archive buttons are intentionally
separate:

- `Download outputs` downloads retained command output payloads such as
  `stdout.bin` and `stderr.bin` by target. It does not add target execution
  status files.
- `Download files` appears for completed file-download jobs. The archive keeps
  each downloaded file at `<target>/<filename>` and adds per-target
  `<target>_status.json` file-download metadata at the archive root. A real
  downloaded file named `status.json` remains `<target>/status.json` and does
  not collide with metadata.
- `Download status` downloads target execution status only. The archive
  contains root `targets.json` for all targets plus root-level
  `<target>_status.json` entries for individual target records.

Durable output history is first-writer-wins by `(job_id, client_id, seq)`.
Duplicate command replay does not insert marker rows into the normal output
stream, and replay conflicts are retained as audit evidence instead of
rewriting output already stored for operators.

Timeout, cancel, terminal close, and process-stop status output includes a
`cleanup` object when the agent had to terminate a process group. Inspect it
for the signal path, fallback use, and final running state during incident
review.

Operator cancellation is active, not just advisory, for shell/script/PTY jobs,
long-running backup, restore, network and terminal workflows, and resumable
file-transfer steps. The API marks running targets `canceled` only after the
agent emits structured `command_canceled` output. A cancellation requested after
host mutation starts can still require a normal rollback or compensating
operation; resumable uploads report completion once a chunk write or final move
has crossed its completion boundary, while download chunks can cancel before
stdout/status is emitted.

Gateway forwarder delivery is RAM-first with disk spool overflow. Final command
output drives target terminal state, so production gateways should keep
`[gateway].command_output_event_ttl_secs` high enough for expected API or
database maintenance windows. The default is 24 hours, and the
`VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS` environment variable overrides
the suite config for smoke tests and emergency tuning. Graceful gateway
shutdown defers pending forwarder events to the spool for controlled restart
replay; a hard process crash before a RAM-resident event is spooled remains a
residual loss boundary.

## Use Record Tables In The Panel

Traditional management tabs such as Jobs, Schedules, Audit, and configuration
presets use the same record-table controls. Check the total and filtered row
counts before acting, use the search-field filter when you know whether you are
searching by VPS, command, status, operator, domain, or preset, and page
through the result set instead of relying on an unbounded list.

Create and refresh controls are in the table header. Select one or more rows,
then use the header **Actions** menu for operations on that selection. On
desktop, right-clicking a row exposes the same row operations without changing
the selection. On mobile, select the card and use the same header **Actions**
menu; tap the card to expand its details. There is intentionally no rightmost
Action column to chase while horizontally scrolling. Wide desktop tables scroll
horizontally inside their panel. The **Fields** menu marks every optional column
as shown or hidden, and saved order, width, visibility, and page-size choices
remain specific to that table.

For daily 20+ VPS operation, this is the preferred browser pattern:

1. Select the fleet scope, pool, or tag.
2. Search within the relevant field.
3. Confirm the filtered count and current page.
4. Select the intended rows and use **Actions**, or open **Details** for one
   record.
5. Follow local progress and the matching audit/job record; status feedback
   stays with the workflow that produced it. A completed outcome scrolls into
   view instead of appearing silently outside the viewport; correcting a draft
   clears the obsolete error from the prior input.

## History Retention And Export

History retention policies are managed by retained-history domain. Exact
resource, network, Ping, traffic, and automatic-reachability evidence has a
fixed one-day lifecycle; it is not an operator retention policy.

`telemetry_rollups`, `telemetry_network_rates`, `telemetry_ping_rollups`,
`traffic_counter_rollups`, and `network_observations` default to a 3,650-day
final horizon. Topology graph and trend exports are derived from the unified
network-observation lifecycle. High-volume serving domains remain enabled while
collection continues.
The running worker processes durable natural owners in bounded transactions;
the bounds limit lock and write bursts, while ready work drains immediately.
Before pruning, the worker transactionally promotes settled monitoring history
through fixed UTC-aligned age tiers: 1 minute through 2 days, 5 minutes through
8 days, 30 minutes through 31 days, 1 hour through 91 days, 3 hours through 181
days, 6 hours through 366 days, and 1 day through 3,650 days. Traffic counters
retain exact minute endpoints for one day plus the current partial hour and
one sequencing predecessor. Completed older hours remain lossless through day
91 before the 3-hour-and-coarser tiers; the final traffic horizon cannot be
configured below 32 days because every monthly reset cycle must remain
reconstructable. The UI labels
the effective source resolution and does not invent fine points from coarse
history.

Automatic declared-tunnel reachability remains exact for 1 day, then follows
the retained monitoring tiers. Its separately retained latest endpoint state
continues to drive topology and OSPF decisions. Manual probes, speed tests, and
network-status evidence are not folded into automatic reachability rollups.

A stored policy override can set the batch limit or final horizon for supported
retained domains; canonical promotion boundaries and exact one-day lifecycles
remain fixed. For automatic reachability, the configured prune limit is
one total terminal-history row budget shared by exact and rollup deletion in a
worker pass; fixed tier promotion and inactive-current lifecycle cleanup are
separate from that terminal budget. Other domains remain explicit maintenance workflows. Use
dry-run before manual pruning, especially for object-backed domains such as job
outputs and backup artifacts:

Dashboard network rates are interval averages derived from cumulative interface
counters, not instantaneous samples. The console presents these RX/TX transfer
rates in decimal `KB/s`, `MB/s`, or `GB/s`; declared bandwidth and active tunnel
speed-test throughput remain bit rates in `Mbps` or `Gbps`. Active tunnel
throughput is a separate bounded-test average. See
[Telemetry metric definitions](../docs/telemetry-metrics.md) before comparing
chart rates, traffic totals, or test throughput.

```sh
cargo run -p vpsctl -- history-retention

cargo run -p vpsctl -- history-retention-upsert \
  --domain audit_logs \
  --retention-days 90 \
  --prune-limit 250 \
  --export-enabled true \
  --confirmed

cargo run -p vpsctl -- history-retention-prune \
  --domain audit_logs \
  --dry-run
```

Webhook-rule event and delivery retention honors
`webhook_rule_retention_days` from the worker config exactly; the shipped
default is 90 days. Telemetry webhooks are materialized directly from the
bounded canonical-sample cursor, so they do not create a second source-event
retention domain. Permanent webhook delivery failures remain visible until the
general retention age and then prune with their linked delivery alert evidence.
Resolved alert lifecycle history remains for 90 days. Endpoint result limits
bound each response without shortening stored history; current and unresolved
episodes are not retention candidates.

For object-backed domains, keep `--metadata-only false` only when the API has
object storage configured and the retained blobs should be deleted together
with metadata. Use metadata-only pruning when an external archival process owns
object cleanup.

Export bounded history for incident review or migration planning:

```sh
cargo run -p vpsctl -- history-export \
  --domains audit_logs,job_outputs,backup_artifacts,topology_history \
  --limit 50
```

Audit > Retention & export exposes the same policy update, dry-run, prune, and
export controls.

## Terminal Sessions

Open one bounded, audited terminal session:

```sh
cargo run -p vpsctl -- terminal-open \
  --session-id <session_uuid> \
  --argv /bin/sh \
  --clients edge-01 \
  --confirmed
```

After that open job is authorized, send input or resize/close the same live
session directly. These controls do not create additional jobs and do not
require another privilege assertion:

```sh
cargo run -p vpsctl -- terminal-input \
  --client-id edge-01 \
  --session-id <session_uuid> \
  --text $'uptime\n'

cargo run -p vpsctl -- terminal-poll \
  --client-id edge-01 \
  --session-id <session_uuid> \
  --replay-from-seq 1

cargo run -p vpsctl -- terminal-resize \
  --client-id edge-01 \
  --session-id <session_uuid> \
  --cols 120 \
  --rows 40

cargo run -p vpsctl -- terminal-close \
  --client-id edge-01 \
  --session-id <session_uuid> \
  --reason operator_finished
```

The agent assigns terminal input order for the selected client and session; do
not provide an input sequence. The `terminal_open` job remains `running` while
the PTY is live and becomes terminal when the session closes, exits, fails, or
is found missing. Session list, replay, control, and job-history reads reconcile
that lifecycle lazily.

List durable sessions and replay persisted output:

```sh
cargo run -p vpsctl -- terminal-sessions --limit 20
cargo run -p vpsctl -- terminal-replay \
  --client-id edge-01 \
  --session-id <session_uuid> \
  --from-seq 1 \
  --output-file ./terminal.log
```

Remote > Terminal exposes the same attach/replay, direct xterm input, automatic
resize, and reviewed close actions from terminal session rows. The xterm surface
sends normal terminal bytes, including Enter, Tab, Escape, arrow keys, and
Ctrl+C; there is no separate plaintext command composer.

## File Transfers

Small privileged pulls and pushes:

```sh
cargo run -p vpsctl -- file-pull --path /etc/hostname --tags edge
cargo run -p vpsctl -- file-push --source ./payload.txt --path /tmp/payload.txt --tags edge --confirmed
```

Resumable transfers:

```sh
cargo run -p vpsctl -- file-transfer-upload \
  --source ./payload.bin \
  --path /tmp/payload.bin \
  --tags edge \
  --confirmed

cargo run -p vpsctl -- file-transfer-download \
  --path /var/log/routing.log \
  --destination ./routing.log \
  --clients edge-01 \
  --confirmed
```

CLI/VTY transfers wait for backend terminal state by default; `--max-polls 0`
means unlimited, and a nonzero `--max-polls` is an explicit operator cap.
The browser console uses backend terminal state for each transfer-step wait.

Inspect sessions and materialize a completed download through server-side
handoff:

```sh
cargo run -p vpsctl -- file-transfers --limit 20
cargo run -p vpsctl -- file-transfer-handoff \
  --client-id edge-01 \
  --session-id <transfer_session_uuid> \
  --output-file ./downloaded.bin \
  --confirmed
```

In Remote > Transfers, select multiple completed download handoffs to download
them together. The browser saves each verified
artifact with a client/session prefix so the same remote path from different
VPSs does not overwrite another file. Select Stream to file when the browser
supports the File System Access API and the artifact should be written without
retaining the whole file in browser memory.

Completed download sessions remain visible after history retention, but
handoff is actionable only when `handoff_evidence_status` is
`artifact_available` or `retained_outputs_available`. If retained chunk output
evidence was pruned, incomplete, or conflicting, the session stays in history
with `handoff_available: false` and a `handoff_unavailable_reason` explaining
why a new handoff cannot be created.

Retain a local file as a verified server-side source artifact for later
transfer reuse:

```sh
cargo run -p vpsctl -- file-transfer-source-upload \
  --source ./payload.bin \
  --confirmed
cargo run -p vpsctl -- file-transfer-sources --limit 20
cargo run -p vpsctl -- file-transfer-source-download \
  --artifact-id <source_artifact_uuid> \
  --output-file ./payload.bin
cargo run -p vpsctl -- file-transfer-upload \
  --source-artifact-id <source_artifact_uuid> \
  --path /tmp/payload.bin \
  --clients edge-01 \
  --confirmed
```

File-transfer handoffs and source-artifact downloads use the same binary
streaming path as job-output downloads, so routine downloads are bounded by the
configured artifact max rather than by the small JSON response limit.

Remote > Files is the one-VPS browser and text editor. Reading a text file
returns its current SHA-256 revision. Replacing an existing file must submit that
revision, and the agent checks it again immediately before atomic placement. If
a service, package manager, or local administrator changed the file meanwhile,
the write fails or reports a stale skip according to the selected policy; refresh
and reapply the edit instead of overwriting the local change. Creating a file is
similarly bound to the destination remaining absent through commit.

## User Sessions And Processes

```sh
cargo run -p vpsctl -- user-sessions --tags edge
cargo run -p vpsctl -- host-process-refresh --limit 50 --tags edge
```

Start and supervise a vpsman-managed process:

```sh
cargo run -p vpsctl -- process-start --name edge-worker --argv /usr/bin/sleep --argv 60 --tags edge --confirmed
cargo run -p vpsctl -- process-status --name edge-worker --tags edge
cargo run -p vpsctl -- process-logs --name edge-worker --tags edge
cargo run -p vpsctl -- process-restart --name edge-worker --tags edge --confirmed
cargo run -p vpsctl -- process-stop --name edge-worker --tags edge --confirmed
cargo run -p vpsctl -- process-supervisor-inventory --limit 20
```

Process inventory includes restart evidence, limit-effectiveness status, and
compact cgroup readback when the process is attached to a cgroup-v2 CPU-share
control group.

Limit-bearing starts on unprivileged agents default to degraded status. Use
`--force-unprivileged` only when a best-effort attempt is intentional.

## Schedules And Job Observation

Create a schedule. The selector is resolved once during preview/confirmation,
and that fixed VPS target snapshot is saved with the schedule. Privilege is
verified when the schedule intent or fixed target list is created or changed;
due execution uses the saved snapshot through the durable dispatch queue:

```sh
cargo run -p vpsctl -- schedule-create \
  --name hourly-uptime \
  --command /usr/bin/uptime \
  --tags edge \
  --cron-expr "0 * * * *" \
  --catch-up-policy run_once \
  --retry-delay-secs 300 \
  --max-failures 5 \
  --confirmed
```

Use `--catch-up-policy skip_missed` to ignore missed runs, `run_once` to work
through missed intervals one worker pass at a time, or `run_all_limited` with
`--catch-up-limit <1-25>` for bounded backlog materialization.
`vpsman-worker --once` performs one bounded scheduler pass and requires no
persistent worker identity. Schedules, alert delivery, and telemetry
maintenance use independent durable owners or row claims.

Inspect schedules and their due-run history:

```sh
cargo run -p vpsctl -- schedules
cargo run -p vpsctl -- jobs --limit 20
```

In the browser, use the Schedules page and its Schedule runs subpage for the
same review flow. If tag changes make a schedule selector resolve to a different
set of VPSs, the schedule shows **Update targets**; use it to deliberately
replace the saved fixed snapshot. Select several changed schedules to review
and update their snapshots together; each schedule still uses its own saved
selector. Tag mutation dialogs show this as a target update notice, not as an
automatic schedule edit.

If a saved fixed target is later deleted, revoked, or otherwise unavailable,
due runs and Apply now keep the reviewed schedule runnable by recording that
fixed ID as skipped and dispatching the remaining available targets. A revoked
VPS remains visible and selector-resolvable; it is unavailable for dispatch
until a new key is assigned.

Manual **Apply now** runs use the same schedule job timeout source as the
worker: `worker.schedule_job_max_timeout_secs`, then the 30 second default. The
server-issued job timeout is authoritative for scheduled and Apply now jobs.

Submitted and scheduled jobs enter the durable queue first. As soon as the
dispatcher claims any target and gives it a control deadline, the parent job is
promoted from `queued` to `running`; individual targets then move through
`dispatching` and `running` as gateway and agent ACKs arrive. A
`control_timeout` target is terminal. Late final output after that timeout is
kept as diagnostic output evidence, but it does not rewrite the target or job
terminal state. Observe jobs with polling commands and run an explicit
compensating operation when a completed result needs recovery:

```sh
cargo run -p vpsctl -- job-follow --job-id <job_uuid>
cargo run -p vpsctl -- job-targets --job-id <job_uuid>
cargo run -p vpsctl -- job-target-status-download \
  --job-id <job_uuid> \
  --output-file ./job-status.tar
cargo run -p vpsctl -- job-outputs --job-id <job_uuid>
```
