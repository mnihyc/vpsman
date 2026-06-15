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
  agent-identity-upsert client-key-revocations client-key-revoke key-lifecycle-report
  summary agents fleet-alerts fleet-alert-export fleet-alert-states fleet-alert-state-update fleet-alert-policies fleet-alert-policy-upsert fleet-alert-notification-channels fleet-alert-notification-channel-upsert fleet-alert-notifications fleet-alert-notification-dispatch fleet-alert-notification-process gateway-sessions telemetry-rollups telemetry-network-rates telemetry-tunnels tags tag-create agent-tag
  data-source-presets data-source-preset-create data-source-preset-clone data-source-preset-diff data-source-preset-test data-source-preset-update data-source-status data-source-assignments data-source-hot-config data-source-hot-config-apply data-source-preset-assign
  jobs schedules schedule-create schedule-update schedule-enable schedule-disable schedule-defer schedule-apply-now schedule-delete job-create job-shell
  terminal-open terminal-input terminal-poll terminal-resize terminal-close terminal-sessions terminal-replay terminal-follow
  file-pull file-push file-transfer-upload file-transfer-download file-transfers file-transfer-handoff file-transfer-sources file-transfer-source-upload file-transfer-source-download user-sessions hot-config agent-update agent-update-check agent-update-activate agent-update-rollback agent-update-signature agent-update-release-publish agent-update-artifact-upload agent-update-release-latest agent-update-releases
  process-list process-start process-stop process-restart process-status process-logs process-supervisor-inventory
  job-targets job-target-status-download job-outputs job-follow job-output-download server-jobs server-job-cancel artifact-cleanup-preview artifact-cleanup-create audit history-retention history-retention-upsert history-retention-prune history-export network-observations network-trends network-ospf-recommendations network-ospf-update-plans topology-graph
  backups backup-artifacts backup-policies backup-policy-upsert backup-policy-prune restore-plans migration-links backup-request backup-run
  backup-artifact-record backup-artifact-upload backup-artifact-upload-chunked backup-artifact-handoff restore-plan restore-run restore-rollback migration-link migration-run
  bulk-resolve tunnel-plans tunnel-allocate tunnel-plan tunnel-promote-telemetry tunnel-promote-adapter tunnel-apply tunnel-ospf-cost-update tunnel-rollback
  tunnel-status tunnel-probe tunnel-speed-test noise-keygen vty
)

root_help="$("$bin" --help)"
parsed_commands=()
in_commands=0
while IFS= read -r line; do
  if [[ "$line" == "Commands:" ]]; then
    in_commands=1
    continue
  fi
  if [[ "$in_commands" -eq 1 && "$line" == "Options:" ]]; then
    break
  fi
  if [[ "$in_commands" -eq 1 && "$line" =~ ^[[:space:]]{2}([a-z0-9][a-z0-9-]*)[[:space:]] ]]; then
    command="${BASH_REMATCH[1]}"
    [[ "$command" == "help" ]] || parsed_commands+=("$command")
  fi
done <<<"$root_help"

expected_sorted="$(printf '%s\n' "${commands[@]}" | LC_ALL=C sort)"
actual_sorted="$(printf '%s\n' "${parsed_commands[@]}" | LC_ALL=C sort)"
if [[ "$actual_sorted" != "$expected_sorted" ]]; then
  smoke_fail "compiled vpsctl command surface differs from smoke inventory"
fi

workflow_for_command() {
  local command="$1"
  case "$command" in
    health|bootstrap|login|refresh|me|operators|operator-create|operator-sessions|operator-session-revoke|totp-setup|totp-confirm|totp-disable)
      printf 'access_control\n'
      ;;
    agent-identity-upsert|client-key-revocations|client-key-revoke|key-lifecycle-report|gateway-sessions)
      printf 'agent_identity_and_gateway_access\n'
      ;;
    summary|agents|tags|tag-create|agent-tag|bulk-resolve|telemetry-rollups|telemetry-network-rates|telemetry-tunnels)
      printf 'fleet_inventory_and_targeting\n'
      ;;
    fleet-alerts|fleet-alert-export|fleet-alert-states|fleet-alert-state-update|fleet-alert-policies|fleet-alert-policy-upsert|fleet-alert-notification-channels|fleet-alert-notification-channel-upsert|fleet-alert-notifications|fleet-alert-notification-dispatch|fleet-alert-notification-process)
      printf 'fleet_alerting_and_notifications\n'
      ;;
    data-source-presets|data-source-preset-create|data-source-preset-clone|data-source-preset-diff|data-source-preset-test|data-source-preset-update|data-source-status|data-source-assignments|data-source-hot-config|data-source-hot-config-apply|data-source-preset-assign)
      printf 'configuration_sources\n'
      ;;
    jobs|job-create|job-shell|job-targets|job-target-status-download|job-outputs|job-follow|job-output-download|server-jobs|server-job-cancel|artifact-cleanup-preview|artifact-cleanup-create)
      printf 'job_dispatch_and_history\n'
      ;;
    schedules|schedule-create|schedule-update|schedule-enable|schedule-disable|schedule-defer|schedule-apply-now|schedule-delete)
      printf 'schedule_lifecycle\n'
      ;;
    terminal-open|terminal-input|terminal-poll|terminal-resize|terminal-close|terminal-sessions|terminal-replay|terminal-follow)
      printf 'terminal_sessions\n'
      ;;
    file-pull|file-push|file-transfer-upload|file-transfer-download|file-transfers|file-transfer-handoff|file-transfer-sources|file-transfer-source-upload|file-transfer-source-download)
      printf 'file_operations\n'
      ;;
    user-sessions|process-list|process-start|process-stop|process-restart|process-status|process-logs|process-supervisor-inventory)
      printf 'process_and_session_inventory\n'
      ;;
    hot-config|agent-update|agent-update-check|agent-update-activate|agent-update-rollback|agent-update-signature|agent-update-release-publish|agent-update-artifact-upload|agent-update-release-latest|agent-update-releases)
      printf 'runtime_config_and_agent_updates\n'
      ;;
    audit|history-retention|history-retention-upsert|history-retention-prune|history-export)
      printf 'audit_and_retention\n'
      ;;
    backups|backup-artifacts|backup-policies|backup-policy-upsert|backup-policy-prune|restore-plans|migration-links|backup-request|backup-run|backup-artifact-record|backup-artifact-upload|backup-artifact-upload-chunked|backup-artifact-handoff|restore-plan|migration-link|migration-run|restore-run|restore-rollback)
      printf 'backup_restore_and_migration\n'
      ;;
    network-observations|network-trends|network-ospf-recommendations|network-ospf-update-plans|topology-graph|tunnel-plans|tunnel-allocate|tunnel-plan|tunnel-promote-telemetry|tunnel-promote-adapter|tunnel-apply|tunnel-ospf-cost-update|tunnel-rollback|tunnel-status|tunnel-probe|tunnel-speed-test)
      printf 'topology_and_network_operations\n'
      ;;
    noise-keygen|vty)
      printf 'local_operator_utilities\n'
      ;;
    *)
      return 1
      ;;
  esac
}

