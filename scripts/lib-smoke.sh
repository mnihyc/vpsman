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

smoke_toml_quote() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  printf '"%s"' "$value"
}

smoke_write_direct_agent_config() {
  local output_file="$1"
  local client_id="$2"
  local display_name="$3"
  local client_private_key_hex="$4"
  local gateway_public_key_hex="$5"
  local gateway_addr="$6"
  local server_signing_public_key_hex="$7"
  local command_timeout_secs="${8:-30}"
  local telemetry_light_secs="${9:-15}"
  local telemetry_full_secs="${10:-60}"

  cat >"$output_file" <<EOF
client_id = $(smoke_toml_quote "$client_id")
display_name = $(smoke_toml_quote "$display_name")
telemetry_light_secs = $telemetry_light_secs
telemetry_full_secs = $telemetry_full_secs
tags = []

[noise]
mode = "enrolled_ik"
client_private_key_hex = $(smoke_toml_quote "$client_private_key_hex")
server_public_key_hex = $(smoke_toml_quote "$gateway_public_key_hex")

[auth]
server_ed25519_public_key_hex = $(smoke_toml_quote "$server_signing_public_key_hex")
command_timeout_secs = $command_timeout_secs
gateway_retry_secs = 1
gateway_connect_timeout_secs = 5

[[tcp_endpoints]]
label = "primary"
tcp_addr = $(smoke_toml_quote "$gateway_addr")
priority = 10
EOF
}

smoke_register_direct_agent_config() {
  local api_url="$1"
  local access_token="$2"
  local output_file="$3"
  local client_id="$4"
  local display_name="$5"
  local tags_csv="$6"
  local gateway_addr="$7"
  local gateway_public_key_hex="$8"
  local server_signing_public_key_hex="$9"
  local command_timeout_secs="${10:-30}"
  local telemetry_light_secs="${11:-15}"
  local telemetry_full_secs="${12:-60}"

  local agent_keys agent_private_hex agent_public_hex
  agent_keys="$(target/debug/vpsctl noise-keygen)"
  agent_private_hex="$(jq -r '.private_key_hex' <<<"$agent_keys")"
  agent_public_hex="$(jq -r '.public_key_hex' <<<"$agent_keys")"

  if [[ -n "$access_token" ]]; then
    VPSMAN_API_TOKEN="$access_token" \
      target/debug/vpsctl --api-url "$api_url" agent-identity-upsert \
        --client-id "$client_id" \
        --client-public-key-hex "$agent_public_hex" \
        --display-name "$display_name" \
        --tags "$tags_csv" \
        --confirmed >/dev/null
  else
    target/debug/vpsctl --api-url "$api_url" agent-identity-upsert \
      --client-id "$client_id" \
      --client-public-key-hex "$agent_public_hex" \
      --display-name "$display_name" \
      --tags "$tags_csv" \
      --confirmed >/dev/null
  fi

  smoke_write_direct_agent_config \
    "$output_file" \
    "$client_id" \
    "$display_name" \
    "$agent_private_hex" \
    "$gateway_public_key_hex" \
    "$gateway_addr" \
    "$server_signing_public_key_hex" \
    "$command_timeout_secs" \
    "$telemetry_light_secs" \
    "$telemetry_full_secs"
}

smoke_register_direct_agent_config_from_private_key() {
  local api_url="$1"
  local access_token="$2"
  local output_file="$3"
  local client_id="$4"
  local display_name="$5"
  local tags_csv="$6"
  local gateway_addr="$7"
  local gateway_public_key_hex="$8"
  local server_signing_public_key_hex="$9"
  local agent_private_hex="${10}"
  local agent_public_hex="${11}"
  local command_timeout_secs="${12:-30}"
  local telemetry_light_secs="${13:-15}"
  local telemetry_full_secs="${14:-60}"

  if [[ -n "$access_token" ]]; then
    VPSMAN_API_TOKEN="$access_token" \
      target/debug/vpsctl --api-url "$api_url" agent-identity-upsert \
        --client-id "$client_id" \
        --client-public-key-hex "$agent_public_hex" \
        --display-name "$display_name" \
        --tags "$tags_csv" \
        --replace-existing-key \
        --confirmed >/dev/null
  else
    target/debug/vpsctl --api-url "$api_url" agent-identity-upsert \
      --client-id "$client_id" \
      --client-public-key-hex "$agent_public_hex" \
      --display-name "$display_name" \
      --tags "$tags_csv" \
      --replace-existing-key \
      --confirmed >/dev/null
  fi

  smoke_write_direct_agent_config \
    "$output_file" \
    "$client_id" \
    "$display_name" \
    "$agent_private_hex" \
    "$gateway_public_key_hex" \
    "$gateway_addr" \
    "$server_signing_public_key_hex" \
    "$command_timeout_secs" \
    "$telemetry_light_secs" \
    "$telemetry_full_secs"
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
