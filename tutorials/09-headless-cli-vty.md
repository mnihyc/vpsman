# Tutorial 09: Headless CLI And VTY

Everything important in the panel should have a headless CLI or VTY path. Use
CLI for scripts and VTY for interactive router-style work.

## CLI Basics

Set API access:

```sh
export VPSMAN_API_URL=https://panel.example.com
export VPSMAN_API_TOKEN=<operator_token>
```

Set local privilege unlock material only when dispatching privileged work:

```sh
export VPSMAN_SUPER_PASSWORD=<local_super_password>
export VPSMAN_SUPER_SALT_HEX=<64_hex_salt>
```

Check available commands:

```sh
cargo run -p vpsctl -- --help
cargo run -p vpsctl -- terminal-replay --help
cargo run -p vpsctl -- tunnel-plan --help
cargo run -p vpsctl -- tunnel-plan-export --help
```

Use global structured output for scripts. `raw` is the default and preserves
historical stdout; `json` and `pretty-json` normalize any one-shot command
stdout into structured JSON. The interactive `vty` shell intentionally rejects
this mode.

```sh
cargo run -p vpsctl -- --output json agents
cargo run -p vpsctl -- --output json jobs --limit 20
cargo run -p vpsctl -- --output json terminal-sessions --limit 20
cargo run -p vpsctl -- --output json file-transfers --limit 20
cargo run -p vpsctl -- --output json operators
cargo run -p vpsctl -- --output json operator-auth-events --limit 20
cargo run -p vpsctl -- --output pretty-json tunnel-plan \
  --name edge-a-b \
  --interface-name gre-ab \
  --kind gre \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-underlay 203.0.113.10 \
  --right-underlay 203.0.113.20 \
  --address-pool-cidr 10.255.0.0/30 \
  --left-tunnel-ipv4-cidr 10.255.0.0/31 \
  --right-tunnel-ipv4-cidr 10.255.0.1/31 \
  --bandwidth 100m \
  --latency-ms 20
```

Save that stdout as `plan.json` for apply/status/rollback, or re-export a
saved plan later:

```sh
cargo run -p vpsctl -- tunnel-plan-export \
  --plan-id <saved_plan_uuid> \
  --output-file ./plan.json
```

## VTY Privileged Mode

Start VTY:

```sh
cargo run -p vpsctl -- vty
```

Inside VTY:

```text
enable
show privilege
show capabilities
show degraded-policy
summary
agents
fleet-alerts --severity critical
fleet-alert-states --state muted
fleet-alert-state-update --alert-id agent_status:agent:<hash> --action acknowledge --confirmed
fleet-alert-export --include-muted --limit 200
fleet-alert-policies --scope-kind tag --scope-value edge
fleet-alert-notification-channels --delivery-kind webhook
fleet-alert-notification-dispatch --dry-run --include-muted
terminal-sessions --limit 20
job-create uptime tag:edge
job-follow <job_uuid> --interval-ms 1000 --max-polls 120
disable
quit
```

`enable` loads local privilege material. It does not send the plaintext super
password to the API. `show privilege` confirms whether local unlock material is
loaded without printing the password or salt. `show capabilities` lists
read-only, privilege-gated, root-sensitive, and `--force-unprivileged` command
families. `show degraded-policy` explains how normal-user agents report
`degraded_unprivileged` by default and when best-effort forced execution is
explicitly available. `disable` clears local privilege material for the current
VTY session and returns the prompt to `vpsman>`.

## Useful VTY Commands

