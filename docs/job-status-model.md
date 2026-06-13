# Job Status Model

VPSMan uses one canonical job lifecycle across API, database, CLI, frontend, gateway dispatch, worker schedules, and generated frontend contracts.

## Jobs

Job statuses are:

- `queued`: the job has at least one target that has not been claimed.
- `running`: at least one target is `queued`, `dispatching`, or `running`.
- `completed`: every target completed successfully.
- `partial_success`: at least one target completed successfully and at least one target did not, or every dispatched target was skipped by capability policy.
- `skipped`: the job had zero durable targets.
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
- `agent_timeout`: final output reported agent-side timeout.
- `control_timeout`: backend control deadline expired before final output.
- `canceled`: operator cancellation completed before target completion.

## Rules

- The database `jobs.status` and `job_targets.status` CHECK constraints are the durable truth boundary.
- UIs use `target_count`, `target_counts.total`, and canonical target records for progress; there is no separate dispatch-admission count.
- `skipped` is neither success nor failure at target level. Jobs with completed plus skipped targets, or all skipped targets, aggregate to `partial_success`.
- Availability is contextual display only. Offline fixed targets remain target records until backend deadline, then become `control_timeout`.
- Frontend TypeScript status unions come from `vpsman_common` via `frontend/src/generated/protocolContracts.ts`; frontend code must not maintain separate status alias lists.
