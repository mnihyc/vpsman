#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
STATE_ROOT="$ROOT_DIR/.tmp/monitoring-real-data"
STATE_DIR="$STATE_ROOT/current"
STATE_FILE="$STATE_DIR/run.json"
SEED_SQL="$ROOT_DIR/scripts/fixtures/review-monitoring-real-data-seed.sql"
REFRESH_SQL="$ROOT_DIR/scripts/fixtures/review-monitoring-real-data-refresh.sql"
PLAYWRIGHT_SPEC="$ROOT_DIR/frontend/tests/monitoring-real-data-review.spec.ts"
STATE_VERSION="1"
EXPECTED_CLIENT_COUNT=8

run_id=""
container_name=""
postgres_port="0"
api_port="0"
frontend_port="0"
postgres_password=""
postgres_url=""
postgres_data_dir=""
cargo_target_dir=""
api_binary_sha256=""
api_url=""
frontend_url=""
api_pid="0"
frontend_pid="0"
operator_username=""
operator_password=""
internal_token=""
artifact_dir_relative=""
artifact_dir=""
visible_share_id=""
visible_share_fragment=""
hidden_share_id=""
hidden_share_fragment=""
started_at=""
owns_current_state=0
start_succeeded=0

die() {
  printf 'monitoring real-data review: %s\n' "$*" >&2
  exit 1
}

require_tools() {
  local tool
  for tool in "$@"; do
    command -v "$tool" >/dev/null 2>&1 || die "missing required tool: $tool"
  done
}

validate_port() {
  local port="$1"
  [[ "$port" =~ ^[0-9]+$ ]] || die "stored port is not numeric"
  ((port >= 41000 && port <= 41999)) || die "stored port is outside 41000-41999"
}

