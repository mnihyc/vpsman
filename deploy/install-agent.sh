#!/usr/bin/env bash
set -euo pipefail
umask 077

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

require_trusted_directory() {
  local name="$1" value="$2"
  local owner mode mode_value

  [[ -d "$value" && ! -L "$value" ]] ||
    die "$name must be an existing non-symlink directory: $value"
  owner="$(stat -c '%u' -- "$value")" ||
    die "could not inspect ownership for $name: $value"
  mode="$(stat -c '%a' -- "$value")" ||
    die "could not inspect permissions for $name: $value"
  [[ "$owner" == "$installer_uid" ]] ||
    die "$name must be owned by installer uid $installer_uid: $value"
  mode_value=$((8#$mode))
  (( (mode_value & 0022) == 0 )) ||
    die "$name must not be group- or world-writable: $value"
}

require_trusted_creation_chain() {
  local name="$1" value="$2" part current=""
  local owner mode mode_value writable_mask
  local -a parts

  # Root must not delegate any writable ancestor to another principal. In
  # unprivileged mode an explicitly writable Unix group is part of the caller's
  # filesystem trust boundary, while world-writable ancestry still requires a
  # sticky, already-created child.
  if [[ "$install_mode" == "root" ]]; then
    writable_mask=0022
  else
    writable_mask=0002
  fi

  IFS='/' read -r -a parts <<<"$value"
  for part in "${parts[@]}"; do
    [[ -n "$part" ]] || continue
    current="$current/$part"
    [[ -d "$current" && ! -L "$current" ]] ||
      die "$name creation path must contain only existing non-symlink directories: $current"
    owner="$(stat -c '%u' -- "$current")" ||
      die "could not inspect ownership for $name creation path: $current"
    [[ "$owner" == "$installer_uid" || "$owner" == "0" ]] ||
      die "$name creation path must be owned by the installer user or root: $current"
    mode="$(stat -c '%a' -- "$current")" ||
      die "could not inspect permissions for $name creation path: $current"
    mode_value=$((8#$mode))
    if (( (mode_value & writable_mask) != 0 )); then
      # A sticky directory protects an already existing child owned by the
      # installer or root, but is not a safe anchor for a new child name.
      if (( (mode_value & 01000) == 0 )) || [[ "$current" == "$value" ]]; then
        die "$name creation path has unsafe writable ancestor: $current"
      fi
    fi
  done
}

record_missing_directory_chain() {
  local label="$1" current="$2" parent

  while [[ ! -e "$current" && ! -L "$current" ]]; do
    if [[ -z "${created_directory_seen[$current]:-}" ]]; then
      created_directories+=("$current")
      created_directory_seen["$current"]=1
    fi
    parent="${current%/*}"
    [[ -n "$parent" ]] || parent="/"
    [[ "$parent" != "$current" ]] ||
      die "could not resolve directory ancestry for $label"
    current="$parent"
  done
  require_trusted_creation_chain "$label" "$current"
}

remove_created_directories() {
  local pass path progress failed=0

  for ((pass = 0; pass <= ${#created_directories[@]}; pass++)); do
    progress=0
    for path in "${created_directories[@]}"; do
      if [[ -d "$path" && ! -L "$path" ]] && rmdir -- "$path" 2>/dev/null; then
        progress=1
      fi
    done
    ((progress)) || break
  done
  for path in "${created_directories[@]}"; do
    if [[ -e "$path" || -L "$path" ]]; then
      log "left newly created non-empty directory in place during rollback: $path"
      failed=1
    fi
  done
  ((failed == 0))
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
    [[ "${#octet}" -eq 1 || "$octet" != 0* ]] || return 1
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

require_boolean_if_set() {
  local name="$1"

  [[ -v "$name" ]] || return 0
  case "${!name}" in
    0 | 1 | false | FALSE | no | NO | true | TRUE | yes | YES) ;;
    *) die "$name must be a boolean: 0, 1, false, true, no, or yes" ;;
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
rollback_paths=()
created_directories=()
declare -A created_directory_seen=()
transaction_started=0
transaction_committed=0
rollback_incomplete=0
install_lock_acquired=0
service_lock_path=""
received_signal=""
signals_deferred=0
deferred_signal=""
deferred_signal_status=0

cleanup_disposable_paths() {
  local path
  for path in "${cleanup_paths[@]}"; do
    rm -rf -- "$path"
  done
}

cleanup_rollback_paths() {
  local path
  for path in "${rollback_paths[@]}"; do
    rm -rf -- "$path"
  done
}

register_cleanup_path() {
  cleanup_paths+=("$1")
}

register_rollback_path() {
  rollback_paths+=("$1")
}

handle_signal() {
  local signal_name="$1" exit_status="$2"

  if ((signals_deferred)); then
    if [[ -z "$deferred_signal" ]]; then
      deferred_signal="$signal_name"
      deferred_signal_status="$exit_status"
    fi
    return
  fi
  received_signal="$signal_name"
  exit "$exit_status"
}

deliver_deferred_signal() {
  local signal_name="$deferred_signal" exit_status="$deferred_signal_status"

  deferred_signal=""
  deferred_signal_status=0
  [[ -z "$signal_name" ]] || handle_signal "$signal_name" "$exit_status"
}

create_registered_temp() {
  local output_name="$1" ownership="$2" path status=0
  shift 2

  case "$ownership" in
    disposable | rollback) ;;
    *) return 2 ;;
  esac
  signals_deferred=1
  path="$(mktemp "$@")" || status=$?
  if ((status == 0)); then
    case "$ownership" in
      disposable) register_cleanup_path "$path" ;;
      rollback) register_rollback_path "$path" ;;
    esac
    printf -v "$output_name" '%s' "$path"
  fi
  signals_deferred=0
  deliver_deferred_signal
  return "$status"
}

handle_exit() {
  local exit_status="$1" path

  # Once rollback begins, a second interactive signal must not strand a mix of
  # old and new files. Preserve the first exit status while restoration and
  # cleanup run to completion.
  trap '' INT TERM HUP
  trap - EXIT
  if ((transaction_started && transaction_committed == 0)); then
    if [[ -n "$received_signal" ]]; then
      log "received $received_signal during installation; restoring the prior state"
    else
      log "installation exited before commit; restoring the prior state"
    fi
    if ! rollback_install_transaction; then
      rollback_incomplete=1
      exit_status=1
      log "automatic rollback was incomplete; preserved rollback originals for manual recovery:"
      for path in "${rollback_paths[@]}"; do
        [[ -e "$path" ]] && log "  $path"
      done
    fi
  fi

  cleanup_disposable_paths
  if ((rollback_incomplete == 0)); then
    cleanup_rollback_paths
  fi
  if ((install_lock_acquired && transaction_started == 0)); then
    remove_created_directories || exit_status=1
  fi
  exit "$exit_status"
}

trap 'handle_exit $?' EXIT
trap 'handle_signal HUP 129' HUP
trap 'handle_signal INT 130' INT
trap 'handle_signal TERM 143' TERM

release_base_url() {
  if [[ -n "${VPSMAN_RELEASE_BASE_URL:-}" ]]; then
    printf '%s\n' "${VPSMAN_RELEASE_BASE_URL%/}"
  elif [[ "$requested_release" == "latest" ]]; then
    printf 'https://github.com/%s/releases/latest/download\n' "${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}"
  else
    printf 'https://github.com/%s/releases/download/%s\n' "${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}" "$requested_release"
  fi
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

read_release_manifest_selection() {
  local metadata="$1"
  local selected_asset="$2"

  awk -v selected_asset="$selected_asset" '
    function json_string_value(line, key, prefix) {
      prefix = "^[[:space:]]*\"" key "\"[[:space:]]*:[[:space:]]*\""
      if (line !~ prefix || line !~ /"[[:space:]]*,?[[:space:]]*$/) {
        return ""
      }
      sub(prefix, "", line)
      sub(/"[[:space:]]*,?[[:space:]]*$/, "", line)
      if (line ~ /["\\]/) {
        invalid = 1
        return ""
      }
      return line
    }

    /^[[:space:]]*"schema_version"[[:space:]]*:/ {
      line = $0
      sub(/^[[:space:]]*"schema_version"[[:space:]]*:[[:space:]]*/, "", line)
      sub(/[[:space:]]*,?[[:space:]]*$/, "", line)
      schema_count++
      schema = line
    }
    /^[[:space:]]*"project"[[:space:]]*:/ {
      project_count++
      project = json_string_value($0, "project")
    }
    /^[[:space:]]*"tag"[[:space:]]*:/ {
      tag_count++
      tag = json_string_value($0, "tag")
    }
    /^[[:space:]]*\{[[:space:]]*$/ {
      object_open = 1
      object_name = ""
      object_url = ""
      next
    }
    object_open && /^[[:space:]]*"name"[[:space:]]*:/ {
      object_name = json_string_value($0, "name")
      next
    }
    object_open && /^[[:space:]]*"download_url"[[:space:]]*:/ {
      object_url = json_string_value($0, "download_url")
      next
    }
    object_open && /^[[:space:]]*\}[[:space:]]*,?[[:space:]]*$/ {
      if (object_name == selected_asset) {
        selected_count++
        selected_url = object_url
      }
      object_open = 0
    }
    END {
      valid = !invalid && schema_count == 1 && (schema == "2" || schema == "3")
      valid = valid && project_count == 1 && project == "vpsman"
      valid = valid && tag_count == 1 && tag != ""
      valid = valid && selected_count == 1 && selected_url != ""
      if (!valid) {
        exit 1
      }
      printf "%s\t%s\n", tag, selected_url
    }
  ' "$metadata"
}

download_release_asset() {
  local url="$1"
  local output="$2"
  local headers=()
  if [[ -n "${GITHUB_TOKEN:-}" ]]; then
    headers=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
  fi
  curl -fL --retry 3 --connect-timeout 10 "${headers[@]}" -o "$output" -- "$url"
}

download_default_agent_binary() {
  local output="$1"
  local asset asset_url base_url download_dir manifest_selection resolved_tag
  local LC_ALL=C
  asset="$(agent_release_asset)"
  base_url="$(release_base_url)"
  create_registered_temp download_dir disposable -d

  require_tool curl

  download_release_asset "$base_url/version.json" "$download_dir/version.json"
  manifest_selection="$(
    read_release_manifest_selection "$download_dir/version.json" "$asset"
  )" ||
    die "release manifest schema, identity, or selected agent asset is invalid"
  IFS=$'\t' read -r resolved_tag asset_url <<<"$manifest_selection"
  valid_release_tag "$resolved_tag" ||
    die "release manifest does not contain a valid semantic-version tag"
  if [[ "$requested_release" != "latest" && "$resolved_tag" != "$requested_release" ]]; then
    die "release manifest resolved $resolved_tag but exact target $requested_release was requested"
  fi
  if [[ "$requested_release" == "latest" && "$resolved_tag" == *-* ]]; then
    die "the stable latest endpoint resolved prerelease $resolved_tag"
  fi
  [[ "$asset_url" =~ ^https://[^/?#[:space:]@]+/[^#[:space:]]+$ &&
    "$asset_url" =~ ^[!-~]+$ &&
    "$asset_url" != *\\* ]] ||
    die "release manifest selected an invalid HTTPS download URL for $asset"
  log "downloading $asset selected by release $resolved_tag"
  download_release_asset "$asset_url" "$download_dir/$asset"
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
require_boolean_if_set VPSMAN_AGENT_ENABLE_SERVICE
require_boolean_if_set VPSMAN_ENABLE_SERVICE
require_boolean_if_set VPSMAN_AGENT_USE_PATH
parse_gateway_endpoints
if service_enable_requested; then
  command -v systemctl >/dev/null 2>&1 ||
    die "systemctl is required when VPSMAN_AGENT_ENABLE_SERVICE=1"
fi

require_tool id
installer_uid="$(id -u)"
if [[ "$install_mode" == "root" ]]; then
  [[ "$installer_uid" -eq 0 ]] || die "root install mode must run as root"
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
canonical_service_name="vpsman-agent.service"
gateway_retry_secs="${VPSMAN_GATEWAY_RETRY_SECS:-60}"
gateway_connect_timeout_secs="${VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS:-10}"

require_safe_agent_home "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_INSTALL_DIR" "$install_dir" "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_CONFIG_DIR" "$config_dir" "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_STATE_DIR" "$state_dir" "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_LOG_DIR" "$log_dir" "$agent_home"
require_safe_agent_subpath "VPSMAN_AGENT_SYSTEMD_DIR" "$systemd_dir" "$agent_home"
require_safe_service_name "$service_name"
[[ "$service_name" == "$canonical_service_name" ]] ||
  die "VPSMAN_AGENT_SERVICE_NAME must be $canonical_service_name; custom service identities are not upgrade-safe"
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

managed_dir_labels=(
  VPSMAN_AGENT_INSTALL_DIR
  VPSMAN_AGENT_CONFIG_DIR
  VPSMAN_AGENT_STATE_DIR
  VPSMAN_AGENT_LOG_DIR
  VPSMAN_AGENT_SYSTEMD_DIR
)
managed_dirs=(
  "$install_dir"
  "$config_dir"
  "$state_dir"
  "$log_dir"
  "$systemd_dir"
)
managed_file_labels=("agent binary" "agent config" "systemd unit")
managed_files=("$agent_bin" "$config_file" "$unit_path")
for managed_dir_index in "${!managed_dirs[@]}"; do
  for managed_file_index in "${!managed_files[@]}"; do
    managed_dir="${managed_dirs[$managed_dir_index]}"
    managed_file="${managed_files[$managed_file_index]}"
    if [[ "$managed_dir" == "$managed_file" ||
      "$managed_dir" == "$managed_file/"* ]]; then
      die "${managed_dir_labels[$managed_dir_index]} must not equal or be nested below the managed ${managed_file_labels[$managed_file_index]} target"
    fi
  done
done
for ((managed_file_index = 0; managed_file_index < ${#managed_files[@]}; managed_file_index++)); do
  for ((other_file_index = managed_file_index + 1; other_file_index < ${#managed_files[@]}; other_file_index++)); do
    [[ "${managed_files[$managed_file_index]}" != "${managed_files[$other_file_index]}" ]] ||
      die "managed file targets must be distinct"
  done
done

require_regular_or_absent_target "agent binary path" "$agent_bin"
require_regular_or_absent_target "agent config path" "$config_file"
require_regular_or_absent_target "systemd unit path" "$unit_path"

canonical_default_agent_bin="$agent_home/bin/vpsman-agent"
canonical_default_config_file="$agent_home/config/agent.toml"
canonical_requested_unit_path="$systemd_dir/$canonical_service_name"
canonical_default_unit_path="$agent_home/systemd/$canonical_service_name"
live_install_present=0
install_markers=(
  "$agent_bin"
  "$config_file"
  "$unit_path"
  "$canonical_default_agent_bin"
  "$canonical_default_config_file"
  "$canonical_requested_unit_path"
  "$canonical_default_unit_path"
)
for install_marker in "${install_markers[@]}"; do
  if [[ -e "$install_marker" || -L "$install_marker" ]]; then
    live_install_present=1
    break
  fi
done
binary_path="${VPSMAN_AGENT_BINARY_PATH:-}"
binary_url="${VPSMAN_AGENT_BINARY_URL:-}"
use_path="${VPSMAN_AGENT_USE_PATH:-0}"
binary_source_count=0
[[ -z "$binary_path" ]] || ((binary_source_count += 1))
[[ -z "$binary_url" ]] || ((binary_source_count += 1))
is_true "$use_path" && ((binary_source_count += 1))
((binary_source_count <= 1)) ||
  die "set only one of VPSMAN_AGENT_BINARY_PATH, VPSMAN_AGENT_BINARY_URL, or VPSMAN_AGENT_USE_PATH=1"
if [[ -n "${VPSMAN_AGENT_BINARY_SHA256:-}" && -z "$binary_url" ]]; then
  die "VPSMAN_AGENT_BINARY_SHA256 is only valid with VPSMAN_AGENT_BINARY_URL"
fi

binary_source="release"
source_agent_bin=""
if [[ -n "$binary_path" ]]; then
  binary_source="file"
  source_agent_bin="$binary_path"
  [[ -f "$source_agent_bin" && -r "$source_agent_bin" ]] ||
    die "VPSMAN_AGENT_BINARY_PATH must be a readable regular file"
elif [[ -n "$binary_url" ]]; then
  binary_source="url"
  [[ "$binary_url" != *$'\r'* && "$binary_url" != *$'\n'* ]] ||
    die "VPSMAN_AGENT_BINARY_URL must not contain control characters"
  require_hex32 VPSMAN_AGENT_BINARY_SHA256
elif is_true "$use_path"; then
  binary_source="path"
  source_agent_bin="$(command -v vpsman-agent || true)"
  [[ -n "$source_agent_bin" && -f "$source_agent_bin" && -r "$source_agent_bin" ]] ||
    die "VPSMAN_AGENT_USE_PATH=1 requires vpsman-agent to resolve to a readable regular file"
fi

for tool in cat chmod flock install ln mkdir mktemp mv rm rmdir stat; do
  require_tool "$tool"
done
case "$binary_source" in
  url)
    require_tool curl
    require_tool sha256sum
    ;;
  release)
    for tool in awk curl uname; do
      require_tool "$tool"
    done
    agent_release_asset >/dev/null
    ;;
esac

if service_enable_requested; then
  if [[ "$install_mode" == "root" ]]; then
    service_lock_dir="/run"
  else
    service_lock_dir="/run/user/$installer_uid"
  fi
  require_trusted_directory \
    "service-manager lock directory" \
    "$service_lock_dir"
  service_lock_path="$service_lock_dir/.vpsman-agent-install.lock"
  require_regular_or_absent_target \
    "service-manager install lock" \
    "$service_lock_path"
  exec {service_lock_fd}>>"$service_lock_path" ||
    die "could not open service-manager install lock: $service_lock_path"
  flock -n "$service_lock_fd" ||
    die "another vpsman agent service install is already in progress for $service_name"
fi

record_missing_directory_chain "VPSMAN_AGENT_HOME" "$agent_home"

staged_agent_bin=""
create_registered_temp staged_agent_bin disposable
case "$binary_source" in
  file | path)
    install -m 0755 -- "$source_agent_bin" "$staged_agent_bin"
    ;;
  url)
    curl -fsSL -o "$staged_agent_bin" -- "$binary_url"
    printf '%s  %s\n' "$VPSMAN_AGENT_BINARY_SHA256" "$staged_agent_bin" |
      sha256sum -c - >/dev/null
    chmod 0755 "$staged_agent_bin"
    ;;
  release)
    download_default_agent_binary "$staged_agent_bin"
    ;;
esac
[[ -f "$staged_agent_bin" && ! -L "$staged_agent_bin" && -s "$staged_agent_bin" ]] ||
  die "selected agent binary source did not produce a non-empty regular file"

service_state_captured=0
service_load_state=""
service_active_state=""
service_unit_file_state=""
service_fragment_path=""
service_registration="unlinked"
service_was_active=0
agent_home_preexisted=0
if [[ -d "$agent_home" && ! -L "$agent_home" ]]; then
  agent_home_preexisted=1
fi

capture_service_snapshot() {
  local output line key value

  service_load_state=""
  service_active_state=""
  service_unit_file_state=""
  service_fragment_path=""
  output="$(
    systemctl "${systemctl_scope[@]}" show "$service_name" \
      --no-pager \
      --property=LoadState \
      --property=ActiveState \
      --property=UnitFileState \
      --property=FragmentPath
  )" || return 1
  while IFS= read -r line; do
    key="${line%%=*}"
    value="${line#*=}"
    case "$key" in
      LoadState) service_load_state="$value" ;;
      ActiveState) service_active_state="$value" ;;
      UnitFileState) service_unit_file_state="$value" ;;
      FragmentPath) service_fragment_path="$value" ;;
    esac
  done <<<"$output"
  [[ -n "$service_load_state" && -n "$service_active_state" ]]
}

classify_service_snapshot() {
  case "$service_active_state" in
    active)
      service_was_active=1
      ;;
    inactive)
      service_was_active=0
      ;;
    *)
      die "$service_name has unsupported active state $service_active_state; make it exactly active or inactive before installing"
      ;;
  esac

  case "$service_load_state" in
    not-found)
      [[ -z "$service_fragment_path" ]] ||
        die "$service_name reports an unexpected fragment while unregistered: $service_fragment_path"
      case "$service_unit_file_state" in
        "" | not-found) ;;
        masked | masked-runtime)
          die "$service_name is masked; unmask and remove the conflicting registration before installing"
          ;;
        linked-runtime | enabled-runtime)
          die "$service_name uses unsupported runtime registration state $service_unit_file_state"
          ;;
        *)
          die "$service_name has inconsistent unregistered state $service_unit_file_state"
          ;;
      esac
      ((service_was_active == 0)) ||
        die "$service_name is active without a supported unit registration"
      service_registration="unlinked"
      ;;
    loaded)
      [[ "$service_fragment_path" == "$unit_path" ]] ||
        die "$service_name is owned by external unit $service_fragment_path; choose a different service name or remove the conflict"
      [[ -e "$unit_path" ]] ||
        die "$service_name is registered to missing owned unit $unit_path; repair or remove the stale registration first"
      case "$service_unit_file_state" in
        enabled)
          service_registration="enabled"
          ;;
        linked | disabled)
          die "$service_name has unsupported preexisting state $service_unit_file_state; use an unregistered staged unit or an already enabled owned unit"
          ;;
        linked-runtime | enabled-runtime)
          die "$service_name uses unsupported runtime registration state $service_unit_file_state"
          ;;
        masked | masked-runtime)
          die "$service_name is masked; unmask it before installing"
          ;;
        *)
          die "$service_name has unsupported unit-file state ${service_unit_file_state:-empty}; expected enabled"
          ;;
      esac
      ;;
    masked)
      die "$service_name is masked; unmask it before installing"
      ;;
    *)
      die "$service_name has unsupported load state $service_load_state"
      ;;
  esac
}

