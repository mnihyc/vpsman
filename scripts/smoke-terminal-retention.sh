#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash cargo rg

cargo test -p vpsman-agent terminal_output_buffer_retains_tail_and_reports_truncation -- --nocapture
cargo test -p vpsman-api repository_terminal_sessions -- --nocapture
cargo test -p vpsctl terminal -- --nocapture

rg -q -- 'output_dropped_bytes == 0' scripts/smoke-postgres-live-job-output.sh
rg -q -- 'output_replay_truncated == false' scripts/smoke-postgres-live-job-output.sh
rg -q -- 'idle_timeout_secs == 300' scripts/smoke-postgres-live-job-output.sh
rg -q -- 'flow_window_bytes == 32768' scripts/smoke-postgres-live-job-output.sh
rg -q -- 'assert_terminal_session_workflow' scripts/smoke-postgres-live-job-output.sh

printf '{\n'
printf '  "terminal_retention_release_gate": "ok",\n'
printf '  "checks": ["agent_flow_window_tail_retention", "api_terminal_retention_read_model", "cli_terminal_contracts", "live_smoke_retention_markers"]\n'
printf '}\n'