validate_run_identity() {
  local database_application_name
  [[ "$run_id" =~ ^review-[0-9]{8}T[0-9]{6}Z-[0-9]+$ ]] \
    || die "stored run label is invalid"
  database_application_name="vpsman-review-api-$run_id"
  ((${#database_application_name} <= 63)) \
    || die "stored run label is too long for exact PostgreSQL backend identity"
  [[ "$container_name" == "vpsman-monitoring-${run_id}-postgres" ]] \
    || die "stored Postgres container name is outside the review harness scope"
  validate_port "$postgres_port"
  validate_port "$api_port"
  validate_port "$frontend_port"
  [[ "$postgres_port" != "$api_port" && "$postgres_port" != "$frontend_port" \
    && "$api_port" != "$frontend_port" ]] || die "stored review ports overlap"
  [[ "$artifact_dir_relative" == "output/playwright/monitoring-real-data-${run_id}" ]] \
    || die "stored artifact path is outside the review harness scope"
  artifact_dir="$ROOT_DIR/$artifact_dir_relative"
  [[ "$(readlink -m "$artifact_dir")" == "$ROOT_DIR/output/playwright/monitoring-real-data-${run_id}" ]] \
    || die "resolved artifact path is outside the review harness scope"
  if [[ -n "$postgres_data_dir" ]]; then
    [[ "$postgres_data_dir" == /mnt/storage/vpsman-tmp/* ]] \
      || die "stored PostgreSQL data path is outside /mnt/storage/vpsman-tmp"
    [[ "$(basename "$postgres_data_dir")" == "postgres" \
      && "$(basename "$(dirname "$postgres_data_dir")")" == "$run_id" ]] \
      || die "stored PostgreSQL data path does not belong to run $run_id"
    [[ "$(readlink -m "$postgres_data_dir")" == "$postgres_data_dir" ]] \
      || die "stored PostgreSQL data path is not canonical"
  fi
  if [[ -n "$cargo_target_dir" ]]; then
    [[ "$cargo_target_dir" == /mnt/storage/vpsman-tmp/* ]] \
      || die "stored Cargo target path is outside /mnt/storage/vpsman-tmp"
    [[ "$(readlink -m "$cargo_target_dir")" == "$cargo_target_dir" ]] \
      || die "stored Cargo target path is not canonical"
  fi
  if [[ -n "$api_binary_sha256" ]]; then
    [[ "$api_binary_sha256" =~ ^[0-9a-f]{64}$ ]] \
      || die "stored API binary SHA-256 is invalid"
  fi
}

load_state() {
  [[ -f "$STATE_FILE" ]] || die "no monitoring review stack is recorded; run '$0 start' first"
  [[ "$(readlink -m "$STATE_FILE")" == "$STATE_ROOT/current/run.json" ]] \
    || die "review state path is invalid"
  jq -e --arg version "$STATE_VERSION" '.state_version == $version' "$STATE_FILE" >/dev/null \
    || die "review state version is missing or unsupported"

  run_id="$(jq -er '.run_id' "$STATE_FILE")"
  container_name="$(jq -er '.container_name' "$STATE_FILE")"
  postgres_port="$(jq -er '.postgres_port' "$STATE_FILE")"
  api_port="$(jq -er '.api_port' "$STATE_FILE")"
  frontend_port="$(jq -er '.frontend_port' "$STATE_FILE")"
  postgres_password="$(jq -er '.postgres_password' "$STATE_FILE")"
  postgres_url="$(jq -er '.postgres_url' "$STATE_FILE")"
  postgres_data_dir="$(jq -er '.postgres_data_dir // ""' "$STATE_FILE")"
  cargo_target_dir="$(jq -er '.cargo_target_dir // ""' "$STATE_FILE")"
  api_binary_sha256="$(jq -er '.api_binary_sha256 // ""' "$STATE_FILE")"
  api_url="$(jq -er '.api_url' "$STATE_FILE")"
  frontend_url="$(jq -er '.frontend_url' "$STATE_FILE")"
  api_pid="$(jq -er '.api_pid' "$STATE_FILE")"
  frontend_pid="$(jq -er '.frontend_pid' "$STATE_FILE")"
  operator_username="$(jq -er '.operator_username' "$STATE_FILE")"
  operator_password="$(jq -er '.operator_password' "$STATE_FILE")"
  internal_token="$(jq -er '.internal_token' "$STATE_FILE")"
  artifact_dir_relative="$(jq -er '.artifact_dir' "$STATE_FILE")"
  visible_share_id="$(jq -er '.visible_share.id' "$STATE_FILE")"
  visible_share_fragment="$(jq -er '.visible_share.fragment' "$STATE_FILE")"
  hidden_share_id="$(jq -er '.hidden_share.id' "$STATE_FILE")"
  hidden_share_fragment="$(jq -er '.hidden_share.fragment' "$STATE_FILE")"
  started_at="$(jq -er '.started_at' "$STATE_FILE")"
  validate_run_identity
}

write_state() {
  local state_tmp="$STATE_DIR/run.json.tmp"
  jq -n \
    --arg state_version "$STATE_VERSION" \
    --arg run_id "$run_id" \
    --arg container_name "$container_name" \
    --argjson postgres_port "$postgres_port" \
    --argjson api_port "$api_port" \
    --argjson frontend_port "$frontend_port" \
    --arg postgres_password "$postgres_password" \
    --arg postgres_url "$postgres_url" \
    --arg postgres_data_dir "$postgres_data_dir" \
    --arg cargo_target_dir "$cargo_target_dir" \
    --arg api_binary_sha256 "$api_binary_sha256" \
    --arg api_url "$api_url" \
    --arg frontend_url "$frontend_url" \
    --argjson api_pid "$api_pid" \
    --argjson frontend_pid "$frontend_pid" \
    --arg operator_username "$operator_username" \
    --arg operator_password "$operator_password" \
    --arg internal_token "$internal_token" \
    --arg artifact_dir "$artifact_dir_relative" \
    --arg visible_share_id "$visible_share_id" \
    --arg visible_share_fragment "$visible_share_fragment" \
    --arg hidden_share_id "$hidden_share_id" \
    --arg hidden_share_fragment "$hidden_share_fragment" \
    --arg started_at "$started_at" \
    '{
      state_version: $state_version,
      run_id: $run_id,
      container_name: $container_name,
      postgres_port: $postgres_port,
      api_port: $api_port,
      frontend_port: $frontend_port,
      postgres_password: $postgres_password,
      postgres_url: $postgres_url,
      postgres_data_dir: (if $postgres_data_dir == "" then null else $postgres_data_dir end),
      cargo_target_dir: (if $cargo_target_dir == "" then null else $cargo_target_dir end),
      api_binary_sha256: (if $api_binary_sha256 == "" then null else $api_binary_sha256 end),
      api_url: $api_url,
      frontend_url: $frontend_url,
      api_pid: $api_pid,
      frontend_pid: $frontend_pid,
      operator_username: $operator_username,
      operator_password: $operator_password,
      internal_token: $internal_token,
      artifact_dir: $artifact_dir,
      visible_share: {
        id: $visible_share_id,
        fragment: $visible_share_fragment
      },
      hidden_share: {
        id: $hidden_share_id,
        fragment: $hidden_share_fragment
      },
      started_at: $started_at
    }' >"$state_tmp"
  chmod 600 "$state_tmp"
  mv "$state_tmp" "$STATE_FILE"
}

process_matches_run() {
  local pid="$1"
  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
  kill -0 "$pid" >/dev/null 2>&1 || return 1
  [[ -r "/proc/$pid/environ" ]] || return 1
  tr '\0' '\n' <"/proc/$pid/environ" \
    | grep -Fqx "VPSMAN_REVIEW_RUN_ID=$run_id"
}

recorded_api_binary_path() {
  if [[ -n "$cargo_target_dir" ]]; then
    printf '%s/debug/vpsman-api\n' "$cargo_target_dir"
  else
    printf '%s/target/debug/vpsman-api\n' "$ROOT_DIR"
  fi
}

require_recorded_api_binary() {
  local api_binary
  local current_sha256
  local resolved_api_binary
  api_binary="$(recorded_api_binary_path)"
  [[ -f "$api_binary" && -x "$api_binary" ]] \
    || die "recorded API binary is missing or not executable: $api_binary"
  resolved_api_binary="$(readlink -e "$api_binary")" \
    || die "recorded API binary cannot be resolved: $api_binary"
  [[ "$resolved_api_binary" == "$api_binary" ]] \
    || die "recorded API binary path is not canonical: $api_binary"
  if [[ -n "$api_binary_sha256" ]]; then
    current_sha256="$(sha256sum "$api_binary" | awk '{print $1}')"
    [[ "$current_sha256" == "$api_binary_sha256" ]] \
      || die "recorded API binary bytes changed after the run was frozen"
  fi
  printf '%s\n' "$api_binary"
}

api_application_name() {
  printf 'vpsman-review-api-%s\n' "$run_id"
}

api_postgres_url() {
  local separator='?'
  [[ "$postgres_url" == *\?* ]] && separator='&'
  printf '%s%sapplication_name=%s\n' \
    "$postgres_url" "$separator" "$(api_application_name)"
}

process_environment_has() {
  local pid="$1"
  local expected="$2"
  [[ -r "/proc/$pid/environ" ]] || return 1
  local environment
  environment="$(
    {
      tr '\0' '\n' <"/proc/$pid/environ"
    } 2>/dev/null
  )" || return 1
  grep -Fqx "$expected" <<<"$environment"
}

api_process_matches_run() {
  local expected_binary_sha256
  local pid="$1"
  local expected_binary
  local process_binary
  local process_binary_sha256
  process_matches_run "$pid" || return 1
  expected_binary="$(recorded_api_binary_path)"
  [[ -x "$expected_binary" ]] || return 1
  expected_binary="$(readlink -e "$expected_binary")" || return 1
  process_binary="$(readlink -e "/proc/$pid/exe")" || return 1
  [[ "$process_binary" == "$expected_binary" ]] || return 1
  expected_binary_sha256="$(sha256sum "$expected_binary" | awk '{print $1}')" || return 1
  process_binary_sha256="$(sha256sum "/proc/$pid/exe" | awk '{print $1}')" || return 1
  [[ "$process_binary_sha256" == "$expected_binary_sha256" ]] || return 1
  if [[ -n "$api_binary_sha256" ]]; then
    [[ "$process_binary_sha256" == "$api_binary_sha256" ]] || return 1
  fi
  process_environment_has "$pid" "VPSMAN_API_BIND=127.0.0.1:$api_port" || return 1
  if [[ -n "$api_binary_sha256" ]]; then
    process_environment_has "$pid" "VPSMAN_POSTGRES_URL=$(api_postgres_url)"
  else
    process_environment_has "$pid" "VPSMAN_POSTGRES_URL=$postgres_url"
  fi
}

process_group_exists() {
  local pid="$1"
  [[ "$pid" =~ ^[1-9][0-9]*$ ]] || return 1
  kill -0 -- "-$pid" >/dev/null 2>&1
}

process_is_group_leader() {
  local pid="$1"
  local process_group
  process_group="$(ps -o pgid= -p "$pid" | tr -d '[:space:]')"
  [[ "$process_group" == "$pid" ]]
}

api_processes_for_run() {
  local environ_path
  local pid
  for environ_path in /proc/[1-9][0-9]*/environ; do
    [[ -r "$environ_path" ]] || continue
    pid="${environ_path#/proc/}"
    pid="${pid%/environ}"
    process_environment_has "$pid" "VPSMAN_REVIEW_RUN_ID=$run_id" || continue
    process_environment_has "$pid" "VPSMAN_API_BIND=127.0.0.1:$api_port" || continue
    printf '%s\n' "$pid"
  done
  return 0
}

container_matches_run() {
  local stored_label
  docker inspect "$container_name" >/dev/null 2>&1 || return 1
  stored_label="$(docker inspect \
    --format '{{ index .Config.Labels "com.vpsman.monitoring-review-run" }}' \
    "$container_name")"
  [[ "$stored_label" == "$run_id" ]]
}

stop_process_group() {
  local pid="$1"
  local attempt
  if ! kill -0 "$pid" >/dev/null 2>&1; then
    if process_group_exists "$pid"; then
      die "refusing to stop process group $pid because its recorded leader is absent"
    fi
    return
  fi
  process_matches_run "$pid" \
    || die "refusing to stop PID $pid because it no longer belongs to run $run_id"
  process_is_group_leader "$pid" \
    || die "refusing to stop PID $pid because it is not its process-group leader"
  kill -TERM -- "-$pid" >/dev/null 2>&1 || true
  for ((attempt = 0; attempt < 100; attempt += 1)); do
    process_group_exists "$pid" || return 0
    sleep 0.1
  done
  kill -KILL -- "-$pid" >/dev/null 2>&1 || true
  for ((attempt = 0; attempt < 50; attempt += 1)); do
    process_group_exists "$pid" || return 0
    sleep 0.1
  done
  die "process group $pid survived SIGTERM and SIGKILL"
}

cleanup_failed_start() {
  [[ "$start_succeeded" == "1" || "$owns_current_state" != "1" ]] && return
  set +e
  if [[ "$frontend_pid" =~ ^[1-9][0-9]*$ ]] && process_matches_run "$frontend_pid"; then
    kill -TERM -- "-$frontend_pid" >/dev/null 2>&1
    wait "$frontend_pid" >/dev/null 2>&1
  fi
  if [[ "$api_pid" =~ ^[1-9][0-9]*$ ]] && process_matches_run "$api_pid"; then
    kill -TERM -- "-$api_pid" >/dev/null 2>&1
    wait "$api_pid" >/dev/null 2>&1
  fi
  if [[ -n "$container_name" ]] && container_matches_run; then
    docker rm -f "$container_name" >/dev/null 2>&1
  fi
  if [[ -n "$artifact_dir" \
    && "$(readlink -m "$artifact_dir")" == "$ROOT_DIR/output/playwright/monitoring-real-data-${run_id}" ]]; then
    rm -rf -- "$artifact_dir"
  fi
  if [[ "$(readlink -m "$STATE_DIR")" == "$STATE_ROOT/current" ]]; then
    rm -rf -- "$STATE_DIR"
  fi
}

wait_for_postgres() {
  local deadline=$((SECONDS + 60))
  local ready_checks=0
  while ((ready_checks < 8)); do
    if docker exec "$container_name" pg_isready -U vpsman -d vpsman >/dev/null 2>&1; then
      ready_checks=$((ready_checks + 1))
    else
      ready_checks=0
    fi
    if ((SECONDS >= deadline)); then
      docker logs "$container_name" >&2 || true
      die "PostgreSQL did not become ready"
    fi
    sleep 0.25
  done
}

wait_for_http() {
  local url="$1"
  local pid="$2"
  local log_path="$3"
  local deadline=$((SECONDS + 90))
  until curl -fsS "$url" >/dev/null 2>&1; do
    if ! kill -0 "$pid" >/dev/null 2>&1; then
      sed -n '1,240p' "$log_path" >&2 || true
      die "process $pid exited before $url became ready"
    fi
    if ((SECONDS >= deadline)); then
      sed -n '1,240p' "$log_path" >&2 || true
      die "timed out waiting for $url"
    fi
    sleep 0.2
  done
}

require_live_postgres_container() {
  container_matches_run \
    || die "PostgreSQL container is absent or has the wrong run label"
  [[ "$(docker inspect --format '{{.State.Running}}' "$container_name")" == "true" ]] \
    || die "PostgreSQL container is not running"
}

api_backend_count() {
  psql_review \
    -qAt \
    -v "api_application_name=$(api_application_name)" <<'SQL'
SELECT count(*) FROM pg_stat_activity
 WHERE datname = current_database()
   AND backend_type = 'client backend'
   AND application_name = :'api_application_name'
SQL
}

wait_for_no_api_backends() {
  local backend_count
  local deadline=$((SECONDS + 15))
  while true; do
    backend_count="$(api_backend_count)"
    [[ "$backend_count" == "0" ]] && return 0
    ((SECONDS < deadline)) \
      || die "API process stopped but $backend_count tagged PostgreSQL backend(s) survived"
    sleep 0.1
  done
}

wait_for_api_backend() {
  local backend_count
  local deadline=$((SECONDS + 15))
  while true; do
    backend_count="$(api_backend_count)"
    [[ "$backend_count" =~ ^[1-9][0-9]*$ ]] && return 0
    ((SECONDS < deadline)) \
      || die "API health is up but no run-tagged PostgreSQL backend appeared"
    sleep 0.1
  done
}

launch_api_process() {
  local api_binary
  local api_log="$STATE_DIR/api.log"
  local database_url
  api_binary="$(require_recorded_api_binary)"
  database_url="$(api_postgres_url)"
  setsid env \
    "VPSMAN_REVIEW_RUN_ID=$run_id" \
    "VPSMAN_API_BIND=127.0.0.1:$api_port" \
    "VPSMAN_POSTGRES_URL=$database_url" \
    "VPSMAN_MIGRATIONS_DIR=$ROOT_DIR/migrations" \
    "VPSMAN_INTERNAL_TOKEN=$internal_token" \
    "VPSMAN_BACKUP_OBJECT_STORE_DIR=$STATE_DIR/object-store/backups" \
    "VPSMAN_SUITE_CONFIG=$STATE_DIR/no-suite.toml" \
    RUST_LOG=vpsman_api=warn \
    "$api_binary" >>"$api_log" 2>&1 </dev/null &
  api_pid="$!"
}

cleanup_failed_api_resume() {
  set +e
  if [[ "$api_pid" =~ ^[1-9][0-9]*$ ]] \
    && kill -0 "$api_pid" >/dev/null 2>&1 \
    && process_environment_has "$api_pid" "VPSMAN_REVIEW_RUN_ID=$run_id" \
    && process_environment_has "$api_pid" "VPSMAN_API_BIND=127.0.0.1:$api_port" \
    && process_is_group_leader "$api_pid"; then
    # The binary hash check is intentionally bypassed only on this failed
    # launch cleanup path: the PID and run-scoped environment still prove
    # ownership, while refusing cleanup here would leak the just-launched
    # process if it failed identity validation.
    kill -TERM -- "-$api_pid" >/dev/null 2>&1 || true
    for ((attempt = 0; attempt < 100; attempt += 1)); do
      process_group_exists "$api_pid" || break
      sleep 0.1
    done
    if process_group_exists "$api_pid"; then
      kill -KILL -- "-$api_pid" >/dev/null 2>&1 || true
    fi
  fi
  api_pid=0
}

psql_review() {
  PGAPPNAME=vpsman-pressure-control PGPASSWORD="$postgres_password" psql \
    -X \
    -v ON_ERROR_STOP=1 \
    -h 127.0.0.1 \
    -p "$postgres_port" \
    -U vpsman \
    -d vpsman \
    "$@"
}

refresh_observations() {
  psql_review -q -f "$REFRESH_SQL"
}

login_json() {
  curl -fsS \
    -H 'Content-Type: application/json' \
    -d "$(jq -nc \
      --arg username "$operator_username" \
      --arg password "$operator_password" \
      '{username: $username, password: $password, totp_code: null}')" \
    "$api_url/api/v1/auth/login"
}

assert_no_playwright_interception() {
  [[ -f "$PLAYWRIGHT_SPEC" ]] || die "Playwright review spec is missing"
  if rg -n \
    'page[.]route|context[.]route|route[.]fulfill|install[A-Za-z0-9_]*Mock|evaluate[(].*fetch' \
    "$PLAYWRIGHT_SPEC" >/dev/null; then
    die "real-data Playwright spec contains a request-interception or mock pattern"
  fi
}

current_worktree_hash() {
  {
    git diff --binary HEAD
    git status --porcelain=v1
    git ls-files --others --exclude-standard -z \
      | sort -z \
      | xargs -0 -r sha256sum
  } | sha256sum | awk '{print $1}'
}

verify_stack() {
  local token
  local cards_json
  local visible_bootstrap
  local visible_visitor
  local visible_secret
  local visible_data
  local hidden_bootstrap
  local hidden_visitor
  local hidden_secret
  local hidden_data

  [[ "$api_pid" != "0" ]] \
    || die "API is quiesced; run '$0 resume-api' before status or capture"
  [[ "$api_pid" =~ ^[1-9][0-9]*$ ]] || die "stored API PID is invalid"
  api_process_matches_run "$api_pid" \
    || die "API process is not the exact recorded binary for $run_id"
  process_matches_run "$frontend_pid" || die "frontend process is not running as $run_id"
  require_live_postgres_container
  curl -fsS "$api_url/health" >/dev/null
  curl -fsS "$frontend_url/" >/dev/null
  psql_review -qAt -c 'SELECT 1' | grep -qx '1' \
    || die "PostgreSQL query check failed"

  token="$(login_json | jq -er '.access_token')"
  cards_json="$(curl -fsS \
    -H "Authorization: Bearer $token" \
    "$api_url/api/v1/monitoring/cards?limit=1000&offset=0")"
  jq -e --argjson expected "$EXPECTED_CLIENT_COUNT" '
    .total == $expected
    and (.items | length) == $expected
    and any(.items[]; .client.display_name == "Total quota · Monthly")
    and any(.items[];
      .client.display_name == "Traffic quota exceeded"
      and .traffic.cycle_percent == 120
      and .traffic.total_bytes == 12000000000)
    and any(.items[]; .client.display_name == "RX quota · Annual")
    and any(.items[]; .traffic.reset_day == -1)
    and any(.items[]; .network_rate_expected == false)
    and any(.items[]; .primary_ping.state? == "ok")
    and any(.items[]; .primary_ping.state? == "degraded")
  ' <<<"$cards_json" >/dev/null || die "authenticated monitoring-card fixture check failed"

  visible_secret="${visible_share_fragment##*/}"
  visible_bootstrap="$(curl -fsS \
    -H "x-vpsman-share-token: $visible_secret" \
    "$api_url/api/v1/public/monitoring-shares/$visible_share_id/bootstrap")"
  visible_visitor="$(jq -er '.visitor_id' <<<"$visible_bootstrap")"
  visible_data="$(curl -fsS \
    -H "x-vpsman-share-token: $visible_secret" \
    -H "x-vpsman-share-visitor: $visible_visitor" \
    "$api_url/api/v1/public/monitoring-shares/$visible_share_id/data?limit=1000&offset=0")"
  jq -e --argjson expected "$EXPECTED_CLIENT_COUNT" '
    .total == $expected
    and (.cards | length) == $expected
    and .share.visibility.billing == true
    and any(.cards[];
      .display_name == "Traffic quota exceeded"
      and .traffic.cycle_percent == 120
      and .traffic.total_bytes == 12000000000)
    and any(.cards[]; .billing.period_code? == "y" and .billing.cycle? == "06-15")
    and any(.cards[]; .traffic.reset_day? == -1)
    and any(.cards[]; .traffic.configured == false)
    and any(.cards[]; .network.rate_expected == false)
    and any(.cards[];
      .display_name == "No primary Ping" and (has("primary_ping") | not))
  ' <<<"$visible_data" >/dev/null || die "Billing-visible public share fixture check failed"

  hidden_secret="${hidden_share_fragment##*/}"
  hidden_bootstrap="$(curl -fsS \
    -H "x-vpsman-share-token: $hidden_secret" \
    "$api_url/api/v1/public/monitoring-shares/$hidden_share_id/bootstrap")"
  hidden_visitor="$(jq -er '.visitor_id' <<<"$hidden_bootstrap")"
  hidden_data="$(curl -fsS \
    -H "x-vpsman-share-token: $hidden_secret" \
    -H "x-vpsman-share-visitor: $hidden_visitor" \
    "$api_url/api/v1/public/monitoring-shares/$hidden_share_id/data?limit=1000&offset=0")"
  jq -e --argjson expected "$EXPECTED_CLIENT_COUNT" '
    .total == $expected
    and .share.visibility.billing == false
    and all(.cards[]; has("billing") | not)
  ' <<<"$hidden_data" >/dev/null || die "Billing-hidden public share projection check failed"

  db_counts_json="$(psql_review -qAt -c "
    SELECT json_build_object(
      'migrations', (SELECT count(*) FROM _sqlx_migrations),
      'clients', (SELECT count(*) FROM clients WHERE id LIKE 'review-%'),
      'resource_points', (
        SELECT count(*) FROM telemetry_rollups WHERE client_id LIKE 'review-%'
      ),
      'raw_samples', (
        SELECT count(*) FROM telemetry_samples WHERE client_id LIKE 'review-%'
      ),
      'network_rate_points', (
        SELECT count(*) FROM telemetry_network_rates WHERE client_id LIKE 'review-%'
      ),
      'traffic_samples', (
        SELECT count(*) FROM traffic_counter_samples WHERE client_id LIKE 'review-%'
      ),
      'ping_points', (
        SELECT count(*)
        FROM telemetry_ping_rollups AS rollup
        JOIN telemetry_ping_series AS series ON series.id = rollup.series_id
        WHERE series.client_id LIKE 'review-%'
      ),
      'shares', (
        SELECT count(*) FROM monitoring_share_links
        WHERE id IN ('$visible_share_id'::uuid, '$hidden_share_id'::uuid)
      )
    );
  ")"
  jq -e --argjson expected "$EXPECTED_CLIENT_COUNT" '
    .clients == $expected
    and .resource_points >= ($expected * 16)
    and .raw_samples >= ($expected * 16)
    and .network_rate_points >= (6 * 16)
    and .traffic_samples >= 16
    and .ping_points >= (3 * 16)
    and .shares == 2
  ' <<<"$db_counts_json" >/dev/null || die "database fixture count check failed"
}

screenshot_hashes_json() {
  local file
  local relative
  local digest
  local records=""
  shopt -s nullglob
  for file in "$artifact_dir"/*.png; do
    relative="${file#"$ROOT_DIR/"}"
    digest="$(sha256sum "$file" | awk '{print $1}')"
    records+="$(jq -nc --arg path "$relative" --arg sha256 "$digest" \
      '{path: $path, sha256: $sha256}')"$'\n'
  done
  shopt -u nullglob
  printf '%s' "$records" | jq -sc '.'
}

write_manifest() {
  local screenshots_json
  local git_head
  local worktree_hash
  local manifest_tmp="$artifact_dir/manifest.json.tmp"
  screenshots_json="$(screenshot_hashes_json)"
  [[ "$(jq 'length' <<<"$screenshots_json")" -ge 6 ]] \
    || die "capture produced fewer than six review screenshots"
  git_head="$(git rev-parse HEAD)"
  worktree_hash="$(current_worktree_hash)"
  verify_stack

  jq -n \
    --arg run_id "$run_id" \
    --arg captured_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg git_head "$git_head" \
    --arg worktree_hash "$worktree_hash" \
    --arg postgres_container "$container_name" \
    --arg postgres_endpoint "127.0.0.1:$postgres_port" \
    --arg postgres_data_dir "$postgres_data_dir" \
    --arg cargo_target_dir "$cargo_target_dir" \
    --arg api_url "$api_url" \
    --arg frontend_url "$frontend_url" \
    --arg artifact_dir "$artifact_dir_relative" \
    --arg visible_share_id "$visible_share_id" \
    --arg visible_share_url "$frontend_url/${visible_share_fragment}" \
    --arg hidden_share_id "$hidden_share_id" \
    --arg hidden_share_url "$frontend_url/${hidden_share_fragment}" \
    --argjson db_counts "$db_counts_json" \
    --argjson screenshots "$screenshots_json" \
    '{
      schema: "vpsman-monitoring-real-data-review/v1",
      run_id: $run_id,
      captured_at: $captured_at,
      source: {
        git_head: $git_head,
        working_tree_sha256: $worktree_hash
      },
      services: {
        postgres_container: $postgres_container,
        postgres_endpoint: $postgres_endpoint,
        postgres_data_dir: (if $postgres_data_dir == "" then null else $postgres_data_dir end),
        cargo_target_dir: (if $cargo_target_dir == "" then null else $cargo_target_dir end),
        api_url: $api_url,
        frontend_url: $frontend_url
      },
      database_counts: $db_counts,
      shares: {
        billing_visible: {id: $visible_share_id, url: $visible_share_url},
        billing_hidden: {id: $hidden_share_id, url: $hidden_share_url}
      },
      fixture_scenarios: [
        "total quota",
        "traffic quota exceeded with yellow progress",
        "RX-only quota with diagnostic TX",
        "TX-only unlimited quota with diagnostic RX",
        "unconfigured traffic",
        "no-reset accumulated traffic with 2020 and 2022 imported samples",
        "monthly billing renewal day",
        "annual billing renewal MM-DD",
        "optional product names projected beside provider identity",
        "billing absent and visibility-hidden",
        "healthy, degraded, and no-primary Ping",
        "selected and intentionally empty network rates",
        "at least sixteen resource, network, and Ping history points"
      ],
      playwright: {
        request_interception: false,
        frontend_mocks: false,
        source_guard_passed: true,
        project: "desktop-chrome"
      },
      artifact_dir: $artifact_dir,
      screenshots: $screenshots
    }' >"$manifest_tmp"
  mv "$manifest_tmp" "$artifact_dir/manifest.json"
}

status_run() {
  local manifest_path="$artifact_dir/manifest.json"
  local manifest_sha=""
  local screenshots='[]'
  local worktree_hash
  assert_no_playwright_interception
  verify_stack
  worktree_hash="$(current_worktree_hash)"
  if [[ -f "$manifest_path" ]]; then
    manifest_sha="$(sha256sum "$manifest_path" | awk '{print $1}')"
    screenshots="$(jq -c '.screenshots // []' "$manifest_path")"
  fi
  jq -n \
    --arg status "running" \
    --arg run_id "$run_id" \
    --arg started_at "$started_at" \
    --arg postgres_container "$container_name" \
    --arg postgres_endpoint "127.0.0.1:$postgres_port" \
    --arg postgres_data_dir "$postgres_data_dir" \
    --arg cargo_target_dir "$cargo_target_dir" \
    --arg api_url "$api_url" \
    --arg frontend_url "$frontend_url" \
    --arg artifact_dir "$artifact_dir_relative" \
    --arg manifest "$artifact_dir_relative/manifest.json" \
    --arg manifest_sha256 "$manifest_sha" \
    --arg worktree_sha256 "$worktree_hash" \
    --arg visible_share_url "$frontend_url/${visible_share_fragment}" \
    --arg hidden_share_url "$frontend_url/${hidden_share_fragment}" \
    --arg cleanup_command "./scripts/review-monitoring-real-data.sh stop" \
    --argjson api_pid "$api_pid" \
    --argjson frontend_pid "$frontend_pid" \
    --argjson database_counts "$db_counts_json" \
    --argjson screenshots "$screenshots" \
    '{
      status: $status,
      run_id: $run_id,
      started_at: $started_at,
      postgres: {
        container: $postgres_container,
        endpoint: $postgres_endpoint,
        data_dir: (if $postgres_data_dir == "" then null else $postgres_data_dir end)
      },
      cargo_target_dir: (if $cargo_target_dir == "" then null else $cargo_target_dir end),
      api: {url: $api_url, pid: $api_pid},
      frontend: {url: $frontend_url, pid: $frontend_pid},
      database_counts: $database_counts,
      visible_share_url: $visible_share_url,
      hidden_share_url: $hidden_share_url,
      artifact_dir: $artifact_dir,
      manifest: $manifest,
      manifest_sha256: $manifest_sha256,
      working_tree_sha256: $worktree_sha256,
      screenshots: $screenshots,
      request_interception: false,
      cleanup_command: $cleanup_command
    }'
}

allocate_ports() {
  local -a selected_ports
  mapfile -t selected_ports < <(python3 - <<'PY'
import random
import socket

ports = list(range(41000, 42000))
random.SystemRandom().shuffle(ports)
sockets = []
try:
    for port in ports:
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            listener.bind(("127.0.0.1", port))
        except OSError:
            listener.close()
            continue
        sockets.append(listener)
        print(port)
        if len(sockets) == 3:
            break
    if len(sockets) != 3:
        raise SystemExit("could not reserve three ports in 41000-41999")
finally:
    for listener in sockets:
        listener.close()
PY
  )
  [[ "${#selected_ports[@]}" == "3" ]] \
    || die "could not allocate three review ports"
  postgres_port="${selected_ports[0]}"
  api_port="${selected_ports[1]}"
  frontend_port="${selected_ports[2]}"
}

create_share() {
  local access_token="$1"
  local name="$2"
  local show_billing="$3"
  local target_ids_json="$4"
  local request_json
  request_json="$(jq -nc \
    --arg name "$name" \
    --argjson billing "$show_billing" \
    --argjson target_client_ids "$target_ids_json" \
    '{
      name: $name,
      selector_expression: "*",
      target_client_ids: $target_client_ids,
      visibility: {
        identity_context: true,
        billing: $billing,
        system_information: true,
        resources: true,
        network: true,
        traffic: true,
        ping: true,
        detail_history: true
      },
      expires_in_secs: 2592000,
      confirmed: true
    }')"
  curl -fsS \
    -H "Authorization: Bearer $access_token" \
    -H 'Content-Type: application/json' \
    -d "$request_json" \
    "$api_url/api/v1/monitoring-shares"
}

start_run() {
  local auth_json
  local access_token
  local target_ids_json
  local visible_response
  local hidden_response
  local api_log
  local frontend_log
  local postgres_data_root
  local configured_cargo_target_dir
  local api_binary
  local -a postgres_mount_args=()
  local -a postgres_command_args=()

  [[ ! -e "$STATE_DIR" ]] \
    || die "a current review stack already exists; inspect it with '$0 status' or remove it with '$0 stop'"
  [[ -f "$SEED_SQL" && -f "$REFRESH_SQL" && -f "$PLAYWRIGHT_SPEC" ]] \
    || die "review harness fixtures or Playwright spec are missing"
  assert_no_playwright_interception

  configured_cargo_target_dir="${VPSMAN_MONITORING_REVIEW_CARGO_TARGET_DIR:-}"
  if [[ -n "$configured_cargo_target_dir" ]]; then
    cargo_target_dir="$(readlink -m "$configured_cargo_target_dir")"
    [[ "$cargo_target_dir" == /mnt/storage/vpsman-tmp/* ]] \
      || die "VPSMAN_MONITORING_REVIEW_CARGO_TARGET_DIR must be below /mnt/storage/vpsman-tmp"
    [[ -d "$cargo_target_dir" && -w "$cargo_target_dir" ]] \
      || die "VPSMAN_MONITORING_REVIEW_CARGO_TARGET_DIR must be an existing writable directory"
  fi

  if [[ "${VPSMAN_MONITORING_REVIEW_SKIP_BUILD:-0}" != "1" ]]; then
    if [[ -n "$cargo_target_dir" ]]; then
      CARGO_TARGET_DIR="$cargo_target_dir" GITHUB_ACTIONS=true cargo build -p vpsman-api
    else
      GITHUB_ACTIONS=true cargo build -p vpsman-api
    fi
    (
      cd "$ROOT_DIR/frontend"
      ./node_modules/.bin/tsc --noEmit
      ./node_modules/.bin/vite build
    )
  fi
  api_binary="$(require_recorded_api_binary)"
  api_binary_sha256="$(sha256sum "$api_binary" | awk '{print $1}')"

  run_id="review-$(date -u +%Y%m%dT%H%M%SZ)-$$"
  container_name="vpsman-monitoring-${run_id}-postgres"
  started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  postgres_password="ReviewPg-${run_id}"
  operator_username="review-operator"
  operator_password="Review-operator-${run_id}"
  internal_token="review-internal-${run_id}"
  artifact_dir_relative="output/playwright/monitoring-real-data-${run_id}"
  artifact_dir="$ROOT_DIR/$artifact_dir_relative"
  postgres_data_root="${VPSMAN_MONITORING_REVIEW_PGDATA_ROOT:-}"
  if [[ -n "$postgres_data_root" ]]; then
    postgres_data_root="$(readlink -m "$postgres_data_root")"
    [[ "$postgres_data_root" == /mnt/storage/vpsman-tmp/* ]] \
      || die "VPSMAN_MONITORING_REVIEW_PGDATA_ROOT must be below /mnt/storage/vpsman-tmp"
    [[ -d "$postgres_data_root" && -w "$postgres_data_root" ]] \
      || die "VPSMAN_MONITORING_REVIEW_PGDATA_ROOT must be an existing writable directory"
    postgres_data_dir="$postgres_data_root/$run_id/postgres"
    mkdir -p "$postgres_data_dir"
    [[ "$(readlink -m "$postgres_data_dir")" == "$postgres_data_dir" ]] \
      || die "resolved PostgreSQL data path is not canonical"
    postgres_mount_args=(
      --mount "type=bind,src=$postgres_data_dir,dst=/var/lib/postgresql/data"
    )
  fi
  allocate_ports
  postgres_url="postgres://vpsman:${postgres_password}@127.0.0.1:${postgres_port}/vpsman"
  api_url="http://127.0.0.1:$api_port"
  frontend_url="http://127.0.0.1:$frontend_port"
  validate_run_identity

  if [[ "${VPSMAN_MONITORING_REVIEW_PG_STAT_STATEMENTS:-0}" == "1" ]]; then
    postgres_command_args=(
      postgres
      -c shared_preload_libraries=pg_stat_statements
      -c pg_stat_statements.track=all
      -c pg_stat_statements.track_utility=on
    )
  fi

  mkdir -p "$STATE_ROOT"
  mkdir "$STATE_DIR"
  owns_current_state=1
  mkdir -p "$artifact_dir" "$STATE_DIR/object-store/backups"
  chmod 700 "$STATE_DIR"
  trap cleanup_failed_start EXIT
  trap 'exit 130' INT TERM
  write_state

  docker run -d \
    --name "$container_name" \
    --label "com.vpsman.monitoring-review-run=$run_id" \
    --label 'com.vpsman.monitoring-review-harness=v1' \
    -e POSTGRES_DB=vpsman \
    -e "POSTGRES_PASSWORD=$postgres_password" \
    -e POSTGRES_USER=vpsman \
    "${postgres_mount_args[@]}" \
    -p "127.0.0.1:$postgres_port:5432" \
    postgres:16-alpine "${postgres_command_args[@]}" >/dev/null
  wait_for_postgres

  api_log="$STATE_DIR/api.log"
  launch_api_process
  write_state
  wait_for_http "$api_url/health" "$api_pid" "$api_log"
  api_process_matches_run "$api_pid" \
    || die "started API process does not match the exact recorded binary and run identity"

  auth_json="$(curl -fsS \
    -H 'Content-Type: application/json' \
    -d "$(jq -nc \
      --arg username "$operator_username" \
      --arg password "$operator_password" \
      '{username: $username, password: $password}')" \
    "$api_url/api/v1/auth/bootstrap")"
  access_token="$(jq -er '.access_token' <<<"$auth_json")"

  psql_review -q -f "$SEED_SQL"
  refresh_observations
  target_ids_json="$(psql_review -qAt -c \
    "SELECT json_agg(id ORDER BY id) FROM clients WHERE id LIKE 'review-%'")"
  jq -e --argjson expected "$EXPECTED_CLIENT_COUNT" \
    'length == $expected' <<<"$target_ids_json" >/dev/null \
    || die "seeded review target count is incorrect"

  visible_response="$(create_share \
    "$access_token" 'Monitoring review · Billing visible' true "$target_ids_json")"
  visible_share_id="$(jq -er '.share.id' <<<"$visible_response")"
  visible_share_fragment="$(jq -er '.fragment_path' <<<"$visible_response")"
  hidden_response="$(create_share \
    "$access_token" 'Monitoring review · Billing hidden' false "$target_ids_json")"
  hidden_share_id="$(jq -er '.share.id' <<<"$hidden_response")"
  hidden_share_fragment="$(jq -er '.fragment_path' <<<"$hidden_response")"
  [[ "$visible_share_fragment" =~ ^#/share/[0-9a-f-]{36}/[0-9a-f]{64}$ \
    && "$hidden_share_fragment" =~ ^#/share/[0-9a-f-]{36}/[0-9a-f]{64}$ ]] \
    || die "API returned an invalid monitoring share fragment"

  frontend_log="$STATE_DIR/frontend.log"
  setsid env \
    "VPSMAN_REVIEW_RUN_ID=$run_id" \
    "VPSMAN_API_PROXY=$api_url" \
    "$ROOT_DIR/frontend/node_modules/.bin/vite" \
      "$ROOT_DIR/frontend" \
      --host 127.0.0.1 \
      --port "$frontend_port" >"$frontend_log" 2>&1 </dev/null &
  frontend_pid="$!"
  write_state
  wait_for_http "$frontend_url/" "$frontend_pid" "$frontend_log"

  status_run
  start_succeeded=1
  trap - EXIT INT TERM
}

capture_run() {
  load_state
  assert_no_playwright_interception
  verify_stack
  refresh_observations

  (
    export VPSMAN_MONITORING_REAL_DATA_CAPTURE=1
    export VPSMAN_MONITORING_REAL_DATA_OUTPUT="$artifact_dir"
    export VPSMAN_MONITORING_REAL_DATA_USERNAME="$operator_username"
    export VPSMAN_MONITORING_REAL_DATA_PASSWORD="$operator_password"
    export VPSMAN_MONITORING_VISIBLE_SHARE_FRAGMENT="$visible_share_fragment"
    export VPSMAN_MONITORING_HIDDEN_SHARE_FRAGMENT="$hidden_share_fragment"
    export VPSMAN_MONITORING_EXPECTED_CLIENTS="$EXPECTED_CLIENT_COUNT"
    export VPSMAN_FRONTEND_TEST_HOST=127.0.0.1
    export VPSMAN_FRONTEND_TEST_PORT="$frontend_port"
    export VPSMAN_API_PROXY="$api_url"
    export VPSMAN_FRONTEND_TEST_SERVER_COMMAND='exit 1'
    export CI=
    cd "$ROOT_DIR/frontend"
    npm run test:ui -- \
      tests/monitoring-real-data-review.spec.ts \
      --project desktop-chrome \
      --workers=1 \
      --output="$artifact_dir/test-results"
  )

  write_manifest
  status_run
}

quiesce_api_run() {
  local recorded_pid
  local run_api_processes
  load_state
  [[ "$api_pid" != "0" ]] || die "API is already quiesced for $run_id"
  [[ "$api_pid" =~ ^[1-9][0-9]*$ ]] || die "stored API PID is invalid"
  [[ -n "$api_binary_sha256" ]] \
    || die "recorded run predates API quiesce identity; stop it and start a new run"
  require_live_postgres_container
  require_recorded_api_binary >/dev/null
  api_process_matches_run "$api_pid" \
    || die "refusing to quiesce API PID $api_pid because its binary or run identity differs"
  process_is_group_leader "$api_pid" \
    || die "refusing to quiesce API PID $api_pid because it is not its process-group leader"
  run_api_processes="$(api_processes_for_run)"
  [[ "$run_api_processes" == "$api_pid" ]] \
    || die "refusing to quiesce because the recorded run does not have exactly one API process"

  recorded_pid="$api_pid"
  stop_process_group "$recorded_pid"
  run_api_processes="$(api_processes_for_run)"
  [[ -z "$run_api_processes" ]] \
    || die "API process identity survived quiesce: $run_api_processes"
  if curl -fsS --max-time 1 "$api_url/health" >/dev/null 2>&1; then
    die "API health endpoint still responds after quiesce"
  fi
  wait_for_no_api_backends
  api_pid=0
  write_state

  jq -n \
    --arg status "api_quiesced" \
    --arg run_id "$run_id" \
    --arg api_url "$api_url" \
    --argjson stopped_pid "$recorded_pid" \
    '{
      status: $status,
      run_id: $run_id,
      api: {url: $api_url, pid: 0, stopped_pid: $stopped_pid},
      postgres_backends: 0
    }'
}

resume_api_run() {
  local api_log="$STATE_DIR/api.log"
  local run_api_processes
  load_state
  [[ "$api_pid" == "0" ]] \
    || die "API is already recorded as running with PID $api_pid"
  [[ -n "$api_binary_sha256" ]] \
    || die "recorded run predates API resume identity; stop it and start a new run"
  require_live_postgres_container
  require_recorded_api_binary >/dev/null
  run_api_processes="$(api_processes_for_run)"
  [[ -z "$run_api_processes" ]] \
    || die "refusing to resume because an unrecorded API process survives: $run_api_processes"
  wait_for_no_api_backends
  if curl -fsS --max-time 1 "$api_url/health" >/dev/null 2>&1; then
    die "refusing to resume because the API health endpoint already responds"
  fi

  trap cleanup_failed_api_resume EXIT
  trap 'exit 130' INT TERM
  launch_api_process
  wait_for_http "$api_url/health" "$api_pid" "$api_log"
  api_process_matches_run "$api_pid" \
    || die "resumed API process does not match the exact recorded binary and run identity"
  process_is_group_leader "$api_pid" \
    || die "resumed API process is not its process-group leader"
  wait_for_api_backend
  write_state
  trap - EXIT INT TERM

  jq -n \
    --arg status "api_resumed" \
    --arg run_id "$run_id" \
    --arg api_url "$api_url" \
    --argjson api_pid "$api_pid" \
    --argjson postgres_backends "$(api_backend_count)" \
    '{
      status: $status,
      run_id: $run_id,
      api: {url: $api_url, pid: $api_pid},
      postgres_backends: $postgres_backends
    }'
}

stop_run() {
  local artifact_to_keep
  local postgres_data_to_keep
  local run_api_processes
  if [[ ! -e "$STATE_DIR" ]]; then
    jq -n '{status: "absent", message: "No monitoring review stack is recorded."}'
    return
  fi
  load_state
  artifact_to_keep="$artifact_dir_relative"
  postgres_data_to_keep="$postgres_data_dir"

  if [[ "$api_pid" != "0" ]]; then
    if kill -0 "$api_pid" >/dev/null 2>&1; then
      api_process_matches_run "$api_pid" \
        || die "refusing cleanup because API PID $api_pid has been reused or changed identity"
    fi
    run_api_processes="$(api_processes_for_run | awk -v recorded_pid="$api_pid" '$0 != recorded_pid')"
    [[ -z "$run_api_processes" ]] \
      || die "refusing cleanup because API descendants survive recorded PID $api_pid: $run_api_processes"
  else
    run_api_processes="$(api_processes_for_run)"
    [[ -z "$run_api_processes" ]] \
      || die "refusing cleanup because API is recorded quiesced but process survives: $run_api_processes"
  fi
  if kill -0 "$frontend_pid" >/dev/null 2>&1; then
    process_matches_run "$frontend_pid" \
      || die "refusing cleanup because frontend PID $frontend_pid has been reused"
  fi
  if docker inspect "$container_name" >/dev/null 2>&1; then
    container_matches_run \
      || die "refusing cleanup because the stored container has another run label"
  fi

  stop_process_group "$frontend_pid"
  if [[ "$api_pid" != "0" ]]; then
    stop_process_group "$api_pid"
  fi
  if docker inspect "$container_name" >/dev/null 2>&1; then
    docker rm -f "$container_name" >/dev/null
  fi
  [[ "$(readlink -m "$STATE_DIR")" == "$STATE_ROOT/current" ]] \
    || die "refusing to remove an unexpected state directory"
  rm -rf -- "$STATE_DIR"
  rmdir "$STATE_ROOT" 2>/dev/null || true

  jq -n \
    --arg run_id "$run_id" \
    --arg artifacts "$artifact_to_keep" \
    --arg postgres_data "$postgres_data_to_keep" \
    '{
      status: "stopped",
      run_id: $run_id,
      retained_artifacts: $artifacts,
      retained_postgres_data: (if $postgres_data == "" then null else $postgres_data end),
      recoverability: "The isolated PostgreSQL container and live processes were removed; screenshots remain."
    }'
}

usage() {
  cat >&2 <<'EOF'
Usage: ./scripts/review-monitoring-real-data.sh start|capture|status|quiesce-api|resume-api|stop

  start        Build and start an isolated PostgreSQL/API/frontend review stack.
  capture      Refresh relative evidence and capture real-data Playwright screenshots.
  status       Verify every live layer and print IDs, paths, and content hashes.
  quiesce-api  Gracefully stop only the exact API and verify its DB sessions are gone.
  resume-api   Restart that API from its recorded prebuilt binary and append its log.
  stop         Stop only the exact recorded run; retained screenshots are not deleted.

Optional benchmark storage:
  VPSMAN_MONITORING_REVIEW_PGDATA_ROOT=/mnt/storage/vpsman-tmp/<scope>
  binds a run-owned PostgreSQL data directory below that existing writable root.
  The directory is retained by stop and is never removed by this harness.
  VPSMAN_MONITORING_REVIEW_CARGO_TARGET_DIR=/mnt/storage/vpsman-tmp/<target>
  uses that existing writable directory for the API build and executable.
EOF
}

main() {
  require_tools awk bash cargo curl docker git grep jq npm psql python3 readlink \
    ps rg sed setsid sha256sum sort tr xargs
  cd "$ROOT_DIR"
  case "${1:-}" in
    start)
      [[ "$#" == "1" ]] || die "start accepts no extra arguments"
      start_run
      ;;
    capture)
      [[ "$#" == "1" ]] || die "capture accepts no extra arguments"
      capture_run
      ;;
    status)
      [[ "$#" == "1" ]] || die "status accepts no extra arguments"
      load_state
      status_run
      ;;
    quiesce-api)
      [[ "$#" == "1" ]] || die "quiesce-api accepts no extra arguments"
      quiesce_api_run
      ;;
    resume-api)
      [[ "$#" == "1" ]] || die "resume-api accepts no extra arguments"
      resume_api_run
      ;;
    stop)
      [[ "$#" == "1" ]] || die "stop accepts no extra arguments"
      stop_run
      ;;
    *)
      usage
      exit 2
      ;;
  esac
}

main "$@"