if ((agent_home_preexisted == 0)) &&
  { service_enable_requested || ((live_install_present)); }; then
  if command -v systemctl >/dev/null 2>&1; then
    capture_service_snapshot ||
      die "could not capture systemd state for $service_name"
    service_state_captured=1
    classify_service_snapshot
  elif service_enable_requested; then
    die "systemctl is required when VPSMAN_AGENT_ENABLE_SERVICE=1"
  else
    die "systemctl is required to prove an existing staging-only unit is unregistered"
  fi
fi

if ! service_enable_requested && ((service_state_captured)); then
  [[ "$service_registration" == "unlinked" && "$service_active_state" == "inactive" ]] ||
    die "staging-only install refuses registered $service_name ($service_registration/$service_active_state); enable service management or unregister it first"
fi

mkdir -p "$agent_home"
exec {install_lock_fd}<"$agent_home" ||
  die "could not open VPSMAN_AGENT_HOME for installation locking"
flock -n "$install_lock_fd" ||
  die "another vpsman agent install is already in progress for $agent_home"
install_lock_acquired=1

# A cooperative lock prevents concurrent installer transactions. Revalidate
# every path and service snapshot while holding it so a process reaching the
# lock after another install cannot act on stale preflight state. A hostile
# process running as the installer uid is already inside this trust boundary.
require_safe_agent_home "$agent_home"
require_trusted_creation_chain "VPSMAN_AGENT_HOME" "$agent_home"
require_trusted_directory "VPSMAN_AGENT_HOME" "$agent_home"
for managed_dir_index in "${!managed_dirs[@]}"; do
  managed_dir="${managed_dirs[$managed_dir_index]}"
  require_safe_agent_subpath \
    "${managed_dir_labels[$managed_dir_index]}" \
    "$managed_dir" \
    "$agent_home"
  if [[ -e "$managed_dir" || -L "$managed_dir" ]]; then
    require_trusted_directory \
      "${managed_dir_labels[$managed_dir_index]}" \
      "$managed_dir"
  fi
