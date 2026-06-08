#!/usr/bin/env bash
set -Eeuo pipefail

REPO="${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}"
TAG="${VPSMAN_RELEASE_TAG:-latest}"
MODE="${VPSMAN_INSTALL_MODE:-root}"
SERVICE_NAME="${VPSMAN_AGENT_SERVICE:-vpsman-agent}"
COMMAND_TIMEOUT_SECS="${VPSMAN_COMMAND_TIMEOUT_SECS:-30}"

INSTALL_ROOT=""
CONFIG_PATH=""
BIN_DIR=""
UNIT_INSTALL_PATH=""
UNIT_STAGE_PATH=""
USER_SYSTEMD_DIR=""

usage() {
  cat <<'USAGE'
Usage:
  Copyable one-line root install:
    curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env VPSMAN_INSTALL_MODE=root VPSMAN_ENROLLMENT_API_URL=https://panel.example.com VPSMAN_ENROLLMENT_TOKEN=<token> bash

  Root privileged install:
    curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env \
      VPSMAN_INSTALL_MODE=root \
      VPSMAN_ENROLLMENT_API_URL=https://panel.example.com \
      VPSMAN_ENROLLMENT_TOKEN=<token> \
      bash

  Unprivileged user install:
    mkdir -p ~/vpsman-agent
    cd ~/vpsman-agent
    curl -fsSL https://raw.githubusercontent.com/mnihyc/vpsman/main/deploy/enroll-agent.sh | env \
      VPSMAN_INSTALL_MODE=unprivileged \
      VPSMAN_ENROLLMENT_API_URL=https://panel.example.com \
      VPSMAN_ENROLLMENT_TOKEN=<token> \
      bash

Required values may also be entered interactively when running from a TTY.

Environment:
  VPSMAN_INSTALL_MODE       root or unprivileged, default: root
  VPSMAN_RELEASE_REPO       GitHub owner/repo, default: mnihyc/vpsman
  VPSMAN_RELEASE_TAG        Release tag or latest, default: latest
  GITHUB_TOKEN              Optional token for private GitHub release downloads
  VPSMAN_ENROLLMENT_API_URL HTTP(S) control-plane API URL used once to claim enrollment
  VPSMAN_INSTALL_ROOT       Root-mode binary install root, default: /opt/vpsman
  VPSMAN_AGENT_CONFIG       Root-mode agent config path, default: /etc/vpsman/agent.toml
  VPSMAN_AGENT_SERVICE      systemd unit name, default: vpsman-agent
  VPSMAN_COMMAND_TIMEOUT_SECS  Command timeout in rendered config
  VPSMAN_SKIP_SERVICE=1     Install binary/config only; do not write/start systemd

Root mode writes:
  /opt/vpsman/bin/vpsman-agent
  /opt/vpsman/bin/vpsctl
  /etc/vpsman/agent.toml
  /etc/systemd/system/vpsman-agent.service

Unprivileged mode writes:
  ./bin/vpsman-agent
  ./bin/vpsctl
  ./agent.toml
  ./supervisor/
  ./systemd/vpsman-agent.service
  ~/.config/systemd/user/vpsman-agent.service
USAGE
}

log() {
  printf '%s\n' "$*" >&2
}

warn() {
  log "warning: $*"
}

die() {
  log "error: $*"
  exit 1
}

have_tty() {
  [[ -r /dev/tty && -w /dev/tty ]] || return 1
  { : < /dev/tty > /dev/tty; } 2>/dev/null
}

prompt_value() {
  local name="$1"
  local label="$2"
  local default_value="${3:-}"
  local value="${!name:-}"
  if [[ -n "$value" ]]; then
    export "${name?}"
    return
  fi
  if ! have_tty; then
    [[ -n "$default_value" ]] || die "$name is required"
    printf -v "$name" '%s' "$default_value"
    export "${name?}"
    return
  fi
  if [[ -n "$default_value" ]]; then
    printf '%s [%s]: ' "$label" "$default_value" > /dev/tty
  else
    printf '%s: ' "$label" > /dev/tty
  fi
  IFS= read -r value < /dev/tty || die "failed to read $name"
  if [[ -z "$value" ]]; then
    value="$default_value"
  fi
  [[ -n "$value" ]] || die "$name is required"
  printf -v "$name" '%s' "$value"
  export "${name?}"
}

prompt_secret() {
  local name="$1"
  local label="$2"
  local value="${!name:-}"
  if [[ -n "$value" ]]; then
    export "${name?}"
    return
  fi
  have_tty || die "$name is required"
  printf '%s: ' "$label" > /dev/tty
  IFS= read -r -s value < /dev/tty || die "failed to read $name"
  printf '\n' > /dev/tty
  [[ -n "$value" ]] || die "$name is required"
  printf -v "$name" '%s' "$value"
  export "${name?}"
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || die "missing required tool: $1"
}