for expected in "operators" "operator-create" "operator-sessions" "operator-session-revoke" "totp-setup" "totp-confirm" "totp-disable" "agent-identity-upsert" "client-key-revocations" "client-key-revoke" "key-lifecycle-report" "fleet-alerts" "fleet-alert-export" "fleet-alert-states" "fleet-alert-state-update" "fleet-alert-policies" "fleet-alert-policy-upsert" "fleet-alert-notification-channels" "fleet-alert-notification-channel-upsert" "fleet-alert-notifications" "fleet-alert-notification-dispatch" "fleet-alert-notification-process" "gateway-sessions" "telemetry-rollups" "telemetry-network-rates" "telemetry-tunnels" "data-source-presets" "data-source-preset-create" "data-source-preset-clone" "data-source-preset-diff" "data-source-preset-test" "data-source-preset-update" "data-source-status" "data-source-assignments" "data-source-hot-config" "data-source-hot-config-apply" "data-source-preset-assign" "job-shell" "job-follow" "job-target-status-download" "job-output-download" "server-jobs" "server-job-cancel" "artifact-cleanup-preview" "artifact-cleanup-create" "history-retention" "history-retention-upsert" "history-retention-prune" "history-export" "terminal-sessions" "terminal-replay" "terminal-follow" "file-transfers" "file-transfer-handoff" "file-transfer-sources" "file-transfer-source-upload" "file-transfer-source-download" "backup-policies" "backup-policy-upsert" "backup-policy-prune" "backup-artifact-upload-chunked" "backup-artifact-handoff" "tunnel-allocate" "tunnel-speed-test" "tunnel-promote-telemetry" "tunnel-promote-adapter" "tunnel-ospf-cost-update" "restore-run" "restore-rollback" "migration-links" "migration-link" "migration-run" "network-observations" "network-trends" "network-ospf-recommendations" "network-ospf-update-plans" "topology-graph" "agent-update" "agent-update-check" "agent-update-activate" "agent-update-rollback" "agent-update-signature" "agent-update-release-publish" "agent-update-artifact-upload" "agent-update-release-latest" "agent-update-releases" "process-supervisor-inventory"; do
  if [[ "$root_help" != *"$expected"* ]]; then
    smoke_fail "root help is missing expected command: $expected"
  fi
done

file_transfer_upload_help="$("$bin" file-transfer-upload --help)"
[[ "$file_transfer_upload_help" == *"--source-artifact-id"* ]] || smoke_fail "file-transfer-upload help missing --source-artifact-id"

declare -A workflow_counts=()
for command in "${commands[@]}"; do
  "$bin" "$command" --help >/dev/null
  workflow="$(workflow_for_command "$command")" || smoke_fail "command lacks workflow taxonomy: $command"
  workflow_counts["$workflow"]=$(( ${workflow_counts["$workflow"]:-0} + 1 ))
done

printf '{\n'
printf '  "vpsctl_cli_help_smoke": "ok",\n'
printf '  "command_count": %s,\n' "${#commands[@]}"
printf '  "workflow_taxonomy": {\n'
first=1
while IFS= read -r workflow; do
  if [[ "$first" -eq 0 ]]; then
    printf ',\n'
  fi
  printf '    "%s": %s' "$workflow" "${workflow_counts[$workflow]}"
  first=0
done < <(printf '%s\n' "${!workflow_counts[@]}" | LC_ALL=C sort)
printf '\n  },\n'
printf '  "checks": ["root_help", "compiled_inventory_match", "subcommand_help", "workflow_taxonomy", "network_command_parser_shape"]\n'
printf '}\n'
