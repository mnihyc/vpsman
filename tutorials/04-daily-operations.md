# Tutorial 04: Daily Operations

This tutorial covers the workflows operators use most often: commands,
terminal sessions, file transfers, process supervision, schedules, and job
output.

## Run Commands

Privileged command execution resolves targets, builds a request-bound
privilege assertion locally, and sends that assertion to the API. The API
recomputes the operation intent and asks the private gateway to verify it:

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

## Inspect Jobs And Output

```sh
cargo run -p vpsctl -- jobs --limit 20
cargo run -p vpsctl -- job-targets --job-id <job_uuid>
cargo run -p vpsctl -- job-outputs --job-id <job_uuid>
cargo run -p vpsctl -- job-follow --job-id <job_uuid> --interval-ms 1000 --max-polls 120
```

If a large output chunk was externalized to local object storage:

```sh
cargo run -p vpsctl -- job-output-artifact \
  --job-id <job_uuid> \
  --client-id edge-01 \
  --seq <output_seq> \
  --output-file ./stdout.bin
```

Timeout, cancel, terminal close, and process-stop status output includes a
`cleanup` object when the agent had to terminate a process group. Inspect it
for the signal path, fallback use, and final running state during incident
review.

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
  --session-id <session_uuid> \
  --input-seq 1 \
  --text $'uptime\n' \
  --clients edge-01 \
  --confirmed

cargo run -p vpsctl -- terminal-poll \
  --session-id <session_uuid> \
  --replay-from-seq 1 \
  --clients edge-01 \
  --confirmed
```

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

Create a schedule. Privilege is verified when the schedule intent is created
or changed; due execution is handled by the trusted worker after the schedule
time:

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

Use `--catch-up-policy skip_missed` for compatibility, `run_once` to work
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
same review flow.

Dispatched jobs are treated as already handed to the target. Observe them with
job polling commands and run an explicit compensating operation when a completed
result needs recovery:

```sh
cargo run -p vpsctl -- job-follow --job-id <job_uuid>
cargo run -p vpsctl -- job-targets --job-id <job_uuid>
cargo run -p vpsctl -- job-outputs --job-id <job_uuid>
```
