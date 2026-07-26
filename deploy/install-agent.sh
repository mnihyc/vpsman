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

require_client_id() {
  local value="${VPSMAN_AGENT_CLIENT_ID:-}"
  local LC_ALL=C

  [[ -n "$value" ]] || die "VPSMAN_AGENT_CLIENT_ID is required"
  ((${#value} <= 128)) ||
    die "VPSMAN_AGENT_CLIENT_ID must not exceed 128 ASCII bytes"
  [[ "$value" =~ ^[0-9A-Za-z._:-]+$ ]] ||
    die "VPSMAN_AGENT_CLIENT_ID must contain only ASCII letters, digits, '.', '_', ':', and '-'"
}

require_safe_path_syntax() {
  local name="$1" value="$2" part
  local -a parts

  [[ "${#value}" -le 1024 ]] ||
    die "$name must not exceed 1024 characters"
  [[ "$value" =~ ^/[0-9A-Za-z._/+:@-]+$ ]] ||
    die "$name must be an absolute path using only systemd-safe path characters"
  [[ "$value" != "/" && "$value" != */ && "$value" != *//* ]] ||
    die "$name must identify a canonical non-root path"
  IFS='/' read -r -a parts <<<"$value"
  for part in "${parts[@]}"; do
    [[ -z "$part" || ( "$part" != "." && "$part" != ".." ) ]] ||
      die "$name must not contain dot or parent-directory traversal segments"
  done
}

require_no_symlink_components() {
  local name="$1" value="$2" part current=""
  local -a parts

  IFS='/' read -r -a parts <<<"$value"
  for part in "${parts[@]}"; do
    [[ -n "$part" ]] || continue
    current="$current/$part"
    [[ ! -L "$current" ]] ||
      die "$name must not traverse symbolic links: $current"
  done
}

require_safe_agent_home() {
  local value="$1" part
  local segment_count=0
  local -a parts

  require_safe_path_syntax "VPSMAN_AGENT_HOME" "$value"
  IFS='/' read -r -a parts <<<"$value"
  for part in "${parts[@]}"; do
    [[ -n "$part" ]] || continue
    ((segment_count += 1))
  done
  ((segment_count >= 2)) ||
    die "VPSMAN_AGENT_HOME must identify a dedicated directory below a filesystem top-level"
  require_no_symlink_components "VPSMAN_AGENT_HOME" "$value"
}

require_safe_agent_subpath() {
  local name="$1" value="$2" agent_home="$3"

  require_safe_path_syntax "$name" "$value"
  [[ "$value" == "$agent_home/"* ]] ||
    die "$name must remain inside VPSMAN_AGENT_HOME"
  require_no_symlink_components "$name" "$value"
}

require_safe_service_name() {
  local value="$1"

  [[ "${#value}" -le 255 &&
    "$value" =~ ^[0-9A-Za-z][0-9A-Za-z_.:@-]*\.service$ ]] ||
    die "VPSMAN_AGENT_SERVICE_NAME must be a safe systemd .service unit name"
}

require_bounded_duration() {
  local name="$1" value="$2" maximum="$3"

  [[ "$value" =~ ^(0|[1-9][0-9]*)$ &&
    "${#value}" -le "${#maximum}" ]] ||
    die "$name must be a canonical integer from 1 through $maximum"
  ((10#$value >= 1 && 10#$value <= maximum)) ||
    die "$name must be a canonical integer from 1 through $maximum"
}

require_regular_or_absent_target() {
  local name="$1" value="$2"

  if [[ -e "$value" || -L "$value" ]]; then
    [[ -f "$value" && ! -L "$value" ]] ||
      die "$name must be absent or an existing regular file"
  fi
}

valid_uint16() {
  local value="$1"
  [[ "$value" =~ ^[0-9]+$ && "${#value}" -le 5 ]] || return 1
  ((10#$value <= 65535))
}

valid_tcp_port() {
  local value="$1"
  valid_uint16 "$value" && ((10#$value > 0))
}

valid_ipv4_literal() {
  local value="$1" octet
  local -a octets
  [[ "$value" =~ ^([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})\.([0-9]{1,3})$ ]] ||
    return 1
  octets=("${BASH_REMATCH[@]:1}")
  for octet in "${octets[@]}"; do
    ((10#$octet <= 255)) || return 1
  done
}

valid_ipv6_literal() {
  local value="$1" ipv4_tail left right side group
  local group_count=0
  local -a groups

  [[ "$value" == *:* ]] || return 1
  if [[ "$value" == *.* ]]; then
    ipv4_tail="${value##*:}"
    [[ "$ipv4_tail" != "$value" ]] || return 1
    valid_ipv4_literal "$ipv4_tail" || return 1
    value="${value%:*}:0:0"
  fi
  [[ "$value" =~ ^[0-9A-Fa-f:]+$ ]] || return 1

  if [[ "$value" == *::* ]]; then
    left="${value%%::*}"
    right="${value#*::}"
    [[ "$right" != *::* && "$left" != *: && "$right" != :* ]] || return 1
    for side in "$left" "$right"; do
      [[ -n "$side" ]] || continue
      IFS=':' read -r -a groups <<<"$side"
      for group in "${groups[@]}"; do
        [[ "$group" =~ ^[0-9A-Fa-f]{1,4}$ ]] || return 1
        ((group_count += 1))
      done
    done
    ((group_count < 8))
    return
  fi

  [[ "$value" != :* && "$value" != *: ]] || return 1
  IFS=':' read -r -a groups <<<"$value"
  for group in "${groups[@]}"; do
    [[ "$group" =~ ^[0-9A-Fa-f]{1,4}$ ]] || return 1
    ((group_count += 1))
  done
  ((group_count == 8))
}

valid_hostname() {
  local value="$1" label
  local -a labels

  [[ "${#value}" -le 253 ]] || return 1
  value="${value%.}"
  [[ -n "$value" && "$value" != .* && "$value" != *. && "$value" != *..* ]] || return 1
  IFS='.' read -r -a labels <<<"$value"
  for label in "${labels[@]}"; do
    [[ "${#label}" -le 63 &&
      "$label" =~ ^[0-9A-Za-z]([0-9A-Za-z-]*[0-9A-Za-z])?$ ]] ||
      return 1
  done
}

valid_tcp_addr() {
  local value="$1" host port

  [[ "${#value}" -le 256 ]] || return 1
  if [[ "$value" == \[* ]]; then
    [[ "$value" =~ ^\[([^][]+)\]:([0-9]+)$ ]] || return 1
    host="${BASH_REMATCH[1]}"
    port="${BASH_REMATCH[2]}"
    valid_ipv6_literal "$host" || return 1
  else
    [[ "$value" =~ ^([^:]+):([0-9]+)$ ]] || return 1
    host="${BASH_REMATCH[1]}"
    port="${BASH_REMATCH[2]}"
    if [[ "$host" =~ ^[0-9.]+$ ]]; then
      valid_ipv4_literal "$host" || return 1
    else
      valid_hostname "$host" || return 1
    fi
  fi
  valid_tcp_port "$port"
}

gateway_endpoint_labels=()
gateway_endpoint_addrs=()
gateway_endpoint_priorities=()
parse_gateway_endpoints() {
  local endpoint label tcp_addr priority
  local -a raw_endpoints

  IFS=$'\n,' read -r -d '' -a raw_endpoints < <(printf '%s\0' "$VPSMAN_GATEWAY_ENDPOINTS") ||
    true
  for endpoint in "${raw_endpoints[@]}"; do
    endpoint="${endpoint%$'\r'}"
    [[ "$endpoint" =~ ^[[:space:]]*$ ]] && continue
    [[ "$endpoint" =~ ^([^=]+)=([^=]+)=([^=]+)$ ]] ||
      die "endpoint must be label=host:port=priority: $endpoint"
    label="${BASH_REMATCH[1]}"
    tcp_addr="${BASH_REMATCH[2]}"
    priority="${BASH_REMATCH[3]}"

    [[ "${#label}" -le 64 && "$label" =~ ^[0-9A-Za-z._:-]+$ ]] ||
      die "endpoint label must be 1-64 identifier characters: $endpoint"
    valid_tcp_addr "$tcp_addr" ||
      die "endpoint address must be IPv4:port, hostname:port, or [IPv6]:port: $endpoint"
    valid_uint16 "$priority" ||
      die "endpoint priority must be an integer from 0 through 65535: $endpoint"
    priority="$((10#$priority))"

    gateway_endpoint_labels+=("$label")
    gateway_endpoint_addrs+=("$tcp_addr")
    gateway_endpoint_priorities+=("$priority")
    ((${#gateway_endpoint_labels[@]} <= 16)) ||
      die "VPSMAN_GATEWAY_ENDPOINTS must not contain more than 16 endpoints"
  done
  ((${#gateway_endpoint_labels[@]} > 0)) ||
    die "VPSMAN_GATEWAY_ENDPOINTS did not contain any endpoints"
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

valid_release_tag() {
  local prerelease
  [[ "$1" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*))?$ ]] ||
    return 1
  prerelease="${BASH_REMATCH[5]:-}"
  [[ -z "$prerelease" || ! "$prerelease" =~ (^|\.)0[0-9]+($|\.) ]]
}

reject_runtime_config_env() {
  local name
  for name in "$@"; do
    [[ -z "${!name:-}" ]] || die "$name is server runtime config; do not set it in bootstrap agent install"
  done
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
  if [[ -n "${VPSMAN_RELEASE_BASE_URL:-}" ]]; then
    printf '%s\n' "${VPSMAN_RELEASE_BASE_URL%/}"
  elif [[ "$requested_release" == "latest" ]]; then
    printf 'https://github.com/%s/releases/latest/download\n' "${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}"
  else
    printf 'https://github.com/%s/releases/download/%s\n' "${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}" "$requested_release"
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
  valid_release_tag "$resolved_tag" ||
    die "release manifest does not contain a valid semantic-version tag"
  if [[ "$requested_release" != "latest" && "$resolved_tag" != "$requested_release" ]]; then
    die "release manifest resolved $resolved_tag but exact target $requested_release was requested"
  fi
  if [[ "$requested_release" == "latest" && "$resolved_tag" == *-* ]]; then
    die "the stable latest endpoint resolved prerelease $resolved_tag"
  fi
  pinned_base_url="$(release_pinned_base_url "$resolved_tag")"
  log "downloading $asset from $pinned_base_url"
  download_release_asset "$pinned_base_url/SHA256SUMS" "$download_dir/SHA256SUMS"
  download_release_asset "$pinned_base_url/$asset" "$download_dir/$asset"
  awk -v asset="$asset" \
    '$2 == asset || $2 == "version.json" {
       print
       seen[$2]++
     }
     END {
       exit (seen[asset] == 1 && seen["version.json"] == 1) ? 0 : 1
     }' \
    "$download_dir/SHA256SUMS" >"$download_dir/SHA256SUMS.selected" \
    || die "release checksum manifest does not contain the agent and version manifest exactly once"
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

requested_release="${VPSMAN_AGENT_RELEASE:-${VPSMAN_RELEASE_TAG:-latest}}"
if [[ "$requested_release" != "latest" ]] && ! valid_release_tag "$requested_release"; then
  die "VPSMAN_AGENT_RELEASE must be latest or an exact vX.Y.Z tag"
fi
if [[ -z "${VPSMAN_RELEASE_BASE_URL:-}" &&
  ! "${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}" =~ ^[0-9A-Za-z_.-]+/[0-9A-Za-z_.-]+$ ]]; then
  die "VPSMAN_RELEASE_REPO must be an owner/repository pair"
fi

install_mode="${VPSMAN_INSTALL_MODE:-root}"
case "$install_mode" in
  root|user|unprivileged) ;;
  *) die "VPSMAN_INSTALL_MODE must be root, user, or unprivileged" ;;
esac

require_client_id
require_env VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX
require_env VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX
require_env VPSMAN_GATEWAY_ENDPOINTS
require_hex32 VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX
require_hex32 VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX
reject_runtime_config_env \
  VPSMAN_AGENT_DISPLAY_NAME \
  VPSMAN_TELEMETRY_LIGHT_SECS \
  VPSMAN_TELEMETRY_FULL_SECS \
  VPSMAN_MAX_JOB_TIMEOUT_SECS \
  VPSMAN_AGENT_UNMANAGED_UPDATE_ENABLED \
  VPSMAN_AGENT_UNMANAGED_UPDATE_VERSION_URL \
  VPSMAN_AGENT_UNMANAGED_UPDATE_INTERVAL_SECS \
  VPSMAN_AGENT_UNMANAGED_UPDATE_JITTER_SECS \
  VPSMAN_AGENT_UNMANAGED_UPDATE_ACTIVATE \
  VPSMAN_AGENT_UNMANAGED_UPDATE_RESTART_AGENT

# Installer preflight must remain ahead of all directory, binary, config, and
# service mutations so bad input or a missing service manager cannot damage an
# existing install.
parse_gateway_endpoints
if service_enable_requested; then
  command -v systemctl >/dev/null 2>&1 ||
    die "systemctl is required when VPSMAN_AGENT_ENABLE_SERVICE=1"
fi

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
gateway_retry_secs="${VPSMAN_GATEWAY_RETRY_SECS:-60}"
gateway_connect_timeout_secs="${VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS:-10}"

require_safe_agent_home "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_INSTALL_DIR" "$install_dir" "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_CONFIG_DIR" "$config_dir" "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_STATE_DIR" "$state_dir" "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_LOG_DIR" "$log_dir" "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_SYSTEMD_DIR" "$systemd_dir" "$agent_home"
require_safe_service_name "$service_name"
require_safe_path_syntax "systemd unit path" "$systemd_dir/$service_name"
require_bounded_duration "VPSMAN_GATEWAY_RETRY_SECS" "$gateway_retry_secs" 3600
require_bounded_duration \
  "VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS" \
  "$gateway_connect_timeout_secs" \
  300
agent_bin="$install_dir/vpsman-agent"
config_file="$config_dir/agent.toml"
unit_file="$service_name"
unit_path="$systemd_dir/$unit_file"
require_safe_agent_subpath "agent binary path" "$agent_bin" "$agent_home"
require_safe_agent_subpath "agent config path" "$config_file" "$agent_home"
require_safe_agent_subpath "systemd unit path" "$unit_path" "$agent_home"
require_regular_or_absent_target "agent binary path" "$agent_bin"
require_regular_or_absent_target "agent config path" "$config_file"
require_regular_or_absent_target "systemd unit path" "$unit_path"

mkdir -p "$install_dir" "$config_dir" "$state_dir" "$log_dir" "$systemd_dir"
chmod 700 "$config_dir"

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

{
  printf 'client_id = %s\n' "$(toml_quote "$VPSMAN_AGENT_CLIENT_ID")"
  printf '\n[noise]\n'
  printf 'mode = "enrolled_ik"\n'
  printf 'client_private_key_hex = %s\n' "$(toml_quote "$VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX")"
  printf 'server_public_key_hex = %s\n' "$(toml_quote "$VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX")"
  printf '\n[auth]\n'
  printf 'gateway_retry_secs = %s\n' "$gateway_retry_secs"
  printf 'gateway_connect_timeout_secs = %s\n' "$gateway_connect_timeout_secs"
} >"$config_file"

for endpoint_index in "${!gateway_endpoint_labels[@]}"; do
  label="${gateway_endpoint_labels[$endpoint_index]}"
  tcp_addr="${gateway_endpoint_addrs[$endpoint_index]}"
  priority="${gateway_endpoint_priorities[$endpoint_index]}"
  printf '\n[[tcp_endpoints]]\n' >>"$config_file"
  {
    printf 'label = %s\n' "$(toml_quote "$label")"
    printf 'tcp_addr = %s\n' "$(toml_quote "$tcp_addr")"
    printf 'priority = %s\n' "$priority"
  } >>"$config_file"
done
chmod 600 "$config_file"

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
  systemctl "${systemctl_scope[@]}" link "$unit_path"
  systemctl "${systemctl_scope[@]}" daemon-reload
  systemctl "${systemctl_scope[@]}" enable --now "$service_name"
  log "installed and enabled direct gateway agent $VPSMAN_AGENT_CLIENT_ID using $config_file"
else
  log "installed direct gateway agent $VPSMAN_AGENT_CLIENT_ID using $config_file"
  log "staging-only install complete; no service was started"
  printf -v foreground_command 'VPSMAN_AGENT_STATE_DIR=%q %q --config %q run' \
    "$state_dir" "$agent_bin" "$config_file"
  log "start in foreground: $foreground_command"
fi
