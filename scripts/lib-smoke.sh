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