done
require_regular_or_absent_target "agent binary path" "$agent_bin"
require_regular_or_absent_target "agent config path" "$config_file"
require_regular_or_absent_target "systemd unit path" "$unit_path"

live_install_present=0
for install_marker in "${install_markers[@]}"; do
  if [[ -e "$install_marker" || -L "$install_marker" ]]; then
    live_install_present=1
    break
  fi
done
service_state_captured=0
service_load_state=""
service_active_state=""
service_unit_file_state=""
service_fragment_path=""
service_registration="unlinked"
service_was_active=0
if service_enable_requested || ((live_install_present)); then
  if command -v systemctl >/dev/null 2>&1; then
    capture_service_snapshot ||
      die "could not capture systemd state for $service_name"
    service_state_captured=1
    classify_service_snapshot
  elif service_enable_requested; then
    die "systemctl is required when VPSMAN_AGENT_ENABLE_SERVICE=1"
  else
    die "systemctl is required to prove an existing staging-only unit is unregistered"
  fi
fi
if ! service_enable_requested && ((service_state_captured)); then
  [[ "$service_registration" == "unlinked" && "$service_active_state" == "inactive" ]] ||
    die "staging-only install refuses registered $service_name ($service_registration/$service_active_state); enable service management or unregister it first"
fi

