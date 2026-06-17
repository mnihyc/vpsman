# Job Status Model

VPSMan uses one canonical job lifecycle across API, database, CLI, frontend, gateway dispatch, worker schedules, and generated frontend contracts.

## Jobs

Job statuses are:

- `queued`: the job has been accepted and no unfinished target has been
  claimed by the dispatcher yet.
- `running`: at least one target has been claimed for dispatch, is waiting for
  gateway/agent ACK, or is actively running, and terminal aggregation has not
  completed.
- `completed`: every target completed successfully.
- `partial_success`: at least one target completed successfully and at least one target did not.
- `skipped`: the job had zero durable targets, or every durable target was skipped by backend capability policy.
- `rejected`: job creation was rejected before dispatch.
- `failed`: target results were terminal and failed without any completed target.
- `agent_timeout`: at least one target timed out inside the agent command runtime and no target completed.
- `control_timeout`: at least one target exceeded the API/gateway control deadline and no target completed.
- `canceled`: target cancellation completed and no target completed.

## Targets

Target statuses are:

- `queued`: durable target exists and is eligible for dispatcher claim.
- `dispatching`: dispatcher has claimed the target and is waiting for gateway/agent ACK or final output.
- `running`: agent ACK was received and final output is still outstanding.
- `completed`: final output completed with exit code `0`.
- `skipped`: backend capability policy intentionally did not dispatch this target.
- `rejected`: gateway or agent rejected the target before command execution.
- `failed`: final output completed with non-zero exit code or dispatch preparation failed.
- `agent_lost`: backend has positive evidence that the executing agent process
  restarted or was otherwise lost before final output.
- `agent_timeout`: final output reported agent-side timeout.
- `control_timeout`: backend control deadline expired before final output.
- `canceled`: operator cancellation completed before target completion.

## Rules

- The database `jobs.status` and `job_targets.status` CHECK constraints are the durable truth boundary.
- UIs use `target_count`, `target_counts.total`, and canonical target records for progress; there is no separate dispatch-admission count.
- Claiming any queued target promotes the parent job from `queued` to `running`
  before agent ACK, so a dispatching target is never represented by a still
  queued parent job.
- `skipped` is neither success nor failure at target level. Jobs with completed plus skipped targets aggregate to `partial_success`; all-skipped jobs aggregate to `skipped`.
- Availability is contextual display only. Offline fixed targets remain target records until backend deadline, then become `control_timeout`.
- `agent_lost` is a target-level terminal unsuccessful status. Parent jobs
  aggregate it through the existing unsuccessful job statuses; there is no
  separate job-level `agent_lost` status.
- `timeout_secs` is the agent execution budget. The API control deadline adds
  dispatch/ACK, internal HTTP/event-post, and `control_deadline_grace_secs`
  grace time so healthy long-running commands near their own timeout are not
  mislabeled as `control_timeout` because of gateway/API latency.
- `agent_lost` and `control_timeout` are deliberately separate. `agent_lost`
  requires positive restart/loss evidence such as a changed agent process
  incarnation or a missing expected update activation heartbeat. `control_timeout`
  means the immutable control deadline elapsed without proof that the process
  restarted.
- `control_timeout` is a terminal control-plane decision. Late final agent
  output may be persisted as diagnostic evidence, but it must not rewrite the
  target or parent job terminal state.
- Agent-side timeouts are reported as structured `command_timeout` status
  output and map to `agent_timeout`. Operator cancellation is operational for
  active shell/script/PTY children, backup, restore, network apply/rollback,
  OSPF updates, network status/probe/speed-test, resumable file-transfer
  steps, and terminal operations:
  cancel requests are acknowledged as accepted while the worker is still
  finalizing, the agent interrupts the operation through its cancel token, and
  the terminal target state becomes `canceled` only after the agent emits
  structured `command_canceled` output. Cancellation can occur after a command
  has already changed host state; operators should use status output and normal
  rollback/remediation workflows to inspect partial effects. For resumable
  uploads, cancellation before a chunk temp-file write or before commit's final
  move reports canceled; once a chunk write or final move has succeeded, the
  completed transfer status wins. Resumable download chunks do not mutate host
  state and can cancel before stdout/status is emitted.
