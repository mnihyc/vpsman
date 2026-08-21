#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REVIEW_HARNESS="$ROOT_DIR/scripts/review-monitoring-real-data.sh"
PRESSURE_FIXTURE="$ROOT_DIR/scripts/fixtures/review-monitoring-pressure-128.sql"
RETAINED_FIXTURE="$ROOT_DIR/scripts/fixtures/prove-monitoring-five-year-retained.sql"
RETAINED_REPORT_SQL="$ROOT_DIR/scripts/fixtures/prove-monitoring-five-year-retained-report.sql"
RETAINED_SEMANTIC_SQL="$ROOT_DIR/scripts/fixtures/prove-monitoring-five-year-semantic-hashes.sql"
REVIEW_STATE="$ROOT_DIR/.tmp/monitoring-real-data/current/run.json"
STORAGE_ROOT="${VPSMAN_VNSTAT_PRESSURE_STORAGE_ROOT:-/mnt/storage/vpsman-tmp/vpsman-vnstat-browser-pressure}"
CARGO_TARGET_STORAGE="${VPSMAN_VNSTAT_PRESSURE_CARGO_TARGET_DIR:-/mnt/storage/vpsman-tmp/vpsman-vnstat-browser-pressure-cargo-target}"
PWCLI="${CODEX_HOME:-$HOME/.codex}/skills/playwright/scripts/playwright_cli.sh"
IMPORT_TEST="postgres_benchmark_120_vps_five_year_vnstat_import_is_exact_and_bounded"
EXACT_ONE_REIMPORT_TEST="postgres_exact_one_client_five_year_vnstat_reimport_is_atomic_and_spill_free"
EXACT_FOUR_REIMPORT_TEST="postgres_exact_four_concurrent_five_year_vnstat_reimport_is_spill_free"
MAINTENANCE_TEST="postgres_pressure_retained_history_worker_is_conservative_and_idempotent"
IMPORT_PHASE_WALL_TIMEOUT_SECS=2400
# Performance acceptance is deliberately separate from the kill timeout.  The
# an earlier valid 120-client run completed in 252.434s; 600s preserves
# substantial host variance while rejecting a multi-fold regression.  The 11,000 raw-row/s
# and 0.23 client/s floors remain independent, stronger throughput gates for
# the expected fixture size.
IMPORT_PHASE_MAX_WALL_SECS=600
REIMPORT_PHASE_MAX_WALL_SECS=600
IMPORT_PHASE_MAX_ELAPSED_MS=$((IMPORT_PHASE_MAX_WALL_SECS * 1000))
REIMPORT_PHASE_MAX_ELAPSED_MS=$((REIMPORT_PHASE_MAX_WALL_SECS * 1000))
MIN_IMPORT_ROWS_PER_SECOND=11000
MIN_REIMPORT_ROWS_PER_SECOND=11000
MIN_IMPORT_CLIENTS_PER_SECOND=0.23
MIN_REIMPORT_CLIENTS_PER_SECOND=0.23
EXACT_REIMPORT_PHASE_WALL_TIMEOUT_SECS=600
RETAINED_FIXTURE_WALL_TIMEOUT_SECS=3600
MAINTENANCE_PHASE_WALL_TIMEOUT_SECS=1800
RETAINED_REPORT_STATEMENT_TIMEOUT_MS=300000
RETAINED_REPORT_PGOPTIONS="-c statement_timeout=$RETAINED_REPORT_STATEMENT_TIMEOUT_MS -c work_mem=256MB -c max_parallel_workers_per_gather=0"
# The semantic hash query contains thirteen independent, ordered hash InitPlans
# over the five-year fixture. Keep its bound/settings separate from the small
# shape report: parallel scans and a larger sort budget reduce false post-
# measurement failures without changing any measured import or browser gate.
# JIT compilation is disabled for this JSONB/md5-heavy validation query; it
# spends more time compiling the many ordered aggregate expressions than it
# saves on the bounded fixture.
RETAINED_SEMANTIC_STATEMENT_TIMEOUT_MS=300000
RETAINED_SEMANTIC_PGOPTIONS="-c statement_timeout=$RETAINED_SEMANTIC_STATEMENT_TIMEOUT_MS -c work_mem=512MB -c max_parallel_workers_per_gather=4 -c jit=off"
PLAYWRIGHT_COMMAND_WALL_TIMEOUT_SECS=60
BROWSER_PHASE_WALL_TIMEOUT_SECS=190
BROWSER_FETCH_WALL_TIMEOUT_MS=10000
MINIMUM_STORAGE_FREE_BYTES=51539607552
FOCUSED_EVIDENCE_MANIFEST="${VPSMAN_VNSTAT_PRESSURE_FOCUSED_EVIDENCE_MANIFEST:-}"

artifact_dir=""
container_name=""
run_id=""
stack_started=0
active_sampler_pid="0"
active_sampler_stop=""
declare -a active_browser_sessions=()
declare -a active_process_groups=()
managed_process_group_pid="0"
focused_evidence_mode="executed"

die() {
  printf 'vnStat/browser pressure proof: %s\n' "$*" >&2
  exit 1
}

