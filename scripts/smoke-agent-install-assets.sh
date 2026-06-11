#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk bash base64 chmod cmp curl find grep jq sha256sum stat wc
smoke_init_tmpdir "vpsman-agent-install"

fake_agent="$SMOKE_TMPDIR/vpsman-agent"
config_a="$SMOKE_TMPDIR/agent-a.toml"
config_b="$SMOKE_TMPDIR/agent-b.toml"
stage_root="$SMOKE_TMPDIR/root"
unprivileged_root="$SMOKE_TMPDIR/unprivileged-root"
download_root="$SMOKE_TMPDIR/download-root"
bad_agent_root="$SMOKE_TMPDIR/bad-agent-root"
missing_config_root="$SMOKE_TMPDIR/missing-config-root"
unsafe_path_root="$SMOKE_TMPDIR/unsafe-path-root"

cat >"$fake_agent" <<'SH'
#!/usr/bin/env sh
echo vpsman-agent-smoke
SH
chmod 0755 "$fake_agent"

fake_agent_sha="$(sha256sum "$fake_agent" | awk '{print $1}')"
wrong_sha="0000000000000000000000000000000000000000000000000000000000000000"
if [[ "$wrong_sha" == "$fake_agent_sha" ]]; then
  wrong_sha="1111111111111111111111111111111111111111111111111111111111111111"
fi

cat >"$config_a" <<'TOML'
client_id = "install-smoke-a"
display_name = "install-smoke-a"
telemetry_light_secs = 15
telemetry_full_secs = 60
tags = ["edge"]

[noise]
mode = "enrolled_ik"
client_private_key_hex = "1111111111111111111111111111111111111111111111111111111111111111"
server_public_key_hex = "2222222222222222222222222222222222222222222222222222222222222222"

[auth]
command_timeout_secs = 30

[[tcp_endpoints]]
label = "primary"
tcp_addr = "panel.example.com:9443"
priority = 10
TOML

cat >"$config_b" <<'TOML'
client_id = "install-smoke-b"
display_name = "install-smoke-b"
telemetry_light_secs = 15
telemetry_full_secs = 60
tags = []

[noise]
mode = "enrolled_ik"
client_private_key_hex = "5555555555555555555555555555555555555555555555555555555555555555"
server_public_key_hex = "6666666666666666666666666666666666666666666666666666666666666666"

[auth]
command_timeout_secs = 30

[[tcp_endpoints]]
label = "primary"
tcp_addr = "panel.example.com:9443"
priority = 10
TOML

config_a_b64="$(base64 <"$config_a" | tr -d '\n')"
config_b_b64="$(base64 <"$config_b" | tr -d '\n')"

VPSMAN_INSTALL_ROOT="$stage_root" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$fake_agent_sha" \
  VPSMAN_AGENT_CONFIG_B64="$config_a_b64" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/install-a.log"

installed_agent="$stage_root/opt/vpsman/vpsman-agent"
installed_config="$stage_root/etc/vpsman/agent.toml"
systemd_unit="$stage_root/etc/systemd/system/vpsman-agent.service"
sysv_script="$stage_root/etc/init.d/vpsman-agent"

test -x "$installed_agent"
test -f "$installed_config"
test -f "$systemd_unit"
test -x "$sysv_script"
cmp -s "$config_a" "$installed_config"

config_mode="$(stat -c '%a' "$installed_config")"
test "$config_mode" = "600"

grep -q '^ExecStart=/opt/vpsman/vpsman-agent --config /etc/vpsman/agent.toml run$' "$systemd_unit"
grep -q '^Restart=always$' "$systemd_unit"
grep -q '^UMask=0077$' "$systemd_unit"
grep -q 'start-stop-daemon --start' "$sysv_script"

config_hash_before="$(sha256sum "$installed_config" | awk '{print $1}')"
VPSMAN_INSTALL_ROOT="$stage_root" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$fake_agent_sha" \
  VPSMAN_AGENT_CONFIG_B64="$config_a_b64" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/install-a-again.log"
config_hash_after="$(sha256sum "$installed_config" | awk '{print $1}')"
test "$config_hash_before" = "$config_hash_after"

if VPSMAN_INSTALL_ROOT="$stage_root" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$fake_agent_sha" \
  VPSMAN_AGENT_CONFIG_B64="$config_b_b64" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/install-b-rejected.log" 2>&1; then
  echo "expected installer to reject config replacement without VPSMAN_FORCE_CONFIG=1" >&2
  exit 1
fi
grep -q 'existing agent config differs' "$SMOKE_TMPDIR/install-b-rejected.log"

