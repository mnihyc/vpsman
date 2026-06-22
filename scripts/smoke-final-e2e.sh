#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash rg

log_dir="${VPSMAN_FINAL_E2E_LOG_DIR:-}"
if [[ -z "$log_dir" ]]; then
  smoke_fail "VPSMAN_FINAL_E2E_LOG_DIR is required; run this after aggregate release smokes"
fi
if [[ ! -d "$log_dir" ]]; then
  smoke_fail "final E2E log directory does not exist: $log_dir"
fi

require_log_marker() {
  local file="$1"
  local marker="$2"
  local path="$log_dir/$file"
  [[ -f "$path" ]] || smoke_fail "final E2E missing log file: $file"
  rg -q -- "$marker" "$path" || smoke_fail "final E2E missing marker '$marker' in $file"
}

require_log_marker smoke-vpsctl-live-api.log '"tag_bulk"'
require_log_marker smoke-vpsctl-live-api.log '"backup_request_restore_plan"'
require_log_marker smoke-vpsctl-live-api.log '"source_template_workflow_readiness"'
require_log_marker smoke-vpsctl-live-api.log '"source_template_process_limit_readiness"'
require_log_marker smoke-vpsctl-live-api.log '"no_plaintext_password_in_cli_outputs"'
require_log_marker smoke-postgres-persistence.log '"direct_identity_key_rotation"'
require_log_marker smoke-postgres-persistence.log '"client_key_revocation"'
require_log_marker smoke-postgres-persistence.log '"backup_restore_metadata"'
require_log_marker smoke-postgres-persistence.log '"backup_policy_retention_prune"'
require_log_marker smoke-postgres-live-job-output.log '"terminal_session_lifecycle"'
require_log_marker smoke-postgres-live-job-output.log '"resumable_file_transfer_upload"'
require_log_marker smoke-postgres-live-job-output.log '"resumable_file_transfer_download"'
require_log_marker smoke-postgres-live-job-output.log '"network_probe_observation"'
require_log_marker smoke-live-backup.log '"restore_run"'
require_log_marker smoke-live-backup.log '"restore_rollback"'
require_log_marker smoke-live-backup.log '"vty_restore_run"'
require_log_marker smoke-live-backup.log '"migration_run_restore"'
require_log_marker smoke-backup-chunked-upload.log '"backup_chunked_upload_smoke": "ok"'
require_log_marker smoke-live-agent-update.log '"direct_agent_update_job_flow": "stage_activate_restart_rollback"'
require_log_marker smoke-live-network-apply.log '"adapter_rollback_job_id"'
require_log_marker smoke-live-source template-config-patch.log '"no_privilege_unlock_rejected": true'
require_log_marker smoke-live-source template-config-patch.log '"command_execution_policy_fields": "verified"'
require_log_marker smoke-live-hot-config.log '"api_restart": "verified"'
require_log_marker smoke-agent-endpoint-failover.log '"dead_tcp_failover"'
require_log_marker smoke-agent-resource-budget.log '"cpu_limit_percent"'
require_log_marker smoke-agent-resource-budget.log '"post_telemetry_rss_budget"'
require_log_marker smoke-agent-reconnect-churn.log '"dropped_connections"'
require_log_marker smoke-ui-cli-vty-parity.log '"ui_cli_vty_parity_smoke": "ok"'
require_log_marker smoke-vpsctl-structured-output.log '"vpsctl_structured_output_smoke": "ok"'
require_log_marker terminal-retention-release-gate.log '"terminal_retention_release_gate": "ok"'
require_log_marker smoke-docker-50-agent-long-running-fleet.log '"agent_count": 50'
require_log_marker smoke-docker-50-agent-long-running-fleet.log '"system_dashboard_queue_pool_cancel_gateway_counters"'
require_log_marker smoke-docker-50-agent-long-running-fleet.log '"long_running_bulk_job_with_api_backlog"'
require_log_marker check-frontend-contracts.log '"frontend_protocol_contracts":"ok"'

printf '{\n'
printf '  "final_e2e_smoke": "ok",\n'
printf '  "log_dir": "%s",\n' "$log_dir"
printf '  "checks": ["auth_and_no_plaintext_cli", "direct_identity_persistence", "client_key_revocation", "terminal_sessions", "terminal_retention_flow_window", "resumable_transfers", "backup_restore_rollback", "backup_policy_retention_prune", "backup_chunked_upload", "migration_run_restore", "agent_update_direct_jobs", "runtime_network_adapter", "source_template_workflow_readiness", "source_template_process_limit_readiness", "source_config_patch", "command_execution_policy_fields", "static_endpoint_failover", "low_resource_reconnect", "low_resource_post_telemetry_budget", "ui_cli_vty_parity", "structured_cli_output", "system_dashboard_counters", "long_running_20_plus_fleet_with_api_backlog", "generated_frontend_contracts"]\n'
printf '}\n'