- Final output must be durably recorded before a target is marked terminal.
  Job-finished side effects are published only after both output and terminal
  target state are durable. Backup artifact auto-recording from async gateway
  ingest is best-effort after target/job finalization; failure to stage or store
  the artifact must not prevent the parent job from reaching a terminal state.
- Gateway forwarder delivery is RAM-first. Forwarder events remain in memory up
  to the configured RAM cap, spill to gateway disk spool when required, replay
  pending spool files after restart, and defer pending events to the spool
  during bounded graceful shutdown. Spool files are promoted with a temp-file
  write, best-effort fsync, atomic rename, schema version, and body checksum;
  corrupt startup entries are quarantined and do not block gateway start.
  Startup replay checks `/internal/v1/gateway/command-output/acks` before
  reposting spooled command outputs. Non-final chunks are ACKed when the output
  row is durable; final chunks are ACKed only after both the output row and
  terminal target state are durable. Command-output retry retention defaults
  to 24 hours and can be adjusted with
  `[gateway].command_output_event_ttl_secs` or the
  `VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS` override. Queue pressure must
  not delete already spooled forwarder files; they remain on disk for later
  replay. This is controlled-restart safety, not hard-crash durability for every
  RAM-resident event. Accepted residual loss boundaries are spool disabled, disk
  cap or disk write failure, hard gateway process crash before a RAM-resident
  event is spooled, and sustained overload beyond configured retention.
- Job finalization is idempotent and repairable. The first process that
  transitions a job from non-terminal to terminal emits terminal side effects,
  and later refresh/replay paths may re-materialize deterministic terminal
  webhooks and schedule outcome events without double-incrementing schedule
  failure counters or moving a schedule backward after a newer job has already
  completed.
- Job output rows are first-writer-wins by `(job_id, client_id, seq)`. Same
  duplicate rows are no-ops; conflicting replay rows are audit evidence and do
  not overwrite previously durable output. Agent duplicate replay preserves the
  original terminal status/result even when cached replay bytes are no longer
  retained.
- Frontend TypeScript status unions come from `vpsman_common` via `frontend/src/generated/protocolContracts.ts`; frontend code must not maintain separate status alias lists.

## Download Archives

Job output and status downloads are intentionally separate so operators can
review payloads without confusing them with execution metadata:

- `Download outputs` archives retained output payload streams by target, such
  as `stdout.bin` and `stderr.bin`.
- `Download files` archives completed file-download payloads at
  `<target>/<filename>` and adds root-level `<target>_status.json`
  file-download metadata. A real downloaded file named `status.json` remains a
  normal target file at `<target>/status.json`.
- `Download status` archives target execution status only. It contains root
  `targets.json` for all target records plus root-level
  `<target>_status.json` entries for each target.

## Shared Workflow Contracts

The same ownership rule applies to command and adjacent workflow models:

- Command safety, confirmation requirements, canonical command type labels, and command-template display groups are defined in `vpsman_common` and generated into frontend contracts.
- Command templates store backend-derived `command_type`; user-facing grouping is `display_group`. API-owned built-in command templates provide immutable default job parameters, while user-defined command templates remain operator editable.
- Terminal session state/status/event, file-transfer direction/status/event/command type, backup/restore/migration/tunnel/update-release statuses, data-source readiness, and topology evidence statuses are closed generated vocabularies.
- Generated frontend contracts also include status-class maps for closed workflow domains. Frontend code may use generic `statusClass` only for free-form display values outside these canonical models.
- API, CLI, agent, worker, database constraints, frontend types, mocks, and tests must update through `vpsman_common` first. Adding a new workflow state without regenerating contracts and updating constraints is a contract drift bug.
