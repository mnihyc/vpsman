# Tutorial 09: Headless CLI And VTY

Everything important in the panel should have a headless CLI or VTY path. Use
CLI for scripts and VTY for interactive router-style work.

## CLI Basics

Set API access:

```sh
export VPSMAN_API_URL=https://panel.example.com
export VPSMAN_API_TOKEN=<operator_token>
```

Set local privileged proof material only when dispatching privileged work:

```sh
export VPSMAN_SUPER_PASSWORD=<local_super_password>
export VPSMAN_SUPER_SALT_HEX=<64_hex_salt>
```

Check available commands:

```sh
cargo run -p vpsctl -- --help
cargo run -p vpsctl -- terminal-replay --help
cargo run -p vpsctl -- tunnel-plan --help
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
cargo run -p vpsctl -- --output json super-password-rotations --limit 20
cargo run -p vpsctl -- --output pretty-json tunnel-plan --name edge-a-b --interface-name gre-ab --kind gre --left-client-id edge-a --right-client-id edge-b --left-underlay 203.0.113.10 --right-underlay 203.0.113.20 --address-pool-cidr 10.255.0.0/30 --bandwidth 100m --latency-ms 20
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
fleet-alert-notification-channels --delivery-kind audit_log
fleet-alert-notification-dispatch --dry-run --include-muted
terminal-sessions --limit 20
job-create uptime tag:edge
job-follow <job_uuid> --interval-ms 1000 --max-polls 120
disable
quit
```

`enable` validates local proof material. It does not send the plaintext super
password to the API. `show privilege` confirms whether local proof material is
loaded without printing the password or salt. `show capabilities` lists
read-only, proof-gated, root-sensitive, and `--force-unprivileged` command
families. `show degraded-policy` explains how normal-user agents report
`degraded_unprivileged` by default and when best-effort forced execution is
explicitly available. `disable` clears local proof material for the current VTY
session and returns the prompt to `vpsman>`.

## Useful VTY Commands

```text
enrollment-tokens
reenrollment-token-create --client-id edge-01 --confirmed
key-lifecycle-report
super-password-rotations --limit 20
client-key-revocations
client-key-revoke --client-id edge-01 --reason rebuilt --confirmed
fleet-alert-state-update --alert-id agent_status:agent:<hash> --action mute --muted-for-secs 14400 --reason maintenance --confirmed
fleet-alert-policy-upsert --name edge-resource-alerts --scope-kind tag --scope-value edge --memory-available-warning-ratio 0.35 --memory-available-critical-ratio 0.15 --cpu-load-warning 1.5 --cpu-load-critical 3.0 --priority 25 --confirmed
fleet-alert-notification-channel-upsert --name edge-audit --scope-kind tag --scope-value edge --min-severity warning --categories agent_status,network --operator-states open,escalated --delivery-kind audit_log --target audit:fleet --cooldown-secs 3600 --confirmed
fleet-alert-notification-dispatch --confirmed --include-muted
fleet-alert-notifications --status delivered
fleet-alert-notification-process --status queued --delivery-kind webhook_json --dry-run
fleet-alert-notification-process --status queued --delivery-kind webhook_json --confirmed
file-pull --path /etc/hostname tag:edge
file-push --source ./payload.txt --path /tmp/payload.txt tag:edge --confirmed
terminal-poll --session-id <uuid> --replay-from-seq 1 --client-id edge-01
terminal-replay --client-id edge-01 --session-id <uuid> --output-file ./terminal.log
process-list tag:edge --limit 50
process-start edge-worker --argv /usr/bin/sleep --argv 60 tag:edge
tunnel-plans
topology-graph --limit 50
backups
backup-policies
backup-policy-upsert nightly-edge --path /etc/hostname --include-config tag:backup-critical --confirmed
backup-policy-prune --dry-run
restore-plans
migration-run <restore_plan_uuid> --confirmed
agent-update-rollouts
agent-update-release-latest --name vpsman-agent --channel stable
agent-update-artifact-upload --name vpsman-agent --version 0.1.1 --artifact-file ./target/vpsman-agent --signing-seed-hex <seed> --rollback-artifact-file ./target/vpsman-agent.previous --stream --confirmed
agent-update-rollout-control --rollout-id <uuid> --pause --pause-reason maintenance --confirmed
agent-update-rollout-control --rollout-id <uuid> --resume --health-gate heartbeat_verified --confirmed
agent-update-rollout-delegate-activation --rollout-id <uuid> --proof-ttl-secs 3600 --restart-agent --force-unprivileged --confirmed
agent-update-rollout-delegate-rollback --rollout-id <uuid> --proof-ttl-secs 3600 --force-unprivileged --confirmed
```

## Headless Operating Pattern

1. Inspect: `summary`, `agents`, `fleet-alerts`, `gateway-sessions`.
2. Resolve targets: `bulk-resolve`, inner `id:<client_id>` or
   `name:<display_name>` selectors, explicit `tag:<name>`, or bare tag names.
3. Dispatch: proof-gated command with confirmation for destructive work.
4. Observe: `jobs`, `job-targets`, `job-outputs`, `job-follow`.
5. Recover: `job-cancel`, `restore-rollback`, `agent-update-rollback`, or
   `tunnel-rollback` as appropriate.

For rollout control, use `agent-update-rollout-control` in normal VTY mode
because it updates server-side metadata only. Activation, immediate rollback,
delegated activation, and delegated rollback proof creation still need
privileged proof through `enable` or explicit CLI proof material. Delegated
activation stores scoped proof envelopes for the exact rollout artifact and
lets the API dispatch only when the worker recommends `operator_activate_batch`
for a still-completed target. Delegated rollback stores only scoped proof
envelopes and lets the API dispatch the frozen rollback command if the rollout
target later becomes `heartbeat_timeout` or `activation_failed`.
`agent-update-rollouts` includes delegation summary arrays with ready,
dispatching, dispatched, expired, and failed counts plus proof expiry windows.
Renew expired proofs by re-running the same delegate command with a fresh proof
TTL. Use `--force-unprivileged` only for a known normal-user agent where the
operator deliberately wants a best-effort activation or rollback attempt; the
flag is dispatch policy, not a server-editable proof payload.

Headless notification delivery has two paths. Use
`fleet-alert-notification-process` for an immediate reviewed run from CLI/VTY,
and keep `vpsman-worker` running for automatic queued delivery and retention
pruning. Worker notification flags mirror environment variables, so scripts can
set delivery limit, retention days, prune limit, and webhook timeout without
editing code.

This mirrors the panel workflow and keeps browser and headless operations
consistent.
