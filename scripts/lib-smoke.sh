#!/usr/bin/env bash

SMOKE_ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SMOKE_TMPDIR=""
SMOKE_PIDS=()
SMOKE_RESERVED_PORTS=()

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
  local deadline=$((SECONDS + timeout_secs))
  local job_json status
  until job_json="$(curl -fsS "$api_url/api/v1/jobs/$job_id" 2>/dev/null)"; do
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
    job_json="$(curl -fsS "$api_url/api/v1/jobs/$job_id")"
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
