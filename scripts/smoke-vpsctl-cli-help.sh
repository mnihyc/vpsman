#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash cargo

if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsctl >/dev/null
fi

bin="${VPSMAN_VPSCTL_BIN:-target/debug/vpsctl}"
if [[ ! -x "$bin" ]]; then
  smoke_fail "vpsctl binary is not executable: $bin"
fi

commands=(
  health bootstrap login refresh me operators operator-create operator-sessions operator-session-revoke totp-setup totp-confirm totp-disable
  enrollment-tokens enrollment-token-create reenrollment-token-create client-key-revocations client-key-revoke key-lifecycle-report enroll-claim enroll-config
  summary agents fleet-alerts fleet-alert-export fleet-alert-states fleet-alert-state-update fleet-alert-policies fleet-alert-policy-upsert fleet-alert-notification-channels fleet-alert-notification-channel-upsert fleet-alert-notifications fleet-alert-notification-dispatch fleet-alert-notification-process gateway-sessions telemetry-rollups telemetry-network-rates telemetry-tunnels tags tag-create agent-tag
  data-source-presets data-source-preset-create data-source-preset-clone data-source-preset-diff data-source-preset-test data-source-preset-update data-source-status data-source-assignments data-source-hot-config data-source-hot-config-apply data-source-preset-assign
  jobs schedules schedule-create schedule-update schedule-enable schedule-disable schedule-defer schedule-apply-now schedule-delete job-create job-shell
  terminal-open terminal-input terminal-poll terminal-resize terminal-close terminal-sessions terminal-replay terminal-follow
  file-pull file-push file-transfer-upload file-transfer-download file-transfers file-transfer-handoff file-transfer-sources file-transfer-source-upload file-transfer-source-download user-sessions hot-config agent-update agent-update-check agent-update-activate agent-update-rollback agent-update-signature agent-update-release-publish agent-update-artifact-upload agent-update-release-latest agent-update-releases
  process-list process-start process-stop process-restart process-status process-logs process-supervisor-inventory
  job-targets job-outputs job-follow job-output-artifact audit history-retention history-retention-upsert history-retention-prune history-export network-observations network-trends network-ospf-recommendations network-ospf-update-plans topology-graph
  backups backup-artifacts backup-policies backup-policy-upsert backup-policy-prune restore-plans migration-links backup-request backup-run
  backup-artifact-record backup-artifact-upload backup-artifact-upload-chunked backup-artifact-handoff restore-plan restore-run restore-rollback migration-link migration-run
  bulk-resolve tunnel-plans tunnel-plan tunnel-promote-telemetry tunnel-promote-adapter tunnel-apply tunnel-ospf-cost-update tunnel-rollback
  tunnel-status tunnel-probe tunnel-speed-test noise-keygen vty
)

root_help="$("$bin" --help)"
for expected in "operators" "operator-create" "operator-sessions" "operator-session-revoke" "totp-setup" "totp-confirm" "totp-disable" "client-key-revocations" "client-key-revoke" "key-lifecycle-report" "fleet-alerts" "fleet-alert-export" "fleet-alert-states" "fleet-alert-state-update" "fleet-alert-policies" "fleet-alert-policy-upsert" "fleet-alert-notification-channels" "fleet-alert-notification-channel-upsert" "fleet-alert-notifications" "fleet-alert-notification-dispatch" "fleet-alert-notification-process" "gateway-sessions" "telemetry-rollups" "telemetry-network-rates" "telemetry-tunnels" "data-source-presets" "data-source-preset-create" "data-source-preset-clone" "data-source-preset-diff" "data-source-preset-test" "data-source-preset-update" "data-source-status" "data-source-assignments" "data-source-hot-config" "data-source-hot-config-apply" "data-source-preset-assign" "job-shell" "job-follow" "job-output-artifact" "history-retention" "history-retention-upsert" "history-retention-prune" "history-export" "terminal-sessions" "terminal-replay" "terminal-follow" "file-transfers" "file-transfer-handoff" "file-transfer-sources" "file-transfer-source-upload" "file-transfer-source-download" "backup-policies" "backup-policy-upsert" "backup-policy-prune" "backup-artifact-upload-chunked" "backup-artifact-handoff" "tunnel-speed-test" "tunnel-promote-telemetry" "tunnel-promote-adapter" "tunnel-ospf-cost-update" "restore-run" "restore-rollback" "migration-links" "migration-link" "migration-run" "network-observations" "network-trends" "network-ospf-recommendations" "network-ospf-update-plans" "topology-graph" "agent-update" "agent-update-check" "agent-update-activate" "agent-update-rollback" "agent-update-signature" "agent-update-release-publish" "agent-update-artifact-upload" "agent-update-release-latest" "agent-update-releases" "process-supervisor-inventory"; do
  if [[ "$root_help" != *"$expected"* ]]; then
    smoke_fail "root help is missing expected command: $expected"
  fi
done

file_transfer_upload_help="$("$bin" file-transfer-upload --help)"
[[ "$file_transfer_upload_help" == *"--source-artifact-id"* ]] || fail "file-transfer-upload help missing --source-artifact-id"

for command in "${commands[@]}"; do
  "$bin" "$command" --help >/dev/null
done

printf '{\n'
printf '  "vpsctl_cli_help_smoke": "ok",\n'
printf '  "command_count": %s,\n' "${#commands[@]}"
printf '  "checks": ["root_help", "subcommand_help", "network_command_parser_shape"]\n'
printf '}\n'