config_dir_preexisted=0
config_dir_mode=""
rollback_agent_dir=""
rollback_config_dir=""
rollback_unit_dir=""
service_mutation_started=0
service_link_attempted=0
service_enable_attempted=0
service_activation_attempted=0
service_stopped_for_rollback=0
publish_agent_bin=""
publish_config_file=""
publish_unit_path=""
published_agent_identity=""
published_config_identity=""
published_unit_identity=""

snapshot_live_file() {
  local target="$1" rollback_template="$2" output_name="$3" rollback_dir

  if [[ ! -e "$target" ]]; then
    printf -v "$output_name" ''
    return 0
  fi
  create_registered_temp rollback_dir rollback -d "$rollback_template"
  ln -- "$target" "$rollback_dir/original"
  printf -v "$output_name" '%s' "$rollback_dir"
}

restore_live_file() {
  local label="$1" target="$2" rollback_dir="$3"
  local restore_path target_dir

  if [[ -n "$rollback_dir" ]]; then
    if [[ -f "$target" &&
      ! -L "$target" &&
      "$rollback_dir/original" -ef "$target" ]]; then
      return 0
    fi
    target_dir="${target%/*}"
    if ! create_registered_temp \
      restore_path \
      disposable \
      "$target_dir/.vpsman-restore.XXXXXX"; then
      log "could not allocate a restore path for the prior $label"
      return 1
    fi
    if ! rm -f -- "$restore_path" ||
      ! ln -- "$rollback_dir/original" "$restore_path" ||
      ! mv -T -- "$restore_path" "$target"; then
      log "could not restore the prior $label at $target"
      return 1
    fi
  elif ! rm -f -- "$target"; then
    log "could not remove the newly published $label at $target"
    return 1
  fi
}

