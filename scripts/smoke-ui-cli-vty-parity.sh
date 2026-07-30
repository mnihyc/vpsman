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
backup_policy_upsert_help="$("$bin" backup-policy-upsert --help)"

require_contains "$file_transfer_upload_help" "--source-artifact-id" "vpsctl file-transfer-upload source artifact help"
require_contains "$vty_help" "--source-artifact-id" "VTY file-transfer-upload source artifact help"
require_contains "$backup_policy_upsert_help" "--schedule-id" "vpsctl backup-policy-upsert update help"
require_contains "$vty_help" "--schedule-id" "VTY backup-policy-upsert update help"
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
  'file transfer object handoff|file-transfer-handoff|file-transfer-handoff|Confirm ready download|frontend/src/panels/jobs/FileTransferSessionsPanel.tsx'
  'file transfer source artifacts|file-transfer-sources|file-transfer-sources|Source artifacts|frontend/src/panels/jobs/FileTransferSessionsPanel.tsx'
  'file transfer source upload|file-transfer-source-upload|file-transfer-source-upload|Upload source artifact|frontend/src/panels/jobs/FileTransferSessionsPanel.tsx'
  'file transfer source download|file-transfer-source-download|file-transfer-source-download|downloadFileTransferSource|frontend/src/hooks/useJobsData.ts frontend/src/panels/jobs/FileTransferSessionsPanel.tsx'
  'user sessions dispatch|user-sessions|user-sessions|mode: "user_sessions"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'operator role records|operators|operators|operator records|frontend/src/panels/SystemPanel.tsx frontend/src/hooks/useAccessData.ts'
  'operator create|operator-create|operator-create|Create user|frontend/src/panels/SystemPanel.tsx'
  'operator sessions|operator-sessions|operator-sessions|operator sessions|frontend/src/panels/SystemPanel.tsx frontend/src/hooks/useAccessData.ts'
  'operator session revoke|operator-session-revoke|operator-session-revoke|Confirm admin session revoke|frontend/src/panels/SystemPanel.tsx'
  'operator totp setup|totp-setup|totp-setup|Set up TOTP|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'operator totp confirm|totp-confirm|totp-confirm|confirmTotp|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'operator totp disable|totp-disable|totp-disable|disableTotp|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'direct agent identity import|agent-identity-upsert|agent-identity-upsert|Register VPS|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'client key revocations|client-key-revocations|client-key-revocations|Client key revocations|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'client key revoke|client-key-revoke|client-key-revoke|Revoke current key|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'key lifecycle report|key-lifecycle-report|key-lifecycle-report|keyLifecycleReport|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'gateway sessions lifecycle|gateway-sessions|gateway-sessions|Gateway sessions|frontend/src/panels/AccessPanel.tsx frontend/src/hooks/useAccessData.ts'
  'fleet alerts|fleet-alerts|fleet-alerts|Fleet alerts|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert export|fleet-alert-export|fleet-alert-export|include_muted|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert states|fleet-alert-states|fleet-alert-states|fleetAlertStates|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert state update|fleet-alert-state-update|fleet-alert-state-update|updateFleetAlertState|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert policy list|alert-policies|alert-policies|Alert policies|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert policy upsert|alert-policy|alert-policy-upsert|upsertFleetAlertPolicy|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'vps rules editor|vps-rules|vps-rules-upsert|VPS Rules|frontend/src/panels/ConfigPanel.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification channels|fleet-alert-notification-channels|fleet-alert-notification-channels|Notification channels|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification upsert|fleet-alert-notification-channel-upsert|fleet-alert-notification-channel-upsert|upsertFleetAlertNotificationChannel|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification delivery list|fleet-alert-notifications|fleet-alert-notifications|fleetAlertNotifications|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification dispatch|fleet-alert-notification-dispatch|fleet-alert-notification-dispatch|dispatchFleetAlertNotifications|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'fleet alert notification process|fleet-alert-notification-process|fleet-alert-notification-process|processFleetAlertNotifications|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'telemetry rollups|telemetry-rollups|telemetry-rollups|telemetryRollups|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'telemetry network rates|telemetry-network-rates|telemetry-network-rates|telemetryNetworkRates|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'telemetry runtime tunnels|telemetry-tunnels|telemetry-tunnels|telemetryTunnels|frontend/src/panels/FleetWorkspace.tsx frontend/src/hooks/useFleetData.ts'
  'agent update dispatch|agent-update|agent-update|mode: "agent_update"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'agent update activation|agent-update-activate|agent-update-activate|mode: "agent_update_activate"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'agent update rollback|agent-update-rollback|agent-update-rollback|mode: "agent_update_rollback"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'agent update release records|agent-update-releases|agent-update-releases|Agent update registry|frontend/src/panels/automation/AgentUpdateReleasesPanel.tsx'
  'agent update release latest|agent-update-release-latest|agent-update-release-latest|Agent update registry|frontend/src/panels/automation/AgentUpdateReleasesPanel.tsx'
  'agent update release record|agent-update-release-record|agent-update-release-record|Register release metadata|frontend/src/panels/automation/AgentUpdateReleasesPanel.tsx'
  'host process refresh|host-process-refresh|host-process-refresh|type: "process_list"|frontend/src/panels/jobs/HostProcessInventoryPanel.tsx'
  'host process snapshot|host-processes|host-processes|Host process inventory|frontend/src/panels/jobs/HostProcessInventoryPanel.tsx'
  'process start dispatch|process-start|process-start|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process stop dispatch|process-stop|process-stop|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process restart dispatch|process-restart|process-restart|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process status dispatch|process-status|process-status|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process logs dispatch|process-logs|process-logs|Managed process|frontend/src/panels/jobs/JobOperationControls.tsx'
  'process supervisor inventory|process-supervisor-inventory|process-supervisor-inventory|Process supervisor inventory|frontend/src/panels/jobs/ProcessSupervisorInventoryPanel.tsx'
  'job targets history|job-targets|job-targets|loadJobTargets|frontend/src/hooks/useJobsData.ts'
  'job target status download|job-target-status-download|job-target-status-download|downloadJobTargetStatuses|frontend/src/hooks/useJobsData.ts frontend/src/panels/JobsPanel.tsx'
  'job outputs history|job-outputs|job-outputs|loadJobOutputs|frontend/src/hooks/useJobsData.ts'
  'job output follow|job-follow|job-follow|loadJobOutputs|frontend/src/hooks/useJobsData.ts'
  'job output download|job-output-download|job-output-download|onDownloadOutputChunk|frontend/src/panels/JobsPanel.tsx'
  'server jobs inventory|server-jobs|server-jobs|Maintenance jobs|frontend/src/panels/jobs/ServerJobsPanel.tsx frontend/src/hooks/useJobsData.ts'
  'artifact cleanup preview|artifact-cleanup-preview|artifact-cleanup-preview|Artifact cleanup|frontend/src/panels/jobs/ServerJobsPanel.tsx frontend/src/hooks/useJobsData.ts'
  'artifact cleanup create|artifact-cleanup-create|artifact-cleanup-create|Delete artifacts|frontend/src/panels/jobs/ServerJobsPanel.tsx frontend/src/hooks/useJobsData.ts'
  'server job cancel|server-job-cancel|server-job-cancel|cancelServerJob|frontend/src/hooks/useJobsData.ts frontend/src/panels/jobs/ServerJobsPanel.tsx'
  'schedule create|schedule-create|schedule-create|Create schedule|frontend/src/panels/SchedulesPanel.tsx'
  'tag create|tag-create|tag-create|Create group|frontend/src/panels/FleetGroupsPanel.tsx'
  'agent tag assign|agent-tag|agent-tag|onAssignTag|frontend/src/panels/FleetGroupsPanel.tsx'
  'bulk resolve|bulk-resolve|bulk-resolve|onResolveBulk|frontend/src/panels/FleetGroupsPanel.tsx'
  'configuration preset list|config-presets|config-presets|Configuration presets|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'configuration preset create|config-preset-create|config-preset-create|New preset|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'configuration preset clone|config-preset-clone|config-preset-clone|Clone to customize|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'configuration preset preview|config-preset-preview|config-preset-preview|Review change|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'configuration preset update|config-preset-update|config-preset-update|Edit|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'configuration preset delete|config-preset-delete|config-preset-delete|Delete custom preset|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'effective configuration sources|config-sources|config-sources|Effective configuration|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'configuration override set|config-source-set|config-source-set|Apply configuration|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'configuration override reset|config-source-reset|config-source-reset|Reset to system default|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'effective configuration render|config-render|config-render|Inspect full effective config|frontend/src/panels/ConfigurationSourcesPanel.tsx'
  'backup policies|backup-policies|backup-policies|Policies|frontend/src/panels/backups/BackupHistoryTables.tsx'
  'backup policy upsert|backup-policy-upsert|backup-policy-upsert|Backup policy|frontend/src/panels/backups/BackupPolicyForm.tsx'
  'backup policy prune|backup-policy-prune|backup-policy-prune|Policy prune|frontend/src/panels/backups/BackupPolicyPruneForm.tsx'
  'backup request|backup-request|backup-request|Review backup|frontend/src/panels/backups/BackupRequestForm.tsx'
  'backup run dispatch|backup-run|backup-run|mode: "backup"|frontend/src/panels/jobs/JobOperationControls.tsx'
  'backup artifact upload|backup-artifact-upload|backup-artifact-upload|Upload artifact|frontend/src/panels/backups/ArtifactUploadForm.tsx'
  'backup artifact chunked upload|backup-artifact-upload-chunked|backup-artifact-upload-chunked|Chunked session|frontend/src/panels/backups/ArtifactUploadForm.tsx'
  'restore plan|restore-plan|restore-plan|Review draft restore|frontend/src/panels/backups/RestorePlanForm.tsx'
  'restore run|restore-run|restore-run|Review dry run|frontend/src/panels/backups/RestoreRunForm.tsx'
  'restore rollback|restore-rollback|restore-rollback|Review rollback|frontend/src/panels/backups/RestoreRollbackForm.tsx'
  'migration link|migration-link|migration-link|Review mapping|frontend/src/panels/backups/MigrationLinkForm.tsx'
  'migration run|migration-run|migration-run|Review cutover restore|frontend/src/panels/backups/MigrationLinkForm.tsx frontend/src/panels/BackupsPanel.tsx'
  'port forward registry|port-forwards|port-forwards|Port forwarding|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'port forward create|port-forward-create|port-forward-create|Create rule|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'port forward update|port-forward-update|port-forward-update|Save changes|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'port forward enable|port-forward-enable|port-forward-enable|Enable rule|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'port forward disable|port-forward-disable|port-forward-disable|Disable rule|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'port forward delete|port-forward-delete|port-forward-delete|Delete rule|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'port forward forget|port-forward-forget|port-forward-forget|Forget only when|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'port forward reapply|port-forward-reapply|port-forward-reapply|Reapply this VPS|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'port forward resolve|port-forward-resolve|port-forward-resolve|Resolve on the control plane|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'port forward bulk mutation|port-forward-bulk|port-forward-bulk|selectedEnableRules.length|frontend/src/panels/topology/PortForwardingPanel.tsx'
  'tunnel plan|tunnel-plan|tunnel-plan|Create tunnel plan|frontend/src/panels/TopologyPanel.tsx'
  'tunnel plan export|tunnel-plan-export|tunnel-plan-export|Export plan JSON|frontend/src/panels/TopologyPanel.tsx frontend/src/hooks/useTopologyData.ts'
  'tunnel plan enable|tunnel-plan-enable|tunnel-plan-enable|Enable plans|frontend/src/panels/TopologyPanel.tsx frontend/src/hooks/useTopologyData.ts'
  'tunnel plan disable|tunnel-plan-disable|tunnel-plan-disable|Disable plans|frontend/src/panels/TopologyPanel.tsx frontend/src/hooks/useTopologyData.ts'
  'tunnel plan delete|tunnel-plan-delete|tunnel-plan-delete|Delete plan|frontend/src/panels/TopologyPanel.tsx frontend/src/hooks/useTopologyData.ts'
  'OSPF updater status refresh|tunnel-ospf-status-refresh|tunnel-ospf-status-refresh|Check OSPF updater status|frontend/src/panels/topology/TopologyOspfUpdateControls.tsx frontend/src/hooks/useTopologyData.ts'
  'external observed tunnel|tunnel-plan|tunnel-plan|External observed|frontend/src/panels/TopologyPanel.tsx'
  'external managed tunnel adapter binding|tunnel-plan|tunnel-plan|External adapter|frontend/src/panels/TopologyPanel.tsx'
  'optional OSPF updater plan override|tunnel-plan|tunnel-plan|OSPF command override (optional)|frontend/src/panels/TopologyPanel.tsx'
  'tunnel status|tunnel-status|tunnel-status|mode === "status"|frontend/src/panels/topology/TopologyNetworkTestControls.tsx'
  'tunnel probe|tunnel-probe|tunnel-probe|mode === "probe"|frontend/src/panels/topology/TopologyNetworkTestControls.tsx'
  'tunnel speed test|tunnel-speed-test|tunnel-speed-test|mode === "speed_test"|frontend/src/panels/topology/TopologyNetworkTestControls.tsx'
  'tunnel ospf cost update|tunnel-ospf-cost-update|tunnel-ospf-cost-update|Confirm OSPF cost update|frontend/src/panels/topology/TopologyOspfUpdateControls.tsx'
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
