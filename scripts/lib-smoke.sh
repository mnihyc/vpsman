#!/usr/bin/env bash

SMOKE_ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SMOKE_TMPDIR=""
SMOKE_PIDS=()
SMOKE_RESERVED_PORTS=()
SMOKE_CONTAINERS=()

smoke_fail() {
  echo "$*" >&2
  exit 1
}

smoke_enter_root() {
  cd "$SMOKE_ROOT_DIR"
}

smoke_require_tools() {
  local tool
  for tool in "$@"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      echo "missing required tool: $tool" >&2
      exit 1
    fi
  done
}

smoke_privilege_verifier_key_hex() {
  local password="$1"
  local salt_hex="$2"
  python3 - "$password" "$salt_hex" <<'PY'
import hashlib
import sys

password = sys.argv[1]
salt = bytes.fromhex(sys.argv[2])
hasher = hashlib.sha256()
hasher.update(b"vpsman-super-key-v1")
hasher.update(len(salt).to_bytes(8, "big"))
hasher.update(salt)
hasher.update(password.encode())
print(hasher.hexdigest())
PY
}

smoke_agent_config_client_id() {
  local config_path="$1"
  sed -n 's/^client_id = "\(.*\)"$/\1/p' "$config_path" | head -n 1
}

smoke_toml_quote() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  printf '"%s"' "$value"
}

