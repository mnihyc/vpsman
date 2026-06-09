#!/usr/bin/env bash
set -euo pipefail

log() { printf '[vpsman-install] %s\n' "$*" >&2; }
die() { printf '[vpsman-install] error: %s\n' "$*" >&2; exit 1; }

require_env() {
  local name="$1"
  [[ -n "${!name:-}" ]] || die "$name is required"
}

require_hex32() {
  local name="$1" value="${!1:-}"
  [[ "$value" =~ ^[0-9A-Fa-f]{64}$ ]] || die "$name must be exactly 64 hex characters"
}

toml_quote() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  printf '"%s"' "$value"
}

install_mode="${VPSMAN_INSTALL_MODE:-root}"
case "$install_mode" in
  root|user) ;;
  *) die "VPSMAN_INSTALL_MODE must be root or user" ;;
esac

require_env VPSMAN_AGENT_CLIENT_ID
require_env VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX
require_env VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX
require_env VPSMAN_GATEWAY_ENDPOINTS
require_hex32 VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX
require_hex32 VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX
if [[ -n "${VPSMAN_SERVER_ED25519_PUBLIC_KEY_HEX:-}" ]]; then
  require_hex32 VPSMAN_SERVER_ED25519_PUBLIC_KEY_HEX
fi

if [[ "$install_mode" == "root" ]]; then
  [[ "$(id -u)" -eq 0 ]] || die "root install mode must run as root"
  install_dir="${VPSMAN_AGENT_INSTALL_DIR:-/opt/vpsman-agent}"
  config_dir="${VPSMAN_AGENT_CONFIG_DIR:-/etc/vpsman-agent}"
  service_name="${VPSMAN_AGENT_SERVICE_NAME:-vpsman-agent.service}"
  run_user="root"
  systemctl_scope=()
else
  install_dir="${VPSMAN_AGENT_INSTALL_DIR:-$HOME/.local/lib/vpsman-agent}"
  config_dir="${VPSMAN_AGENT_CONFIG_DIR:-$HOME/.config/vpsman-agent}"
  service_name="${VPSMAN_AGENT_SERVICE_NAME:-vpsman-agent.service}"
  run_user="${USER:-vpsman}"
  systemctl_scope=(--user)
fi

mkdir -p "$install_dir" "$config_dir"
chmod 700 "$config_dir"
agent_bin="$install_dir/vpsman-agent"

if [[ -n "${VPSMAN_AGENT_BINARY_PATH:-}" ]]; then
  install -m 0755 "$VPSMAN_AGENT_BINARY_PATH" "$agent_bin"
elif [[ -n "${VPSMAN_AGENT_BINARY_URL:-}" ]]; then
  tmp_bin="$(mktemp)"
  trap 'rm -f "$tmp_bin"' EXIT
  curl -fsSL "$VPSMAN_AGENT_BINARY_URL" -o "$tmp_bin"
  if [[ -n "${VPSMAN_AGENT_BINARY_SHA256:-}" ]]; then
    printf '%s  %s\n' "$VPSMAN_AGENT_BINARY_SHA256" "$tmp_bin" | sha256sum -c - >/dev/null
  fi
  install -m 0755 "$tmp_bin" "$agent_bin"
else
  if command -v vpsman-agent >/dev/null 2>&1; then
    cp "$(command -v vpsman-agent)" "$agent_bin"
    chmod 0755 "$agent_bin"
  elif [[ -x "$agent_bin" ]]; then
    :
  else
    die "provide VPSMAN_AGENT_BINARY_PATH, VPSMAN_AGENT_BINARY_URL, or put vpsman-agent on PATH"
  fi
fi

config_file="$config_dir/agent.toml"
{
  printf 'client_id = %s\n' "$(toml_quote "$VPSMAN_AGENT_CLIENT_ID")"
  printf 'display_name = %s\n' "$(toml_quote "${VPSMAN_AGENT_DISPLAY_NAME:-$VPSMAN_AGENT_CLIENT_ID}")"
  printf 'telemetry_light_secs = %s\n' "${VPSMAN_TELEMETRY_LIGHT_SECS:-15}"
  printf 'telemetry_full_secs = %s\n' "${VPSMAN_TELEMETRY_FULL_SECS:-60}"
  printf 'tags = []\n'
  printf '\n[noise]\n'
  printf 'mode = "enrolled_ik"\n'
  printf 'client_private_key_hex = %s\n' "$(toml_quote "$VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX")"
  printf 'server_public_key_hex = %s\n' "$(toml_quote "$VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX")"
  printf '\n[auth]\n'
  if [[ -n "${VPSMAN_SERVER_ED25519_PUBLIC_KEY_HEX:-}" ]]; then
    printf 'server_ed25519_public_key_hex = %s\n' "$(toml_quote "$VPSMAN_SERVER_ED25519_PUBLIC_KEY_HEX")"
  fi
  printf 'command_timeout_secs = %s\n' "${VPSMAN_COMMAND_TIMEOUT_SECS:-30}"
  printf 'gateway_retry_secs = %s\n' "${VPSMAN_GATEWAY_RETRY_SECS:-60}"
  printf 'gateway_connect_timeout_secs = %s\n' "${VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS:-10}"
} >"$config_file"

first=1
IFS=$'\n,' read -r -d '' -a endpoints < <(printf '%s\0' "$VPSMAN_GATEWAY_ENDPOINTS") || true
for endpoint in "${endpoints[@]}"; do
  endpoint="${endpoint//[$'\r\n']/}"
  [[ -n "${endpoint// /}" ]] || continue
  IFS='=' read -r label tcp_addr priority extra <<<"$endpoint"
  [[ -z "${extra:-}" && -n "${label:-}" && -n "${tcp_addr:-}" && -n "${priority:-}" ]] \
    || die "endpoint must be label=host:port=priority: $endpoint"
  [[ "$priority" =~ ^[0-9]+$ ]] || die "endpoint priority must be an integer: $endpoint"
  printf '\n[[tcp_endpoints]]\n' >>"$config_file"
  first=0
  {
    printf 'label = %s\n' "$(toml_quote "$label")"
    printf 'tcp_addr = %s\n' "$(toml_quote "$tcp_addr")"
    printf 'priority = %s\n' "$priority"
  } >>"$config_file"
done
[[ "$first" -eq 0 ]] || die "VPSMAN_GATEWAY_ENDPOINTS did not contain any endpoints"
chmod 600 "$config_file"

unit_file="$service_name"
if [[ "$install_mode" == "root" ]]; then
  unit_path="/etc/systemd/system/$unit_file"
else
  unit_path="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user/$unit_file"
  mkdir -p "$(dirname "$unit_path")"
fi
{
  cat <<UNIT
[Unit]
Description=vpsman agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
UNIT
  if [[ "$install_mode" == "root" ]]; then
    printf 'User=%s\n' "$run_user"
  fi
  cat <<UNIT
ExecStart=$agent_bin --config $config_file
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
UNIT
} >"$unit_path"

systemctl "${systemctl_scope[@]}" daemon-reload
systemctl "${systemctl_scope[@]}" enable --now "$service_name"
log "installed direct gateway agent $VPSMAN_AGENT_CLIENT_ID using $config_file"
