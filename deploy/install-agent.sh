#!/usr/bin/env bash
set -euo pipefail

log() { printf '[vpsman-install] %s\n' "$*" >&2; }
die() { printf '[vpsman-install] error: %s\n' "$*" >&2; exit 1; }

require_env() {
  local name="$1"
  [[ -n "${!name:-}" ]] || die "$name is required"
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1"
}

require_hex32() {
  local name="$1" value="${!1:-}"
  [[ "$value" =~ ^[0-9A-Fa-f]{64}$ ]] || die "$name must be exactly 64 hex characters"
}

require_absolute_path() {
  local name="$1" value="$2"
  case "$value" in
    /*) ;;
    *) die "$name must be an absolute path" ;;
  esac
}

is_true() {
  case "${1:-0}" in
    1 | true | TRUE | yes | YES) return 0 ;;
    *) return 1 ;;
  esac
}

service_enable_requested() {
  is_true "${VPSMAN_AGENT_ENABLE_SERVICE:-${VPSMAN_ENABLE_SERVICE:-1}}"
}

require_uint() {
  local name="$1" value="${!1:-}"
  [[ "$value" =~ ^[0-9]+$ ]] || die "$name must be an unsigned integer"
}

cleanup_paths=()
cleanup() {
  local path
  for path in "${cleanup_paths[@]:-}"; do
    rm -rf "$path"
  done
}
trap cleanup EXIT

register_cleanup_path() {
  cleanup_paths+=("$1")
}

release_base_url() {
  local release="${VPSMAN_AGENT_RELEASE:-${VPSMAN_RELEASE_TAG:-latest}}"
  if [[ -n "${VPSMAN_RELEASE_BASE_URL:-}" ]]; then
    printf '%s\n' "${VPSMAN_RELEASE_BASE_URL%/}"
  elif [[ "$release" == "latest" ]]; then
    printf 'https://github.com/%s/releases/latest/download\n' "${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}"
  else
    printf 'https://github.com/%s/releases/download/%s\n' "${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}" "$release"
  fi
}

release_pinned_base_url() {
  local tag="$1"
  if [[ -n "${VPSMAN_RELEASE_BASE_URL:-}" ]]; then
    printf '%s\n' "${VPSMAN_RELEASE_BASE_URL%/}"
  else
    printf 'https://github.com/%s/releases/download/%s\n' "${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}" "$tag"
  fi
}

extract_release_tag() {
  local metadata="$1"
  sed -n 's/.*"tag"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$metadata" | head -n 1
}

agent_release_asset() {
  local machine
  machine="$(uname -m)"
  case "$machine" in
    x86_64|amd64) printf 'vpsman-agent-linux-x86_64-musl\n' ;;
    aarch64|arm64) printf 'vpsman-agent-linux-aarch64-musl\n' ;;
    *) die "unsupported machine architecture for default agent download: $machine" ;;
  esac
}

download_release_asset() {
  local url="$1"
  local output="$2"
  local headers=()
  if [[ -n "${GITHUB_TOKEN:-}" ]]; then
    headers=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
  fi
  curl -fL --retry 3 --connect-timeout 10 "${headers[@]}" -o "$output" "$url"
}

download_default_agent_binary() {
  local output="$1"
  local asset base_url pinned_base_url download_dir resolved_tag
  asset="$(agent_release_asset)"
  base_url="$(release_base_url)"
  download_dir="$(mktemp -d)"
  register_cleanup_path "$download_dir"

  require_tool curl
  require_tool sha256sum
  require_tool awk

  download_release_asset "$base_url/version.json" "$download_dir/version.json"
  resolved_tag="$(extract_release_tag "$download_dir/version.json")"
  [[ -n "$resolved_tag" ]] || die "release manifest does not contain a tag"
  pinned_base_url="$(release_pinned_base_url "$resolved_tag")"
  log "downloading $asset from $pinned_base_url"
  download_release_asset "$pinned_base_url/$asset" "$download_dir/$asset"
  download_release_asset "$pinned_base_url/SHA256SUMS" "$download_dir/SHA256SUMS"
  awk -v asset="$asset" '$2 == asset { print; found = 1 } END { exit found ? 0 : 1 }' \
    "$download_dir/SHA256SUMS" >"$download_dir/SHA256SUMS.selected" \
    || die "release checksum manifest does not contain $asset"
  (cd "$download_dir" && sha256sum -c SHA256SUMS.selected >/dev/null)
  install -m 0755 "$download_dir/$asset" "$output"
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
  root|user|unprivileged) ;;
  *) die "VPSMAN_INSTALL_MODE must be root, user, or unprivileged" ;;
esac

require_env VPSMAN_AGENT_CLIENT_ID
require_env VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX
require_env VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX
require_env VPSMAN_GATEWAY_ENDPOINTS
require_hex32 VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX
require_hex32 VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX

if [[ "$install_mode" == "root" ]]; then
  [[ "$(id -u)" -eq 0 ]] || die "root install mode must run as root"
  agent_home="${VPSMAN_AGENT_HOME:-/opt/vpsman-agent}"
  run_user="root"
  systemctl_scope=()
else
  agent_home="${VPSMAN_AGENT_HOME:-$(pwd)/vpsman-agent}"
  run_user="${USER:-vpsman}"
  systemctl_scope=(--user)
fi
install_dir="${VPSMAN_AGENT_INSTALL_DIR:-$agent_home/bin}"
config_dir="${VPSMAN_AGENT_CONFIG_DIR:-$agent_home/config}"
state_dir="${VPSMAN_AGENT_STATE_DIR:-$agent_home/state}"
log_dir="${VPSMAN_AGENT_LOG_DIR:-$agent_home/log}"
systemd_dir="${VPSMAN_AGENT_SYSTEMD_DIR:-$agent_home/systemd}"
service_name="${VPSMAN_AGENT_SERVICE_NAME:-vpsman-agent.service}"

require_absolute_path "VPSMAN_AGENT_HOME" "$agent_home"
require_absolute_path "VPSMAN_AGENT_INSTALL_DIR" "$install_dir"
require_absolute_path "VPSMAN_AGENT_CONFIG_DIR" "$config_dir"
require_absolute_path "VPSMAN_AGENT_STATE_DIR" "$state_dir"
require_absolute_path "VPSMAN_AGENT_LOG_DIR" "$log_dir"
require_absolute_path "VPSMAN_AGENT_SYSTEMD_DIR" "$systemd_dir"

mkdir -p "$install_dir" "$config_dir" "$state_dir" "$log_dir" "$systemd_dir"
chmod 700 "$config_dir"
agent_bin="$install_dir/vpsman-agent"

if [[ -n "${VPSMAN_AGENT_BINARY_PATH:-}" ]]; then
  install -m 0755 "$VPSMAN_AGENT_BINARY_PATH" "$agent_bin"
elif [[ -n "${VPSMAN_AGENT_BINARY_URL:-}" ]]; then
  tmp_bin="$(mktemp)"
  register_cleanup_path "$tmp_bin"
  require_tool curl
  require_tool sha256sum
  require_hex32 VPSMAN_AGENT_BINARY_SHA256
  curl -fsSL "$VPSMAN_AGENT_BINARY_URL" -o "$tmp_bin"
  printf '%s  %s\n' "$VPSMAN_AGENT_BINARY_SHA256" "$tmp_bin" | sha256sum -c - >/dev/null
  install -m 0755 "$tmp_bin" "$agent_bin"
elif is_true "${VPSMAN_AGENT_USE_PATH:-0}" && command -v vpsman-agent >/dev/null 2>&1; then
  cp "$(command -v vpsman-agent)" "$agent_bin"
  chmod 0755 "$agent_bin"
else
  download_default_agent_binary "$agent_bin"
fi

config_file="$config_dir/agent.toml"
VPSMAN_AGENT_UNMANAGED_UPDATE_INTERVAL_SECS="${VPSMAN_AGENT_UNMANAGED_UPDATE_INTERVAL_SECS:-86400}"
VPSMAN_AGENT_UNMANAGED_UPDATE_JITTER_SECS="${VPSMAN_AGENT_UNMANAGED_UPDATE_JITTER_SECS:-86400}"
require_uint VPSMAN_AGENT_UNMANAGED_UPDATE_INTERVAL_SECS
require_uint VPSMAN_AGENT_UNMANAGED_UPDATE_JITTER_SECS
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
  printf 'command_timeout_secs = %s\n' "${VPSMAN_COMMAND_TIMEOUT_SECS:-30}"
  printf 'gateway_retry_secs = %s\n' "${VPSMAN_GATEWAY_RETRY_SECS:-60}"
  printf 'gateway_connect_timeout_secs = %s\n' "${VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS:-10}"
  printf '\n[update]\n'
  printf 'unmanaged_enabled = %s\n' "$(is_true "${VPSMAN_AGENT_UNMANAGED_UPDATE_ENABLED:-0}" && printf true || printf false)"
  printf 'unmanaged_version_url = %s\n' "$(toml_quote "${VPSMAN_AGENT_UNMANAGED_UPDATE_VERSION_URL:-https://github.com/mnihyc/vpsman/releases/latest/download/version.json}")"
  printf 'unmanaged_interval_secs = %s\n' "$VPSMAN_AGENT_UNMANAGED_UPDATE_INTERVAL_SECS"
  printf 'unmanaged_jitter_secs = %s\n' "$VPSMAN_AGENT_UNMANAGED_UPDATE_JITTER_SECS"
  printf 'unmanaged_activate = %s\n' "$(is_true "${VPSMAN_AGENT_UNMANAGED_UPDATE_ACTIVATE:-1}" && printf true || printf false)"
  printf 'unmanaged_restart_agent = %s\n' "$(is_true "${VPSMAN_AGENT_UNMANAGED_UPDATE_RESTART_AGENT:-1}" && printf true || printf false)"
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
unit_path="$systemd_dir/$unit_file"
{
  cat <<UNIT
[Unit]
Description=vpsman agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$agent_home
UNIT
  if [[ "$install_mode" == "root" ]]; then
    printf 'User=%s\n' "$run_user"
  fi
  cat <<UNIT
ExecStart=$agent_bin --config $config_file run
Environment=VPSMAN_AGENT_RESTART_MODE=signal_only
Restart=always
RestartSec=5
UMask=0077

[Install]
WantedBy=default.target
UNIT
} >"$unit_path"

if service_enable_requested; then
  command -v systemctl >/dev/null 2>&1 || die "systemctl is required when VPSMAN_AGENT_ENABLE_SERVICE=1"
  systemctl "${systemctl_scope[@]}" link "$unit_path"
  systemctl "${systemctl_scope[@]}" daemon-reload
  systemctl "${systemctl_scope[@]}" enable --now "$service_name"
  log "installed and enabled direct gateway agent $VPSMAN_AGENT_CLIENT_ID using $config_file"
else
  log "installed direct gateway agent $VPSMAN_AGENT_CLIENT_ID using $config_file"
  log "systemd unit written to $unit_path; set VPSMAN_AGENT_ENABLE_SERVICE=0 only for staging-only installs"
fi