VPSMAN_INSTALL_ROOT="$stage_root" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$fake_agent_sha" \
  VPSMAN_AGENT_CONFIG_B64="$config_b_b64" \
  VPSMAN_FORCE_CONFIG=1 \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/install-b-force.log"
cmp -s "$config_b" "$installed_config"
test "$(find "$stage_root/etc/vpsman" -name 'agent.toml.backup.*' | wc -l)" -eq 1

mkdir -p "$stage_root/var/lib/vpsman" "$stage_root/var/log/vpsman"
printf 'state\n' >"$stage_root/var/lib/vpsman/state.marker"
printf 'log\n' >"$stage_root/var/log/vpsman/install.log"

VPSMAN_INSTALL_ROOT="$stage_root" \
  VPSMAN_UNINSTALL=1 \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/uninstall-preserve.log"
test ! -e "$installed_agent"
test ! -e "$systemd_unit"
test ! -e "$sysv_script"
test -f "$installed_config"
cmp -s "$config_b" "$installed_config"
test -f "$stage_root/var/lib/vpsman/state.marker"
test -f "$stage_root/var/log/vpsman/install.log"
grep -q 'agent config preserved' "$SMOKE_TMPDIR/uninstall-preserve.log"

VPSMAN_INSTALL_ROOT="$stage_root" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$fake_agent_sha" \
  VPSMAN_AGENT_CONFIG_B64="$config_b_b64" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/install-after-uninstall.log"
test -x "$installed_agent"
test -f "$systemd_unit"
test -x "$sysv_script"
cmp -s "$config_b" "$installed_config"

VPSMAN_INSTALL_ROOT="$stage_root" \
  VPSMAN_UNINSTALL=1 \
  VPSMAN_PURGE_CONFIG=1 \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/uninstall-purge.log"
test ! -e "$installed_agent"
test ! -e "$systemd_unit"
test ! -e "$sysv_script"
test ! -e "$installed_config"
test ! -e "$stage_root/var/lib/vpsman/state.marker"
test ! -e "$stage_root/var/log/vpsman/install.log"
grep -q 'agent config, state, and logs purged' "$SMOKE_TMPDIR/uninstall-purge.log"

unprivileged_home="/home/vpsman-user"
VPSMAN_INSTALL_ROOT="$unprivileged_root" \
  VPSMAN_INSTALL_MODE=unprivileged \
  VPSMAN_SERVICE_HOME="$unprivileged_home" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$fake_agent_sha" \
  VPSMAN_AGENT_CONFIG_B64="$config_a_b64" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/unprivileged-install.log"

unprivileged_agent="$unprivileged_root$unprivileged_home/.local/lib/vpsman/vpsman-agent"
unprivileged_config="$unprivileged_root$unprivileged_home/.config/vpsman/agent.toml"
unprivileged_state_dir="$unprivileged_root$unprivileged_home/.local/state/vpsman"
unprivileged_log_dir="$unprivileged_root$unprivileged_home/.local/state/vpsman/log"
unprivileged_unit="$unprivileged_root$unprivileged_home/.config/systemd/user/vpsman-agent.service"

test -x "$unprivileged_agent"
test -f "$unprivileged_config"
test -d "$unprivileged_state_dir"
test -d "$unprivileged_log_dir"
test -f "$unprivileged_unit"
test ! -e "$unprivileged_root/etc/systemd/system/vpsman-agent.service"
test ! -e "$unprivileged_root/etc/init.d/vpsman-agent"
cmp -s "$config_a" "$unprivileged_config"
test "$(stat -c '%a' "$unprivileged_config")" = "600"
grep -q "^WorkingDirectory=$unprivileged_home/.local/lib/vpsman$" "$unprivileged_unit"
grep -q "^ExecStart=$unprivileged_home/.local/lib/vpsman/vpsman-agent --config $unprivileged_home/.config/vpsman/agent.toml run$" "$unprivileged_unit"
grep -q '^WantedBy=default.target$' "$unprivileged_unit"
if grep -q '^User=' "$unprivileged_unit"; then
  echo "user systemd unit must not embed a root/system User= directive" >&2
  exit 1
fi

printf 'state\n' >"$unprivileged_state_dir/state.marker"
printf 'log\n' >"$unprivileged_log_dir/install.log"

VPSMAN_INSTALL_ROOT="$unprivileged_root" \
  VPSMAN_INSTALL_MODE=unprivileged \
  VPSMAN_SERVICE_HOME="$unprivileged_home" \
  VPSMAN_UNINSTALL=1 \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/unprivileged-uninstall-preserve.log"