verify_live_file() {
  local target="$1" rollback_dir="$2"

  if [[ -n "$rollback_dir" ]]; then
    [[ -f "$target" &&
      ! -L "$target" &&
      "$rollback_dir/original" -ef "$target" ]]
  else
    [[ ! -e "$target" && ! -L "$target" ]]
  fi
}

rollback_live_files() {
  local failed=0

  restore_live_file "systemd unit" "$unit_path" "$rollback_unit_dir" ||
    failed=1
  restore_live_file "agent config" "$config_file" "$rollback_config_dir" ||
    failed=1
  restore_live_file "agent binary" "$agent_bin" "$rollback_agent_dir" ||
    failed=1
  verify_live_file "$unit_path" "$rollback_unit_dir" || failed=1
  verify_live_file "$config_file" "$rollback_config_dir" || failed=1
  verify_live_file "$agent_bin" "$rollback_agent_dir" || failed=1
  ((failed == 0))
}

rollback_systemctl() {
  local label="$1"
  shift

  if systemctl "${systemctl_scope[@]}" "$@"; then
    return 0
  fi
  log "could not $label during rollback"
  return 1
}

restore_service_manager_state() {
  ((service_state_captured &&
    (service_mutation_started || service_stopped_for_rollback))) ||
    return 0
  rollback_systemctl "reload the restored unit" daemon-reload
}

