#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

fail() {
  echo "ui/cli/vty parity smoke failed: $*" >&2
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

require_source_token() {
  local token="$1"
  shift
  if ! rg -F -q -- "$token" "$@"; then
    fail "frontend source is missing expected token: $token in $*"
  fi
}

smoke_enter_root
smoke_require_tools bash cargo rg

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

require_contains "$file_transfer_upload_help" "--source-artifact-id" "vpsctl file-transfer-upload source artifact help"
require_contains "$vty_help" "--source-artifact-id" "VTY file-transfer-upload source artifact help"
require_contains "$vty_help" "disable" "VTY privilege disable help"
require_contains "$vty_help" "show privilege" "VTY privilege status help"
require_contains "$vty_help" "show capabilities" "VTY capability display help"
require_contains "$vty_help" "show degraded-policy" "VTY degraded-operation policy help"
require_source_token "Resumable upload source artifact" frontend/src/panels/jobs/JobOperationControls.tsx

workflows=(
  'job dispatch argv|job-create|job-create|mode: "shell"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'job dispatch shell wrapper|job-shell|job-shell|mode: "shell_script"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'terminal session controls|terminal-open|terminal-open|mode: "terminal_session"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'terminal input controls|terminal-input|terminal-input|terminal_input|frontend/src/panels/jobDispatchModel.ts frontend/src/types.ts'
  'terminal poll controls|terminal-poll|terminal-poll|terminal_poll|frontend/src/panels/jobDispatchModel.ts frontend/src/types.ts frontend/src/panels/jobs/TerminalSessionsPanel.tsx'
  'terminal resize controls|terminal-resize|terminal-resize|terminal_resize|frontend/src/panels/jobDispatchModel.ts frontend/src/types.ts'
  'terminal close controls|terminal-close|terminal-close|terminal_close|frontend/src/panels/jobDispatchModel.ts frontend/src/types.ts'
  'terminal session inventory|terminal-sessions|terminal-sessions|Terminal sessions|frontend/src/panels/jobs/TerminalSessionsPanel.tsx'
  'terminal durable replay|terminal-replay|terminal-replay|Durable replay|frontend/src/panels/jobs/TerminalSessionsPanel.tsx'
  'file pull dispatch|file-pull|file-pull|mode: "file_pull"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'file push dispatch|file-push|file-push|mode: "file_push"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'file transfer upload dispatch|file-transfer-upload|file-transfer-upload|Resumable upload|frontend/src/panels/jobs/JobOperationControls.tsx frontend/src/resumableFileTransfer.ts'
  'file transfer download dispatch|file-transfer-download|file-transfer-download|Resumable download|frontend/src/panels/jobs/JobOperationControls.tsx frontend/src/resumableFileTransfer.ts'
  'file transfer session inventory|file-transfers|file-transfers|File transfer sessions|frontend/src/panels/jobs/FileTransferSessionsPanel.tsx'
  'file transfer object handoff|file-transfer-handoff|file-transfer-handoff|Create server-side transfer handoff|frontend/src/panels/jobs/FileTransferSessionsPanel.tsx'
  'file transfer source artifacts|file-transfer-sources|file-transfer-sources|Source artifacts|frontend/src/panels/jobs/FileTransferSessionsPanel.tsx'
  'file transfer source upload|file-transfer-source-upload|file-transfer-source-upload|Upload source artifact|frontend/src/panels/jobs/FileTransferSessionsPanel.tsx'
  'file transfer source download|file-transfer-source-download|file-transfer-source-download|downloadFileTransferSource|frontend/src/hooks/useJobsData.ts frontend/src/panels/jobs/FileTransferSessionsPanel.tsx'
  'user sessions dispatch|user-sessions|user-sessions|mode: "user_sessions"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'operator role records|operators|operators|Operators|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'operator create|operator-create|operator-create|Create operator|frontend/src/panels/AccessPanel.tsx'
  'operator sessions|operator-sessions|operator-sessions|Operator sessions|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'operator session revoke|operator-session-revoke|operator-session-revoke|Revoke|frontend/src/panels/AccessPanel.tsx'
  'operator totp setup|totp-setup|totp-setup|Setup TOTP|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'operator totp confirm|totp-confirm|totp-confirm|confirmTotp|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'operator totp disable|totp-disable|totp-disable|disableTotp|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'direct agent identity import|agent-identity-upsert|agent-identity-upsert|Import identity|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'client key revocations|client-key-revocations|client-key-revocations|Client key revocations|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'client key revoke|client-key-revoke|client-key-revoke|Revoke current key|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'key lifecycle report|key-lifecycle-report|key-lifecycle-report|keyLifecycleReport|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'gateway sessions lifecycle|gateway-sessions|gateway-sessions|Gateway sessions|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'fleet alerts|fleet-alerts|fleet-alerts|Fleet alerts|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert export|fleet-alert-export|fleet-alert-export|include_muted|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert states|fleet-alert-states|fleet-alert-states|fleetAlertStates|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert state update|fleet-alert-state-update|fleet-alert-state-update|updateFleetAlertState|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert policy list|fleet-alert-policies|fleet-alert-policies|Alert policies|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert policy upsert|fleet-alert-policy-upsert|fleet-alert-policy-upsert|upsertFleetAlertPolicy|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification channels|fleet-alert-notification-channels|fleet-alert-notification-channels|Notification channels|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification upsert|fleet-alert-notification-channel-upsert|fleet-alert-notification-channel-upsert|upsertFleetAlertNotificationChannel|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification delivery list|fleet-alert-notifications|fleet-alert-notifications|fleetAlertNotifications|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification dispatch|fleet-alert-notification-dispatch|fleet-alert-notification-dispatch|dispatchFleetAlertNotifications|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification process|fleet-alert-notification-process|fleet-alert-notification-process|processFleetAlertNotifications|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'telemetry rollups|telemetry-rollups|telemetry-rollups|telemetryRollups|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'telemetry network rates|telemetry-network-rates|telemetry-network-rates|telemetryNetworkRates|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'telemetry runtime tunnels|telemetry-tunnels|telemetry-tunnels|telemetryTunnels|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'hot config dispatch|hot-config|hot-config|mode: "hot_config"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'agent update dispatch|agent-update|agent-update|mode: "agent_update"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'agent update activation|agent-update-activate|agent-update-activate|mode: "agent_update_activate"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'agent update rollback|agent-update-rollback|agent-update-rollback|mode: "agent_update_rollback"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'agent update release records|agent-update-releases|agent-update-releases|Agent update releases|frontend/src/panels/jobs/AgentUpdateReleasesPanel.tsx'
  'agent update release latest|agent-update-release-latest|agent-update-release-latest|Agent update releases|frontend/src/panels/jobs/AgentUpdateReleasesPanel.tsx'
  'agent update release publish|agent-update-release-publish|agent-update-release-record|Record signed release|frontend/src/panels/jobs/AgentUpdateReleasesPanel.tsx'
  'agent update artifact upload|agent-update-artifact-upload|agent-update-artifact-upload|Upload|frontend/src/panels/jobs/AgentUpdateReleasesPanel.tsx frontend/src/hooks/useJobsData.ts'
  'process list dispatch|process-list|process-list|mode: "process_list"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process start dispatch|process-start|process-start|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process stop dispatch|process-stop|process-stop|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process restart dispatch|process-restart|process-restart|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process status dispatch|process-status|process-status|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process logs dispatch|process-logs|process-logs|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process supervisor inventory|process-supervisor-inventory|process-supervisor-inventory|Process supervisor inventory|frontend/src/panels/jobs/ProcessSupervisorInventoryPanel.tsx'
  'job targets history|job-targets|job-targets|loadJobTargets|frontend/src/hooks/useJobsData.ts'
  'job outputs history|job-outputs|job-outputs|loadJobOutputs|frontend/src/hooks/useJobsData.ts'
  'job output follow|job-follow|job-follow|loadJobOutputs|frontend/src/hooks/useJobsData.ts'
  'job output artifact download|job-output-artifact|job-output-artifact|onDownloadOutputArtifact|frontend/src/panels/JobHistoryPanel.tsx'
  'schedule create|schedule-create|schedule-create|Create schedule|frontend/src/panels/SchedulesPanel.tsx'
  'tag create|tag-create|tag-create|Create tag|frontend/src/panels/TagsPanel.tsx'
  'agent tag assign|agent-tag|agent-tag|onAssignTag|frontend/src/panels/TagsPanel.tsx'
  'bulk resolve|bulk-resolve|bulk-resolve|onResolveBulk|frontend/src/panels/TagsPanel.tsx'
  'data source preset list|data-source-presets|data-source-presets|Data-source presets|frontend/src/panels/DataSourcePresetPanel.tsx'
  'data source preset create|data-source-preset-create|data-source-preset-create|Preset definition|frontend/src/panels/DataSourcePresetPanel.tsx'
  'data source assignments|data-source-assignments|data-source-assignments|selected preset records|frontend/src/panels/DataSourcePresetPanel.tsx'
  'data source status|data-source-status|data-source-status|Active source status|frontend/src/panels/DataSourcePresetPanel.tsx'
  'data source hot config render|data-source-hot-config|data-source-hot-config|Render selected config|frontend/src/panels/DataSourcePresetPanel.tsx'
  'data source hot config apply|data-source-hot-config-apply|data-source-hot-config-apply|Review apply|frontend/src/panels/DataSourcePresetPanel.tsx'
  'data source preset assign|data-source-preset-assign|data-source-preset-assign|Assign selected preset|frontend/src/panels/DataSourcePresetPanel.tsx'
  'backup policies|backup-policies|backup-policies|Policies|frontend/src/panels/backups/BackupHistoryTables.tsx'
  'backup policy upsert|backup-policy-upsert|backup-policy-upsert|Backup policy|frontend/src/panels/backups/BackupPolicyForm.tsx'
  'backup policy prune|backup-policy-prune|backup-policy-prune|Policy prune|frontend/src/panels/backups/BackupPolicyPruneForm.tsx'
  'backup request|backup-request|backup-request|Review backup|frontend/src/panels/backups/BackupRequestForm.tsx'
  'backup run dispatch|backup-run|backup-run|mode: "backup"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'backup artifact upload|backup-artifact-upload|backup-artifact-upload|Upload artifact|frontend/src/panels/backups/ArtifactUploadForm.tsx'
  'backup artifact chunked upload|backup-artifact-upload-chunked|backup-artifact-upload-chunked|Chunked session|frontend/src/panels/backups/ArtifactUploadForm.tsx'
  'restore plan|restore-plan|restore-plan|Plan restore|frontend/src/panels/backups/RestorePlanForm.tsx'
  'restore run|restore-run|restore-run|Review restore|frontend/src/panels/backups/RestoreRunForm.tsx'
  'restore rollback|restore-rollback|restore-rollback|Review rollback|frontend/src/panels/backups/RestoreRollbackForm.tsx'
  'migration link|migration-link|migration-link|Review link|frontend/src/panels/backups/MigrationLinkForm.tsx'
  'migration run|migration-run|migration-run|Review migration restore|frontend/src/panels/backups/MigrationLinkForm.tsx frontend/src/panels/BackupsPanel.tsx'
  'tunnel plan|tunnel-plan|tunnel-plan|Create tunnel plan|frontend/src/panels/TopologyPanel.tsx'
  'tunnel promote telemetry|tunnel-promote-telemetry|tunnel-promote-telemetry|Observed import|frontend/src/panels/topology/TopologyPromotionPanel.tsx'
  'tunnel promote adapter|tunnel-promote-adapter|tunnel-promote-adapter|Adapter contract|frontend/src/panels/topology/TopologyPromotionPanel.tsx frontend/src/hooks/useTopologyData.ts'
  'tunnel apply|tunnel-apply|tunnel-apply|mode === "apply"|frontend/src/panels/topology/TopologyApplyControls.tsx'
  'tunnel rollback|tunnel-rollback|tunnel-rollback|mode === "rollback"|frontend/src/panels/topology/TopologyApplyControls.tsx'
  'tunnel status|tunnel-status|tunnel-status|mode === "status"|frontend/src/panels/topology/TopologyApplyControls.tsx'
  'tunnel probe|tunnel-probe|tunnel-probe|mode === "probe"|frontend/src/panels/topology/TopologyApplyControls.tsx'
  'tunnel speed test|tunnel-speed-test|tunnel-speed-test|mode === "speed_test"|frontend/src/panels/topology/TopologyApplyControls.tsx'
  'tunnel ospf cost update|tunnel-ospf-cost-update|tunnel-ospf-cost-update|OSPF cost apply|frontend/src/panels/topology/TopologyOspfUpdateControls.tsx'
  'network observations|network-observations|network-observations|observations|frontend/src/panels/topology/TopologyEvidencePanel.tsx frontend/src/hooks/useTopologyData.ts'
  'network trends|network-trends|network-trends|trends|frontend/src/panels/topology/TopologyEvidencePanel.tsx frontend/src/hooks/useTopologyData.ts'
  'network ospf recommendations|network-ospf-recommendations|network-ospf-recommendations|ospfRecommendations|frontend/src/panels/topology/TopologyEvidencePanel.tsx frontend/src/hooks/useTopologyData.ts'
  'network ospf update plans|network-ospf-update-plans|network-ospf-update-plans|ospfUpdatePlans|frontend/src/panels/topology/TopologyEvidencePanel.tsx frontend/src/hooks/useTopologyData.ts'
  'topology graph|topology-graph|topology-graph|Topology graph|frontend/src/panels/topology/TopologyGraphPanel.tsx frontend/src/hooks/useTopologyData.ts'
  'audit log|audit|audit|Audit log|frontend/src/panels/AuditLogPanel.tsx'
  'history retention policies|history-retention|history-retention|History retention|frontend/src/panels/AuditLogPanel.tsx frontend/src/hooks/useAuditData.ts'
  'history retention update|history-retention-upsert|history-retention-upsert|upsertHistoryRetentionPolicy|frontend/src/panels/AuditLogPanel.tsx frontend/src/hooks/useAuditData.ts'
  'history retention prune|history-retention-prune|history-retention-prune|pruneHistoryRetention|frontend/src/panels/AuditLogPanel.tsx frontend/src/hooks/useAuditData.ts'
  'history export|history-export|history-export|historyExport|frontend/src/panels/AuditLogPanel.tsx frontend/src/hooks/useAuditData.ts'
)

workflow_count=0
for workflow in "${workflows[@]}"; do
  IFS='|' read -r name cli_command vty_command frontend_token frontend_paths <<< "$workflow"
  require_contains "$root_help" "$cli_command" "vpsctl root help for $name"
  "$bin" "$cli_command" --help >/dev/null
  require_contains "$vty_help" "$vty_command" "VTY help for $name"
  read -r -a paths <<< "$frontend_paths"
  require_source_token "$frontend_token" "${paths[@]}"
  workflow_count=$((workflow_count + 1))
done

printf '{\n'
printf '  "ui_cli_vty_parity_smoke": "ok",\n'
printf '  "workflow_count": %s,\n' "$workflow_count"
printf '  "checks": ["compiled_cli_help", "compiled_vty_help", "frontend_workflow_tokens"]\n'
printf '}\n'
