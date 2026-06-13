#!/usr/bin/env bash
set -euo pipefail

fail() {
  printf 'install-agent: %s\n' "$*" >&2
  exit 1
}

info() {
  printf 'install-agent: %s\n' "$*"
}

is_true() {
  case "${1:-0}" in
    1 | true | TRUE | yes | YES) return 0 ;;
    *) return 1 ;;
  esac
}

service_enable_requested() {
  is_true "${VPSMAN_ENABLE_SERVICE:-${VPSMAN_AGENT_ENABLE_SERVICE:-0}}"
}

INSTALLER_PRESET_ROOT_WORK_DIR="/opt/vpsman-agent"

require_tool() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required tool: $1"
}

require_absolute() {
  case "$2" in
    /*) ;;
    *) fail "$1 must be an absolute path" ;;
  esac
}

require_not_root_dir() {
  local normalized="${2%/}"
  if [[ -z "$normalized" || "$normalized" == "/" ]]; then
    fail "$1 must not be the filesystem root"
  fi
}

verify_sha256() {
  local path="$1"
  local expected="${2:-}"
  local label="$3"
  local actual

  if [[ -z "$expected" ]]; then
    return 0
  fi
  require_tool sha256sum
  expected="${expected,,}"
  if [[ ! "$expected" =~ ^[0-9a-f]{64}$ ]]; then
    rm -f "$path"
    fail "$label sha256 must be a 64-character hex digest"
  fi
  read -r actual _ < <(sha256sum "$path")
  if [[ "$actual" != "$expected" ]]; then
    rm -f "$path"
    fail "$label sha256 mismatch"
  fi
  info "$label sha256 verified"
}

root_path() {
  local path="$1"
  if [[ "$install_root" == "/" ]]; then
    printf '%s\n' "$path"
  else
    printf '%s%s\n' "${install_root%/}" "$path"
  fi
}

service_management_enabled() {
  [[ "$install_root" == "/" ]] \
    && [[ "$install_mode" == "root" ]] \
    && service_enable_requested \
    && ! is_true "${VPSMAN_SKIP_SERVICE_ENABLE:-0}"
}

user_service_management_enabled() {
  [[ "$install_root" == "/" ]] \
    && [[ "$install_mode" == "unprivileged" ]] \
    && service_enable_requested \
    && ! is_true "${VPSMAN_SKIP_SERVICE_ENABLE:-0}"
}

remove_empty_dir() {
  local path="$1"
  rmdir "$path" 2>/dev/null || true
}

purge_agent_config() {
  local config_path="$stage_config_dir/agent.toml"
  local backup

  rm -f "$config_path"
  for backup in "$stage_config_dir"/agent.toml.backup.*; do
    [[ -e "$backup" ]] || continue
    rm -f "$backup"
  done
  rm -rf "$stage_var_dir" "$stage_log_dir"
  remove_empty_dir "$stage_config_dir"
}

uninstall_agent() {
  local agent_path="$stage_install_dir/vpsman-agent"
  local previous_path="$stage_install_dir/vpsman-agent.previous"
  local systemd_unit="$stage_systemd_dir/$service_name.service"
  local sysv_script="$stage_initd_dir/$service_name"
  local user_systemd_unit="$stage_user_systemd_dir/$service_name.service"

  if service_management_enabled; then
    if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
      systemctl disable --now "$service_name.service" >/dev/null 2>&1 || true
    elif command -v update-rc.d >/dev/null 2>&1 && command -v service >/dev/null 2>&1; then
      service "$service_name" stop >/dev/null 2>&1 || true
      update-rc.d -f "$service_name" remove >/dev/null 2>&1 || true
    else
      info "no supported service manager detected for disable/stop"
    fi
  elif user_service_management_enabled; then
    if command -v systemctl >/dev/null 2>&1; then
      systemctl --user disable --now "$service_name.service" >/dev/null 2>&1 || true
    else
      info "no supported user service manager detected for disable/stop"
    fi
  else
    info "service disable/stop skipped"
  fi

  rm -f "$systemd_unit" "$sysv_script" "$user_systemd_unit" "$agent_path" "$previous_path"
  remove_empty_dir "$stage_install_dir"

  if is_true "${VPSMAN_PURGE_CONFIG:-0}"; then
    purge_agent_config
    info "agent config, state, and logs purged"
  else
    info "agent config preserved; set VPSMAN_PURGE_CONFIG=1 to remove it"
  fi

  if service_management_enabled && command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
    systemctl daemon-reload >/dev/null 2>&1 || true
  fi
  info "agent uninstall completed"
}

write_systemd_unit() {
  local path="$1"
  cat >"$path" <<EOF
[Unit]
Description=vpsman headless agent
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
WorkingDirectory=$work_dir
ExecStart=$install_dir/vpsman-agent --config $config_dir/agent.toml run
Restart=always
RestartSec=5
KillSignal=SIGINT
LimitNOFILE=65536
UMask=0077

[Install]
WantedBy=multi-user.target
EOF
}

write_user_systemd_unit() {
  local path="$1"
  cat >"$path" <<EOF
[Unit]
Description=vpsman headless agent (unprivileged)
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
WorkingDirectory=$work_dir
ExecStart=$install_dir/vpsman-agent --config $config_dir/agent.toml run
Restart=always
RestartSec=5
KillSignal=SIGINT
LimitNOFILE=65536
UMask=0077

[Install]
WantedBy=default.target
EOF
}

write_sysv_init() {
  local path="$1"
  cat >"$path" <<EOF
#!/bin/sh
### BEGIN INIT INFO
# Provides:          vpsman-agent
# Required-Start:    \$remote_fs \$network
# Required-Stop:     \$remote_fs \$network
# Default-Start:     2 3 4 5
# Default-Stop:      0 1 6
# Short-Description: vpsman headless agent
### END INIT INFO

DAEMON="$install_dir/vpsman-agent"
CONFIG="$config_dir/agent.toml"
PIDFILE="$state_dir/vpsman-agent.pid"

case "\$1" in
  start)
    start-stop-daemon --start --background --make-pidfile --pidfile "\$PIDFILE" --exec "\$DAEMON" -- --config "\$CONFIG" run
    ;;
  stop)
    start-stop-daemon --stop --pidfile "\$PIDFILE" --retry=TERM/10/KILL/5
    rm -f "\$PIDFILE"
    ;;
  restart)
    "\$0" stop
    "\$0" start
    ;;
  status)
    start-stop-daemon --status --pidfile "\$PIDFILE"
    ;;
  *)
    echo "Usage: \$0 {start|stop|restart|status}" >&2
    exit 2
    ;;
esac
EOF
}

verify_service_assets() {
  if [[ "$install_mode" == "unprivileged" ]]; then
    grep -F "ExecStart=$install_dir/vpsman-agent --config $config_dir/agent.toml run" "$user_systemd_unit" >/dev/null \
      || fail "user systemd unit ExecStart verification failed"
    grep -F "Restart=always" "$user_systemd_unit" >/dev/null \
      || fail "user systemd unit restart policy verification failed"
  else
    grep -F "ExecStart=$install_dir/vpsman-agent --config $config_dir/agent.toml run" "$systemd_unit" >/dev/null \
      || fail "systemd unit ExecStart verification failed"
    grep -F "Restart=always" "$systemd_unit" >/dev/null \
      || fail "systemd unit restart policy verification failed"
    grep -F 'start-stop-daemon --start' "$sysv_script" >/dev/null \
      || fail "sysv init start command verification failed"
  fi
  info "service assets verified"
}

verify_live_service() {
  if [[ "$install_root" != "/" ]] || is_true "${VPSMAN_SKIP_SERVICE_ENABLE:-0}"; then
    return 0
  fi
  if [[ "$install_mode" == "unprivileged" ]]; then
    if command -v systemctl >/dev/null 2>&1 && systemctl --user is-active --quiet "$service_name.service"; then
      info "user systemd service live verification passed"
    fi
    return 0
  fi
  if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
    systemctl is-active --quiet "$service_name.service" \
      || fail "systemd service did not become active after install"
    info "systemd service live verification passed"
  elif command -v service >/dev/null 2>&1; then
    service "$service_name" status >/dev/null 2>&1 \
      || fail "sysvinit service status verification failed after install"
    info "sysvinit service live verification passed"
  fi
}

validate_install_mode() {
  case "$install_mode" in
    root | unprivileged) ;;
    *) fail "VPSMAN_INSTALL_MODE must be root or unprivileged" ;;
  esac
}

default_install_dir() {
  printf '%s\n' "$work_dir/bin"
}

default_config_dir() {
  printf '%s\n' "$work_dir/config"
}

default_state_dir() {
  printf '%s\n' "$work_dir/state"
}

default_log_dir() {
  printf '%s\n' "$work_dir/log"
}

default_systemd_dir() {
  printf '%s\n' "$work_dir/systemd"
}

default_initd_dir() {
  printf '%s\n' "$work_dir/init.d"
}

default_user_systemd_dir() {
  printf '%s\n' "$work_dir/systemd/user"
}

write_requested_config() {
  local tmp_config="$1"
  if [[ -n "${VPSMAN_AGENT_CONFIG_B64:-}" ]]; then
    require_tool base64
    printf '%s' "$VPSMAN_AGENT_CONFIG_B64" | base64 -d >"$tmp_config"
  elif [[ -n "${VPSMAN_AGENT_CONFIG_PATH:-}" ]]; then
    cp "$VPSMAN_AGENT_CONFIG_PATH" "$tmp_config"
  elif [[ -n "${VPSMAN_AGENT_CONFIG_URL:-}" ]]; then
    require_tool curl
    curl -fsSL "$VPSMAN_AGENT_CONFIG_URL" -o "$tmp_config"
  else
    fail "provide VPSMAN_AGENT_CONFIG_B64, VPSMAN_AGENT_CONFIG_PATH, or VPSMAN_AGENT_CONFIG_URL"
  fi
}

install_mode="${VPSMAN_INSTALL_MODE:-root}"
validate_install_mode
if [[ "$install_mode" == "unprivileged" ]]; then
  work_dir="${VPSMAN_WORK_DIR:-$PWD/vpsman-agent}"
else
  work_dir="${VPSMAN_WORK_DIR:-$INSTALLER_PRESET_ROOT_WORK_DIR}"
fi
service_home="${VPSMAN_SERVICE_HOME:-${HOME:-}}"
if [[ "$install_mode" == "unprivileged" ]]; then
  if [[ -n "${VPSMAN_SERVICE_HOME:-}" ]]; then
    require_absolute "VPSMAN_SERVICE_HOME" "$service_home"
  fi
fi
install_root="${VPSMAN_INSTALL_ROOT:-/}"
install_dir="${VPSMAN_INSTALL_DIR:-$(default_install_dir)}"
config_dir="${VPSMAN_CONFIG_DIR:-$(default_config_dir)}"
service_name="${VPSMAN_SERVICE_NAME:-vpsman-agent}"
state_dir="${VPSMAN_STATE_DIR:-$(default_state_dir)}"
log_dir="${VPSMAN_LOG_DIR:-$(default_log_dir)}"
systemd_dir="${VPSMAN_SYSTEMD_DIR:-$(default_systemd_dir)}"
initd_dir="${VPSMAN_INITD_DIR:-$(default_initd_dir)}"
user_systemd_dir="${VPSMAN_USER_SYSTEMD_DIR:-$(default_user_systemd_dir)}"

require_absolute "VPSMAN_INSTALL_ROOT" "$install_root"
require_absolute "VPSMAN_WORK_DIR" "$work_dir"
require_absolute "VPSMAN_INSTALL_DIR" "$install_dir"
require_absolute "VPSMAN_CONFIG_DIR" "$config_dir"
require_absolute "VPSMAN_STATE_DIR" "$state_dir"
require_absolute "VPSMAN_LOG_DIR" "$log_dir"
require_absolute "VPSMAN_SYSTEMD_DIR" "$systemd_dir"
require_absolute "VPSMAN_INITD_DIR" "$initd_dir"
require_absolute "VPSMAN_USER_SYSTEMD_DIR" "$user_systemd_dir"
require_not_root_dir "VPSMAN_WORK_DIR" "$work_dir"
require_not_root_dir "VPSMAN_INSTALL_DIR" "$install_dir"
require_not_root_dir "VPSMAN_CONFIG_DIR" "$config_dir"
require_not_root_dir "VPSMAN_STATE_DIR" "$state_dir"
require_not_root_dir "VPSMAN_LOG_DIR" "$log_dir"
require_not_root_dir "VPSMAN_SYSTEMD_DIR" "$systemd_dir"
require_not_root_dir "VPSMAN_INITD_DIR" "$initd_dir"
require_tool install

if [[ "$install_root" == "/" && "$install_mode" == "root" && "$(id -u)" != "0" ]]; then
  fail "run as root for host install, or set VPSMAN_INSTALL_ROOT to stage assets without root"
fi

stage_install_dir="$(root_path "$install_dir")"
stage_config_dir="$(root_path "$config_dir")"
stage_systemd_dir="$(root_path "$systemd_dir")"
stage_initd_dir="$(root_path "$initd_dir")"
stage_user_systemd_dir="$(root_path "$user_systemd_dir")"
stage_var_dir="$(root_path "$state_dir")"
stage_log_dir="$(root_path "$log_dir")"

if is_true "${VPSMAN_UNINSTALL:-0}"; then
  uninstall_agent
  exit 0
fi

install_dirs=("$stage_install_dir" "$stage_config_dir" "$stage_var_dir" "$stage_log_dir")
if [[ "$install_mode" == "unprivileged" ]]; then
  install_dirs+=("$stage_user_systemd_dir")
else
  install_dirs+=("$stage_systemd_dir" "$stage_initd_dir")
fi
install -d -m 0755 "${install_dirs[@]}"

tmp_agent="$(mktemp "$stage_install_dir/vpsman-agent.XXXXXX")"
if [[ -n "${VPSMAN_AGENT_BINARY:-}" ]]; then
  cp "$VPSMAN_AGENT_BINARY" "$tmp_agent"
else
  : "${VPSMAN_AGENT_URL:?set VPSMAN_AGENT_URL or VPSMAN_AGENT_BINARY}"
  : "${VPSMAN_AGENT_SHA256_HEX:?set VPSMAN_AGENT_SHA256_HEX when downloading agent binary}"
  require_tool curl
  curl -fsSL "$VPSMAN_AGENT_URL" -o "$tmp_agent"
fi
verify_sha256 "$tmp_agent" "${VPSMAN_AGENT_SHA256_HEX:-}" "agent binary"
chmod 0755 "$tmp_agent"

tmp_config="$(mktemp "$stage_config_dir/agent.toml.XXXXXX")"
write_requested_config "$tmp_config"
chmod 0600 "$tmp_config"

agent_path="$stage_install_dir/vpsman-agent"
if [[ -f "$agent_path" ]]; then
  if cmp -s "$tmp_agent" "$agent_path"; then
    rm -f "$tmp_agent"
    info "agent binary unchanged"
  else
    cp -p "$agent_path" "$agent_path.previous"
    mv "$tmp_agent" "$agent_path"
    info "agent binary replaced; previous copy saved"
  fi
else
  mv "$tmp_agent" "$agent_path"
  info "agent binary installed"
fi

config_path="$stage_config_dir/agent.toml"
if [[ -f "$config_path" ]]; then
  if cmp -s "$tmp_config" "$config_path"; then
    rm -f "$tmp_config"
    info "agent config unchanged"
  elif is_true "${VPSMAN_FORCE_CONFIG:-0}"; then
    backup_path="$config_path.backup.$(date +%Y%m%d%H%M%S).$$"
    cp -p "$config_path" "$backup_path"
    mv "$tmp_config" "$config_path"
    chmod 0600 "$config_path"
    info "agent config replaced; backup saved"
  else
    rm -f "$tmp_config"
    fail "existing agent config differs; set VPSMAN_FORCE_CONFIG=1 to replace it with a backup"
  fi
else
  mv "$tmp_config" "$config_path"
  chmod 0600 "$config_path"
  info "agent config installed"
fi

systemd_unit="$stage_systemd_dir/$service_name.service"
sysv_script="$stage_initd_dir/$service_name"
user_systemd_unit="$stage_user_systemd_dir/$service_name.service"
if [[ "$install_mode" == "unprivileged" ]]; then
  write_user_systemd_unit "$user_systemd_unit"
  chmod 0644 "$user_systemd_unit"
  rm -f "$systemd_unit" "$sysv_script"
  info "user service asset written"
else
  write_systemd_unit "$systemd_unit"
  chmod 0644 "$systemd_unit"
  write_sysv_init "$sysv_script"
  chmod 0755 "$sysv_script"
  rm -f "$user_systemd_unit"
  info "service assets written"
fi
verify_service_assets

if is_true "${VPSMAN_SKIP_SERVICE_ENABLE:-0}" || [[ "$install_root" != "/" ]]; then
  info "service enable/start skipped"
elif [[ "$install_mode" == "unprivileged" ]] && ! service_enable_requested; then
  info "service enable/start skipped; user unit written to $user_systemd_unit"
  info "set VPSMAN_ENABLE_SERVICE=1 to link and enable it with systemctl --user"
elif [[ "$install_mode" == "unprivileged" ]]; then
  if command -v systemctl >/dev/null 2>&1; then
    systemctl --user link "$user_systemd_unit"
    systemctl --user daemon-reload || true
    if systemctl --user enable --now "$service_name.service"; then
      info "user systemd service enabled and started"
    else
      info "user systemd service not started; run: systemctl --user enable --now $service_name.service"
    fi
  else
    info "no supported user service manager detected; run $install_dir/vpsman-agent --config $config_dir/agent.toml run manually"
  fi
elif ! service_enable_requested; then
  info "service enable/start skipped; systemd unit written to $systemd_unit"
  info "set VPSMAN_ENABLE_SERVICE=1 to link and enable it with systemctl"
elif command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
  systemctl link "$systemd_unit"
  systemctl daemon-reload
  systemctl enable --now "$service_name.service"
  info "systemd service enabled and started"
elif [[ "$initd_dir" == "/etc/init.d" ]] && command -v update-rc.d >/dev/null 2>&1 && command -v service >/dev/null 2>&1; then
  update-rc.d "$service_name" defaults
  service "$service_name" restart
  info "sysvinit service enabled and restarted"
else
  info "no supported service manager detected; run $sysv_script start manually"
fi
verify_live_service