verify_service_restored() {
  local current_load current_active current_unit_file current_fragment

  ((service_state_captured &&
    (service_mutation_started || service_stopped_for_rollback))) ||
    return 0
  capture_service_snapshot || return 1
  current_load="$service_load_state"
  current_active="$service_active_state"
  current_unit_file="$service_unit_file_state"
  current_fragment="$service_fragment_path"

  [[ "$current_active" == "$service_active_state_before" ]] || return 1
  case "$service_registration" in
    unlinked)
      [[ "$current_load" == "not-found" &&
        -z "$current_fragment" &&
        ( -z "$current_unit_file" || "$current_unit_file" == "not-found" ) ]]
      ;;
    enabled)
      [[ "$current_load" == "loaded" &&
        "$current_fragment" == "$unit_path" &&
        "$current_unit_file" == "enabled" ]]
      ;;
  esac
}

verify_service_inactive() {
  ((service_state_captured)) || return 0
  capture_service_snapshot && [[ "$service_active_state" == "inactive" ]]
}

rollback_install_transaction() {
  local failed=0 filesystem_restored=1 service_quiesced=1 manager_restored=1

  service_stopped_for_rollback=0
  if ((service_state_captured)); then
    if ((service_was_active || service_activation_attempted)); then
      service_stopped_for_rollback=1
      rollback_systemctl "stop $service_name before restoring files" \
        stop "$service_name" ||
        {
          failed=1
          service_quiesced=0
        }
    fi
    if [[ "$service_registration" == "unlinked" ]] &&
      ((service_link_attempted || service_enable_attempted)); then
      rollback_systemctl "remove candidate registration for $service_name" \
        disable "$service_name" ||
        failed=1
    fi
  fi

  rollback_live_files || {
    failed=1
    filesystem_restored=0
  }
  if ((config_dir_preexisted)); then
    chmod "$config_dir_mode" "$config_dir" || {
      log "could not restore config directory mode $config_dir_mode"
      failed=1
      filesystem_restored=0
    }
  fi
  if ((config_dir_preexisted)); then
    if [[ "$(stat -c '%a' "$config_dir" 2>/dev/null || true)" != "$config_dir_mode" ]]; then
      failed=1
      filesystem_restored=0
    fi
  fi

  if ((filesystem_restored && service_quiesced)); then
    restore_service_manager_state || {
      failed=1
      manager_restored=0
    }
    if ((manager_restored && service_was_active)); then
      rollback_systemctl "start the restored $service_name" \
        start "$service_name" ||
        failed=1
    fi
    verify_service_restored || {
      log "systemd state does not match the pre-install snapshot after rollback"
      failed=1
    }
  else
    log "filesystem rollback was incomplete; leaving $service_name inactive"
    verify_service_inactive || {
      log "$service_name could not be confirmed inactive after incomplete rollback"
      failed=1
    }
  fi
  # Publication temps for later phases can otherwise keep freshly created
  # directories non-empty until after the only safe rmdir pass.
  cleanup_disposable_paths
  remove_created_directories || failed=1
  ((failed == 0))
}