download() {
  local url="$1"
  local output="$2"
  if command -v curl >/dev/null 2>&1; then
    local headers=()
    if [[ -n "${GITHUB_TOKEN:-}" ]]; then
      headers=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
    fi
    curl -fL --retry 3 --connect-timeout 10 "${headers[@]}" -o "$output" "$url"
  elif command -v wget >/dev/null 2>&1; then
    local headers=()
    if [[ -n "${GITHUB_TOKEN:-}" ]]; then
      headers=(--header="Authorization: Bearer ${GITHUB_TOKEN}")
    fi
    wget -O "$output" "${headers[@]}" "$url"
  else
    die "missing required tool: curl or wget"
  fi
}

release_base_url() {
  if [[ "$TAG" == "latest" ]]; then
    printf 'https://github.com/%s/releases/latest/download\n' "$REPO"
  else
    printf 'https://github.com/%s/releases/download/%s\n' "$REPO" "$TAG"
  fi
}

detect_arch() {
  case "$(uname -m)" in
    x86_64 | amd64)
      printf 'x86_64\n'
      ;;
    aarch64 | arm64)
      printf 'aarch64\n'
      ;;
    *)
      die "unsupported architecture: $(uname -m)"
      ;;
  esac
}

require_absolute_path() {
  local value="$1"
  local label="$2"
  [[ "$value" == /* ]] || die "$label must be an absolute path: $value"
}

systemd_quote_arg() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '"%s"' "$value"
}

resolve_enrollment_api_url() {
  if [[ -n "${VPSMAN_ENROLLMENT_API_URL:-}" ]]; then
    ENROLLMENT_API_URL="$VPSMAN_ENROLLMENT_API_URL"
  else
    prompt_value VPSMAN_ENROLLMENT_API_URL "Enrollment HTTP API URL"
    ENROLLMENT_API_URL="$VPSMAN_ENROLLMENT_API_URL"
  fi

  [[ "$ENROLLMENT_API_URL" == http://* || "$ENROLLMENT_API_URL" == https://* ]] \
    || die "VPSMAN_ENROLLMENT_API_URL must start with http:// or https://"
}

configure_paths() {
  [[ "$SERVICE_NAME" =~ ^[A-Za-z0-9_.@-]+$ ]] \
    || die "VPSMAN_AGENT_SERVICE must be a simple systemd unit name"

  case "$MODE" in
    root)
      if [[ "$(id -u)" != "0" ]]; then
        die "root install requires running this script as root"
      fi
      INSTALL_ROOT="${VPSMAN_INSTALL_ROOT:-/opt/vpsman}"
      CONFIG_PATH="${VPSMAN_AGENT_CONFIG:-/etc/vpsman/agent.toml}"
      require_absolute_path "$INSTALL_ROOT" "VPSMAN_INSTALL_ROOT"
      require_absolute_path "$CONFIG_PATH" "VPSMAN_AGENT_CONFIG"
      BIN_DIR="$INSTALL_ROOT/bin"
      UNIT_INSTALL_PATH="/etc/systemd/system/${SERVICE_NAME}.service"
      ;;
    unprivileged)
      if [[ "$(id -u)" == "0" ]]; then
        die "unprivileged install must be run as a normal user, not root"
      fi
      if [[ -n "${VPSMAN_INSTALL_ROOT:-}" ]]; then
        die "VPSMAN_INSTALL_ROOT is not supported in unprivileged mode; run from the desired agent directory"
      fi
      if [[ -n "${VPSMAN_AGENT_CONFIG:-}" ]]; then
        die "VPSMAN_AGENT_CONFIG is not supported in unprivileged mode; config is ./agent.toml"
      fi
      [[ -n "${HOME:-}" ]] || die "HOME is required for unprivileged user systemd"
      INSTALL_ROOT="$(pwd -P)"
      CONFIG_PATH="$INSTALL_ROOT/agent.toml"
      BIN_DIR="$INSTALL_ROOT/bin"
      UNIT_STAGE_PATH="$INSTALL_ROOT/systemd/${SERVICE_NAME}.service"
      USER_SYSTEMD_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
      UNIT_INSTALL_PATH="$USER_SYSTEMD_DIR/${SERVICE_NAME}.service"
      ;;
    *)
      die "VPSMAN_INSTALL_MODE must be root or unprivileged"
      ;;
  esac
}

write_root_systemd_unit() {
  local agent_bin="$1"
  local exec_start
  local working_directory
  exec_start="$(systemd_quote_arg "$agent_bin") --config $(systemd_quote_arg "$CONFIG_PATH") run"
  working_directory="$(systemd_quote_arg "$INSTALL_ROOT")"

  require_tool systemctl
  cat > "$UNIT_INSTALL_PATH" <<UNIT
[Unit]
Description=vpsman agent
Wants=network-online.target
After=network-online.target

[Service]
Type=simple
WorkingDirectory=${working_directory}
Environment=VPSMAN_AGENT_RESTART_MODE=signal_only
ExecStart=${exec_start}
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
UNIT
  chmod 0644 "$UNIT_INSTALL_PATH"
  systemctl daemon-reload
  systemctl enable --now "$SERVICE_NAME"
}

write_user_systemd_unit() {
  local agent_bin="$1"
  local exec_start
  local working_directory
  local supervisor_environment
  exec_start="$(systemd_quote_arg "$agent_bin") --config $(systemd_quote_arg "$CONFIG_PATH") run"
  working_directory="$(systemd_quote_arg "$INSTALL_ROOT")"
  supervisor_environment="$(systemd_quote_arg "VPSMAN_SUPERVISOR_DIR=$INSTALL_ROOT/supervisor")"

  install -d -m 0755 "$(dirname "$UNIT_STAGE_PATH")" "$USER_SYSTEMD_DIR"
  cat > "$UNIT_STAGE_PATH" <<UNIT
[Unit]
Description=vpsman agent (user)
After=default.target

[Service]
Type=simple
WorkingDirectory=${working_directory}
Environment=VPSMAN_AGENT_RESTART_MODE=signal_only
Environment=${supervisor_environment}
ExecStart=${exec_start}
Restart=always
RestartSec=5

[Install]
WantedBy=default.target
UNIT
  chmod 0644 "$UNIT_STAGE_PATH"
  install -m 0644 "$UNIT_STAGE_PATH" "$UNIT_INSTALL_PATH"

  if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    systemctl --user daemon-reload
    systemctl --user enable --now "$SERVICE_NAME"
    if command -v loginctl >/dev/null 2>&1; then
      local linger
      linger="$(loginctl show-user "$(id -un)" -p Linger --value 2>/dev/null || true)"
      if [[ "$linger" != "yes" ]]; then
        warn "user lingering is not enabled; the agent may start only after this user logs in"
        warn "an administrator can enable boot start with: loginctl enable-linger $(id -un)"
      fi
    else
      warn "loginctl not found; verify user lingering if the agent must start before login"
    fi
  else
    warn "user systemd is not available in this session; service file installed at $UNIT_INSTALL_PATH"
    warn "start later with: systemctl --user daemon-reload && systemctl --user enable --now $SERVICE_NAME"
  fi
}

install_service() {
  local agent_bin="$1"
  case "$MODE" in
    root)
      write_root_systemd_unit "$agent_bin"
      ;;
    unprivileged)
      write_user_systemd_unit "$agent_bin"
      ;;
  esac
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

configure_paths

resolve_enrollment_api_url
prompt_secret VPSMAN_ENROLLMENT_TOKEN "Enrollment token"

require_tool uname
require_tool sha256sum
require_tool install

arch="$(detect_arch)"
agent_asset="vpsman-agent-linux-${arch}-musl"
ctl_asset="vpsctl-linux-${arch}-musl"
base_url="$(release_base_url)"
tmp_dir="$(mktemp -d)"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

log "install mode: ${MODE}"
log "downloading vpsman release assets for ${arch} from ${REPO} (${TAG})"
download "$base_url/SHA256SUMS" "$tmp_dir/SHA256SUMS"
download "$base_url/$agent_asset" "$tmp_dir/$agent_asset"
download "$base_url/$ctl_asset" "$tmp_dir/$ctl_asset"

grep -E "  (${agent_asset}|${ctl_asset})$" "$tmp_dir/SHA256SUMS" \
  > "$tmp_dir/SHA256SUMS.selected"
if [[ "$(wc -l < "$tmp_dir/SHA256SUMS.selected" | tr -d ' ')" != "2" ]]; then
  die "release checksum manifest does not contain required agent/vpsctl assets"
fi
(cd "$tmp_dir" && sha256sum -c SHA256SUMS.selected)

agent_bin="$BIN_DIR/vpsman-agent"
ctl_bin="$BIN_DIR/vpsctl"
config_dir="$(dirname "$CONFIG_PATH")"
install -d -m 0755 "$BIN_DIR" "$config_dir"
if [[ "$MODE" == "unprivileged" ]]; then
  install -d -m 0700 "$INSTALL_ROOT/supervisor"
fi
install -m 0755 "$tmp_dir/$agent_asset" "$agent_bin"
install -m 0755 "$tmp_dir/$ctl_asset" "$ctl_bin"

log "claiming enrollment token and rendering agent config"
"$ctl_bin" \
  --api-url "$ENROLLMENT_API_URL" \
  enroll-config \
  --token "$VPSMAN_ENROLLMENT_TOKEN" \
  --command-timeout-secs "$COMMAND_TIMEOUT_SECS" \
  --output-file "$tmp_dir/agent.toml"
tmp_config="$tmp_dir/agent.toml"
enrolled_client_id="$(sed -n 's/^client_id = "\(.*\)"$/\1/p' "$tmp_config" | head -n 1)"
install -m 0600 "$tmp_config" "$CONFIG_PATH"

if [[ "${VPSMAN_SKIP_SERVICE:-0}" == "1" ]]; then
  log "installed $agent_bin and $CONFIG_PATH; service start skipped"
  if [[ "$MODE" == "unprivileged" ]]; then
    log "manual start: VPSMAN_SUPERVISOR_DIR=$INSTALL_ROOT/supervisor $agent_bin --config $CONFIG_PATH run"
  else
    log "manual start: $agent_bin --config $CONFIG_PATH run"
  fi
else
  log "installing and starting systemd service ${SERVICE_NAME}"
  install_service "$agent_bin"
fi

log "agent enrollment complete for ${enrolled_client_id:-server-assigned client}"