test ! -e "$unprivileged_agent"
test ! -e "$unprivileged_unit"
test -f "$unprivileged_config"
test -f "$unprivileged_state_dir/state.marker"
test -f "$unprivileged_log_dir/install.log"
grep -q 'agent config preserved' "$SMOKE_TMPDIR/unprivileged-uninstall-preserve.log"

VPSMAN_INSTALL_ROOT="$unprivileged_root" \
  VPSMAN_INSTALL_MODE=unprivileged \
  VPSMAN_SERVICE_HOME="$unprivileged_home" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$fake_agent_sha" \
  VPSMAN_AGENT_CONFIG_B64="$config_a_b64" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/unprivileged-reinstall.log"
test -x "$unprivileged_agent"
test -f "$unprivileged_unit"

VPSMAN_INSTALL_ROOT="$unprivileged_root" \
  VPSMAN_INSTALL_MODE=unprivileged \
  VPSMAN_SERVICE_HOME="$unprivileged_home" \
  VPSMAN_UNINSTALL=1 \
  VPSMAN_PURGE_CONFIG=1 \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/unprivileged-uninstall-purge.log"
test ! -e "$unprivileged_agent"
test ! -e "$unprivileged_unit"
test ! -e "$unprivileged_config"
test ! -e "$unprivileged_state_dir/state.marker"
test ! -e "$unprivileged_log_dir/install.log"
grep -q 'agent config, state, and logs purged' "$SMOKE_TMPDIR/unprivileged-uninstall-purge.log"

if VPSMAN_INSTALL_ROOT="$download_root" \
  VPSMAN_AGENT_URL="file://$fake_agent" \
  VPSMAN_AGENT_CONFIG_B64="$config_a_b64" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/download-missing-hash.log" 2>&1; then
  echo "expected installer to reject URL agent download without sha256" >&2
  exit 1
fi
grep -q 'VPSMAN_AGENT_SHA256_HEX' "$SMOKE_TMPDIR/download-missing-hash.log"

if VPSMAN_INSTALL_ROOT="$bad_agent_root" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$wrong_sha" \
  VPSMAN_AGENT_CONFIG_B64="$config_a_b64" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/bad-agent-hash.log" 2>&1; then
  echo "expected installer to reject agent binary sha256 mismatch" >&2
  exit 1
fi
grep -q 'agent binary sha256 mismatch' "$SMOKE_TMPDIR/bad-agent-hash.log"
test ! -e "$bad_agent_root/opt/vpsman/vpsman-agent"

if VPSMAN_INSTALL_ROOT="$unsafe_path_root" \
  VPSMAN_INSTALL_DIR="/" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$fake_agent_sha" \
  VPSMAN_AGENT_CONFIG_B64="$config_a_b64" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/unsafe-install-dir.log" 2>&1; then
  echo "expected installer to reject filesystem-root install dir" >&2
  exit 1
fi
grep -q 'VPSMAN_INSTALL_DIR must not be the filesystem root' "$SMOKE_TMPDIR/unsafe-install-dir.log"

if VPSMAN_INSTALL_ROOT="$missing_config_root" \
  VPSMAN_AGENT_BINARY="$fake_agent" \
  VPSMAN_AGENT_SHA256_HEX="$fake_agent_sha" \
  VPSMAN_SKIP_SERVICE_ENABLE=1 \
  bash scripts/install-agent.sh >"$SMOKE_TMPDIR/missing-config.log" 2>&1; then
  echo "expected installer to reject missing agent config" >&2
  exit 1
fi
grep -q 'provide VPSMAN_AGENT_CONFIG_B64, VPSMAN_AGENT_CONFIG_PATH, or VPSMAN_AGENT_CONFIG_URL' \
  "$SMOKE_TMPDIR/missing-config.log"

jq -n \
  --arg stage_root "$stage_root" \
  '{
    agent_install_assets: "ok",
    staged_root: $stage_root,
    checks: [
      "binary_install",
      "config_0600",
      "systemd_unit",
      "sysvinit_fallback",
      "idempotent_reinstall",
      "force_config_backup",
      "uninstall_preserves_config",
      "uninstall_purge_config_state_logs",
      "unprivileged_user_install",
      "unprivileged_user_systemd_unit",
      "unprivileged_no_root_autostart_assets",
      "unprivileged_uninstall_preserves_config",
      "unprivileged_uninstall_purge_config_state_logs",
      "agent_hash_verified",
      "url_hash_required",
      "hash_mismatch_rejected",
      "filesystem_root_paths_rejected",
      "explicit_config_required"
    ]
  }'