publish_live_file() {
  local label="$1" replacement="$2" target="$3" output_name="$4"
  local expected_identity

  expected_identity="$(stat -c '%d:%i' "$replacement")"
  mv -T -- "$replacement" "$target" ||
    die "could not publish the $label; rollback will restore the prior installation"
  [[ -f "$target" &&
    ! -L "$target" &&
    "$(stat -c '%d:%i' "$target")" == "$expected_identity" ]] ||
    die "published $label failed exact regular-file identity verification"
  printf -v "$output_name" '%s' "$expected_identity"
}

verify_published_live_file() {
  local label="$1" target="$2" expected_identity="$3"

  [[ -f "$target" &&
    ! -L "$target" &&
    "$(stat -c '%d:%i' "$target")" == "$expected_identity" ]] ||
    die "published $label changed type or identity before transaction commit"
}

service_transaction_step() {
  local label="$1" phase="$2"
  shift 2

  service_mutation_started=1
  case "$phase" in
    link) service_link_attempted=1 ;;
    enable) service_enable_attempted=1 ;;
    activate) service_activation_attempted=1 ;;
  esac
  systemctl "${systemctl_scope[@]}" "$@" ||
    die "could not $label; rollback will restore the prior installation and service state"
}

record_missing_directory_chain "VPSMAN_AGENT_INSTALL_DIR" "$install_dir"
record_missing_directory_chain "VPSMAN_AGENT_CONFIG_DIR" "$config_dir"
record_missing_directory_chain "VPSMAN_AGENT_STATE_DIR" "$state_dir"
record_missing_directory_chain "VPSMAN_AGENT_LOG_DIR" "$log_dir"
record_missing_directory_chain "VPSMAN_AGENT_SYSTEMD_DIR" "$systemd_dir"
if [[ -d "$config_dir" ]]; then
  config_dir_preexisted=1
  config_dir_mode="$(stat -c '%a' "$config_dir")"