verify_reused_focused_evidence() {
  local manifest="$1"
  local resolved_manifest
  local expected_importer_sha256
  local expected_migration_sha256
  local expected_reliability_sha256
  local current_sha256
  local phase
  local phase_key
  local phase_dir_rel
  local phase_dir
  local test_name
  local expected_log_sha256
  local expected_postgres_log_sha256
  local actual_sha256

  resolved_manifest="$(readlink -m "$manifest")"
  [[ "$resolved_manifest" == "$ROOT_DIR/output/playwright/"* ]] \
    || die "focused evidence manifest must be inside output/playwright"
  [[ -f "$resolved_manifest" ]] \
    || die "focused evidence manifest is missing: $resolved_manifest"
  jq -e '.schema == "vpsman-focused-reimport-evidence/v1"' "$resolved_manifest" \
    >/dev/null \
    || die "focused evidence manifest schema is invalid"

  expected_importer_sha256="$(jq -er '.source.importer_sha256' "$resolved_manifest")"
  expected_migration_sha256="$(jq -er '.source.migration_0019_sha256' "$resolved_manifest")"
  expected_reliability_sha256="$(jq -er '.source.reliability_sha256' "$resolved_manifest")"
  current_sha256="$(sha256sum "$ROOT_DIR/crates/api/src/repository/network/repository_network_traffic_import.rs" | awk '{print $1}')"
  [[ "$current_sha256" == "$expected_importer_sha256" ]] \
    || die "focused evidence importer source hash does not match current source"
  current_sha256="$(sha256sum "$ROOT_DIR/migrations/0019_traffic_import_same_shape_update.sql" | awk '{print $1}')"
  [[ "$current_sha256" == "$expected_migration_sha256" ]] \
    || die "focused evidence migration 0019 hash does not match current source"
  current_sha256="$(sha256sum "$ROOT_DIR/crates/api/src/repository/core/tests_postgres_reliability.rs" | awk '{print $1}')"
  [[ "$current_sha256" == "$expected_reliability_sha256" ]] \
    || die "focused evidence reliability-test source hash does not match current source"

  for phase in one_client four_client; do
    phase_key="$phase"
    phase_dir_rel="$(jq -er ".${phase_key}.directory" "$resolved_manifest")"
    phase_dir="$(readlink -m "$ROOT_DIR/$phase_dir_rel")"
    [[ "$phase_dir" == "$ROOT_DIR/output/playwright/"* ]] \
      || die "focused evidence directory escapes output/playwright: $phase_dir_rel"
    [[ -d "$phase_dir" ]] || die "focused evidence directory is missing: $phase_dir"
    test_name="$(jq -er ".${phase_key}.test" "$resolved_manifest")"
    expected_log_sha256="$(jq -er ".${phase_key}.log_sha256" "$resolved_manifest")"
    expected_postgres_log_sha256="$(jq -er ".${phase_key}.postgres_log_sha256" "$resolved_manifest")"
    [[ -f "$phase_dir/${phase//_/-}.log" ]] \
      || die "focused evidence test log is missing for $phase"
    [[ -f "$phase_dir/postgres.log" ]] \
      || die "focused evidence PostgreSQL log is missing for $phase"
    actual_sha256="$(sha256sum "$phase_dir/${phase//_/-}.log" | awk '{print $1}')"
    [[ "$actual_sha256" == "$expected_log_sha256" ]] \
      || die "focused evidence test log hash changed for $phase"
    actual_sha256="$(sha256sum "$phase_dir/postgres.log" | awk '{print $1}')"
    [[ "$actual_sha256" == "$expected_postgres_log_sha256" ]] \
      || die "focused evidence PostgreSQL log hash changed for $phase"
    grep -Fqx "running 1 test" <(grep -F "running 1 test" "$phase_dir/${phase//_/-}.log") \
      || die "focused evidence did not run exactly one test for $phase"
    grep -F "test $test_name ... ok" "$phase_dir/${phase//_/-}.log" >/dev/null \
      || die "focused evidence test did not pass for $phase"
    grep -F "1 passed; 0 failed" "$phase_dir/${phase//_/-}.log" >/dev/null \
      || die "focused evidence result summary is not a single pass for $phase"
    ! grep -Eq '^skipping exact reimport probe:|^test .* \.\.\. FAILED$|^test result: FAILED' \
      "$phase_dir/${phase//_/-}.log" \
      || die "focused evidence contains a skip/failure for $phase"
    ! grep -Fqi 'temporary file' "$phase_dir/postgres.log" \
      || die "focused evidence PostgreSQL log contains a temporary-file event for $phase"
  done
  focused_evidence_mode="reused_verified"
}

write_reused_probe_summary() {
  local manifest="$1"
  local phase_key="$2"
  local output_name="$3"
  jq -n \
    --arg schema "vpsman-vnstat-exact-reimport-probe/v1" \
    --arg phase "$phase_key" \
    --arg source_manifest "${manifest#"$ROOT_DIR/"}" \
    --arg test "$(jq -er ".${phase_key}.test" "$manifest")" \
    --arg log_sha256 "$(jq -er ".${phase_key}.log_sha256" "$manifest")" \
    --arg postgres_log_sha256 "$(jq -er ".${phase_key}.postgres_log_sha256" "$manifest")" \
    '{schema:$schema,status:"passed",phase:$phase,evidence_reused:true,source_manifest:$source_manifest,test:$test,log_sha256:$log_sha256,postgres_log_sha256:$postgres_log_sha256}' \
    >"$artifact_dir/$output_name"
}

require_tools() {
  local tool
  for tool in "$@"; do
    command -v "$tool" >/dev/null 2>&1 || die "missing required tool: $tool"
  done
}

process_group_alive() {
  local pid="$1"
  kill -0 -- "-$pid" >/dev/null 2>&1
}

forget_process_group() {
  local forgotten_pid="$1"
  local pid
  local -a retained=()
  for pid in "${active_process_groups[@]}"; do
    [[ "$pid" == "$forgotten_pid" ]] || retained+=("$pid")
  done
  active_process_groups=("${retained[@]}")
}

terminate_process_group() {
  local pid="$1"
  local attempt
  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 0
  process_group_alive "$pid" || return 0
  kill -TERM -- "-$pid" >/dev/null 2>&1 || true
  for ((attempt = 0; attempt < 50; attempt += 1)); do
    process_group_alive "$pid" || return 0
    sleep 0.1
  done
  kill -KILL -- "-$pid" >/dev/null 2>&1 || true
}

start_managed_process_group() {
  local output="$1"
  shift
  local pid
  local pgid=""
  local attempt
  setsid "$@" >"$output" 2>&1 &
  pid="$!"
  for ((attempt = 0; attempt < 20; attempt += 1)); do
    kill -0 "$pid" >/dev/null 2>&1 || break
    pgid="$(ps -o pgid= -p "$pid" 2>/dev/null | tr -d '[:space:]' || true)"
    [[ -n "$pgid" ]] && break
    sleep 0.01
  done
  if kill -0 "$pid" >/dev/null 2>&1 && [[ "$pgid" != "$pid" ]]; then
    kill -TERM "$pid" >/dev/null 2>&1 || true
    wait "$pid" >/dev/null 2>&1 || true
    die "managed command did not start in its own process group"
  fi
  active_process_groups+=("$pid")
  managed_process_group_pid="$pid"
}

wait_managed_process_group_until() {
  local pid="$1"
  local deadline="$2"
  local label="$3"
  local status
  while kill -0 "$pid" >/dev/null 2>&1; do
    if ((SECONDS >= deadline)); then
      terminate_process_group "$pid"
      wait "$pid" >/dev/null 2>&1 || true
      forget_process_group "$pid"
      printf '%s exceeded its hard wall deadline\n' "$label" >&2
      return 124
    fi
    sleep 0.2
  done
  set +e
  wait "$pid"
  status="$?"
  set -e
  if process_group_alive "$pid"; then
    terminate_process_group "$pid"
    status=125
  fi
  forget_process_group "$pid"
  return "$status"
}

run_managed_process_group() {
  local timeout_secs="$1"
  local label="$2"
  local output="$3"
  shift 3
  start_managed_process_group "$output" "$@"
  local pid="$managed_process_group_pid"
  if ! wait_managed_process_group_until "$pid" "$((SECONDS + timeout_secs))" "$label"; then
    die "$label failed or exceeded its ${timeout_secs}s hard wall deadline"
  fi
}

close_browser_session_best_effort() {
  local session="$1"
  local pid
  local deadline
  setsid env PLAYWRIGHT_CLI_SESSION="$session" \
    bash "$PWCLI" close >/dev/null 2>&1 &
  pid="$!"
  deadline=$((SECONDS + 15))
  while process_group_alive "$pid" && ((SECONDS < deadline)); do
    sleep 0.2
  done
  process_group_alive "$pid" && terminate_process_group "$pid"
  wait "$pid" >/dev/null 2>&1 || true
}

current_worktree_hash() {
  (
    cd "$ROOT_DIR"
    {
      git diff --binary HEAD
      git status --porcelain=v1
      git ls-files --others --exclude-standard -z \
        | sort -z \
        | xargs -0 -r sha256sum
    } | sha256sum | awk '{print $1}'
  )
}

validate_storage_root() {
  STORAGE_ROOT="$(readlink -m "$STORAGE_ROOT")"
  [[ "$STORAGE_ROOT" == /mnt/storage/vpsman-tmp/* ]] \
    || die "storage root must be below /mnt/storage/vpsman-tmp"
  CARGO_TARGET_STORAGE="$(readlink -m "$CARGO_TARGET_STORAGE")"
  [[ "$CARGO_TARGET_STORAGE" == /mnt/storage/vpsman-tmp/* ]] \
    || die "Cargo target must be below /mnt/storage/vpsman-tmp"
}

psql_proof() {
  PGAPPNAME="${PGAPPNAME:-vpsman-pressure-control}" \
    PGPASSWORD="$postgres_password" psql \
    -X \
    -v ON_ERROR_STOP=1 \
    -h 127.0.0.1 \
    -p "$postgres_port" \
    -U vpsman \
    -d vpsman \
    "$@"
}

stop_stack() {
  if [[ "$stack_started" == "1" && -f "$REVIEW_STATE" ]]; then
    local stop_output="/dev/null"
    if [[ -n "$artifact_dir" && -d "$artifact_dir" ]]; then
      stop_output="$artifact_dir/stack-stop.json"
    fi
    "$REVIEW_HARNESS" stop >"$stop_output" 2>&1 || true
    stack_started=0
  fi
}

capture_failure_evidence() {
  [[ -n "$artifact_dir" && -d "$artifact_dir" ]] || return 0
  local failure_dir="$artifact_dir/failure-evidence"
  mkdir -p "$failure_dir"
  if [[ -n "$container_name" ]] \
    && docker inspect "$container_name" >/dev/null 2>&1 \
    && [[ "$(docker inspect --format '{{ index .Config.Labels "com.vpsman.monitoring-review-run" }}' "$container_name" 2>/dev/null || true)" == "$run_id" ]]; then
    docker logs --timestamps "$container_name" \
      >"$failure_dir/postgres-full.log" 2>&1 || true
    { rg 'temporary file:' "$failure_dir/postgres-full.log" || true; } \
      >"$failure_dir/postgres-temp-files.log"
  fi
  if [[ -n "${postgres_port:-}" && -n "${postgres_password:-}" ]]; then
    psql_proof -qAt -c \
      "SELECT json_build_object('captured_at', clock_timestamp(), 'database', current_database(), 'database_stats', (SELECT row_to_json(stats) FROM (SELECT xact_commit, xact_rollback, temp_files, temp_bytes, deadlocks, checksum_failures, stats_reset FROM pg_stat_database WHERE datname = current_database()) stats), 'activity', (SELECT coalesce(json_agg(activity), '[]'::json) FROM (SELECT pid, leader_pid, application_name, backend_type, state, wait_event_type, wait_event, xact_start, query_start, query_id, left(query, 1024) AS query FROM pg_stat_activity WHERE datname = current_database() ORDER BY pid) activity))" \
      >"$failure_dir/postgres-state.json" 2>&1 || true
    psql_proof -qAt -c \
      "SELECT coalesce(json_agg(statements), '[]'::json) FROM (SELECT userid, dbid, queryid, calls, rows, total_plan_time, total_exec_time, max_exec_time, shared_blks_read, shared_blks_hit, temp_blks_read, temp_blks_written, wal_records, wal_fpi, wal_bytes, left(regexp_replace(query, '[[:space:]]+', ' ', 'g'), 2048) AS query FROM pg_stat_statements WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database()) ORDER BY temp_blks_written DESC, total_exec_time DESC LIMIT 200) statements" \
      >"$failure_dir/pg-stat-statements.json" 2>&1 || true
    psql_proof -qAt -c \
      "SELECT coalesce(json_agg(tables), '[]'::json) FROM (SELECT relname, n_live_tup, n_dead_tup, last_vacuum, last_autovacuum, last_analyze, last_autoanalyze, vacuum_count, autovacuum_count, analyze_count, autoanalyze_count FROM pg_stat_user_tables ORDER BY n_dead_tup DESC, relname) tables" \
      >"$failure_dir/table-maintenance.json" 2>&1 || true
    psql_proof -qAt -c \
      "SELECT json_object_agg(name, setting ORDER BY name) FROM pg_settings WHERE name = ANY(ARRAY['work_mem','hash_mem_multiplier','temp_file_limit','log_temp_files','log_autovacuum_min_duration','log_line_prefix','log_min_duration_statement','idle_in_transaction_session_timeout','pg_stat_statements.track','max_parallel_workers_per_gather'])" \
      >"$failure_dir/postgres-settings.json" 2>&1 || true
  fi
  if [[ -f "$ROOT_DIR/.tmp/monitoring-real-data/current/api.log" ]]; then
    tail -n 1000 "$ROOT_DIR/.tmp/monitoring-real-data/current/api.log" \
      >"$failure_dir/api-tail.log" 2>&1 || true
  fi
  current_worktree_hash >"$failure_dir/worktree-sha256.txt" 2>&1 || true
}

strict_stop_stack() {
  [[ "$stack_started" == "1" ]] || die "success cleanup requires the live benchmark stack"
  [[ -f "$REVIEW_STATE" ]] || die "success cleanup requires the recorded benchmark state"
  "$REVIEW_HARNESS" stop >"$artifact_dir/stack-stop.json" 2>&1
  jq -e \
    --arg run_id "$run_id" \
    --arg postgres_data "$postgres_data_dir" \
    '.status == "stopped" and .run_id == $run_id and .retained_postgres_data == $postgres_data' \
    "$artifact_dir/stack-stop.json" >/dev/null \
    || die "review harness did not confirm the exact stopped stack"
  [[ ! -e "$REVIEW_STATE" ]] || die "review state, including ephemeral credentials, survived stop"
  if docker inspect "$container_name" >/dev/null 2>&1; then
    die "PostgreSQL container survived the verified stop"
  fi
  if { [[ "$api_pid" =~ ^[1-9][0-9]*$ ]] && kill -0 "$api_pid" >/dev/null 2>&1; } \
    || { [[ "$frontend_pid" =~ ^[1-9][0-9]*$ ]] && kill -0 "$frontend_pid" >/dev/null 2>&1; }; then
    die "an API or frontend PID survived the verified stop"
  fi
  stack_started=0
}

assert_no_container_mounts_postgres_data() {
  local postgres_path="$1"
  local candidate
  local source
  while IFS= read -r candidate; do
    [[ -n "$candidate" ]] || continue
    while IFS= read -r source; do
      if [[ "$source" == "$postgres_path" \
        || "$source" == "$postgres_path"/* \
        || "$source" == "/" \
        || "$postgres_path" == "$source"/* ]]; then
        die "cleanup refuses PostgreSQL data still mounted by container $candidate"
      fi
    done < <(docker inspect --format '{{range .Mounts}}{{println .Source}}{{end}}' "$candidate")
  done < <(docker ps -aq)
}

on_exit() {
  local status="$?"
  trap - EXIT
  set +e
  local pid
  for pid in "${active_process_groups[@]}"; do
    terminate_process_group "$pid"
    wait "$pid" >/dev/null 2>&1 || true
  done
  active_process_groups=()
  if [[ "$active_sampler_pid" =~ ^[1-9][0-9]*$ ]]; then
    [[ -n "$active_sampler_stop" ]] && touch "$active_sampler_stop"
    wait "$active_sampler_pid" >/dev/null 2>&1 || true
  fi
  local session
  for session in "${active_browser_sessions[@]}"; do
    close_browser_session_best_effort "$session"
  done
  if ((status != 0)); then
    capture_failure_evidence
  fi
  stop_stack
  exit "$status"
}

resolve_container_cpu_stat() {
  local container_pid
  local cgroup_relative
  container_pid="$(docker inspect --format '{{.State.Pid}}' "$container_name")"
  [[ "$container_pid" =~ ^[1-9][0-9]*$ ]] || die "PostgreSQL container PID is invalid"
  cgroup_relative="$(awk -F: '$1 == "0" {print $3}' "/proc/$container_pid/cgroup")"
  [[ "$cgroup_relative" == /* ]] || die "PostgreSQL cgroup path is invalid"
  container_cpu_stat="/sys/fs/cgroup${cgroup_relative}/cpu.stat"
  [[ -r "$container_cpu_stat" ]] || die "PostgreSQL cgroup cpu.stat is unreadable"
}

sample_container_cpu() {
  local output="$1"
  local stop_file="$2"
  local online_cpus="$3"
  local activity_mode="${4:-activity}"
  local previous_usage
  local previous_ns
  local current_usage
  local current_ns
  local delta_usage
  local delta_ns
  local interval_ms
  local one_core_percent
  local capacity_percent
  local activity
  local active_over_five_seconds
  local idle_in_transaction
  previous_usage="$(awk '$1 == "usage_usec" {print $2}' "$container_cpu_stat")"
  previous_ns="$(date +%s%N)"
  printf 'timestamp_epoch_ns\tusage_usec\tinterval_ms\tcpu_one_core_pct\tcpu_capacity_pct\tactive_over_five_seconds\tidle_in_transaction\n' >"$output"
  while [[ ! -e "$stop_file" ]]; do
    sleep 1
    current_usage="$(awk '$1 == "usage_usec" {print $2}' "$container_cpu_stat")"
    current_ns="$(date +%s%N)"
    delta_usage=$((current_usage - previous_usage))
    delta_ns=$((current_ns - previous_ns))
    ((delta_usage >= 0 && delta_ns > 0)) || return 1
    interval_ms="$(awk -v elapsed="$delta_ns" 'BEGIN {printf "%.3f", elapsed / 1000000}')"
    one_core_percent="$(awk -v usage="$delta_usage" -v elapsed="$delta_ns" \
      'BEGIN {printf "%.3f", 100000 * usage / elapsed}')"
    capacity_percent="$(awk -v cpu="$one_core_percent" -v cores="$online_cpus" \
      'BEGIN {printf "%.3f", cpu / cores}')"
    if [[ "$activity_mode" == "none" ]]; then
      activity=$'0\t0'
    else
      activity="$(PGAPPNAME=vpsman-pressure-sampler PGOPTIONS='-c statement_timeout=5s' \
        psql_proof -qAt -F $'\t' -c \
        "SELECT count(*) FILTER (WHERE pid <> pg_backend_pid() AND state = 'active' AND query_start < clock_timestamp() - interval '5 seconds'), count(*) FILTER (WHERE state LIKE 'idle in transaction%') FROM pg_stat_activity WHERE datname = current_database()")"
    fi
    IFS=$'\t' read -r active_over_five_seconds idle_in_transaction <<<"$activity"
    [[ "$active_over_five_seconds" =~ ^[0-9]+$ && "$idle_in_transaction" =~ ^[0-9]+$ ]] \
      || return 1
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$current_ns" "$current_usage" "$interval_ms" \
      "$one_core_percent" "$capacity_percent" \
      "$active_over_five_seconds" "$idle_in_transaction" >>"$output"
    previous_usage="$current_usage"
    previous_ns="$current_ns"
  done
}

filter_cpu_window() {
  local input="$1"
  local output="$2"
  local started_ns="$3"
  local finished_ns="$4"
  awk -F '\t' -v started="$started_ns" -v finished="$finished_ns" '
    NR == 1 || ($1 >= started && ($1 - $3 * 1000000) <= finished)
  ' "$input" >"$output"
  [[ "$(awk 'END {print NR}' "$output")" -gt 1 ]] \
    || die "CPU sampler did not cover the requested phase"
}

assert_activity_gate() {
  local input="$1"
  awk -F '\t' '
    NR > 1 && ($6 != 0 || $7 != 0) {failed = 1}
    END {if (NR <= 1 || failed) exit 1}
  ' "$input" || die "PostgreSQL had a historical >5s active or idle-in-transaction session"
}

assert_import_activity_gate() {
  local input="$1"
  awk -F '\t' '
    NR > 1 && $7 != 0 {failed = 1}
    END {if (NR <= 1 || failed) exit 1}
  ' "$input" || die "PostgreSQL had an idle-in-transaction session during import"
}

assert_postgres_window_log() {
  local input="$1"
  local label="$2"
  local maximum_allowed_ms="$3"
  local summary_output="$4"
  local allowed_error_pattern="${5:-}"
  local postgres_severity_line_pattern
  local error_lines
  local severity_event_count
  local allowed_error_event_count="0"
  local unexpected_severity_event_count
  # Statement/detail continuation lines are not prefixed and may legitimately
  # contain words such as "timeout". Only structured server-severity records
  # emitted under the configured log_line_prefix are failures here.
  postgres_severity_line_pattern='^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2}[.][0-9]{3} [^[:space:]]+ \[[0-9]+\] leader=[^[:space:]]* app=[^[:space:]]* user=[^[:space:]]* db=[^[:space:]]* backend=.* qid=-?[0-9]+ (ERROR|FATAL|PANIC):[[:space:]]+'
  error_lines="$({ rg \
    -- "$postgres_severity_line_pattern" \
    "$input" || true; } )"
  severity_event_count="$(
    printf '%s\n' "$error_lines" \
      | awk 'NF {count += 1} END {print count + 0}'
  )"
  if [[ -n "$allowed_error_pattern" ]]; then
    allowed_error_event_count="$(
      printf '%s\n' "$error_lines" \
        | awk -v expected="$allowed_error_pattern" '
            match($0, / ERROR:[[:space:]]+/) {
              payload = substr($0, RSTART + RLENGTH);
              sub(/[[:space:]]+$/, "", payload);
              if (payload == expected) count += 1;
            }
            END {print count + 0}
          '
    )"
    error_lines="$(
      printf '%s\n' "$error_lines" \
        | awk -v expected="$allowed_error_pattern" '
            match($0, / ERROR:[[:space:]]+/) {
              payload = substr($0, RSTART + RLENGTH);
              sub(/[[:space:]]+$/, "", payload);
              if (payload == expected) next;
            }
            {print}
          '
    )"
  fi
  unexpected_severity_event_count="$(
    printf '%s\n' "$error_lines" \
      | awk 'NF {count += 1} END {print count + 0}'
  )"
  local duration_summary
  local temporary_file_summary
  duration_summary="$(
    { rg -o 'duration: [0-9]+([.][0-9]+)? ms' "$input" || true; } \
      | awk '
          NF >= 3 {
            count += 1;
            duration = $2 + 0;
            if (duration > maximum || count == 1) maximum = duration;
          }
          END {
            printf "{\"logged_statement_count\":%d,\"max_statement_duration_ms\":%.3f}", count, maximum;
          }
        '
  )"
  temporary_file_summary="$(
    { rg -o 'temporary file:.*size [0-9]+' "$input" || true; } \
      | awk '
          {
            count += 1;
            bytes += $NF;
          }
          END {
            printf "{\"temporary_file_count\":%d,\"temporary_bytes\":%.0f}", count, bytes;
          }
        '
  )"
  jq -n \
    --arg label "$label" \
    --argjson maximum_allowed_ms "$maximum_allowed_ms" \
    --argjson severity_event_count "$severity_event_count" \
    --argjson allowed_error_event_count "$allowed_error_event_count" \
    --argjson unexpected_severity_event_count "$unexpected_severity_event_count" \
    --argjson observed "$duration_summary" \
    --argjson temporary_files "$temporary_file_summary" \
    '{
        phase: $label,
        failure_threshold_ms: $maximum_allowed_ms,
        severity_event_count: $severity_event_count,
        allowed_error_event_count: $allowed_error_event_count,
        unexpected_severity_event_count: $unexpected_severity_event_count
      }
      + $observed + $temporary_files' \
    >"$summary_output"
  if [[ -n "$allowed_error_pattern" && "$allowed_error_event_count" != "1" ]]; then
    die "$label PostgreSQL log did not contain exactly one expected fault-injection error"
  fi
  if [[ "$unexpected_severity_event_count" != "0" ]]; then
    die "$label PostgreSQL log contains an unexpected ERROR, FATAL, or PANIC record"
  fi
  if awk -v observed="$(jq -r '.max_statement_duration_ms' "$summary_output")" \
    -v limit="$maximum_allowed_ms" 'BEGIN {exit !(observed >= limit)}'; then
    die "$label PostgreSQL statement duration reached the ${maximum_allowed_ms}ms failure threshold"
  fi
}

assert_zero_postgres_temp_log() {
  local summary="$1"
  local label="$2"
  jq -e '
    .temporary_file_count == 0
    and .temporary_bytes == 0
  ' "$summary" >/dev/null \
    || die "$label PostgreSQL log contains a temporary file"
}

run_exact_reimport_probe() {
  local label="$1"
  local test_name="$2"
  local expected_error_pattern="${3:-}"
  local output="$artifact_dir/${label}-probe.log"
  local postgres_log="$artifact_dir/${label}-postgres.log"
  local postgres_summary="$artifact_dir/${label}-postgres-log-summary.json"
  local started_at
  local finished_at
  local started_unix_ms
  local finished_unix_ms
  local process_group_pid

  started_at="$(date -u --iso-8601=ns)"
  started_unix_ms="$(date +%s%3N)"
  start_managed_process_group \
    "$output" \
    env \
      GITHUB_ACTIONS=true \
      CARGO_INCREMENTAL=0 \
      CARGO_TARGET_DIR="$CARGO_TARGET_STORAGE" \
      VPSMAN_BUILD_NUMBER_DIR="$build_number_dir" \
      VPSMAN_TEST_POSTGRES_URL="$postgres_url" \
      cargo test -p vpsman-api "$test_name" -- --ignored --nocapture
  process_group_pid="$managed_process_group_pid"
  if ! wait_managed_process_group_until \
    "$process_group_pid" \
    "$((SECONDS + EXACT_REIMPORT_PHASE_WALL_TIMEOUT_SECS))" \
    "$label exact PostgreSQL reimport probe"; then
    finished_at="$(date -u --iso-8601=ns)"
    docker logs --since "$started_at" --until "$finished_at" "$container_name" \
      >"$postgres_log" 2>&1 || true
    die "$label exact PostgreSQL reimport probe failed or exceeded its ${EXACT_REIMPORT_PHASE_WALL_TIMEOUT_SECS}s hard wall deadline"
  fi
  rg -q "running 1 test" "$output" \
    || die "$label exact PostgreSQL reimport probe did not run exactly one filtered test"
  rg -q "test .*${test_name}.*ok" "$output" \
    || die "$label exact PostgreSQL reimport probe did not report a passing filtered test"
  local leaked_probe_databases
  leaked_probe_databases="$(psql_proof -qAt -c \
    "SELECT count(*) FROM pg_database WHERE datname LIKE 'vpsman_exact_reimport_%'")"
  printf '%s\n' "$leaked_probe_databases" >"$artifact_dir/${label}-leaked-databases.txt"
  [[ "$leaked_probe_databases" == "0" ]] \
    || die "$label exact PostgreSQL reimport probe leaked disposable databases"
  finished_at="$(date -u --iso-8601=ns)"
  finished_unix_ms="$(date +%s%3N)"
  docker logs --since "$started_at" --until "$finished_at" "$container_name" \
    >"$postgres_log" 2>&1
  if [[ -n "$expected_error_pattern" ]]; then
    assert_postgres_window_log \
      "$postgres_log" \
      "$label exact reimport" \
      60000 \
      "$postgres_summary" \
      "$expected_error_pattern"
  else
    assert_postgres_window_log \
      "$postgres_log" \
      "$label exact reimport" \
      60000 \
      "$postgres_summary"
  fi
  assert_zero_postgres_temp_log "$postgres_summary" "$label exact reimport"
  jq -n \
    --arg schema "vpsman-vnstat-exact-reimport-probe/v1" \
    --arg status "passed" \
    --arg label "$label" \
    --arg test_name "$test_name" \
    --arg started_at "$started_at" \
    --arg finished_at "$finished_at" \
    --arg log "${postgres_log#"$ROOT_DIR/"}" \
    --arg summary "${postgres_summary#"$ROOT_DIR/"}" \
    --arg cargo_output "${output#"$ROOT_DIR/"}" \
    --arg expected_error_pattern "$expected_error_pattern" \
    --argjson started_unix_ms "$started_unix_ms" \
    --argjson finished_unix_ms "$finished_unix_ms" \
    --argjson wall_timeout_secs "$EXACT_REIMPORT_PHASE_WALL_TIMEOUT_SECS" \
    --argjson postgres_log_summary "$(jq -c . "$postgres_summary")" \
    '{
      schema: $schema,
      status: $status,
      label: $label,
      test_name: $test_name,
      started_at: $started_at,
      finished_at: $finished_at,
      started_unix_ms: $started_unix_ms,
      finished_unix_ms: $finished_unix_ms,
      wall_timeout_secs: $wall_timeout_secs,
      expected_error_pattern: (if $expected_error_pattern == "" then null else $expected_error_pattern end),
      cargo_output: $cargo_output,
      postgres_log: $log,
      postgres_log_summary_path: $summary,
      postgres_log_summary: $postgres_log_summary
    }' >"$artifact_dir/${label}-probe.json"
}

reset_postgres_phase_statistics() {
  # pg_stat_database is cumulative and each backend may still hold a local
  # report.  Resetting it here creates a false phase boundary (and can make
  # pre-boundary temp files reappear later).  The measured importer uses its
  # all-backend flush/delta helper; the shell phases use log windows and a
  # current-database pg_stat_statements epoch instead.
  psql_proof -qAt -c \
    "SELECT pg_stat_statements_reset()" >/dev/null
}

capture_pg_stat_statements() {
  local phase="$1"
  local output="$2"
  # psql expands :variables for stdin/-f input, but not for a -c argument.
  # Keep the delimiter quoted so the phase value is supplied only through
  # psql's SQL-literal quoting, including any unexpected shell characters.
  psql_proof -qAt -v "phase=$phase" >"$output" <<'SQL'
WITH statements AS (
  SELECT
    calls,
    rows,
    total_plan_time,
    total_exec_time,
    max_exec_time,
    shared_blks_read,
    shared_blks_hit,
    temp_blks_read,
    temp_blks_written,
    left(regexp_replace(query, '[[:space:]]+', ' ', 'g'), 512) AS query
  FROM pg_stat_statements
  WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database())
), top_statements AS (
  SELECT * FROM statements
  ORDER BY total_exec_time DESC, calls DESC
  LIMIT 50
)
SELECT json_build_object(
  'schema', 'vpsman-pg-stat-statements-phase/v1',
  'phase', :'phase',
  'captured_at', clock_timestamp(),
  'stats_reset_at', (SELECT stats_reset FROM pg_stat_statements_info),
  'summary', json_build_object(
    'statements', (SELECT count(*) FROM statements),
    'calls', (SELECT COALESCE(sum(calls), 0) FROM statements),
    'rows', (SELECT COALESCE(sum(rows), 0) FROM statements),
    'total_plan_time_ms', (SELECT COALESCE(sum(total_plan_time), 0) FROM statements),
    'total_exec_time_ms', (SELECT COALESCE(sum(total_exec_time), 0) FROM statements),
    'max_exec_time_ms', (SELECT COALESCE(max(max_exec_time), 0) FROM statements),
    'shared_blks_read', (SELECT COALESCE(sum(shared_blks_read), 0) FROM statements),
    'shared_blks_hit', (SELECT COALESCE(sum(shared_blks_hit), 0) FROM statements),
    'temp_blks_read', (SELECT COALESCE(sum(temp_blks_read), 0) FROM statements),
    'temp_blks_written', (SELECT COALESCE(sum(temp_blks_written), 0) FROM statements)
  ),
  'top_statements', COALESCE((SELECT json_agg(top_statements) FROM top_statements), '[]'::json)
)
SQL
  jq -e --arg phase "$phase" '
    .schema == "vpsman-pg-stat-statements-phase/v1"
    and .phase == $phase
    and (.stats_reset_at | type == "string")
    and (.summary.statements >= 1)
  ' "$output" >/dev/null \
    || die "$phase pg_stat_statements evidence is invalid"
}

cpu_summary() {
  local input="$1"
  awk -F '\t' '
    NR > 1 {
      samples += 1;
      sum += $4;
      if ($4 > max || samples == 1) max = $4;
      if ($5 > capacity_max || samples == 1) capacity_max = $5;
      if ($6 > active_max || samples == 1) active_max = $6;
      if ($7 > idle_max || samples == 1) idle_max = $7;
    }
    END {
      if (samples == 0) exit 1;
      printf "{\"samples\":%d,\"mean_one_core_pct\":%.3f,\"max_one_core_pct\":%.3f,\"max_capacity_pct\":%.3f,\"max_active_over_five_seconds\":%d,\"max_idle_in_transaction\":%d}", samples, sum / samples, max, capacity_max, active_max, idle_max;
    }
  ' "$input"
}

assert_browser_cpu_gate() {
  local input="$1"
  awk -F '\t' '
    NR > 1 {
      samples += 1;
      if ($3 < 800 || $3 > 1300 || $4 >= 50.0) failed = 1;
    }
    END {
      if (samples < 85 || failed) exit 1;
    }
  ' "$input" || die "PostgreSQL CPU was not strictly below 50.0% in every one-second browser window"
}

extract_cli_result() {
  local input="$1"
  local output="$2"
  awk '
    /^### Result$/ {capture=1; found=1; next}
    capture && /^### / {exit}
    capture {print}
    END {if (!found) exit 1}
  ' \
    "$input" >"$output" \
    || die "Playwright CLI did not return a machine-readable result: $input"
  jq -e . "$output" >/dev/null \
    || die "Playwright CLI returned invalid JSON: $input"
}

cleanup_storage() {
  local manifest_path="${1:-}"
  [[ -n "$manifest_path" ]] || die "cleanup requires a final-report.json path"
  manifest_path="$(readlink -m "$manifest_path")"
  [[ "$manifest_path" == "$ROOT_DIR"/output/playwright/vnstat-browser-pressure-*/final-report.json ]] \
    || die "cleanup manifest is outside the pressure-proof artifact scope"
  [[ -f "$manifest_path" ]] || die "cleanup manifest does not exist"
  local checksum_path="${manifest_path%.json}.sha256"
  [[ -f "$checksum_path" ]] || die "cleanup report checksum is missing"
  local checksum_digest
  local checksum_name
  local checksum_extra
  read -r checksum_digest checksum_name checksum_extra <"$checksum_path"
  [[ "$checksum_digest" =~ ^[0-9a-f]{64}$ \
    && "$checksum_name" == "final-report.json" && -z "$checksum_extra" ]] \
    || die "cleanup report checksum has an invalid format"
  [[ "$(sha256sum "$manifest_path" | awk '{print $1}')" == "$checksum_digest" ]] \
    || die "cleanup report checksum does not match"
  jq -e '.schema == "vpsman-vnstat-browser-pressure/v1" and .status == "passed"' \
    "$manifest_path" >/dev/null || die "cleanup manifest is not a passed pressure proof"
  local manifest_run_id
  local postgres_data_dir
  local storage_run_dir
  local trash_root
  local trash_target
  local manifest_container
  local owner_marker
  local manifest_artifact_dir
  manifest_run_id="$(jq -er '.run_id' "$manifest_path")"
  [[ "$manifest_run_id" =~ ^review-[0-9]{8}T[0-9]{6}Z-[0-9]+$ ]] \
    || die "cleanup manifest run ID is invalid"
  postgres_data_dir="$(jq -er '.retained_postgres_data' "$manifest_path")"
  manifest_container="$(jq -er '.postgres_container' "$manifest_path")"
  owner_marker="$(jq -er '.storage_owner_marker' "$manifest_path")"
  manifest_artifact_dir="$(jq -er '.artifact_dir' "$manifest_path")"
  [[ "$manifest_artifact_dir" == "output/playwright/vnstat-browser-pressure-$manifest_run_id" \
    && "$manifest_path" == "$ROOT_DIR/$manifest_artifact_dir/final-report.json" ]] \
    || die "cleanup artifact ownership does not match the run ID"
  [[ "$manifest_container" == "vpsman-monitoring-${manifest_run_id}-postgres" ]] \
    || die "cleanup container ownership does not match the run ID"
  [[ "$postgres_data_dir" == /mnt/storage/vpsman-tmp/*/"$manifest_run_id"/postgres ]] \
    || die "cleanup PostgreSQL path is outside the exact run scope"
  [[ "$(readlink -m "$postgres_data_dir")" == "$postgres_data_dir" ]] \
    || die "cleanup PostgreSQL path is not canonical"
  if docker inspect "$manifest_container" >/dev/null 2>&1; then
    die "cleanup refuses a PostgreSQL directory whose container still exists"
  fi
  storage_run_dir="$(dirname "$postgres_data_dir")"
  [[ -d "$storage_run_dir" ]] || die "retained PostgreSQL run directory is absent"
  [[ "$owner_marker" == "$storage_run_dir/.vpsman-vnstat-browser-pressure-owner.json" \
    && -f "$owner_marker" && ! -L "$owner_marker" \
    && "$(readlink -m "$owner_marker")" == "$owner_marker" ]] \
    || die "cleanup ownership marker is missing or outside the run"
  jq -e \
    --arg run_id "$manifest_run_id" \
    --arg postgres_data "$postgres_data_dir" \
    --arg artifact_dir "$manifest_artifact_dir" \
    '.schema == "vpsman-vnstat-browser-pressure-owner/v1"
      and .run_id == $run_id
      and .postgres_data == $postgres_data
      and .artifact_dir == $artifact_dir' \
    "$owner_marker" >/dev/null || die "cleanup ownership marker does not match the passed report"
  assert_no_container_mounts_postgres_data "$postgres_data_dir"
  trash_root="$(dirname "$(dirname "$storage_run_dir")")/vpsman-vnstat-browser-pressure-trash"
  [[ "$trash_root" == /mnt/storage/vpsman-tmp/* ]] \
    || die "cleanup trash root is outside /mnt/storage/vpsman-tmp"
  mkdir -p "$trash_root"
  trash_target="$trash_root/${manifest_run_id}-$(date -u +%Y%m%dT%H%M%SZ)"
  [[ ! -e "$trash_target" ]] || die "cleanup trash target already exists"
  mv "$storage_run_dir" "$trash_target"
  jq -n \
    --arg run_id "$manifest_run_id" \
    --arg retained_artifacts "${manifest_path#"$ROOT_DIR/"}" \
    --arg recoverable_postgres_data "$trash_target" \
    '{
      status: "moved_to_recoverable_trash",
      run_id: $run_id,
      retained_artifacts: $retained_artifacts,
      recoverable_postgres_data: $recoverable_postgres_data
    }'
}

run_proof() {
  require_tools awk bash cargo curl date df docker git grep jq mv npx ps psql readlink rg sed setsid sha256sum sort stat tr xargs
  # Pin all proof-control SQL sessions to a stable application identity.  A
  # sampler can still override PGAPPNAME on its own child invocation.
  export PGAPPNAME=vpsman-pressure-control
  command -v npx >/dev/null 2>&1 || die "npx is required by the Playwright CLI wrapper"
  [[ -r "$PWCLI" ]] || die "Playwright CLI wrapper is missing or unreadable: $PWCLI"
  [[ -x "$ROOT_DIR/frontend/node_modules/.bin/vite" ]] \
    || die "frontend dependencies are missing"
  [[ ! -e "$REVIEW_STATE" ]] \
    || die "an existing monitoring review stack must be stopped first"
  validate_storage_root
  mkdir -p "$STORAGE_ROOT" "$CARGO_TARGET_STORAGE"
  [[ -d "$STORAGE_ROOT" && -w "$STORAGE_ROOT" ]] \
    || die "storage root is not writable"
  [[ -d "$CARGO_TARGET_STORAGE" && -w "$CARGO_TARGET_STORAGE" ]] \
    || die "Cargo target is not writable"
  local available_storage_bytes
  local pre_mutation_projection
  local resolved_focused_manifest
  available_storage_bytes="$(df -PB1 "$STORAGE_ROOT" | awk 'NR == 2 {print $4}')"
  [[ "$available_storage_bytes" =~ ^[0-9]+$ ]] \
    || die "could not measure storage-backed free space"
  ((available_storage_bytes >= MINIMUM_STORAGE_FREE_BYTES)) \
    || die "retained-history proof requires at least 48 GiB free below /mnt/storage/vpsman-tmp"
  pre_mutation_projection="$STORAGE_ROOT/pre-mutation-projection-$(date -u +%Y%m%dT%H%M%SZ)-$$.json"
  [[ ! -e "$pre_mutation_projection" ]] \
    || die "pre-mutation projection path already exists"
  jq -n \
    --arg captured_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg storage_root "$STORAGE_ROOT" \
    --argjson available_storage_bytes "$available_storage_bytes" \
    --argjson minimum_storage_free_bytes "$MINIMUM_STORAGE_FREE_BYTES" \
    '{
      schema: "vpsman-five-year-retained-projection/v1",
      captured_at: $captured_at,
      emitted_before_stack_or_fixture_mutation: true,
      storage_root: $storage_root,
      available_storage_bytes: $available_storage_bytes,
      minimum_storage_free_bytes: $minimum_storage_free_bytes,
      projected_seed_seconds: {minimum: 900, maximum: 2700},
      projected_analyze_seconds: {minimum: 120, maximum: 900},
      projected_database_bytes: {minimum: 16106127360, maximum: 32212254720},
      projected_postgres_peak_bytes: 34359738368,
      projected_rows: {
        imported_traffic_source_storage_hard_maximum: 6302280,
        imported_hourly_ledger: {minimum: 5256000, maximum: 5256120},
        imported_stream_registry: 120,
        non_traffic_history_facts: {minimum: 8638400, maximum: 8645870},
        non_traffic_current_latest: 360,
        non_traffic_fixture_identities: 421,
        telemetry_pressure_owned_hard_maximum: 20205171
      },
      retained_shape: {
        clients: 120,
        history_days_minimum: 1825,
        raw_resource_rows_per_client: 10080,
        raw_counter_fact_rows_per_client: 10080,
        raw_ping_rows_per_client: 10080,
        rollup_rows_per_stream: {minimum: 9952, maximum: 9967},
        resource_streams: 120,
        network_rate_streams: 120,
        ping_streams: 120,
        network_observation_streams: 120,
        network_observation_exact_rows_per_stream: 552,
        network_observation_rollup_rows_per_stream: {minimum: 7192, maximum: 7203},
        system_metric_streams: 50,
        current_latest_rows: 360,
        support_identity_rows: 421,
        telemetry_bucket_secs: [60, 300, 1800, 3600, 10800, 21600, 86400],
        network_observation_bucket_secs: [300, 1800, 3600, 10800, 21600, 86400]
      }
    }' >"$pre_mutation_projection"
  unset CARGO_BUILD_TARGET CARGO_TARGET_DIR \
    VPSMAN_MONITORING_REVIEW_SKIP_BUILD \
    VPSMAN_MONITORING_REVIEW_CARGO_TARGET_DIR \
    VPSMAN_MONITORING_REVIEW_PGDATA_ROOT

  trap on_exit EXIT INT TERM
  local frozen_worktree_sha256
  frozen_worktree_sha256="$(current_worktree_hash)"
  local frozen_build_output
  local build_number_dir
  frozen_build_output="$(mktemp "$ROOT_DIR/.tmp/vnstat-pressure-build.XXXXXX")"
  build_number_dir="$(mktemp -d "$ROOT_DIR/.tmp/vnstat-pressure-build-numbers.XXXXXX")"
  cp "$ROOT_DIR"/build/build-numbers/*.txt "$build_number_dir/"
  GITHUB_ACTIONS=true \
  CARGO_INCREMENTAL=0 \
  VPSMAN_BUILD_NUMBER_DIR="$build_number_dir" \
  VPSMAN_MONITORING_REVIEW_SKIP_BUILD=0 \
  VPSMAN_MONITORING_REVIEW_PGDATA_ROOT="$STORAGE_ROOT" \
  VPSMAN_MONITORING_REVIEW_CARGO_TARGET_DIR="$CARGO_TARGET_STORAGE" \
  VPSMAN_MONITORING_REVIEW_PG_STAT_STATEMENTS=1 \
    "$REVIEW_HARNESS" start >/dev/null 2>"$frozen_build_output"
  stack_started=1
  [[ -f "$REVIEW_STATE" ]] || die "review harness did not persist its state"

  run_id="$(jq -er '.run_id' "$REVIEW_STATE")"
  [[ "$run_id" =~ ^review-[0-9]{8}T[0-9]{6}Z-[0-9]+$ ]] \
    || die "review harness returned an invalid run ID"
  container_name="$(jq -er '.container_name' "$REVIEW_STATE")"
  postgres_port="$(jq -er '.postgres_port' "$REVIEW_STATE")"
  postgres_password="$(jq -er '.postgres_password' "$REVIEW_STATE")"
  postgres_url="$(jq -er '.postgres_url' "$REVIEW_STATE")"
  postgres_data_dir="$(jq -er '.postgres_data_dir' "$REVIEW_STATE")"
  cargo_target_dir="$(jq -er '.cargo_target_dir' "$REVIEW_STATE")"
  api_pid="$(jq -er '.api_pid' "$REVIEW_STATE")"
  frontend_pid="$(jq -er '.frontend_pid' "$REVIEW_STATE")"
  frontend_url="$(jq -er '.frontend_url' "$REVIEW_STATE")"
  operator_username="$(jq -er '.operator_username' "$REVIEW_STATE")"
  operator_password="$(jq -er '.operator_password' "$REVIEW_STATE")"
  visible_share_id="$(jq -er '.visible_share.id' "$REVIEW_STATE")"
  visible_share_fragment="$(jq -er '.visible_share.fragment' "$REVIEW_STATE")"
  [[ "$visible_share_fragment" =~ ^#/share/([0-9a-f-]{36})/([0-9a-f]{64})$ \
    && "${BASH_REMATCH[1]}" == "$visible_share_id" ]] \
    || die "review harness returned an invalid visible public-share identity"
  visible_share_secret="${BASH_REMATCH[2]}"
  artifact_dir="$ROOT_DIR/output/playwright/vnstat-browser-pressure-$run_id"
  mkdir -p "$artifact_dir/browser"
  chmod 700 "$artifact_dir"
  mv "$pre_mutation_projection" "$artifact_dir/pre-mutation-projection.json"
  mv "$frozen_build_output" "$artifact_dir/frozen-source-build.log"
  [[ "$postgres_data_dir" == "$STORAGE_ROOT/$run_id/postgres" ]] \
    || die "review harness did not use the required storage-backed PGDATA"
  [[ "$cargo_target_dir" == "$CARGO_TARGET_STORAGE" ]] \
    || die "review harness did not use the required storage-backed Cargo target"
  local api_binary
  local api_process_binary
  local api_binary_sha256
  local api_process_binary_sha256
  api_binary="$CARGO_TARGET_STORAGE/debug/vpsman-api"
  [[ -x "$api_binary" ]] || die "frozen-source API binary is missing from the exact Cargo target"
  api_process_binary="$(readlink -e "/proc/$api_pid/exe")"
  [[ "$api_process_binary" == "$api_binary" ]] \
    || die "running API executable does not match the exact storage-backed build"
  api_binary_sha256="$(sha256sum "$api_binary" | awk '{print $1}')"
  api_process_binary_sha256="$(sha256sum "/proc/$api_pid/exe" | awk '{print $1}')"
  [[ "$api_process_binary_sha256" == "$api_binary_sha256" ]] \
    || die "running API bytes do not match the frozen-source binary"
  rg -q 'Finished .+ profile' "$artifact_dir/frozen-source-build.log" \
    || die "mandatory frozen-source Cargo build evidence is absent"
  jq -n \
    --arg frozen_worktree_sha256 "$frozen_worktree_sha256" \
    --arg cargo_target_dir "$cargo_target_dir" \
    --arg api_binary "$api_binary" \
    --arg api_process_binary "$api_process_binary" \
    --arg api_binary_sha256 "$api_binary_sha256" \
    '{
      frozen_worktree_sha256: $frozen_worktree_sha256,
      inherited_build_skip_disabled: true,
      cargo_build_completed: true,
      cargo_target_dir: $cargo_target_dir,
      api_binary: $api_binary,
      api_process_binary: $api_process_binary,
      api_binary_sha256: $api_binary_sha256,
      process_binary_matches_built_binary: true
    }' >"$artifact_dir/frozen-source-binary.json"
  local storage_run_dir
  local owner_marker
  local owner_marker_tmp
  storage_run_dir="$(dirname "$postgres_data_dir")"
  owner_marker="$storage_run_dir/.vpsman-vnstat-browser-pressure-owner.json"
  owner_marker_tmp="$owner_marker.tmp"
  [[ ! -e "$owner_marker" && ! -e "$owner_marker_tmp" ]] \
    || die "benchmark storage ownership marker already exists"
  jq -n \
    --arg run_id "$run_id" \
    --arg postgres_data "$postgres_data_dir" \
    --arg artifact_dir "${artifact_dir#"$ROOT_DIR/"}" \
    '{
      schema: "vpsman-vnstat-browser-pressure-owner/v1",
      run_id: $run_id,
      postgres_data: $postgres_data,
      artifact_dir: $artifact_dir
    }' >"$owner_marker_tmp"
  mv "$owner_marker_tmp" "$owner_marker"

  {
    psql_proof -q -c "ALTER SYSTEM SET log_min_duration_statement = '5000ms'"
    psql_proof -q -c "ALTER SYSTEM SET idle_in_transaction_session_timeout = '5000ms'"
    psql_proof -q -c "ALTER SYSTEM SET log_temp_files = '0'"
    psql_proof -q -c "ALTER SYSTEM SET log_autovacuum_min_duration = '0'"
    psql_proof -q -c "ALTER SYSTEM SET log_line_prefix = '%m [%p] leader=%P app=%a user=%u db=%d backend=%b qid=%Q '"
    psql_proof -q -c "ALTER SYSTEM SET log_parameter_max_length = '0'"
    psql_proof -qAt -c "SELECT pg_reload_conf()"
    psql_proof -q -c "CREATE EXTENSION IF NOT EXISTS pg_stat_statements"
  } >"$artifact_dir/postgres-observability-config.log" 2>&1
  psql_proof -qAt -c \
    "SELECT json_build_object('log_min_duration_statement_ms', (SELECT setting::bigint FROM pg_settings WHERE name = 'log_min_duration_statement'), 'idle_in_transaction_session_timeout_ms', (SELECT setting::bigint FROM pg_settings WHERE name = 'idle_in_transaction_session_timeout'), 'log_temp_files_kb', (SELECT setting::bigint FROM pg_settings WHERE name = 'log_temp_files'), 'log_autovacuum_min_duration_ms', (SELECT setting::bigint FROM pg_settings WHERE name = 'log_autovacuum_min_duration'), 'log_line_prefix', (SELECT setting FROM pg_settings WHERE name = 'log_line_prefix'), 'log_parameter_max_length_bytes', (SELECT setting::bigint FROM pg_settings WHERE name = 'log_parameter_max_length'), 'pg_stat_statements_preloaded', (SELECT position('pg_stat_statements' in setting) > 0 FROM pg_settings WHERE name = 'shared_preload_libraries'), 'pg_stat_statements_track', (SELECT setting FROM pg_settings WHERE name = 'pg_stat_statements.track'))" \
    >"$artifact_dir/postgres-observability-settings.json"
  jq -e '
    .log_min_duration_statement_ms == 5000
    and .idle_in_transaction_session_timeout_ms == 5000
    and .log_temp_files_kb == 0
    and .log_autovacuum_min_duration_ms == 0
    and .log_line_prefix == "%m [%p] leader=%P app=%a user=%u db=%d backend=%b qid=%Q "
    and .log_parameter_max_length_bytes == 0
    and .pg_stat_statements_preloaded == true
    and .pg_stat_statements_track == "all"
  ' "$artifact_dir/postgres-observability-settings.json" >/dev/null \
    || die "PostgreSQL historical slow/idle observability settings are not active"

  psql_proof -q -v pressure_skip_traffic=true -f "$PRESSURE_FIXTURE" \
    >"$artifact_dir/fixture.log" 2>&1
  psql_proof -qAt -c \
    "SELECT count(*) = 128 AND count(*) FILTER (WHERE id LIKE 'pressure-%') = 120 FROM clients" \
    | grep -qx t || die "pressure fixture did not create the exact 128-client scope"
  psql_proof -qAt -c \
    "SELECT count(*) FROM traffic_counter_samples WHERE client_id LIKE 'pressure-%'" \
    | grep -qx 0 || die "import-only pressure fixture wrote traffic directly"

  GITHUB_ACTIONS=true \
  CARGO_INCREMENTAL=0 \
  CARGO_TARGET_DIR="$CARGO_TARGET_STORAGE" \
  VPSMAN_BUILD_NUMBER_DIR="$build_number_dir" \
    cargo test --no-run -p vpsman-api "$IMPORT_TEST" \
      >"$artifact_dir/import-test-build.log" 2>&1
  GITHUB_ACTIONS=true \
  CARGO_INCREMENTAL=0 \
  CARGO_TARGET_DIR="$CARGO_TARGET_STORAGE" \
  VPSMAN_BUILD_NUMBER_DIR="$build_number_dir" \
    cargo test --no-run -p vpsman-worker "$MAINTENANCE_TEST" \
      >"$artifact_dir/maintenance-test-build.log" 2>&1

  # Keep the measured import/reimport and retained-history windows
  # database-only. The API resumes only after seed/maintenance, immediately
  # before share-management and browser phases need live HTTP traffic again.
  "$REVIEW_HARNESS" quiesce-api >"$artifact_dir/api-quiesce.json" 2>&1
  jq -e \
    --arg run_id "$run_id" \
    '.status == "api_quiesced"
      and .run_id == $run_id
      and .api.pid == 0
      and .postgres_backends == 0' \
    "$artifact_dir/api-quiesce.json" >/dev/null \
    || die "review harness did not confirm the exact API quiesce"
  api_pid="0"
  if [[ -n "$FOCUSED_EVIDENCE_MANIFEST" ]]; then
    resolved_focused_manifest="$(readlink -m "$FOCUSED_EVIDENCE_MANIFEST")"
    verify_reused_focused_evidence "$resolved_focused_manifest"
    cp -- "$resolved_focused_manifest" "$artifact_dir/focused-reimport-evidence-manifest.json"
    write_reused_probe_summary \
      "$resolved_focused_manifest" one_client exact-one-client-reimport-probe.json
    write_reused_probe_summary \
      "$resolved_focused_manifest" four_client exact-four-client-reimport-probe.json
  else
    run_exact_reimport_probe \
      "exact-one-client-reimport" \
      "$EXACT_ONE_REIMPORT_TEST" \
      "vpsman_exact_reimport_intentional_failure"
    run_exact_reimport_probe \
      "exact-four-client-reimport" \
      "$EXACT_FOUR_REIMPORT_TEST"
  fi

  resolve_container_cpu_stat
  local online_cpus
  local import_stop="$artifact_dir/import-cpu.stop"
  local import_cpu="$artifact_dir/import-postgres-cpu.tsv"
  local reimport_cpu="$artifact_dir/reimport-postgres-cpu.tsv"
  local import_test_cpu="$artifact_dir/import-test-postgres-cpu.tsv"
  online_cpus="$(docker exec "$container_name" getconf _NPROCESSORS_ONLN)"
  [[ "$online_cpus" =~ ^[1-9][0-9]*$ ]] || die "container CPU count is invalid"
  rm -f "$import_stop"
  sample_container_cpu "$import_test_cpu" "$import_stop" "$online_cpus" none &
  local import_sampler_pid="$!"
  active_sampler_pid="$import_sampler_pid"
  active_sampler_stop="$import_stop"
  local import_test_started_at
  import_test_started_at="$(date -u --iso-8601=ns)"
  start_managed_process_group \
    "$artifact_dir/import-test.log" \
    env \
      GITHUB_ACTIONS=true \
      CARGO_INCREMENTAL=0 \
      CARGO_TARGET_DIR="$CARGO_TARGET_STORAGE" \
      VPSMAN_BUILD_NUMBER_DIR="$build_number_dir" \
      VPSMAN_VNSTAT_BROWSER_PRESSURE=1 \
      VPSMAN_VNSTAT_BROWSER_PRESSURE_DATABASE_URL="$postgres_url" \
      VPSMAN_VNSTAT_BROWSER_PRESSURE_REPORT="$artifact_dir/import-report.json" \
      cargo test -p vpsman-api "$IMPORT_TEST" -- --ignored --nocapture
  local import_process_group_pid="$managed_process_group_pid"
  if ! wait_managed_process_group_until \
    "$import_process_group_pid" \
    "$((SECONDS + IMPORT_PHASE_WALL_TIMEOUT_SECS))" \
    "120-client ignored Cargo import phase"; then
    touch "$import_stop"
    wait "$import_sampler_pid" >/dev/null 2>&1 || true
    active_sampler_pid="0"
    active_sampler_stop=""
    rm -f "$import_stop"
    docker logs --since "$import_test_started_at" "$container_name" \
      >"$artifact_dir/import-test-postgres-failure.log" 2>&1 || true
    psql_proof -qAt -c \
      "SELECT json_build_object('captured_at', clock_timestamp(), 'database', current_database(), 'stats', (SELECT row_to_json(stats) FROM (SELECT xact_commit, xact_rollback, temp_files, temp_bytes, deadlocks, stats_reset FROM pg_stat_database WHERE datname = current_database()) stats), 'activity', (SELECT coalesce(json_agg(activity), '[]'::json) FROM (SELECT pid, application_name, backend_type, state, wait_event_type, wait_event, xact_start, query_start, left(query, 512) AS query FROM pg_stat_activity WHERE datname = current_database() ORDER BY pid) activity))" \
      >"$artifact_dir/import-test-postgres-failure-stats.json" 2>&1 || true
    psql_proof -qAt -c \
      "SELECT coalesce(json_agg(statements), '[]'::json) FROM (SELECT queryid, calls, rows, total_exec_time, max_exec_time, temp_blks_read, temp_blks_written, wal_records, wal_fpi, wal_bytes, left(regexp_replace(query, '[[:space:]]+', ' ', 'g'), 1024) AS query FROM pg_stat_statements WHERE dbid = (SELECT oid FROM pg_database WHERE datname = current_database()) ORDER BY temp_blks_written DESC, total_exec_time DESC LIMIT 100) statements" \
      >"$artifact_dir/import-test-postgres-failure-statements.json" 2>&1 || true
    die "120-client ignored Cargo import phase failed or exceeded its ${IMPORT_PHASE_WALL_TIMEOUT_SECS}s hard wall deadline"
  fi
  touch "$import_stop"
  wait "$import_sampler_pid"
  active_sampler_pid="0"
  active_sampler_stop=""
  rm -f "$import_stop"
  jq -e \
   --argjson import_max_elapsed_ms "$IMPORT_PHASE_MAX_ELAPSED_MS" \
   --argjson reimport_max_elapsed_ms "$REIMPORT_PHASE_MAX_ELAPSED_MS" \
   --argjson import_min_rows_per_second "$MIN_IMPORT_ROWS_PER_SECOND" \
   --argjson reimport_min_rows_per_second "$MIN_REIMPORT_ROWS_PER_SECOND" \
    --argjson import_min_clients_per_second "$MIN_IMPORT_CLIENTS_PER_SECOND" \
    --argjson reimport_min_clients_per_second "$MIN_REIMPORT_CLIENTS_PER_SECOND" \
   '
    .schema == "vpsman-vnstat-browser-pressure-import/v1"
    and .client_count == 120
    and .failed_clients == 0
    and .clean_ledger_streams == 120
    and .hourly_usage_parity.clean_streams == 120
    and .hourly_usage_parity.total_streams == 120
    and .hourly_usage_parity.mismatch_rows == 0
    and .hourly_usage_parity.raw_oracle_rows == .hourly_usage_parity.materialized_rows
    and .provenance.jobs == 120
    and .provenance.completed_targets == 120
    and .provenance.outputs == 240
    and .provenance.invalid_lineages == 0
    and .live_successor_boundaries.total == 120
    and .live_successor_boundaries.timestamp_preserved == 120
    and .live_successor_boundaries.counter_values_preserved == 120
    and .live_successor_boundaries.epoch_advanced_once == 120
   and .expected_bytes == .observed_bytes
   and .raw_rows.hard_max_per_client == 47520
    and ((.raw_rows.expected_per_client | type) == "number")
    and ((.raw_rows.expected_total | type) == "number")
    and .raw_rows.expected_per_client > 0
    and .raw_rows.expected_per_client
      == (.raw_rows.expected_per_client | floor)
    and .raw_rows.expected_total == (.raw_rows.expected_total | floor)
    and .raw_rows.expected_per_client <= .raw_rows.hard_max_per_client
    and .raw_rows.expected_total
      == (.raw_rows.expected_per_client * .client_count)
    and .raw_rows.min_per_client == .raw_rows.expected_per_client
    and .raw_rows.max_per_client == .raw_rows.expected_per_client
    and .raw_rows.total == .raw_rows.expected_total
   and .raw_rows.max_per_client <= .raw_rows.hard_max_per_client
    and .rollup_rows.max_per_client <= .rollup_rows.hard_max_per_client
    and .import_postgres.temporary_files == 0
    and .import_postgres.temporary_bytes == 0
   and .import_postgres.deadlocks == 0
   and .import_postgres.rollbacks == 0
   and .import_postgres.activity.idle_in_transaction == 0
    and .import_postgres.activity.client_active_over_five_seconds == 0
    and .import_postgres.activity.unknown_active_over_five_seconds == 0
    and ((.import_postgres.activity.autovacuum_active_over_five_seconds | type) == "number")
    and .import_postgres.activity.autovacuum_active_over_five_seconds >= 0
    and .import_postgres.activity.autovacuum_active_over_five_seconds
      == (.import_postgres.activity.autovacuum_active_over_five_seconds | floor)
    and .import_postgres.activity.active_over_five_seconds
      == (.import_postgres.activity.client_active_over_five_seconds
        + .import_postgres.activity.autovacuum_active_over_five_seconds
        + .import_postgres.activity.unknown_active_over_five_seconds)
    and .import_postgres.phase_attribution.schema
      == "vpsman-postgres-measured-phase-attribution/v2"
    and .import_postgres.activity == .import_postgres.phase_attribution.activity
    and ((.elapsed_ms | type) == "number")
    and .elapsed_ms > 0
	   and .elapsed_ms <= $import_max_elapsed_ms
	   and ((.performance | type) == "object")
	   and .performance.scope
	     == "repository_import_plus_job_completion_persistence"
	   and ((.performance.rows_per_second | type) == "number")
   and ((.performance.clients_per_second | type) == "number")
    and (.performance.rows_per_second | isfinite)
    and (.performance.clients_per_second | isfinite)
    and .performance.rows_per_second >= 0
    and .performance.clients_per_second >= 0
  and .performance.rows_per_second >= $import_min_rows_per_second
   and .performance.rows_per_second
     >= (.raw_rows.total * 1000 / $import_max_elapsed_ms)
   and .performance.clients_per_second >= $import_min_clients_per_second
    and ((.performance.rows_per_second * .elapsed_ms / 1000
      - .raw_rows.total) | fabs <= 1)
    and ((.performance.clients_per_second * .elapsed_ms / 1000
      - .client_count) | fabs <= 0.001)
   and (.import_finished_unix_ms - .import_started_unix_ms) > 0
    and (.import_finished_unix_ms - .import_started_unix_ms) <= $import_max_elapsed_ms
   and .successful_imports_per_client == 2
    and .production_importer_attempts.successful_attempts == 240
    and .production_importer_attempts.injected_failed_attempts == 1
    and .production_importer_attempts.total_attempts == 241
    and .production_importer_attempts.clients_with_two_total_attempts == 119
    and .production_importer_attempts.clients_with_three_total_attempts == 1
    and .production_importer_attempts.successful_attempts_with_job_lineage == 240
    and .production_importer_attempts.failed_attempts_with_job_lineage == 0
    and .import_finished_unix_ms < .reimport.started_unix_ms
    and .epoch_reset_semantics.imported_rows > 0
    and .epoch_reset_semantics.imported_rows
      == .epoch_reset_semantics.imported_rows_at_epoch_zero
    and .epoch_reset_semantics.hourly_rx_resets == 0
    and .epoch_reset_semantics.hourly_tx_resets == 0
    and .reimport.normal_rerun_on_existing_database == true
    and .reimport.manual_delete_before_rerun == false
    and .reimport.client_count == 120
    and .reimport.failed_clients == 0
    and .reimport.clean_ledger_streams == 120
    and .reimport.hourly_usage_parity.clean_streams == 120
    and .reimport.hourly_usage_parity.total_streams == 120
    and .reimport.hourly_usage_parity.mismatch_rows == 0
    and .reimport.hourly_usage_parity.raw_oracle_rows
      == .reimport.hourly_usage_parity.materialized_rows
    and .reimport.provenance.jobs == 120
    and .reimport.provenance.completed_targets == 120
    and .reimport.provenance.outputs == 240
    and .reimport.provenance.invalid_lineages == 0
    and .reimport.live_successor_boundaries.total == 120
    and .reimport.live_successor_boundaries.timestamp_preserved == 120
    and .reimport.live_successor_boundaries.counter_values_preserved == 120
    and .reimport.live_successor_boundaries.epoch_advanced_once == 120
    and .reimport.expected_bytes == .reimport.observed_bytes
   and .reimport.expected_bytes == .expected_bytes
   and .reimport.raw_rows.hard_max_per_client == 47520
    and ((.reimport.raw_rows.expected_per_client | type) == "number")
    and ((.reimport.raw_rows.expected_total | type) == "number")
    and .reimport.raw_rows.expected_per_client > 0
    and .reimport.raw_rows.expected_per_client
      == (.reimport.raw_rows.expected_per_client | floor)
    and .reimport.raw_rows.expected_total
      == (.reimport.raw_rows.expected_total | floor)
    and .reimport.raw_rows.expected_per_client
      <= .reimport.raw_rows.hard_max_per_client
    and .reimport.raw_rows.expected_total
      == (.reimport.raw_rows.expected_per_client * .reimport.client_count)
    and .reimport.raw_rows.expected_per_client
      == .raw_rows.expected_per_client
    and .reimport.raw_rows.expected_total == .raw_rows.expected_total
    and .reimport.raw_rows.min_per_client
      == .reimport.raw_rows.expected_per_client
    and .reimport.raw_rows.max_per_client
      == .reimport.raw_rows.expected_per_client
    and .reimport.raw_rows.total == .reimport.raw_rows.expected_total
   and .reimport.raw_rows.max_per_client <= .reimport.raw_rows.hard_max_per_client
    and .reimport.rollup_rows.max_per_client
      <= .reimport.rollup_rows.hard_max_per_client
    and .reimport.epoch_reset_semantics.imported_rows > 0
    and .reimport.epoch_reset_semantics.imported_rows
      == .reimport.epoch_reset_semantics.imported_rows_at_epoch_zero
    and .reimport.epoch_reset_semantics.hourly_rx_resets == 0
    and .reimport.epoch_reset_semantics.hourly_tx_resets == 0
    and .reimport.postgres.temporary_files == 0
    and .reimport.postgres.temporary_bytes == 0
    and .reimport.postgres.deadlocks == 0
    and .reimport.postgres.rollbacks == 0
   and .reimport.postgres.activity.idle_in_transaction == 0
    and .reimport.postgres.activity.client_active_over_five_seconds == 0
    and .reimport.postgres.activity.unknown_active_over_five_seconds == 0
    and ((.reimport.postgres.activity.autovacuum_active_over_five_seconds | type) == "number")
    and .reimport.postgres.activity.autovacuum_active_over_five_seconds >= 0
    and .reimport.postgres.activity.autovacuum_active_over_five_seconds
      == (.reimport.postgres.activity.autovacuum_active_over_five_seconds | floor)
    and .reimport.postgres.activity.active_over_five_seconds
      == (.reimport.postgres.activity.client_active_over_five_seconds
        + .reimport.postgres.activity.autovacuum_active_over_five_seconds
        + .reimport.postgres.activity.unknown_active_over_five_seconds)
    and .reimport.postgres.phase_attribution.schema
      == "vpsman-postgres-measured-phase-attribution/v2"
    and .reimport.postgres.activity == .reimport.postgres.phase_attribution.activity
    and ((.reimport.elapsed_ms | type) == "number")
    and .reimport.elapsed_ms > 0
	   and .reimport.elapsed_ms <= $reimport_max_elapsed_ms
	   and ((.reimport.performance | type) == "object")
	   and .reimport.performance.scope
	     == "repository_import_plus_job_completion_persistence"
	   and ((.reimport.performance.rows_per_second | type) == "number")
   and ((.reimport.performance.clients_per_second | type) == "number")
    and (.reimport.performance.rows_per_second | isfinite)
    and (.reimport.performance.clients_per_second | isfinite)
    and .reimport.performance.rows_per_second >= 0
    and .reimport.performance.clients_per_second >= 0
  and .reimport.performance.rows_per_second >= $reimport_min_rows_per_second
   and .reimport.performance.rows_per_second
     >= (.reimport.raw_rows.total * 1000 / $reimport_max_elapsed_ms)
   and .reimport.performance.clients_per_second >= $reimport_min_clients_per_second
    and ((.reimport.performance.rows_per_second * .reimport.elapsed_ms / 1000
      - .reimport.raw_rows.total) | fabs <= 1)
    and ((.reimport.performance.clients_per_second * .reimport.elapsed_ms / 1000
      - .reimport.client_count) | fabs <= 0.001)
   and (.reimport.finished_unix_ms - .reimport.started_unix_ms) > 0
    and (.reimport.finished_unix_ms - .reimport.started_unix_ms) <= $reimport_max_elapsed_ms
   and .atomic_imported_only_replacement.intentional_insert_failure_observed == true
    and .atomic_imported_only_replacement.failure_fingerprint_unchanged == true
    and .atomic_imported_only_replacement.failure_left_idle_in_transaction == 0
    and .atomic_imported_only_replacement.previous_import_rows_remaining == 0
    and .atomic_imported_only_replacement.logical_fingerprint_preserved_after_successful_rerun
      == true
    and .atomic_imported_only_replacement.initial_jobs_retained_for_audit == 120
    and .atomic_imported_only_replacement.reimport_jobs_retained_for_audit == 120
    and .atomic_imported_only_replacement.total_completed_job_history == 240
  ' "$artifact_dir/import-report.json" >/dev/null \
    || die "120-client import/reimport report failed its exactness/boundedness gate"
  local import_started_ms
  local import_finished_ms
  import_started_ms="$(jq -er '.import_started_unix_ms' "$artifact_dir/import-report.json")"
  import_finished_ms="$(jq -er '.import_finished_unix_ms' "$artifact_dir/import-report.json")"
  [[ "$import_started_ms" =~ ^[0-9]+$ && "$import_finished_ms" =~ ^[0-9]+$ \
    && "$import_finished_ms" -gt "$import_started_ms" ]] \
    || die "import report contains invalid phase timestamps"
  filter_cpu_window \
    "$import_test_cpu" \
    "$import_cpu" \
    "$((import_started_ms * 1000000))" \
    "$((import_finished_ms * 1000000))"
  cpu_summary "$import_cpu" >"$artifact_dir/import-cpu-summary.json"
  assert_import_activity_gate "$import_cpu"
  local reimport_started_ms
  local reimport_finished_ms
  reimport_started_ms="$(jq -er '.reimport.started_unix_ms' "$artifact_dir/import-report.json")"
  reimport_finished_ms="$(jq -er '.reimport.finished_unix_ms' "$artifact_dir/import-report.json")"
  [[ "$reimport_started_ms" =~ ^[0-9]+$ && "$reimport_finished_ms" =~ ^[0-9]+$ \
    && "$reimport_finished_ms" -gt "$reimport_started_ms" ]] \
    || die "reimport report contains invalid phase timestamps"
  filter_cpu_window \
    "$import_test_cpu" \
    "$reimport_cpu" \
    "$((reimport_started_ms * 1000000))" \
    "$((reimport_finished_ms * 1000000))"
  cpu_summary "$reimport_cpu" >"$artifact_dir/reimport-cpu-summary.json"
  assert_import_activity_gate "$reimport_cpu"
  docker logs \
    --since "$(jq -er '.import_started_at' "$artifact_dir/import-report.json")" \
    --until "$(jq -er '.import_finished_at' "$artifact_dir/import-report.json")" \
    "$container_name" >"$artifact_dir/import-postgres.log" 2>&1
  assert_postgres_window_log \
    "$artifact_dir/import-postgres.log" \
    "import window" \
    60000 \
    "$artifact_dir/import-postgres-log-summary.json"
  assert_zero_postgres_temp_log \
    "$artifact_dir/import-postgres-log-summary.json" \
    "import window"
  docker logs \
    --since "$(jq -er '.reimport.started_at' "$artifact_dir/import-report.json")" \
    --until "$(jq -er '.reimport.finished_at' "$artifact_dir/import-report.json")" \
    "$container_name" >"$artifact_dir/reimport-postgres.log" 2>&1
  assert_postgres_window_log \
    "$artifact_dir/reimport-postgres.log" \
    "reimport window" \
    60000 \
    "$artifact_dir/reimport-postgres-log-summary.json"
  assert_zero_postgres_temp_log \
    "$artifact_dir/reimport-postgres-log-summary.json" \
    "reimport window"

  # The API remains quiesced and the importer process has exited.  Start a
  # fresh statement-statistics epoch for retained-history seed attribution;
  # pg_stat_database is intentionally never reset here.
  reset_postgres_phase_statistics
  local retained_seed_started_at
  local retained_seed_finished_at
  local retained_seed_stop="$artifact_dir/retained-seed-cpu.stop"
  local retained_seed_cpu="$artifact_dir/retained-seed-postgres-cpu.tsv"
  retained_seed_started_at="$(date -u --iso-8601=ns)"
  rm -f "$retained_seed_stop"
  sample_container_cpu "$retained_seed_cpu" "$retained_seed_stop" "$online_cpus" &
  local retained_seed_sampler_pid="$!"
  active_sampler_pid="$retained_seed_sampler_pid"
  active_sampler_stop="$retained_seed_stop"
  run_managed_process_group \
    "$RETAINED_FIXTURE_WALL_TIMEOUT_SECS" \
    "five-year retained-telemetry fixture and ANALYZE" \
    "$artifact_dir/retained-fixture.log" \
    env PGAPPNAME=vpsman-pressure-retained-seed PGPASSWORD="$postgres_password" psql \
      -X -qAt -v ON_ERROR_STOP=1 \
      -h 127.0.0.1 -p "$postgres_port" -U vpsman -d vpsman \
      -f "$RETAINED_FIXTURE"
  retained_seed_finished_at="$(date -u --iso-8601=ns)"
  touch "$retained_seed_stop"
  wait "$retained_seed_sampler_pid"
  active_sampler_pid="0"
  active_sampler_stop=""
  rm -f "$retained_seed_stop"
  cpu_summary "$retained_seed_cpu" >"$artifact_dir/retained-seed-cpu-summary.json"
  { rg '^\{' "$artifact_dir/retained-fixture.log" || true; } \
    | tail -n 1 >"$artifact_dir/retained-fixture.json"
  jq -e '
    .schema == "vpsman-five-year-retained-fixture/v1"
    and .rollup_rows_per_stream >= 9952
    and .rollup_rows_per_stream <= 9967
    and .represented_minutes_per_stream >= 2628000
    and .represented_minutes_per_stream <= 2629439
    and .raw_resource_rows_per_client == 10080
    and .raw_ping_rows_per_client == 10080
    and .network_observation_exact_rows_per_stream == 552
    and .network_observation_rollup_rows_per_stream >= 7192
    and .network_observation_rollup_rows_per_stream <= 7203
    and .network_observation_represented_checks_per_stream >= 525600
    and .network_observation_represented_checks_per_stream <= 525887
    and .system_metric_series == 50
    and ([.tier_rows_per_stream | keys[]]
      | sort) == (["60", "300", "1800", "3600", "10800", "21600", "86400"] | sort)
  ' "$artifact_dir/retained-fixture.json" >/dev/null \
    || die "five-year retained fixture did not emit its exact expected shape"
  docker logs --since "$retained_seed_started_at" --until "$retained_seed_finished_at" \
    "$container_name" >"$artifact_dir/retained-seed-postgres.log" 2>&1
  assert_postgres_window_log \
    "$artifact_dir/retained-seed-postgres.log" \
    "retained seed window" \
    1800000 \
    "$artifact_dir/retained-seed-postgres-log-summary.json"
  capture_pg_stat_statements \
    "retained_seed" "$artifact_dir/retained-seed-pg-stat-statements.json"

  PGOPTIONS="$RETAINED_REPORT_PGOPTIONS" \
    psql_proof -qAt -f "$RETAINED_REPORT_SQL" \
    >"$artifact_dir/retained-report-before-maintenance.json"
  PGOPTIONS="$RETAINED_SEMANTIC_PGOPTIONS" \
    psql_proof -qAt -f "$RETAINED_SEMANTIC_SQL" \
    >"$artifact_dir/retained-semantic-hashes-before-maintenance.json"
  jq -e '
    .schema == "vpsman-five-year-retained-report/v1"
    and .raw.resource_rows == 1209600
    and .raw.counter_fact_rows == 1209600
    and .raw.ping_fact_rows == 1209600
    and .resource.streams == 120
    and .resource.min_rows_per_stream == .resource.max_rows_per_stream
    and .resource.min_rows_per_stream >= 9952
    and .resource.max_rows_per_stream <= 9967
    and .network_rates.streams == 120
    and .ping.streams == 120
    and .ping.current_rows == 120
    and .network_observations.streams == 120
    and .network_observations.latest_rows == 120
    and .network_observations.min_exact_rows_per_stream == 552
    and .network_observations.max_exact_rows_per_stream == 552
    and .network_observations.min_rollup_rows_per_stream
      == .network_observations.max_rollup_rows_per_stream
    and .network_observations.min_rollup_rows_per_stream >= 7192
    and .network_observations.max_rollup_rows_per_stream <= 7203
    and .system_metrics.series == 50
    and .maintenance_eligible_source_rows.resource == 0
    and .maintenance_eligible_source_rows.network_rates == 0
    and .maintenance_eligible_source_rows.ping == 0
    and .maintenance_eligible_source_rows.system_metrics == 0
    and .maintenance_eligible_source_rows.raw_resource == 0
    and .maintenance_eligible_source_rows.raw_ping == 0
    and .maintenance_eligible_source_rows.network_observations == 0
  ' "$artifact_dir/retained-report-before-maintenance.json" >/dev/null \
    || die "five-year retained-history pre-maintenance invariants failed"
  jq -e '
    .schema == "vpsman-five-year-semantic-hashes/v1"
    and .raw_resource.streams == 120
    and .raw_counter_facts.streams == 120
    and .resource_rollups.streams == 120
    and .resource_latest.streams == 120
    and .network_rate_rollups.streams == 120
    and .raw_ping_facts.streams == 120
    and .ping_rollups.streams == 120
    and .ping_current.streams == 120
    and .network_observations.streams == 120
    and .network_observation_latest.streams == 120
    and .system_metric_rollups.streams == 50
    and .traffic_hourly_ledger.streams == 120
    and .traffic_latest_counter_epochs.streams == 120
    and all(to_entries[]; .key == "schema" or (.value.hash | test("^[0-9a-f]{32}$")))
  ' "$artifact_dir/retained-semantic-hashes-before-maintenance.json" >/dev/null \
    || die "five-year retained semantic-hash scope is incomplete"

  reset_postgres_phase_statistics
  local maintenance_started_at
  local maintenance_finished_at
  local maintenance_stop="$artifact_dir/maintenance-cpu.stop"
  local maintenance_cpu="$artifact_dir/maintenance-postgres-cpu.tsv"
  maintenance_started_at="$(date -u --iso-8601=ns)"
  rm -f "$maintenance_stop"
  sample_container_cpu "$maintenance_cpu" "$maintenance_stop" "$online_cpus" &
  local maintenance_sampler_pid="$!"
  active_sampler_pid="$maintenance_sampler_pid"
  active_sampler_stop="$maintenance_stop"
  run_managed_process_group \
    "$MAINTENANCE_PHASE_WALL_TIMEOUT_SECS" \
    "bounded full-rotation retained-history worker maintenance" \
    "$artifact_dir/maintenance-test.log" \
    env \
      GITHUB_ACTIONS=true \
      CARGO_INCREMENTAL=0 \
      CARGO_TARGET_DIR="$CARGO_TARGET_STORAGE" \
      VPSMAN_BUILD_NUMBER_DIR="$build_number_dir" \
      VPSMAN_RETAINED_HISTORY_PRESSURE=1 \
      VPSMAN_RETAINED_HISTORY_SEMANTICS_DELEGATED=1 \
      VPSMAN_RETAINED_HISTORY_PRESSURE_DATABASE_URL="${postgres_url}?application_name=vpsman-pressure-maintenance" \
      VPSMAN_RETAINED_HISTORY_PRESSURE_REPORT="$artifact_dir/maintenance-report.json" \
      cargo test -p vpsman-worker "$MAINTENANCE_TEST" -- --ignored --nocapture
  maintenance_finished_at="$(date -u --iso-8601=ns)"
  touch "$maintenance_stop"
  wait "$maintenance_sampler_pid"
  active_sampler_pid="0"
  active_sampler_stop=""
  rm -f "$maintenance_stop"
  cpu_summary "$maintenance_cpu" >"$artifact_dir/maintenance-cpu-summary.json"
  jq -e '
    .schema == "vpsman-five-year-retained-maintenance/v1"
    and .pressure_clients == 120
    and .traffic_streams == 120
    and .registry_streams == 127
    and .non_pressure_registry_streams == 7
    and .traffic_registry_clean == true
    and .cursor_reset_before_first_rotation == true
    and .traffic_stream_scan_limit == 13
    and .calls_per_full_rotation == 10
    and .completed_rotations >= 2
    and .completed_rotations <= .maximum_rotations
    and .stable_zero_write_rotation == .completed_rotations
    and .rotations[-1].mutations == 0
    and .rotations[-1].destination_conflicts == 0
    and .rotations[-1].network_observation_destination_conflicts == 0
    and .full_cursor_rotation_proven == true
    and .idempotent_empty_rotation_proven == true
    and .conservation_proven == true
    and .semantic_conservation_proven == true
    and .conservation.resource_raw_rows == 1209600
    and .conservation.counter_fact_rows == 1209600
    and .conservation.ping_fact_rows == 1209600
    and .conservation.resource_latest_rows == 120
    and .conservation.ping_current_rows == 120
    and .conservation.network_observation_latest_rows == 120
    and .conservation.clean_traffic_streams == 120
    and (
      (.semantic_conservation_delegated == true)
      or (
        .conservation.semantic_hashes.schema
          == "vpsman-five-year-semantic-hashes/v1"
        and .conservation.semantic_hashes.traffic_hourly_ledger.streams == 120
        and .conservation.semantic_hashes.traffic_latest_counter_epochs.streams == 120
      )
    )
  ' "$artifact_dir/maintenance-report.json" >/dev/null \
    || die "retained-history worker did not prove conservation and an empty full rotation"
  docker logs --since "$maintenance_started_at" --until "$maintenance_finished_at" \
    "$container_name" >"$artifact_dir/maintenance-postgres.log" 2>&1
  assert_postgres_window_log \
    "$artifact_dir/maintenance-postgres.log" \
    "maintenance window" \
    120000 \
    "$artifact_dir/maintenance-postgres-log-summary.json"
  capture_pg_stat_statements \
    "maintenance" "$artifact_dir/maintenance-pg-stat-statements.json"
  jq -e '
    .summary.temp_blks_read == 0
    and .summary.temp_blks_written == 0
    and .summary.max_exec_time_ms < 120000
  ' "$artifact_dir/maintenance-pg-stat-statements.json" >/dev/null \
    || die "retained-history maintenance spilled to temp or exceeded its statement bound"
  PGOPTIONS="$RETAINED_REPORT_PGOPTIONS" \
    psql_proof -qAt -f "$RETAINED_REPORT_SQL" \
    >"$artifact_dir/retained-report-after-maintenance.json"
  PGOPTIONS="$RETAINED_SEMANTIC_PGOPTIONS" \
    psql_proof -qAt -f "$RETAINED_SEMANTIC_SQL" \
    >"$artifact_dir/retained-semantic-hashes-after-maintenance.json"
  jq -e \
    --slurpfile before "$artifact_dir/retained-report-before-maintenance.json" '
    .schema == "vpsman-five-year-retained-report/v1"
    and .raw == $before[0].raw
    and .resource.represented_minutes_per_stream
      == $before[0].resource.represented_minutes_per_stream
    and .network_rates.represented_minutes
      == $before[0].network_rates.represented_minutes
    and .ping.represented_minutes == $before[0].ping.represented_minutes
    and .network_observations.min_represented_checks_per_stream
      == $before[0].network_observations.min_represented_checks_per_stream
    and .network_observations.max_represented_checks_per_stream
      == $before[0].network_observations.max_represented_checks_per_stream
    and .system_metrics.represented_minutes
      == $before[0].system_metrics.represented_minutes
    and .traffic.hourly_rows == $before[0].traffic.hourly_rows
    and .traffic.hourly_rx_bytes == $before[0].traffic.hourly_rx_bytes
    and .traffic.hourly_tx_bytes == $before[0].traffic.hourly_tx_bytes
    and .maintenance_eligible_source_rows.resource == 0
    and .maintenance_eligible_source_rows.network_rates == 0
    and .maintenance_eligible_source_rows.ping == 0
    and .maintenance_eligible_source_rows.system_metrics == 0
    and .maintenance_eligible_source_rows.raw_resource == 0
    and .maintenance_eligible_source_rows.raw_ping == 0
    and .maintenance_eligible_source_rows.network_observations == 0
  ' "$artifact_dir/retained-report-after-maintenance.json" >/dev/null \
    || die "retained-history worker changed represented evidence"
  jq -e \
    --slurpfile before \
      "$artifact_dir/retained-semantic-hashes-before-maintenance.json" '
    .schema == "vpsman-five-year-semantic-hashes/v1"
    and . == $before[0]
  ' "$artifact_dir/retained-semantic-hashes-after-maintenance.json" >/dev/null \
    || die "retained-history worker changed canonical monitoring semantics"

  psql_proof -q -c \
    "ANALYZE clients, vps_rule_values, telemetry_samples, telemetry_counter_facts, telemetry_rollups, telemetry_resource_latest, telemetry_network_rates, telemetry_ping_series, telemetry_ping_facts, telemetry_ping_rollups, telemetry_ping_current, network_observation_series, network_observations, network_observation_latest, network_observation_rollups, system_metric_rollups, traffic_counter_samples, traffic_counter_rollups, traffic_counter_hourly_usage, traffic_counter_hourly_usage_streams" \
    >"$artifact_dir/analyze.log" 2>&1

  # The API is needed again only for share-management and browser requests.
  # Reset the statement epoch immediately before that work, after all
  # database-only seed/maintenance phases, and never reset pg_stat_database.
  reset_postgres_phase_statistics
  "$REVIEW_HARNESS" resume-api >"$artifact_dir/api-resume.json" 2>&1
  jq -e \
    --arg run_id "$run_id" \
    '.status == "api_resumed"
      and .run_id == $run_id
      and (.api.pid | numbers)
      and .postgres_backends >= 1' \
    "$artifact_dir/api-resume.json" >/dev/null \
    || die "review harness did not confirm the exact API resume"
  api_pid="$(jq -er '.api.pid' "$artifact_dir/api-resume.json")"
  [[ "$api_pid" =~ ^[1-9][0-9]*$ ]] || die "resumed API PID is invalid"
  api_process_binary="$(readlink -e "/proc/$api_pid/exe")"
  [[ "$api_process_binary" == "$api_binary" ]] \
    || die "resumed API executable does not match the exact storage-backed build"
  api_process_binary_sha256="$(sha256sum "/proc/$api_pid/exe" | awk '{print $1}')"
  [[ "$api_process_binary_sha256" == "$api_binary_sha256" ]] \
    || die "resumed API bytes do not match the frozen-source binary"

  local share_access_token
  local share_preview_hash
  local public_pressure_client_key
  share_access_token="$(curl -fsS \
    -H 'Content-Type: application/json' \
    -d "$(jq -nc \
      --arg username "$operator_username" \
      --arg password "$operator_password" \
      '{username: $username, password: $password}')" \
    "$frontend_url/api/v1/auth/login" | jq -er '.access_token')"
  curl -fsS \
    -H "Authorization: Bearer $share_access_token" \
    -H 'Content-Type: application/json' \
    -d "$(jq -nc --arg share_id "$visible_share_id" \
      '{share_ids: [$share_id]}')" \
    "$frontend_url/api/v1/monitoring-shares/update-targets" \
    >"$artifact_dir/public-share-target-preview.json"
  share_preview_hash="$(jq -er '.preview_hash' \
    "$artifact_dir/public-share-target-preview.json")"
  curl -fsS \
    -H "Authorization: Bearer $share_access_token" \
    -H 'Content-Type: application/json' \
    -d "$(jq -nc \
      --arg share_id "$visible_share_id" \
      --arg preview_hash "$share_preview_hash" \
      '{share_ids: [$share_id], preview_hash: $preview_hash, confirmed: true}')" \
    "$frontend_url/api/v1/monitoring-shares/update-targets" \
    >"$artifact_dir/public-share-target-update.json"
  psql_proof -qAt -v "share_id=$visible_share_id" <<'SQL' \
    | grep -qx 128 \
    || die "public pressure share does not freeze all 128 visible clients"
SELECT count(*) FROM monitoring_share_targets WHERE share_id = :'share_id'::uuid
SQL
  public_pressure_client_key="$(psql_proof -qAt \
    -v "share_id=$visible_share_id" <<'SQL'
SELECT public_client_key FROM monitoring_share_targets
 WHERE share_id = :'share_id'::uuid AND client_id = 'pressure-001'
SQL
  )"
  [[ "$public_pressure_client_key" =~ ^[0-9a-f]{64}$ ]] \
    || die "public pressure share did not retain pressure-001"

  export VPSMAN_PROOF_USERNAME="$operator_username"
  export VPSMAN_PROOF_PASSWORD="$operator_password"
  export VPSMAN_PROOF_SHARE_ID="$visible_share_id"
  export VPSMAN_PROOF_SHARE_SECRET="$visible_share_secret"
  export VPSMAN_PROOF_PUBLIC_CLIENT_KEY="$public_pressure_client_key"
  local proof_history_start_unix
  local proof_history_end_unix
  proof_history_start_unix="$(date -d "$(jq -er '.history_start' \
    "$artifact_dir/retained-fixture.json")" +%s)"
  proof_history_end_unix="$(date -d "$(jq -er '.history_end' \
    "$artifact_dir/retained-fixture.json")" +%s)"
  export VPSMAN_PROOF_HISTORY_START_UNIX="$proof_history_start_unix"
  export VPSMAN_PROOF_HISTORY_END_UNIX="$proof_history_end_unix"
  local -a sessions=()
  local session
  local ordinal
  cd "$artifact_dir"
  for ordinal in 1 2 3 4 5; do
    session="vnstat-${run_id//[^a-zA-Z0-9]/}-${ordinal}"
    sessions+=("$session")
    active_browser_sessions+=("$session")
    run_managed_process_group \
      "$PLAYWRIGHT_COMMAND_WALL_TIMEOUT_SECS" \
      "Playwright session $ordinal launch" \
      "browser/$ordinal-open.txt" \
      env VPSMAN_PROOF_SESSION="$session" PLAYWRIGHT_CLI_SESSION="$session" \
        bash "$PWCLI" open "$frontend_url"
    run_managed_process_group \
      "$PLAYWRIGHT_COMMAND_WALL_TIMEOUT_SECS" \
      "Playwright session $ordinal sign-in snapshot" \
      "browser/$ordinal-sign-in-snapshot.txt" \
      env PLAYWRIGHT_CLI_SESSION="$session" bash "$PWCLI" snapshot
  done

  local api_log="$ROOT_DIR/.tmp/monitoring-real-data/current/api.log"
  local api_log_start
  api_log_start="$(stat -c %s "$api_log")"
  local browser_started_at
  browser_started_at="$(date -u --iso-8601=ns)"
  local browser_stop="$artifact_dir/browser-cpu.stop"
  local browser_cpu="$artifact_dir/browser-postgres-cpu.tsv"
  rm -f "$browser_stop"
  sample_container_cpu "$browser_cpu" "$browser_stop" "$online_cpus" &
  local browser_sampler_pid="$!"
  active_sampler_pid="$browser_sampler_pid"
  active_sampler_stop="$browser_stop"
  local barrier_ms=$(( $(date +%s) * 1000 + 10000 ))
  local browser_code
  read -r -d '' browser_code <<'JAVASCRIPT' || true
await (async page => {
  const barrierMs = __BARRIER_MS__;
  const fetchWallTimeoutMs = __FETCH_WALL_TIMEOUT_MS__;
  const session = process.env.VPSMAN_PROOF_SESSION ?? "anonymous-session";
  const historyStartUnix = Number(process.env.VPSMAN_PROOF_HISTORY_START_UNIX);
  const historyEndUnix = Number(process.env.VPSMAN_PROOF_HISTORY_END_UNIX);
  if (!Number.isSafeInteger(historyStartUnix) || !Number.isSafeInteger(historyEndUnix) ||
      historyStartUnix >= historyEndUnix) {
    throw new Error("retained-history fixture range is missing or invalid");
  }
  page.setDefaultTimeout(30000);
  page.setDefaultNavigationTimeout(30000);
  const endpoint = request => {
    const url = new URL(request.url());
    if (url.pathname === "/api/v1/home/snapshot") return "home_snapshot";
    if (url.pathname === "/api/v1/monitoring/cards") return "monitoring_cards";
    if (url.pathname === "/api/v1/dashboard/overview") return "dashboard_overview";
    if (url.pathname === "/api/v1/system/dashboard") return "system_dashboard";
    if (url.pathname === "/api/v1/clients/pressure-001/monitoring") return "client_detail";
    if (url.pathname === "/api/v1/network/observation-trends") return "network_trends";
    if (/^\/api\/v1\/public\/monitoring-shares\/[^/]+\/bootstrap$/.test(url.pathname)) {
      return "public_bootstrap";
    }
    if (/^\/api\/v1\/public\/monitoring-shares\/[^/]+\/data$/.test(url.pathname)) {
      return "public_detail";
    }
    if (url.pathname === "/api/v1/fleet/snapshot") {
      return url.searchParams.get("mode") === "live" ? "fleet_live" : "fleet_full";
    }
    return null;
  };
  const records = [];
  const controlled = [];
  const pending = new Map();
  const active = {};
  const maxInFlight = {};
  const failures = [];
  const pageErrors = [];
  const consoleErrors = [];
  const retainedCoverage = {};
  page.on("pageerror", error => pageErrors.push(error.message));
  page.on("console", message => {
    if (message.type() === "error") consoleErrors.push(message.text());
  });
  page.on("request", request => {
    const key = endpoint(request);
    if (!key) return;
    const controlledWave = request.headers()["x-vpsman-benchmark-wave"] ?? null;
    const record = {endpoint: key, controlled_wave: controlledWave, started_at_ms: Date.now(), finished_at_ms: null, status: null, duration_ms: null, failure: null};
    records.push(record);
    pending.set(request, record);
    active[key] = (active[key] ?? 0) + 1;
    maxInFlight[key] = Math.max(maxInFlight[key] ?? 0, active[key]);
  });
  page.on("response", response => {
    const record = pending.get(response.request());
    if (record) record.status = response.status();
  });
  page.on("requestfinished", request => {
    const record = pending.get(request);
    if (!record) return;
    record.finished_at_ms = Date.now();
    record.duration_ms = record.finished_at_ms - record.started_at_ms;
    active[record.endpoint] -= 1;
    pending.delete(request);
  });
  page.on("requestfailed", request => {
    const record = pending.get(request);
    if (!record) return;
    record.finished_at_ms = Date.now();
    record.duration_ms = record.finished_at_ms - record.started_at_ms;
    record.failure = request.failure()?.errorText ?? "request failed";
    active[record.endpoint] -= 1;
    pending.delete(request);
  });
  const waitUntil = async absoluteMs => {
    await page.waitForTimeout(Math.max(0, absoluteMs - Date.now()));
  };
  const fetchJson = async (endpointName, path) => {
    const controlledRecord = {endpoint: endpointName, started_at_ms: Date.now(), finished_at_ms: null};
    try {
      return await page.evaluate(async ({requestPath, endpointName, fetchWallTimeoutMs}) => {
        const token = window.localStorage.getItem("vpsman.accessToken");
        if (!token) throw new Error("access token missing for synchronized browser wave");
        const controller = new AbortController();
        const timer = window.setTimeout(() => controller.abort(), fetchWallTimeoutMs);
        try {
          const response = await window.fetch(requestPath, {
            signal: controller.signal,
            headers: {
              Authorization: `Bearer ${token}`,
              "X-Vpsman-Benchmark-Wave": endpointName,
            },
          });
          const payload = await response.json();
          if (!response.ok) throw new Error(`${requestPath} returned ${response.status}`);
          return payload;
        } finally {
          window.clearTimeout(timer);
        }
      }, {requestPath: path, endpointName, fetchWallTimeoutMs});
    } finally {
      controlledRecord.finished_at_ms = Date.now();
      controlled.push(controlledRecord);
    }
  };
  const fetchPublicJson = async (endpointName, path, visitorId = null) => {
    const controlledRecord = {endpoint: endpointName, started_at_ms: Date.now(), finished_at_ms: null};
    const shareSecret = process.env.VPSMAN_PROOF_SHARE_SECRET ?? "";
    if (!shareSecret) throw new Error("public share secret missing for synchronized browser wave");
    try {
      return await page.evaluate(async ({requestPath, endpointName, fetchWallTimeoutMs, shareSecret, visitorId}) => {
        const controller = new AbortController();
        const timer = window.setTimeout(() => controller.abort(), fetchWallTimeoutMs);
        try {
          const response = await window.fetch(requestPath, {
            credentials: "same-origin",
            signal: controller.signal,
            headers: {
              "X-Vpsman-Benchmark-Wave": endpointName,
              "x-vpsman-share-token": shareSecret,
              ...(visitorId ? {"x-vpsman-share-visitor": visitorId} : {}),
            },
          });
          const payload = await response.json();
          if (!response.ok) throw new Error(`${requestPath} returned ${response.status}`);
          return payload;
        } finally {
          window.clearTimeout(timer);
        }
      }, {requestPath: path, endpointName, fetchWallTimeoutMs, shareSecret, visitorId});
    } finally {
      controlledRecord.finished_at_ms = Date.now();
      controlled.push(controlledRecord);
    }
  };
  const requireFullHorizon = (name, rows, timestampField = "bucket_start") => {
    const ranges = Array.isArray(rows)
      ? rows.map(row => {
          const start = Date.parse(row?.[timestampField]) / 1000;
          const bucketSecs = Number(row?.bucket_secs ?? 0);
          const latest = Date.parse(row?.latest_observed_at ?? row?.latest_checked_at ??
            row?.checked_at ?? row?.observed_at) / 1000;
          return {
            start,
            end: Math.max(
              start + (Number.isFinite(bucketSecs) ? bucketSecs : 0),
              Number.isFinite(latest) ? latest : start
            ),
          };
        }).filter(range => Number.isFinite(range.start) && Number.isFinite(range.end))
      : [];
    const oldestUnix = ranges.length > 0 ? Math.min(...ranges.map(range => range.start)) : null;
    const newestUnix = ranges.length > 0 ? Math.max(...ranges.map(range => range.end)) : null;
    retainedCoverage[name] = {
      rows: Array.isArray(rows) ? rows.length : 0,
      oldest_unix: oldestUnix,
      newest_unix: newestUnix,
    };
    const alignmentToleranceSecs = 2 * 24 * 60 * 60;
    if (oldestUnix === null || oldestUnix > historyStartUnix + alignmentToleranceSecs ||
        newestUnix === null || newestUnix < historyEndUnix - alignmentToleranceSecs) {
      failures.push(`${name} did not span the fixture's five-year retained horizon`);
    }
  };
  const requireFullHorizonPerSeries = (
    name, seriesRows, expectedSeries, timestampField = "bucket_start"
  ) => {
    const alignmentToleranceSecs = 2 * 24 * 60 * 60;
    const coverage = seriesRows.map(({key, rows}) => {
      const ranges = rows.map(row => {
        const start = Date.parse(row?.[timestampField]) / 1000;
        const bucketSecs = Number(row?.bucket_secs ?? 0);
        const latest = Date.parse(row?.latest_observed_at ?? row?.latest_checked_at ??
          row?.checked_at ?? row?.observed_at) / 1000;
        return {
          start,
          end: Math.max(
            start + (Number.isFinite(bucketSecs) ? bucketSecs : 0),
            Number.isFinite(latest) ? latest : start
          ),
        };
      }).filter(range => Number.isFinite(range.start) && Number.isFinite(range.end));
      return {
        key,
        rows: rows.length,
        oldest_unix: ranges.length > 0
          ? Math.min(...ranges.map(range => range.start)) : null,
        newest_unix: ranges.length > 0
          ? Math.max(...ranges.map(range => range.end)) : null,
      };
    });
    const fullHorizonSeries = coverage.filter(item =>
      item.oldest_unix !== null &&
      item.oldest_unix <= historyStartUnix + alignmentToleranceSecs &&
      item.newest_unix !== null &&
      item.newest_unix >= historyEndUnix - alignmentToleranceSecs
    ).length;
    retainedCoverage[name] = {
      rows: coverage.reduce((total, item) => total + item.rows, 0),
      series: coverage.length,
      full_horizon_series: fullHorizonSeries,
      oldest_unix: coverage.length > 0
        ? Math.min(...coverage.map(item => item.oldest_unix ?? Infinity)) : null,
      newest_unix: coverage.length > 0
        ? Math.max(...coverage.map(item => item.newest_unix ?? -Infinity)) : null,
    };
    if (coverage.length !== expectedSeries || fullHorizonSeries !== expectedSeries) {
      failures.push(`${name} did not return ${expectedSeries} complete five-year series`);
    }
  };

  await page.getByLabel("Username").fill(process.env.VPSMAN_PROOF_USERNAME ?? "");
  await page.getByLabel("Password").fill(process.env.VPSMAN_PROOF_PASSWORD ?? "");
  await waitUntil(barrierMs);
  await page.getByRole("button", {name: "Sign in"}).click();
  await page.locator(".shell").waitFor({state: "visible", timeout: 30000});
  await page.locator(".vpsMonitorCard").first().waitFor({state: "visible", timeout: 30000});

  await waitUntil(barrierMs + 20_000);
  const live = await fetchJson("fleet_live", "/api/v1/fleet/snapshot?mode=live");
  if (!Array.isArray(live?.agents?.data) || live.agents.data.length !== 128) {
    failures.push("fleet live did not return 128 agents");
  }
  await waitUntil(barrierMs + 30_000);
  const full = await fetchJson("fleet_full", "/api/v1/fleet/snapshot?mode=full");
  if (!Array.isArray(full?.agents?.data) || full.agents.data.length !== 128) {
    failures.push("fleet full did not return 128 agents");
  }
  await waitUntil(barrierMs + 40_000);
  const cards = await fetchJson("monitoring_cards", "/api/v1/monitoring/cards?limit=1000&offset=0");
  if (cards?.total !== 128 || cards?.items?.length !== 128) {
    failures.push("monitoring cards did not return 128 agents");
  }
  await waitUntil(barrierMs + 50_000);
  const overview = await fetchJson("dashboard_overview", "/api/v1/dashboard/overview?group_by=labels&resource_metric=cpu_load&scope_kind=all&window=1d&chart_points=340");
  if (!overview || typeof overview !== "object") {
    failures.push("dashboard overview returned no object");
  }
  const startUnix = historyStartUnix;
  const endUnix = historyEndUnix;
  await waitUntil(barrierMs + 60_000);
  const detail = await fetchJson("client_detail", `/api/v1/clients/pressure-001/monitoring?window=custom&start_unix=${startUnix}&end_unix=${endUnix}&points=720`);
  if (!Array.isArray(detail?.resources) || detail.resources.length === 0 ||
      !Array.isArray(detail?.network) || detail.network.length === 0 ||
      !Array.isArray(detail?.ping) || detail.ping.length === 0 ||
      !Array.isArray(detail?.traffic_history) || detail.traffic_history.length === 0) {
    failures.push("five-year private detail omitted a retained family");
  } else {
    if (detail?.range?.source !== "retained" || detail.range.start_unix !== startUnix ||
        detail.range.end_unix !== endUnix) {
      failures.push("five-year private detail did not honor the fixture range");
    }
    requireFullHorizon("private_resources", detail.resources);
    requireFullHorizon("private_network", detail.network);
    requireFullHorizon("private_ping", detail.ping);
    requireFullHorizon("private_traffic", detail.traffic_history);
  }
  await waitUntil(barrierMs + 70_000);
  const system = await fetchJson("system_dashboard", "/api/v1/system/dashboard?window=all&chart_points=720");
  const pressureSystemSeries = Array.isArray(system?.series)
    ? system.series.filter(series => String(series?.metric ?? "").startsWith("pressure."))
    : [];
  if (system?.window !== "all" || pressureSystemSeries.length === 0) {
    failures.push("system dashboard omitted retained pressure metrics");
  } else {
    requireFullHorizonPerSeries(
      "system_metrics",
      pressureSystemSeries.map(series => ({
        key: String(series.metric),
        rows: Array.isArray(series?.points)
          ? series.points.map(point => ({...point, bucket_secs: system.bucket_secs})) : [],
      })),
      50
    );
  }
  await waitUntil(barrierMs + 80_000);
  const trends = await fetchJson("network_trends", "/api/v1/network/observation-trends?window=all&source=automatic&kind=tunnel_reachability&limit=10000");
  const pressureTrends = Array.isArray(trends)
    ? trends.filter(trend => String(trend?.plan_name ?? "").startsWith("pressure-history-plan-"))
    : [];
  if (pressureTrends.length === 0) {
    failures.push("network trends omitted retained automatic observations");
  } else {
    const trendsBySeries = new Map();
    for (const trend of pressureTrends) {
      const key = [trend?.plan_id, trend?.client_id, trend?.peer_client_id].join(":");
      if (!trendsBySeries.has(key)) trendsBySeries.set(key, []);
      trendsBySeries.get(key).push(trend);
    }
    requireFullHorizonPerSeries(
      "network_observations",
      [...trendsBySeries.entries()].map(([key, rows]) => ({key, rows})),
      120
    );
  }
  const shareId = process.env.VPSMAN_PROOF_SHARE_ID ?? "";
  const publicClientKey = process.env.VPSMAN_PROOF_PUBLIC_CLIENT_KEY ?? "";
  await waitUntil(barrierMs + 90_000);
  const publicBootstrap = await fetchPublicJson("public_bootstrap", `/api/v1/public/monitoring-shares/${encodeURIComponent(shareId)}/bootstrap`);
  if (!publicBootstrap?.share || publicBootstrap.share.target_count !== 128) {
    failures.push("public share bootstrap did not expose the frozen 128-client scope");
  }
  await waitUntil(barrierMs + 100_000);
  const publicDetail = await fetchPublicJson("public_detail", `/api/v1/public/monitoring-shares/${encodeURIComponent(shareId)}/data?client_key=${encodeURIComponent(publicClientKey)}&window=custom&start_unix=${startUnix}&end_unix=${endUnix}&points=720&limit=1000&offset=0`, publicBootstrap?.visitor_id ?? null);
  if (publicDetail?.total !== 128 ||
      !Array.isArray(publicDetail?.detail?.resources) || publicDetail.detail.resources.length === 0 ||
      !Array.isArray(publicDetail?.detail?.network) || publicDetail.detail.network.length === 0 ||
      !Array.isArray(publicDetail?.detail?.ping) || publicDetail.detail.ping.length === 0 ||
      !Array.isArray(publicDetail?.detail?.traffic) || publicDetail.detail.traffic.length === 0) {
    failures.push("five-year public detail omitted a retained family");
  } else {
    if (publicDetail?.detail?.range?.source !== "retained" ||
        publicDetail.detail.range.start_unix !== startUnix ||
        publicDetail.detail.range.end_unix !== endUnix) {
      failures.push("five-year public detail did not honor the fixture range");
    }
    requireFullHorizon("public_resources", publicDetail.detail.resources);
    requireFullHorizon("public_network", publicDetail.detail.network);
    requireFullHorizon("public_ping", publicDetail.detail.ping);
    requireFullHorizon("public_traffic", publicDetail.detail.traffic);
  }
  await waitUntil(barrierMs + 130_000);
  await page.waitForTimeout(1000);
  const visibleErrors = await page.locator(".workspaceRouteError,.panelError").count();
  if (visibleErrors !== 0) failures.push(`visible errors: ${visibleErrors}`);
  for (const record of records) {
    if (record.failure || record.status === null || record.status < 200 || record.status >= 300) {
      failures.push(`${record.endpoint} status=${record.status} failure=${record.failure}`);
    }
  }
  const counts = records.reduce((result, record) => {
    result[record.endpoint] = (result[record.endpoint] ?? 0) + 1;
    return result;
  }, {});
  return {
    schema: "vpsman-vnstat-browser-session/v1",
    session,
    controlled_fetch_wall_timeout_ms: fetchWallTimeoutMs,
    counts,
    max_in_flight: maxInFlight,
    records,
    controlled,
    retained_fixture_range: {start_unix: historyStartUnix, end_unix: historyEndUnix},
    retained_coverage: retainedCoverage,
    pending_relevant_requests: pending.size,
    failures,
    page_errors: pageErrors,
    console_errors: consoleErrors,
  };
})(page);
JAVASCRIPT
  browser_code="${browser_code/__BARRIER_MS__/$barrier_ms}"
  browser_code="${browser_code/__FETCH_WALL_TIMEOUT_MS__/$BROWSER_FETCH_WALL_TIMEOUT_MS}"
  local -a browser_pids=()
  local browser_phase_deadline=$((SECONDS + BROWSER_PHASE_WALL_TIMEOUT_SECS))
  for ordinal in 1 2 3 4 5; do
    session="${sessions[$((ordinal - 1))]}"
    start_managed_process_group \
      "browser/$ordinal-run.txt" \
      env VPSMAN_PROOF_SESSION="$session" PLAYWRIGHT_CLI_SESSION="$session" \
        bash "$PWCLI" run-code "$browser_code"
    browser_pids+=("$managed_process_group_pid")
  done
  local browser_pid
  for browser_pid in "${browser_pids[@]}"; do
    if ! wait_managed_process_group_until \
      "$browser_pid" "$browser_phase_deadline" \
      "five-browser synchronized run-code phase"; then
      die "five-browser synchronized run-code phase failed or exceeded its ${BROWSER_PHASE_WALL_TIMEOUT_SECS}s aggregate hard wall deadline"
    fi
  done
  local browser_finished_at
  browser_finished_at="$(date -u --iso-8601=ns)"
  touch "$browser_stop"
  wait "$browser_sampler_pid"
  active_sampler_pid="0"
  active_sampler_stop=""
  rm -f "$browser_stop"
  assert_browser_cpu_gate "$browser_cpu"
  assert_activity_gate "$browser_cpu"
  cpu_summary "$browser_cpu" >"$artifact_dir/browser-cpu-summary.json"
  psql_proof -qAt -c \
    "SELECT json_build_object(
      'commits', xact_commit,
      'rollbacks', xact_rollback,
      'temporary_files', temp_files,
      'temporary_bytes', temp_bytes,
      'deadlocks', deadlocks,
      'block_reads', blks_read,
      'buffer_hits', blks_hit,
      'returned_rows', tup_returned,
      'fetched_rows', tup_fetched,
      'active_over_five_seconds', (
        SELECT count(*) FROM pg_stat_activity
        WHERE datname = current_database() AND pid <> pg_backend_pid()
          AND state = 'active' AND query_start < clock_timestamp() - interval '5 seconds'
      ),
      'idle_in_transaction', (
        SELECT count(*) FROM pg_stat_activity
        WHERE datname = current_database() AND state LIKE 'idle in transaction%'
      )
    ) FROM pg_stat_database WHERE datname = current_database()" \
    >"$artifact_dir/browser-postgres-stats.json"
  capture_pg_stat_statements \
    "five_browser" "$artifact_dir/browser-pg-stat-statements.json"

  for ordinal in 1 2 3 4 5; do
    extract_cli_result "browser/$ordinal-run.txt" "browser/$ordinal.json"
  done
  jq -s '.' browser/{1,2,3,4,5}.json >"$artifact_dir/browser-sessions.json"
  jq -e '
    def overlaps($rows):
      ($rows | length) == 5
      and (($rows | map(.started_at_ms) | max)
        < ($rows | map(.finished_at_ms) | min));
    . as $sessions
    |
    length == 5
    and all(.[]; .schema == "vpsman-vnstat-browser-session/v1")
    and all(.[]; .controlled_fetch_wall_timeout_ms == 10000)
    and all(.[]; .failures | length == 0)
    and all(.[]; .page_errors | length == 0)
    and all(.[]; .console_errors | length == 0)
    and all(.[]; .pending_relevant_requests == 0)
    and ([.[].retained_fixture_range] | unique | length) == 1
    and all(.[];
      (.retained_fixture_range.end_unix - .retained_fixture_range.start_unix)
        >= 1825 * 24 * 60 * 60
      and (.retained_coverage | keys | sort) ==
        (["network_observations", "private_network", "private_ping",
          "private_resources", "private_traffic", "public_network",
          "public_ping", "public_resources", "public_traffic",
          "system_metrics"] | sort)
      and all(.retained_coverage[];
        .rows > 0 and .oldest_unix != null and .newest_unix != null)
      and .retained_coverage.system_metrics.series == 50
      and .retained_coverage.system_metrics.full_horizon_series == 50
      and .retained_coverage.network_observations.series == 120
      and .retained_coverage.network_observations.full_horizon_series == 120
    )
    and all(.[]; ([.records[] | select(.controlled_wave != null)] | length) == 9)
    and all(.[]; .counts.home_snapshot == 1)
    and all(.[]; .counts.monitoring_cards >= 2 and .counts.monitoring_cards <= 8)
    and all(.[]; .counts.dashboard_overview >= 2 and .counts.dashboard_overview <= 8)
    and all(.[]; .counts.fleet_live == 1)
    and all(.[]; .counts.fleet_full >= 1 and .counts.fleet_full <= 3)
    and all(.[]; .counts.client_detail == 1)
    and all(.[]; .counts.system_dashboard == 1)
    and all(.[]; .counts.network_trends == 1)
    and all(.[]; .counts.public_bootstrap == 1)
    and all(.[]; .counts.public_detail == 1)
    and all(.[]; .max_in_flight.home_snapshot <= 1)
    and all(.[]; .max_in_flight.monitoring_cards <= 1)
    and all(.[]; .max_in_flight.dashboard_overview <= 1)
    and all(.[]; .max_in_flight.fleet_live <= 1)
    and all(.[]; .max_in_flight.fleet_full <= 1)
    and all(.[]; .max_in_flight.client_detail <= 1)
    and all(.[]; .max_in_flight.system_dashboard <= 1)
    and all(.[]; .max_in_flight.network_trends <= 1)
    and all(.[]; .max_in_flight.public_bootstrap <= 1)
    and all(.[]; .max_in_flight.public_detail <= 1)
    and all(.[].records[];
      (.endpoint == "home_snapshot" and .duration_ms < 10000)
      or (.endpoint == "fleet_live" and .duration_ms < 2000)
      or (.endpoint == "client_detail" and .duration_ms < 10000)
      or (.endpoint == "public_detail" and .duration_ms < 10000)
      or (.endpoint != "home_snapshot" and .endpoint != "fleet_live"
          and .endpoint != "client_detail" and .endpoint != "public_detail"
          and .duration_ms < 5000)
    )
    and overlaps([$sessions[].records[] | select(.endpoint == "home_snapshot")])
    and all(
      ["fleet_live", "fleet_full", "monitoring_cards", "dashboard_overview",
       "client_detail", "system_dashboard", "network_trends",
       "public_bootstrap", "public_detail"][];
      . as $endpoint
      | overlaps([$sessions[].records[] | select(.controlled_wave == $endpoint)])
    )
  ' "$artifact_dir/browser-sessions.json" >/dev/null \
    || die "five-browser overlap, request-count, in-flight, status, or latency gate failed"

  local api_log_first_byte=$((api_log_start + 1))
  tail -c "+$api_log_first_byte" "$api_log" >"$artifact_dir/browser-api.log"
  docker logs --since "$browser_started_at" --until "$browser_finished_at" "$container_name" \
    >"$artifact_dir/browser-postgres.log" 2>&1
  assert_postgres_window_log \
    "$artifact_dir/browser-postgres.log" \
    "browser window" \
    5000 \
    "$artifact_dir/browser-postgres-log-summary.json"
  if rg -i \
    '(^|[^[:alpha:]])error([^[:alpha:]]|$)|deadlock detected|canceling statement|admission[^[:cntrl:]]*busy|timeout|panic|fatal|out of memory' \
    "$artifact_dir/browser-api.log" >/dev/null; then
    die "browser window logs contain a deadlock, timeout, admission-busy, panic, or fatal error"
  fi
  jq -e '
    .active_over_five_seconds == 0
    and .idle_in_transaction == 0
  ' "$artifact_dir/browser-postgres-stats.json" >/dev/null \
    || die "PostgreSQL browser-window activity counters failed"
  jq -e '
    .summary.temp_blks_read == 0
    and .summary.temp_blks_written == 0
    and .summary.max_exec_time_ms < 5000
    and .summary.calls > 0
  ' "$artifact_dir/browser-pg-stat-statements.json" >/dev/null \
    || die "five-browser pg_stat_statements history spilled to temp or exceeded five seconds"

  for ordinal in 1 2 3 4 5; do
    session="${sessions[$((ordinal - 1))]}"
    run_managed_process_group \
      "$PLAYWRIGHT_COMMAND_WALL_TIMEOUT_SECS" \
      "Playwright session $ordinal screenshot" \
      "browser/$ordinal-screenshot.txt" \
      env PLAYWRIGHT_CLI_SESSION="$session" bash "$PWCLI" screenshot
    run_managed_process_group \
      "$PLAYWRIGHT_COMMAND_WALL_TIMEOUT_SECS" \
      "Playwright session $ordinal close" \
      "browser/$ordinal-close.txt" \
      env PLAYWRIGHT_CLI_SESSION="$session" bash "$PWCLI" close
  done
  active_browser_sessions=()

  local browser_aggregate
  browser_aggregate="$(jq '
    def overlaps($rows):
      ($rows | length) == 5
      and (($rows | map(.started_at_ms) | max)
        < ($rows | map(.finished_at_ms) | min));
    . as $sessions
    |
    {
      sessions: length,
      controlled_fetch_wall_timeout_ms: (map(.controlled_fetch_wall_timeout_ms) | unique | first),
      endpoint_counts: ([.[].records[].endpoint] | group_by(.) | map({key: .[0], value: length}) | from_entries),
      max_latency_ms: ([.[].records[]] | group_by(.endpoint) | map({key: .[0].endpoint, value: (map(.duration_ms) | max)}) | from_entries),
      max_in_flight_per_session: ([.[].max_in_flight | to_entries[] | .value] | max),
      max_in_flight_per_session_by_endpoint: ([.[].max_in_flight | to_entries] | flatten | group_by(.key) | map({key: .[0].key, value: (map(.value) | max)}) | from_entries),
      retained_fixture_range: (map(.retained_fixture_range) | unique | first),
      retained_coverage_by_session: map({session, coverage: .retained_coverage}),
      true_cross_session_overlap: {
        home_snapshot: overlaps([$sessions[].records[] | select(.endpoint == "home_snapshot")]),
        fleet_live: overlaps([$sessions[].records[] | select(.controlled_wave == "fleet_live")]),
        fleet_full: overlaps([$sessions[].records[] | select(.controlled_wave == "fleet_full")]),
        monitoring_cards: overlaps([$sessions[].records[] | select(.controlled_wave == "monitoring_cards")]),
        dashboard_overview: overlaps([$sessions[].records[] | select(.controlled_wave == "dashboard_overview")]),
        client_detail: overlaps([$sessions[].records[] | select(.controlled_wave == "client_detail")]),
        system_dashboard: overlaps([$sessions[].records[] | select(.controlled_wave == "system_dashboard")]),
        network_trends: overlaps([$sessions[].records[] | select(.controlled_wave == "network_trends")]),
        public_bootstrap: overlaps([$sessions[].records[] | select(.controlled_wave == "public_bootstrap")]),
        public_detail: overlaps([$sessions[].records[] | select(.controlled_wave == "public_detail")])
      },
      failures: ([.[].failures[]] | length),
      page_errors: ([.[].page_errors[]] | length),
      console_errors: ([.[].console_errors[]] | length)
    }
  ' "$artifact_dir/browser-sessions.json")"
  strict_stop_stack
  assert_no_container_mounts_postgres_data "$postgres_data_dir"
  local completed_worktree_sha256
  completed_worktree_sha256="$(current_worktree_hash)"
  [[ "$completed_worktree_sha256" == "$frozen_worktree_sha256" ]] \
    || die "working tree changed during the measured proof"
  jq -n \
    --arg schema "vpsman-vnstat-browser-pressure/v1" \
    --arg status "passed" \
    --arg run_id "$run_id" \
    --arg captured_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg git_head "$(git -C "$ROOT_DIR" rev-parse HEAD)" \
    --arg frozen_worktree_sha256 "$frozen_worktree_sha256" \
    --arg postgres_container "$container_name" \
    --arg retained_postgres_data "$postgres_data_dir" \
    --arg retained_cargo_target_dir "$CARGO_TARGET_STORAGE" \
    --arg storage_owner_marker "$owner_marker" \
    --arg focused_evidence_mode "$focused_evidence_mode" \
    --arg focused_evidence_manifest "${FOCUSED_EVIDENCE_MANIFEST#"$ROOT_DIR/"}" \
    --arg browser_started_at "$browser_started_at" \
    --arg browser_finished_at "$browser_finished_at" \
    --arg artifact_dir "${artifact_dir#"$ROOT_DIR/"}" \
    --arg cleanup_command "./scripts/prove-vnstat-browser-pressure.sh cleanup ${artifact_dir#"$ROOT_DIR/"}/final-report.json" \
   --argjson import_phase_wall_timeout_secs "$IMPORT_PHASE_WALL_TIMEOUT_SECS" \
    --argjson import_phase_max_wall_secs "$IMPORT_PHASE_MAX_WALL_SECS" \
    --argjson reimport_phase_max_wall_secs "$REIMPORT_PHASE_MAX_WALL_SECS" \
   --argjson import_phase_max_elapsed_ms "$IMPORT_PHASE_MAX_ELAPSED_MS" \
   --argjson reimport_phase_max_elapsed_ms "$REIMPORT_PHASE_MAX_ELAPSED_MS" \
   --argjson minimum_import_rows_per_second "$MIN_IMPORT_ROWS_PER_SECOND" \
   --argjson minimum_reimport_rows_per_second "$MIN_REIMPORT_ROWS_PER_SECOND" \
    --argjson minimum_import_clients_per_second "$MIN_IMPORT_CLIENTS_PER_SECOND" \
    --argjson minimum_reimport_clients_per_second "$MIN_REIMPORT_CLIENTS_PER_SECOND" \
   --argjson playwright_command_wall_timeout_secs "$PLAYWRIGHT_COMMAND_WALL_TIMEOUT_SECS" \
    --argjson browser_phase_wall_timeout_secs "$BROWSER_PHASE_WALL_TIMEOUT_SECS" \
    --argjson browser_fetch_wall_timeout_ms "$BROWSER_FETCH_WALL_TIMEOUT_MS" \
    --argjson exact_source_build "$(jq -c . "$artifact_dir/frozen-source-binary.json")" \
    --argjson api_quiesce "$(jq -c . "$artifact_dir/api-quiesce.json")" \
    --argjson exact_one_reimport "$(jq -c . "$artifact_dir/exact-one-client-reimport-probe.json")" \
    --argjson exact_four_reimport "$(jq -c . "$artifact_dir/exact-four-client-reimport-probe.json")" \
    --argjson api_resume "$(jq -c . "$artifact_dir/api-resume.json")" \
    --argjson import "$(jq -c . "$artifact_dir/import-report.json")" \
    --argjson import_cpu "$(jq -c . "$artifact_dir/import-cpu-summary.json")" \
    --argjson import_postgres_log "$(jq -c . "$artifact_dir/import-postgres-log-summary.json")" \
    --argjson reimport_cpu "$(jq -c . "$artifact_dir/reimport-cpu-summary.json")" \
    --argjson reimport_postgres_log "$(jq -c . "$artifact_dir/reimport-postgres-log-summary.json")" \
    --argjson projection "$(jq -c . "$artifact_dir/pre-mutation-projection.json")" \
    --argjson retained_fixture "$(jq -c . "$artifact_dir/retained-fixture.json")" \
    --argjson retained_before "$(jq -c . "$artifact_dir/retained-report-before-maintenance.json")" \
    --argjson retained_after "$(jq -c . "$artifact_dir/retained-report-after-maintenance.json")" \
    --argjson retained_semantic_before "$(jq -c . "$artifact_dir/retained-semantic-hashes-before-maintenance.json")" \
    --argjson retained_semantic_after "$(jq -c . "$artifact_dir/retained-semantic-hashes-after-maintenance.json")" \
    --argjson retained_seed_cpu "$(jq -c . "$artifact_dir/retained-seed-cpu-summary.json")" \
    --argjson retained_seed_log "$(jq -c . "$artifact_dir/retained-seed-postgres-log-summary.json")" \
    --argjson retained_seed_statements "$(jq -c . "$artifact_dir/retained-seed-pg-stat-statements.json")" \
    --argjson maintenance "$(jq -c . "$artifact_dir/maintenance-report.json")" \
    --argjson maintenance_cpu "$(jq -c . "$artifact_dir/maintenance-cpu-summary.json")" \
    --argjson maintenance_log "$(jq -c . "$artifact_dir/maintenance-postgres-log-summary.json")" \
    --argjson maintenance_statements "$(jq -c . "$artifact_dir/maintenance-pg-stat-statements.json")" \
    --argjson postgres_observability "$(jq -c . "$artifact_dir/postgres-observability-settings.json")" \
    --argjson browser "$browser_aggregate" \
    --argjson browser_cpu "$(jq -c . "$artifact_dir/browser-cpu-summary.json")" \
    --argjson browser_postgres_log "$(jq -c . "$artifact_dir/browser-postgres-log-summary.json")" \
    --argjson browser_statements "$(jq -c . "$artifact_dir/browser-pg-stat-statements.json")" \
    --argjson postgres "$(jq -c . "$artifact_dir/browser-postgres-stats.json")" \
    --argjson retained_fixture_wall_timeout_secs "$RETAINED_FIXTURE_WALL_TIMEOUT_SECS" \
    --argjson maintenance_phase_wall_timeout_secs "$MAINTENANCE_PHASE_WALL_TIMEOUT_SECS" \
    --argjson retained_report_statement_timeout_ms "$RETAINED_REPORT_STATEMENT_TIMEOUT_MS" \
    --argjson retained_semantic_statement_timeout_ms "$RETAINED_SEMANTIC_STATEMENT_TIMEOUT_MS" \
    --argjson minimum_storage_free_bytes "$MINIMUM_STORAGE_FREE_BYTES" \
    '{
      schema: $schema,
      status: $status,
      run_id: $run_id,
      captured_at: $captured_at,
      git_head: $git_head,
      frozen_worktree_sha256: $frozen_worktree_sha256,
      postgres_container: $postgres_container,
      retained_postgres_data: $retained_postgres_data,
      retained_cargo_target_dir: $retained_cargo_target_dir,
      storage_owner_marker: $storage_owner_marker,
      focused_reimport_evidence: {
        mode: $focused_evidence_mode,
        manifest: (if $focused_evidence_manifest == "" then null else $focused_evidence_manifest end)
      },
      stack_stop_verified: true,
      artifact_dir: $artifact_dir,
      exact_source_build: $exact_source_build,
      api_quiesce: $api_quiesce,
      exact_reimport_probes: {
        one_client: $exact_one_reimport,
        four_concurrent_clients: $exact_four_reimport
      },
      api_resume: $api_resume,
      import: $import,
	     performance: {
	        schema: "vpsman-vnstat-browser-performance-acceptance/v1",
	        import_scope: $import.performance.scope,
	      import_elapsed_ms: $import.elapsed_ms,
	        import_rows: $import.raw_rows.total,
	        import_expected_rows_per_client: $import.raw_rows.expected_per_client,
	        import_expected_rows_total: $import.raw_rows.expected_total,
	        import_min_rows_per_client: $import.raw_rows.min_per_client,
	        import_max_rows_per_client: $import.raw_rows.max_per_client,
	        import_clients: $import.client_count,
	       import_rows_per_second: $import.performance.rows_per_second,
	       import_clients_per_second: $import.performance.clients_per_second,
	        reimport_scope: $import.reimport.performance.scope,
	       reimport_elapsed_ms: $import.reimport.elapsed_ms,
	        reimport_rows: $import.reimport.raw_rows.total,
	        reimport_expected_rows_per_client:
	          $import.reimport.raw_rows.expected_per_client,
	        reimport_expected_rows_total: $import.reimport.raw_rows.expected_total,
	        reimport_min_rows_per_client: $import.reimport.raw_rows.min_per_client,
	        reimport_max_rows_per_client: $import.reimport.raw_rows.max_per_client,
	        reimport_clients: $import.reimport.client_count,
       reimport_rows_per_second: $import.reimport.performance.rows_per_second,
	       reimport_clients_per_second: $import.reimport.performance.clients_per_second,
	        import_autovacuum_active_over_five_seconds_at_finish:
	          $import.import_postgres.activity.autovacuum_active_over_five_seconds,
	        reimport_autovacuum_active_over_five_seconds_at_finish:
	          $import.reimport.postgres.activity.autovacuum_active_over_five_seconds,
      import_max_elapsed_ms: $import_phase_max_elapsed_ms,
      reimport_max_elapsed_ms: $reimport_phase_max_elapsed_ms,
       import_max_wall_secs: $import_phase_max_wall_secs,
       reimport_max_wall_secs: $reimport_phase_max_wall_secs,
       minimum_import_rows_per_second: $minimum_import_rows_per_second,
       minimum_reimport_rows_per_second: $minimum_reimport_rows_per_second,
        import_rows_per_second_floor_from_wall:
          ($import.raw_rows.total * 1000 / $import_phase_max_elapsed_ms),
        reimport_rows_per_second_floor_from_wall:
          ($import.reimport.raw_rows.total * 1000 / $reimport_phase_max_elapsed_ms),
       minimum_import_clients_per_second: $minimum_import_clients_per_second,
       minimum_reimport_clients_per_second: $minimum_reimport_clients_per_second,
        import_timestamp_wall_elapsed_ms:
          ($import.import_finished_unix_ms - $import.import_started_unix_ms),
        reimport_timestamp_wall_elapsed_ms:
          ($import.reimport.finished_unix_ms - $import.reimport.started_unix_ms),
	       rows_basis: "raw_rows.total",
	        raw_row_shape_exact: true,
	        rate_consistency_tolerance: {rows: 1, clients: 0.001},
        rates_consistent_with_elapsed_and_counts: true,
       accepted: true
     },
      import_postgres_cpu: $import_cpu,
      import_postgres_log: $import_postgres_log,
      reimport_postgres_cpu: $reimport_cpu,
      reimport_postgres_log: $reimport_postgres_log,
      pre_mutation_projection: $projection,
      retained_telemetry: {
        fixture: $retained_fixture,
        before_maintenance: $retained_before,
        after_maintenance: $retained_after,
        semantic_hashes_before_maintenance: $retained_semantic_before,
        semantic_hashes_after_maintenance: $retained_semantic_after,
        seed_postgres_cpu: $retained_seed_cpu,
        seed_postgres_log: $retained_seed_log,
        seed_pg_stat_statements: $retained_seed_statements
      },
      worker_maintenance: {
        proof: $maintenance,
        postgres_cpu: $maintenance_cpu,
        postgres_log: $maintenance_log,
        pg_stat_statements: $maintenance_statements
      },
      postgres_observability: $postgres_observability,
      browser: $browser,
      browser_started_at: $browser_started_at,
      browser_finished_at: $browser_finished_at,
      browser_postgres_cpu: $browser_cpu,
      browser_postgres_log: $browser_postgres_log,
      browser_pg_stat_statements: $browser_statements,
      browser_postgres: $postgres,
      gates: {
        imported_clients_exact: 120,
        existing_database_reimported_clients_exact: 120,
        exact_one_client_reimport_probe_spill_free: true,
        exact_four_concurrent_client_reimport_probe_spill_free: true,
        api_quiesced_during_exact_and_120_client_imports: true,
        successful_imports_per_client_exact: 2,
        injected_atomicity_failure_attempts_exact: 1,
        production_importer_attempts_total_exact: 241,
        browser_sessions_exact: 5,
        browser_postgres_cpu_one_core_strictly_below_pct: 50,
        cpu_sample_interval_secs: 1,
        import_per_client_timeout_secs: 60,
       import_phase_aggregate_wall_timeout_secs: $import_phase_wall_timeout_secs,
        import_phase_performance_max_wall_secs: $import_phase_max_wall_secs,
        reimport_phase_performance_max_wall_secs: $reimport_phase_max_wall_secs,
       import_phase_performance_max_elapsed_ms: $import_phase_max_elapsed_ms,
        reimport_phase_performance_max_elapsed_ms: $reimport_phase_max_elapsed_ms,
       import_minimum_rows_per_second: $minimum_import_rows_per_second,
       reimport_minimum_rows_per_second: $minimum_reimport_rows_per_second,
        import_minimum_clients_per_second: $minimum_import_clients_per_second,
        reimport_minimum_clients_per_second: $minimum_reimport_clients_per_second,
	       import_performance_fields_present_and_bounded: true,
	       reimport_performance_fields_present_and_bounded: true,
	        import_performance_scope_exact: true,
	        reimport_performance_scope_exact: true,
	        import_raw_row_shape_exact: true,
	        reimport_raw_row_shape_exact: true,
	        import_performance_rates_consistent_with_report: true,
        reimport_performance_rates_consistent_with_report: true,
	        import_no_long_client_or_unclassified_activity: true,
	        reimport_no_long_client_or_unclassified_activity: true,
	        autovacuum_enabled_charged_to_performance_and_reported: true,
       retained_fixture_wall_timeout_secs: $retained_fixture_wall_timeout_secs,
        maintenance_phase_wall_timeout_secs: $maintenance_phase_wall_timeout_secs,
        retained_report_statement_timeout_ms: $retained_report_statement_timeout_ms,
        retained_semantic_statement_timeout_ms: $retained_semantic_statement_timeout_ms,
        minimum_storage_free_bytes_before_mutation: $minimum_storage_free_bytes,
        playwright_command_wall_timeout_secs: $playwright_command_wall_timeout_secs,
        five_browser_aggregate_wall_timeout_secs: $browser_phase_wall_timeout_secs,
        controlled_browser_fetch_wall_timeout_ms: $browser_fetch_wall_timeout_ms,
        import_postgres_statement_failure_threshold_ms: 60000,
        browser_postgres_statement_failure_threshold_ms: 5000,
        true_cross_session_overlap: true,
        every_retained_family_queried_by_five_browsers: true,
        private_and_public_five_year_detail_queried: true,
        worker_full_cursor_rotation_and_idempotent_empty_rotation: true,
        worker_conserved_represented_samples_and_hourly_traffic_bytes: true,
        historical_pg_stat_statements_captured: true,
        no_request_flood: true,
        no_http_errors: true,
        import_no_deadlocks_timeouts_or_sixty_second_statements: true,
        reimport_no_deadlocks_timeouts_temp_files_idle_transactions_or_sixty_second_statements: true,
        reimport_is_atomic_imported_only_replacement_without_manual_delete: true,
        browser_no_deadlocks_timeouts_or_five_second_statements: true
      },
      cleanup_command: $cleanup_command
    }' >"$artifact_dir/final-report.json"
  (
    cd "$artifact_dir"
    sha256sum final-report.json >final-report.sha256
  )
  trap - EXIT INT TERM
  jq . "$artifact_dir/final-report.json"
}

usage() {
  cat >&2 <<'EOF'
Usage:
  ./scripts/prove-vnstat-browser-pressure.sh run
  ./scripts/prove-vnstat-browser-pressure.sh cleanup output/playwright/vnstat-browser-pressure-<run>/final-report.json

run requires 48 GiB free and performs the opt-in, storage-backed 120-client
five-year import, retained-family maintenance, and five-browser proof. It never
removes PGDATA, the Cargo target, or artifacts. cleanup validates one passed
manifest and moves only that run's stopped PGDATA to recoverable storage trash.
EOF
}

case "${1:-}" in
  run)
    [[ "$#" == "1" ]] || die "run accepts no extra arguments"
    run_proof
    ;;
  cleanup)
    [[ "$#" == "2" ]] || die "cleanup requires exactly one final-report.json"
    require_tools awk docker jq mv readlink sha256sum
    cleanup_storage "$2"
    ;;
  *)
    usage
    exit 2
    ;;
esac
