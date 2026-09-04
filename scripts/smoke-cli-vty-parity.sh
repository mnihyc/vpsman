#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

fail() {
  echo "cli/vty parity smoke failed: $*" >&2
  exit 1
}

require_contains() {
  local haystack="$1"
  local needle="$2"
  local context="$3"
  if [[ "$haystack" != *"$needle"* ]]; then
    fail "$context is missing expected token: $needle"
  fi
}

require_not_contains() {
  local haystack="$1"
  local needle="$2"
  local context="$3"
  if [[ "$haystack" == *"$needle"* ]]; then
    fail "$context contains unexpected token: $needle"
  fi
}

smoke_enter_root
smoke_require_tools bash cargo

if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsctl >/dev/null
fi

bin="${VPSMAN_VPSCTL_BIN:-target/debug/vpsctl}"
if [[ ! -x "$bin" ]]; then
  fail "vpsctl binary is not executable: $bin"
fi

root_help="$("$bin" --help)"
vty_help="$(printf 'help\nexit\n' | "$bin" --api-url http://127.0.0.1:1 vty)"
file_transfer_upload_help="$("$bin" file-transfer-upload --help)"
backup_policy_upsert_help="$("$bin" backup-policy-upsert --help)"
terminal_open_help="$("$bin" terminal-open --help)"
terminal_input_help="$("$bin" terminal-input --help)"
terminal_poll_help="$("$bin" terminal-poll --help)"
terminal_resize_help="$("$bin" terminal-resize --help)"
terminal_close_help="$("$bin" terminal-close --help)"

require_contains "$file_transfer_upload_help" "--source-artifact-id" "vpsctl file-transfer-upload source artifact help"
require_contains "$vty_help" "--source-artifact-id" "VTY file-transfer-upload source artifact help"
require_contains "$backup_policy_upsert_help" "--schedule-id" "vpsctl backup-policy-upsert update help"
require_contains "$vty_help" "--schedule-id" "VTY backup-policy-upsert update help"
require_contains "$vty_help" "disable" "VTY privilege disable help"
require_contains "$vty_help" "show privilege" "VTY privilege status help"
require_contains "$vty_help" "show capabilities" "VTY capability display help"
require_contains "$vty_help" "show degraded-policy" "VTY degraded-operation policy help"
require_contains "$terminal_open_help" "--password-env" "privileged terminal-open help"
require_not_contains "$terminal_input_help" "--password-env" "authorized terminal-input help"
require_not_contains "$terminal_poll_help" "--password-env" "read-only terminal-poll help"
require_not_contains "$terminal_resize_help" "--password-env" "authorized terminal-resize help"
require_not_contains "$terminal_close_help" "--password-env" "authorized terminal-close help"