fi

# A single rename cannot atomically cover three directories. Preserve hard-link
# snapshots first, outside disposable cleanup, so an incomplete rollback always
# leaves the original bytes available for manual recovery.
snapshot_live_file \
  "$agent_bin" \
  "$install_dir/.vpsman-agent.rollback.XXXXXX" \
  rollback_agent_dir
snapshot_live_file \
  "$config_file" \
  "$config_dir/.agent.toml.rollback.XXXXXX" \
  rollback_config_dir
snapshot_live_file \
  "$unit_path" \
  "$systemd_dir/.$unit_file.rollback.XXXXXX" \
  rollback_unit_dir

service_active_state_before="$service_active_state"
transaction_started=1
mkdir -p "$install_dir" "$config_dir" "$state_dir" "$log_dir" "$systemd_dir"
chmod 700 "$config_dir"

# Build every replacement beside its live target. A failed write (including a
# full destination filesystem) therefore leaves the current installation
# untouched, and each final rename is atomic within that filesystem.
create_registered_temp \
  publish_agent_bin \
  disposable \
  "$install_dir/.vpsman-agent.install.XXXXXX"
create_registered_temp \
  publish_config_file \
  disposable \
  "$config_dir/.agent.toml.install.XXXXXX"
create_registered_temp \
  publish_unit_path \
  disposable \
  "$systemd_dir/.$unit_file.install.XXXXXX"
install -m 0755 -- "$staged_agent_bin" "$publish_agent_bin"

{
  printf 'client_id = %s\n' "$(toml_quote "$VPSMAN_AGENT_CLIENT_ID")"
  printf '\n[noise]\n'
  printf 'mode = "enrolled_ik"\n'
  printf 'client_private_key_hex = %s\n' "$(toml_quote "$VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX")"
  printf 'server_public_key_hex = %s\n' "$(toml_quote "$VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX")"
  printf '\n[auth]\n'
  printf 'gateway_retry_secs = %s\n' "$gateway_retry_secs"
  printf 'gateway_connect_timeout_secs = %s\n' "$gateway_connect_timeout_secs"
} >"$publish_config_file"

for endpoint_index in "${!gateway_endpoint_labels[@]}"; do
  label="${gateway_endpoint_labels[$endpoint_index]}"
  tcp_addr="${gateway_endpoint_addrs[$endpoint_index]}"
  priority="${gateway_endpoint_priorities[$endpoint_index]}"
  printf '\n[[tcp_endpoints]]\n' >>"$publish_config_file"
  {
    printf 'label = %s\n' "$(toml_quote "$label")"
    printf 'tcp_addr = %s\n' "$(toml_quote "$tcp_addr")"
    printf 'priority = %s\n' "$priority"
  } >>"$publish_config_file"
done
chmod 600 "$publish_config_file"

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
} >"$publish_unit_path"
chmod 0644 "$publish_unit_path"

publish_live_file \
  "agent binary" "$publish_agent_bin" "$agent_bin" published_agent_identity
publish_live_file \
  "agent config" "$publish_config_file" "$config_file" published_config_identity
publish_live_file \
  "systemd unit" "$publish_unit_path" "$unit_path" published_unit_identity
verify_published_live_file \
  "agent binary" "$agent_bin" "$published_agent_identity"
verify_published_live_file \
  "agent config" "$config_file" "$published_config_identity"
verify_published_live_file \
  "systemd unit" "$unit_path" "$published_unit_identity"

if service_enable_requested; then
  if [[ "$service_registration" == "unlinked" ]]; then
    service_transaction_step \
      "link the systemd unit" \
      link \
      link --force "$unit_path"
  fi
  service_transaction_step \
    "reload the systemd manager" \
    reload \
    daemon-reload
  if [[ "$service_registration" == "unlinked" ]]; then
    service_transaction_step \
      "enable $service_name" \
      enable \
      enable "$service_name"
  fi
  if ((service_was_active)); then
    service_transaction_step \
      "restart $service_name" \
      activate \
      restart "$service_name"
    transaction_committed=1
    log "installed and restarted direct gateway agent $VPSMAN_AGENT_CLIENT_ID using $config_file"
  else
    service_transaction_step \
      "start $service_name" \
      activate \
      start "$service_name"
    transaction_committed=1
    log "installed and enabled direct gateway agent $VPSMAN_AGENT_CLIENT_ID using $config_file"
  fi
else
  transaction_committed=1
  log "installed direct gateway agent $VPSMAN_AGENT_CLIENT_ID using $config_file"
  log "staging-only install complete; no service was started"
  printf -v foreground_command 'VPSMAN_AGENT_STATE_DIR=%q %q --config %q run' \
    "$state_dir" "$agent_bin" "$config_file"
  log "start in foreground: $foreground_command"
fi
