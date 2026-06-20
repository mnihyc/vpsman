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
export VPSMAN_SUPER_SALT_HEX=<64_hex_salt>

cargo run -p vpsctl -- job-create --command uptime --tags edge
cargo run -p vpsctl -- job-create --command /bin/sh --argv '/bin/sh,-lc,uname -a' --clients edge-01
```

For PTY-backed noninteractive output:

```sh
cargo run -p vpsctl -- job-create --command /bin/sh --argv '/bin/sh,-lc,tty && id' --pty --tags edge
```

Command execution behavior is selected through the `command_execution_policy`
data-source preset domain. Use it to choose shell argv prefix, default working
directory, inherited/clean/minimal environment handling, explicit env values,
PTY enabled/disabled policy, and process-group or direct-child cleanup for a
VPS, pool, or tag. Explicit argv jobs remain the preferred frequent-use path.

### Target confirmation is the execution boundary

Selectors are resolved before submission. In the browser, the confirmation modal
shows the concrete VPS list and that list is sent as `target_client_ids`. In the
CLI, preview/confirmation performs the same freeze. The API stores the selector
for audit but dispatches only the fixed target list it receives.

Schedules also store a fixed target snapshot. Tag changes may show schedules
that involve the edited VPSs, but this is a maintenance notification, not a
warning that schedule targets changed automatically. Use the Schedules table
Target Update action when the saved snapshot should be replaced by the selector's
current resolution.

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

Traditional management tabs such as Jobs, Schedules, Audit, and data-source
presets use the same record-table controls. Check the total and filtered row
counts before acting, use the field selector when you know whether you are
searching by VPS, command, status, operator, domain, or preset, and page
through the result set instead of relying on an unbounded list.

For daily 20+ VPS operation, this is the preferred browser pattern:

1. Select the fleet scope, pool, or tag.
2. Search within the relevant field.
3. Confirm the filtered count and current page.
4. Open the row, dispatch the action, or follow the matching audit/job record.

## History Retention And Export

History retention policies are managed by domain. Use dry-run before pruning,
especially for object-backed domains such as job outputs and backup artifacts:

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

The Audit panel exposes the same policy update, dry-run, prune, and export
controls.

## Terminal Sessions

Open a bounded polled terminal session:

```sh
cargo run -p vpsctl -- terminal-open \
  --session-id <session_uuid> \
  --argv /bin/sh \
  --clients edge-01 \
  --confirmed
```

Send input and poll output:

```sh
cargo run -p vpsctl -- terminal-input \
  --client-id edge-01 \
  --session-id <session_uuid> \
  --text $'uptime\n' \
  --confirmed

cargo run -p vpsctl -- terminal-poll \
  --session-id <session_uuid> \
  --replay-from-seq 1 \
  --clients edge-01 \
  --confirmed
```

Terminal input order is reserved by the server for the selected client and
session; do not provide an input sequence.

List durable sessions and replay persisted output:

```sh
cargo run -p vpsctl -- terminal-sessions --limit 20
cargo run -p vpsctl -- terminal-replay \
  --client-id edge-01 \
  --session-id <session_uuid> \
  --from-seq 1 \
  --output-file ./terminal.log
```

The panel Jobs view exposes the same attach/replay, durable replay preview,
poll, input, resize, and close actions from terminal session rows.

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
  --path /var/log/bird.log \
  --destination ./bird.log \
  --clients edge-01 \
  --confirmed
```

CLI/VTY transfers expose `--poll-interval-ms` and `--max-polls` for unusually
slow links. The browser console uses the job `timeout_secs` plus control-plane
grace for each transfer-step wait.

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

In the Jobs panel, use File transfer sessions to select multiple completed
download handoffs and download them together. The browser saves each verified
artifact with a client/session prefix so the same remote path from different
VPSs does not overwrite another file. Select Stream to file when the browser
supports the File System Access API and the artifact should be written without
retaining the whole file in browser memory.

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

## User Sessions And Processes

```sh
cargo run -p vpsctl -- user-sessions --tags edge
cargo run -p vpsctl -- process-list --limit 50 --tags edge
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
  --interval-secs 3600 \
  --catch-up-policy run_once \
  --retry-delay-secs 300 \
  --max-failures 5
```

Use `--catch-up-policy skip_missed` to ignore missed runs, `run_once` to work
through missed intervals one worker pass at a time, or `run_all_limited` with
`--catch-up-limit <1-25>` for bounded backlog materialization. Keep a stable
`--worker-id` for repeated `vpsman-worker --once` runs in the same smoke or
maintenance script so the worker can renew its singleton leases. Current
singleton leases cover schedules, alert notifications, telemetry rollups,
network-rate rollups, and telemetry pruning.

Inspect schedules and their due-run history:

```sh
cargo run -p vpsctl -- schedules
cargo run -p vpsctl -- jobs --limit 20
```

In the browser, use the Schedules page and its Schedule runs subpage for the
same review flow. If tag changes make a schedule selector resolve to a different
set of VPSs, the schedule shows **Update targets**; use it to deliberately
replace the saved fixed snapshot. Tag mutation dialogs show this as a target
update notice, not as an automatic schedule edit.

Manual **Apply now** runs use the same schedule command timeout source as the
worker: `worker.schedule_command_timeout_secs`, then legacy
`timeout.worker_schedule_command_secs`, then the 30 second default, with the
existing target capability clamp applied during job creation.

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