```text
agent-identity-upsert --client-id edge-01 --client-public-key-hex <hex> --confirmed
client-key-revoke --client-id edge-01 --confirmed
key-lifecycle-report
client-key-revocations
client-key-revoke --client-id edge-01 --reason rebuilt --confirmed
operators
operator-create edge-operator operator VPSMAN_NEW_OPERATOR_PASSWORD --confirmed
operator-update --operator-id <uuid> --role operator --scopes fleet:read,jobs:read --confirmed
operator-disable <uuid> --confirmed
operator-enable <uuid> --confirmed
operator-password-reset <uuid> VPSMAN_NEW_OPERATOR_PASSWORD --confirmed
operator-totp-clear <uuid> --confirmed
operator-auth-events --limit 50
operator-sessions --limit 50
operator-session-revoke <session_uuid> --confirmed
fleet-alert-state-update --alert-id agent_status:agent:<hash> --action mute --muted-for-secs 14400 --reason maintenance --confirmed
fleet-alert-policy-upsert --name edge-resource-alerts --scope-kind tag --scope-value edge --memory-available-warning-ratio 0.35 --memory-available-critical-ratio 0.15 --cpu-load-warning 1.5 --cpu-load-critical 3.0 --priority 25 --confirmed
fleet-alert-notification-channel-upsert --name edge-webhook --scope-kind tag --scope-value edge --min-severity warning --categories agent_status,network --operator-states open,escalated --delivery-kind webhook --target https://hooks.example/vpsman --cooldown-secs 3600 --confirmed
fleet-alert-notification-dispatch --confirmed --include-muted
fleet-alert-notifications --status queued
fleet-alert-notification-process --status queued --delivery-kind webhook --dry-run
fleet-alert-notification-process --status queued --delivery-kind webhook --confirmed
file-pull --path /etc/hostname tag:edge
file-push --source ./payload.txt --path /tmp/payload.txt tag:edge --confirmed
terminal-poll --session-id <uuid> --replay-from-seq 1 --client-id edge-01
terminal-replay --client-id edge-01 --session-id <uuid> --output-file ./terminal.log
process-list tag:edge --limit 50
process-start edge-worker --argv /usr/bin/sleep --argv 60 tag:edge
tunnel-plans
tunnel-plan-export --plan-id <saved_plan_uuid> --output-file ./plan.json
topology-graph --limit 50
backups
backup-policies
backup-policy-upsert nightly-edge --path /etc/hostname --include-config tag:backup-critical --confirmed
backup-policy-prune --dry-run
restore-plans
migration-run <restore_plan_uuid> --archive-transfer-session-id <completed_upload_session_uuid> --confirmed
agent-update-releases --limit 10
agent-update-release-latest --name vpsman-agent --channel stable
agent-update-release-record --name vpsman-agent --version 0.1.1 --artifact-url https://github.com/mnihyc/vpsman/releases/download/v0.1.1/vpsman-agent-linux-x86_64-musl --sha256-hex <sha256> --rollback-artifact-url https://github.com/mnihyc/vpsman/releases/download/v0.1.0/vpsman-agent-linux-x86_64-musl --rollback-sha256-hex <rollback_sha256> --confirmed
agent-update-check --version-url https://github.com/mnihyc/vpsman/releases/latest/download/version.json tag:edge --confirmed
agent-update --artifact-url https://github.com/mnihyc/vpsman/releases/download/v0.1.1/vpsman-agent-linux-x86_64-musl --sha256-hex <sha256> tag:edge --confirmed
agent-update-activate --staged-sha256-hex <sha256> tag:edge --restart-agent --confirmed
agent-update-rollback --rollback-sha256-hex <sha256> tag:edge --confirmed
```

## Headless Operating Pattern

1. Inspect: `summary`, `agents`, `fleet-alerts`, `gateway-sessions`.
2. Resolve targets: `bulk-resolve`, inner `id:<client_id>` or
   `name:<display_name>` selectors, explicit `tag:<name>`, or bare tag names.
   Job and schedule submissions send the resolved VPS IDs as the fixed target
   set; the selector remains audit context.
3. Dispatch: privilege-gated command with confirmation for destructive work.
4. Observe: `jobs`, `job-targets`, `job-target-status-download`,
   `job-outputs`, `job-follow`.
5. Recover: inspect job outputs, then run an explicit compensating operation such as
   `restore-rollback`, `agent-update-rollback`, or `tunnel-rollback` as appropriate.

Custom headless operator tokens need the read scopes for the data they inspect.
`fleet:read` covers metadata/status only. Add `jobs:read`, `terminal:read`,
`integrations:read`, `templates:read`, `schedules:read`, `config:read`,
`network:read`, and `backups:read` when scripts read those sensitive surfaces.

Operator access-management mutations require `enable` and `--confirmed`.
When an action creates, grants, targets, disables, deletes, resets, clears TOTP
for, or revokes a session for an admin operator, add
`--admin-risk-acknowledged`.

Agent update staging, activation, and rollback use the same direct job model as
other privileged commands. Activation and rollback need privilege unlock through
`enable` or explicit CLI unlock environment, and operators observe progress
through `jobs`, `job-targets`, `job-target-status-download`, `job-outputs`,
and `job-follow`. Use
`--force-unprivileged` only for a known normal-user agent where the operator
deliberately wants a best-effort activation or rollback attempt.

Headless notification delivery has two paths. Use
`fleet-alert-notification-process` for an immediate reviewed run from CLI/VTY,
and keep `vpsman-worker` running for automatic queued delivery and retention
pruning. Worker notification flags mirror environment variables, so scripts can
set delivery limit, retention days, prune limit, and webhook timeout without
editing code. Webhook-rule retention uses the configured day count directly
within the 1-3650 day range; the shipped default is 90 days.

This mirrors the panel workflow and keeps browser and headless operations
consistent.