smoke_csv_to_toml_string_array() {
  local csv="${1:-}"
  local first=1
  local item
  local -a items=()
  IFS=',' read -r -a items <<<"$csv"
  for item in "${items[@]}"; do
    item="${item#"${item%%[![:space:]]*}"}"
    item="${item%"${item##*[![:space:]]}"}"
    [[ -n "$item" ]] || continue
    if [[ "$first" -eq 0 ]]; then
      printf ', '
    fi
    smoke_toml_quote "$item"
    first=0
  done
}

smoke_write_enrolled_agent_config() {
  local config_path="$1"
  local client_id="$2"
  local display_name="$3"
  local tags_csv="$4"
  local client_private_hex="$5"
  local gateway_public_hex="$6"
  local endpoints_csv="$7"
  local command_timeout_secs="${8:-30}"
  local network_root="${9:-}"
  local telemetry_light_secs="${10:-15}"
  local telemetry_full_secs="${11:-60}"

  mkdir -p "$(dirname "$config_path")"
  {
    printf 'client_id = %s\n' "$(smoke_toml_quote "$client_id")"
    printf 'display_name = %s\n' "$(smoke_toml_quote "${display_name:-$client_id}")"
    printf 'telemetry_light_secs = %s\n' "$telemetry_light_secs"
    printf 'telemetry_full_secs = %s\n' "$telemetry_full_secs"
    printf 'tags = [%s]\n' "$(smoke_csv_to_toml_string_array "$tags_csv")"
    printf '\n[noise]\n'
    printf 'mode = "enrolled_ik"\n'
    printf 'client_private_key_hex = %s\n' "$(smoke_toml_quote "$client_private_hex")"
    printf 'server_public_key_hex = %s\n' "$(smoke_toml_quote "$gateway_public_hex")"
    printf '\n[auth]\n'
    printf 'command_timeout_secs = %s\n' "$command_timeout_secs"
    printf 'gateway_retry_secs = 1\n'
    printf 'gateway_connect_timeout_secs = 1\n'
    if [[ -n "$network_root" ]]; then
      printf '\n[network]\n'
      printf 'root_dir = %s\n' "$(smoke_toml_quote "$network_root")"
    fi
  } >"$config_path"

  local endpoint
  local -a endpoints=()
  IFS=',' read -r -a endpoints <<<"$endpoints_csv"
  for endpoint in "${endpoints[@]}"; do
    endpoint="${endpoint//[$'\r\n']/}"
    [[ -n "${endpoint// /}" ]] || continue
    local label tcp_addr priority extra
    IFS='=' read -r label tcp_addr priority extra <<<"$endpoint"
    [[ -z "${extra:-}" && -n "${label:-}" && -n "${tcp_addr:-}" && -n "${priority:-}" ]] \
      || smoke_fail "invalid direct agent endpoint spec: $endpoint"
    {
      printf '\n[[tcp_endpoints]]\n'
      printf 'label = %s\n' "$(smoke_toml_quote "$label")"
      printf 'tcp_addr = %s\n' "$(smoke_toml_quote "$tcp_addr")"
      printf 'priority = %s\n' "$priority"
    } >>"$config_path"
  done
}

smoke_create_direct_agent_config() {
  local api_url="$1"
  local access_token="$2"
  local config_path="$3"
  local client_id="$4"
  local display_name="$5"
  local tags_csv="$6"
  local gateway_public_hex="$7"
  local endpoints_csv="$8"
  local command_timeout_secs="${9:-30}"
  local keypair private_hex public_hex

  keypair="$(target/debug/vpsctl noise-keygen)"
  private_hex="$(jq -r '.private_key_hex' <<<"$keypair")"
  public_hex="$(jq -r '.public_key_hex' <<<"$keypair")"

  local -a upsert_args=(
    --api-url "$api_url"
    agent-identity-upsert
    --client-id "$client_id"
    --client-public-key-hex "$public_hex"
    --confirmed
  )
  if [[ -n "$display_name" ]]; then
    upsert_args+=(--display-name "$display_name")
  fi
  if [[ -n "$tags_csv" ]]; then
    upsert_args+=(--tags "$tags_csv")
  fi
  VPSMAN_API_TOKEN="$access_token" target/debug/vpsctl "${upsert_args[@]}" >/dev/null

  smoke_write_enrolled_agent_config \
    "$config_path" \
    "$client_id" \
    "$display_name" \
    "$tags_csv" \
    "$private_hex" \
    "$gateway_public_hex" \
    "$endpoints_csv" \
    "$command_timeout_secs"
}

smoke_build_binaries() {
  if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
    cargo build -p vpsman-api -p vpsman-gateway -p vpsman-agent -p vpsctl
  fi
}

smoke_init_tmpdir() {
  local name="$1"
  mkdir -p .tmp
  SMOKE_TMPDIR="$(mktemp -d "$SMOKE_ROOT_DIR/.tmp/${name}.XXXXXX")"
  SMOKE_PIDS=()
  SMOKE_RESERVED_PORTS=()
  SMOKE_CONTAINERS=()
  trap smoke_cleanup EXIT
}

smoke_cleanup() {
  local pid
  for pid in "${SMOKE_PIDS[@]:-}"; do
    kill "$pid" >/dev/null 2>&1 || true
  done
  for pid in "${SMOKE_PIDS[@]:-}"; do
    wait "$pid" >/dev/null 2>&1 || true
  done
  local container
  for container in "${SMOKE_CONTAINERS[@]:-}"; do
    docker rm -f "$container" >/dev/null 2>&1 || true
  done
  if [[ -n "${SMOKE_TMPDIR:-}" ]]; then
    if [[ "${VPSMAN_SMOKE_KEEP_TMP:-0}" == "1" ]]; then
      echo "keeping smoke tmpdir: $SMOKE_TMPDIR" >&2
      return
    fi
    rm -rf "$SMOKE_TMPDIR"
  fi
}

smoke_track_pid() {
  SMOKE_PIDS+=("$1")
}

smoke_track_container() {
  SMOKE_CONTAINERS+=("$1")
}

smoke_free_port() {
  local port
  for port in $(shuf -i 20000-45000 -n 200); do
    if smoke_port_is_reserved "$port"; then
      continue
    fi
    if ! timeout 0.2 bash -c "</dev/tcp/127.0.0.1/$port" >/dev/null 2>&1; then
      SMOKE_RESERVED_PORTS+=("$port")
      printf '%s\n' "$port"
      return 0
    fi
  done
  echo "failed to find free local TCP port" >&2
  return 1
}

smoke_port_is_reserved() {
  local candidate="$1"
  local reserved
  for reserved in "${SMOKE_RESERVED_PORTS[@]:-}"; do
    if [[ "$reserved" == "$candidate" ]]; then
      return 0
    fi
  done
  return 1
}

smoke_wait_http() {
  local url="$1"
  local timeout_secs="${SMOKE_WAIT_HTTP_SECS:-45}"
  local deadline=$((SECONDS + timeout_secs))
  until curl -fsS "$url" >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      echo "timed out waiting for $url" >&2
      return 1
    fi
    sleep 0.1
  done
}

smoke_start_postgres() {
  local label="$1"
  local port="$2"
  local container_name="${label}-$(date +%s%N)"
  docker run --rm -d \
    --name "$container_name" \
    -e POSTGRES_DB=vpsman \
    -e POSTGRES_PASSWORD=vpsman \
    -e POSTGRES_USER=vpsman \
    -p "127.0.0.1:$port:5432" \
    postgres:16-alpine >/dev/null
  smoke_track_container "$container_name"

  local deadline=$((SECONDS + 45))
  until docker exec "$container_name" pg_isready -U vpsman -d vpsman >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      docker logs "$container_name" >&2 || true
      smoke_fail "timed out waiting for Postgres container"
    fi
    sleep 0.25
  done
  smoke_wait_tcp 127.0.0.1 "$port"
  printf 'postgres://vpsman:vpsman@127.0.0.1:%s/vpsman\n' "$port"
}

smoke_wait_tcp() {
  local host="$1"
  local port="$2"
  local timeout_secs="${SMOKE_WAIT_TCP_SECS:-45}"
  local deadline=$((SECONDS + timeout_secs))
  until timeout 0.2 bash -c "</dev/tcp/$host/$port" >/dev/null 2>&1; do
    if (( SECONDS >= deadline )); then
      echo "timed out waiting for $host:$port" >&2
      return 1
    fi
    sleep 0.1
  done
}

smoke_dump_logs() {
  local title="$1"
  shift
  local log
  echo "$title" >&2
  for log in "$@"; do
    echo "--- $log ---" >&2
    cat "$log" >&2 || true
  done
}

smoke_wait_api_job_status() {
  local api_url="$1"
  local job_id="$2"
  local expected_status="$3"
  local timeout_secs="${4:-45}"
  local token="${5:-${VPSMAN_API_TOKEN:-}}"
  local deadline=$((SECONDS + timeout_secs))
  local job_json status
  local -a curl_args=(-fsS)
  if [[ -n "$token" ]]; then
    curl_args+=(-H "Authorization: Bearer $token")
  fi
  until job_json="$(curl "${curl_args[@]}" "$api_url/api/v1/jobs/$job_id" 2>/dev/null)"; do
    if (( SECONDS >= deadline )); then
      echo "timed out waiting for job $job_id to become $expected_status" >&2
      return 1
    fi
    sleep 0.1
  done
  while true; do
    status="$(jq -r '.status // empty' <<<"$job_json")"
    if [[ "$expected_status" == "terminal" ]]; then
      case "$status" in
        queued|dispatching|running|accepted) ;;
        *) printf '%s\n' "$job_json"; return 0 ;;
      esac
    elif [[ "$status" == "$expected_status" ]]; then
      printf '%s\n' "$job_json"
      return 0
    fi
    if (( SECONDS >= deadline )); then
      echo "timed out waiting for job $job_id to become $expected_status; last status=$status" >&2
      printf '%s\n' "$job_json" >&2
      return 1
    fi
    sleep 0.1
    job_json="$(curl "${curl_args[@]}" "$api_url/api/v1/jobs/$job_id")"
  done
}

smoke_assert_job_create_queued() {
  local create_json="$1"
  local expected_targets="$2"
  jq -e --argjson expected_targets "$expected_targets" '
    (.job_id | length == 36)
    and .target_count == $expected_targets
    and (.accepted_targets == 0 or .accepted_targets <= .target_count)
    and (.status == "dispatching" or .status == "queued" or .status == "completed" or .status == "timed_out" or .status == "failed" or .status == "degraded_unprivileged")
  ' <<<"$create_json" >/dev/null
}