workflows=(
  'job dispatch argv|job-create|job-create'
  'job dispatch shell wrapper|job-shell|job-shell'
  'terminal session open job|terminal-open|terminal-open'
  'terminal authorized input control|terminal-input|terminal-input'
  'terminal replay read|terminal-poll|terminal-poll'
  'terminal authorized resize control|terminal-resize|terminal-resize'
  'terminal authorized close control|terminal-close|terminal-close'
  'terminal session inventory|terminal-sessions|terminal-sessions'
  'terminal durable replay|terminal-replay|terminal-replay'
  'file pull dispatch|file-pull|file-pull'
  'file push dispatch|file-push|file-push'
  'file transfer upload dispatch|file-transfer-upload|file-transfer-upload'
  'file transfer download dispatch|file-transfer-download|file-transfer-download'
  'file transfer session inventory|file-transfers|file-transfers'
  'file transfer object handoff|file-transfer-handoff|file-transfer-handoff'
  'file transfer source artifacts|file-transfer-sources|file-transfer-sources'
  'file transfer source upload|file-transfer-source-upload|file-transfer-source-upload'
  'file transfer source download|file-transfer-source-download|file-transfer-source-download'
  'user sessions dispatch|user-sessions|user-sessions'
  'operator role records|operators|operators'
  'operator create|operator-create|operator-create'
  'operator sessions|operator-sessions|operator-sessions'
  'operator session revoke|operator-session-revoke|operator-session-revoke'
  'operator totp setup|totp-setup|totp-setup'
  'operator totp confirm|totp-confirm|totp-confirm'
  'operator totp disable|totp-disable|totp-disable'
  'direct agent identity import|agent-identity-upsert|agent-identity-upsert'
  'client key revocations|client-key-revocations|client-key-revocations'
  'client key revoke|client-key-revoke|client-key-revoke'
  'key lifecycle report|key-lifecycle-report|key-lifecycle-report'
  'gateway sessions lifecycle|gateway-sessions|gateway-sessions'
  'fleet alerts|fleet-alerts|fleet-alerts'
  'fleet alert states|fleet-alert-states|fleet-alert-states'
  'fleet alert state update|fleet-alert-state-update|fleet-alert-state-update'
  'fleet alert policy list|alert-policies|alert-policies'
  'fleet alert policy upsert|alert-policy|alert-policy-upsert'
  'vps rules editor|vps-rules|vps-rules-upsert'
  'fleet alert notification channels|fleet-alert-notification-channels|fleet-alert-notification-channels'
  'fleet alert notification upsert|fleet-alert-notification-channel-upsert|fleet-alert-notification-channel-upsert'
  'fleet alert notification delivery list|fleet-alert-notifications|fleet-alert-notifications'
  'fleet alert notification dispatch|fleet-alert-notification-dispatch|fleet-alert-notification-dispatch'
  'fleet alert notification process|fleet-alert-notification-process|fleet-alert-notification-process'
  'telemetry rollups|telemetry-rollups|telemetry-rollups'
  'telemetry network rates|telemetry-network-rates|telemetry-network-rates'
  'telemetry runtime tunnels|telemetry-tunnels|telemetry-tunnels'
  'agent update dispatch|agent-update|agent-update'
  'agent update activation|agent-update-activate|agent-update-activate'
  'agent update rollback|agent-update-rollback|agent-update-rollback'
  'agent update release records|agent-update-releases|agent-update-releases'
  'agent update release latest|agent-update-release-latest|agent-update-release-latest'
  'agent update release record|agent-update-release-record|agent-update-release-record'
  'host process refresh|host-process-refresh|host-process-refresh'
  'host process snapshot|host-processes|host-processes'
  'process start dispatch|process-start|process-start'
  'process stop dispatch|process-stop|process-stop'
  'process restart dispatch|process-restart|process-restart'
  'process status dispatch|process-status|process-status'
  'process logs dispatch|process-logs|process-logs'
  'process supervisor inventory|process-supervisor-inventory|process-supervisor-inventory'
  'job targets history|job-targets|job-targets'
  'job target status download|job-target-status-download|job-target-status-download'
  'job outputs history|job-outputs|job-outputs'
  'job output follow|job-follow|job-follow'
  'job output download|job-output-download|job-output-download'
  'server jobs inventory|server-jobs|server-jobs'
  'artifact cleanup preview|artifact-cleanup-preview|artifact-cleanup-preview'
  'artifact cleanup create|artifact-cleanup-create|artifact-cleanup-create'
  'server job cancel|server-job-cancel|server-job-cancel'
  'schedule create|schedule-create|schedule-create'
  'tag create|tag-create|tag-create'
  'agent tag assign|agent-tag|agent-tag'
  'bulk resolve|bulk-resolve|bulk-resolve'
  'configuration preset list|config-presets|config-presets'
  'configuration preset create|config-preset-create|config-preset-create'
  'configuration preset clone|config-preset-clone|config-preset-clone'
  'configuration preset preview|config-preset-preview|config-preset-preview'
  'configuration preset update|config-preset-update|config-preset-update'
  'configuration preset delete|config-preset-delete|config-preset-delete'
  'effective configuration sources|config-sources|config-sources'
  'configuration override set|config-source-set|config-source-set'
  'configuration override reset|config-source-reset|config-source-reset'
  'effective configuration render|config-render|config-render'
  'backup policies|backup-policies|backup-policies'
  'backup policy upsert|backup-policy-upsert|backup-policy-upsert'
  'backup policy prune|backup-policy-prune|backup-policy-prune'
  'backup request|backup-request|backup-request'
  'backup run dispatch|backup-run|backup-run'
  'backup artifact upload|backup-artifact-upload|backup-artifact-upload'
  'backup artifact chunked upload|backup-artifact-upload-chunked|backup-artifact-upload-chunked'
  'restore plan|restore-plan|restore-plan'
  'restore run|restore-run|restore-run'
  'restore rollback|restore-rollback|restore-rollback'
  'migration link|migration-link|migration-link'
  'migration run|migration-run|migration-run'
  'port forward registry|port-forwards|port-forwards'
  'port forward create|port-forward-create|port-forward-create'
  'port forward update|port-forward-update|port-forward-update'
  'port forward enable|port-forward-enable|port-forward-enable'
  'port forward disable|port-forward-disable|port-forward-disable'
  'port forward delete|port-forward-delete|port-forward-delete'
  'port forward forget|port-forward-forget|port-forward-forget'
  'port forward reapply|port-forward-reapply|port-forward-reapply'
  'port forward resolve|port-forward-resolve|port-forward-resolve'
  'port forward bulk mutation|port-forward-bulk|port-forward-bulk'
  'tunnel plan|tunnel-plan|tunnel-plan'
  'tunnel plan export|tunnel-plan-export|tunnel-plan-export'
  'tunnel plan enable|tunnel-plan-enable|tunnel-plan-enable'
  'tunnel plan disable|tunnel-plan-disable|tunnel-plan-disable'
  'tunnel plan credential rotation|tunnel-plan-rotate-credentials|tunnel-plan-rotate-credentials'
  'tunnel plan delete|tunnel-plan-delete|tunnel-plan-delete'
  'OSPF updater status refresh|tunnel-ospf-status-refresh|tunnel-ospf-status-refresh'
  'external observed tunnel|tunnel-plan|tunnel-plan'
  'custom tunnel adapter binding|tunnel-plan|tunnel-plan'
  'optional OSPF updater plan override|tunnel-plan|tunnel-plan'
  'tunnel status|tunnel-status|tunnel-status'
  'tunnel probe|tunnel-probe|tunnel-probe'
  'tunnel speed test|tunnel-speed-test|tunnel-speed-test'
  'tunnel ospf cost update|tunnel-ospf-cost-update|tunnel-ospf-cost-update'
  'network observations|network-observations|network-observations'
  'network trends|network-trends|network-trends'
  'network ospf recommendations|network-ospf-recommendations|network-ospf-recommendations'
  'network ospf update plans|network-ospf-update-plans|network-ospf-update-plans'
  'topology graph|topology-graph|topology-graph'
  'audit log|audit|audit'
  'history retention policies|history-retention|history-retention'
  'history retention update|history-retention-upsert|history-retention-upsert'
  'history retention prune|history-retention-prune|history-retention-prune'
  'history export|history-export|history-export'
)

workflow_count=0
for workflow in "${workflows[@]}"; do
  IFS='|' read -r name cli_command vty_command <<< "$workflow"
  require_contains "$root_help" "$cli_command" "vpsctl root help for $name"
  "$bin" "$cli_command" --help >/dev/null
  require_contains "$vty_help" "$vty_command" "VTY help for $name"
  workflow_count=$((workflow_count + 1))
done

printf '{\n'
printf '  "cli_vty_parity_smoke": "ok",\n'
printf '  "workflow_count": %s,\n' "$workflow_count"
printf '  "checks": ["compiled_cli_help", "compiled_vty_help"]\n'
printf '}\n'
