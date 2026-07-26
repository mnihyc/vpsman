#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk bash chmod grep jq ln sha256sum tar
smoke_init_tmpdir "vpsman-deploy-install-agent"

fake_bin_dir="$SMOKE_TMPDIR/bin"
no_systemctl_bin="$SMOKE_TMPDIR/no-systemctl-bin"
fake_systemctl_log="$SMOKE_TMPDIR/systemctl.log"
fake_agent="$SMOKE_TMPDIR/vpsman-agent"
replacement_agent="$SMOKE_TMPDIR/vpsman-agent-replacement"
agent_home="$SMOKE_TMPDIR/agent-home"
staged_home="$SMOKE_TMPDIR/staged-home"
download_home="$SMOKE_TMPDIR/download-home"
missing_hash_home="$SMOKE_TMPDIR/missing-hash-home"
invalid_fresh_home="$SMOKE_TMPDIR/invalid-fresh-home"

mkdir -p "$fake_bin_dir" "$no_systemctl_bin"
cat >"$fake_bin_dir/systemctl" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${VPSMAN_FAKE_SYSTEMCTL_LOG:?}"
SH
chmod 0755 "$fake_bin_dir/systemctl"
for tool in bash cat chmod install mkdir rm; do
  ln -s "$(command -v "$tool")" "$no_systemctl_bin/$tool"
done

cat >"$fake_agent" <<'SH'
#!/usr/bin/env sh
echo vpsman-agent-deploy-smoke
SH
chmod 0755 "$fake_agent"
cat >"$replacement_agent" <<'SH'
#!/usr/bin/env sh
echo replacement-must-not-be-installed
SH
chmod 0755 "$replacement_agent"
fake_agent_sha="$(sha256sum "$fake_agent" | awk '{print $1}')"

common_env=(
  VPSMAN_INSTALL_MODE=user
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke_A:1-2
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=1111111111111111111111111111111111111111111111111111111111111111
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=2222222222222222222222222222222222222222222222222222222222222222
  'VPSMAN_GATEWAY_ENDPOINTS=primary=127.0.0.1:9443=10,dns=gw.example.com:9443=00020,ipv6=[2001:db8::1]:9443=30'
)

env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/default-start.log" 2>&1

test -x "$agent_home/bin/vpsman-agent"
test -f "$agent_home/config/agent.toml"
test -f "$agent_home/systemd/vpsman-agent.service"
grep -Fq 'tcp_addr = "127.0.0.1:9443"' "$agent_home/config/agent.toml"
grep -Fq 'tcp_addr = "gw.example.com:9443"' "$agent_home/config/agent.toml"
grep -Fq 'tcp_addr = "[2001:db8::1]:9443"' "$agent_home/config/agent.toml"
grep -Fq 'priority = 20' "$agent_home/config/agent.toml"
if grep -Fq 'priority = 00020' "$agent_home/config/agent.toml"; then
  echo "endpoint priority must be written as a canonical TOML integer" >&2
  exit 1
fi
grep -q -- "--user link $agent_home/systemd/vpsman-agent.service" "$fake_systemctl_log"
grep -q -- "--user enable --now vpsman-agent.service" "$fake_systemctl_log"
grep -q "installed and enabled direct gateway agent" "$SMOKE_TMPDIR/default-start.log"

symlink_config_target="$SMOKE_TMPDIR/symlink-config-target"
mkdir -p "$symlink_config_target"
ln -s "$symlink_config_target" "$agent_home/config-link"
symlink_unit_target="$SMOKE_TMPDIR/symlink-unit-target"
ln -s "$symlink_unit_target" "$agent_home/systemd/linked.service"

install_tree_digest() {
  tar --sort=name --numeric-owner -C "$agent_home" -cf - . |
    sha256sum |
    awk '{print $1}'
}

install_tree_digest_before="$(install_tree_digest)"
systemctl_hash_before="$(sha256sum "$fake_systemctl_log" | awk '{print $1}')"

assert_existing_install_unchanged() {
  test "$install_tree_digest_before" = "$(install_tree_digest)"
  test "$systemctl_hash_before" = \
    "$(sha256sum "$fake_systemctl_log" | awk '{print $1}')"
}

invalid_override_count=0
run_invalid_override() {
  local case_name="$1" expected_error="$2" override="$3"
  local log_file="$SMOKE_TMPDIR/invalid-override-$case_name.log"

  if (
    cd "$SMOKE_TMPDIR"
    env \
      PATH="$fake_bin_dir:$PATH" \
      VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
      VPSMAN_AGENT_HOME="$agent_home" \
      VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
      "${common_env[@]}" \
      "$override" \
      bash "$ROOT_DIR/deploy/install-agent.sh"
  ) >"$log_file" 2>&1; then
    echo "expected deploy installer to reject override case: $case_name" >&2
    exit 1
  fi
  grep -Fq "$expected_error" "$log_file"
  assert_existing_install_unchanged
  ((invalid_override_count += 1))
}

invalid_client_id_count=0
run_invalid_client_id() {
  local case_name="$1" expected_error="$2" override="$3"
  local fresh_home="$SMOKE_TMPDIR/fresh-invalid-client-id-$case_name"
  local fresh_log="$SMOKE_TMPDIR/invalid-client-id-$case_name-fresh.log"

  run_invalid_override \
    "client-id-$case_name-existing" \
    "$expected_error" \
    "$override"

  if env \
    PATH="$fake_bin_dir:$PATH" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
    VPSMAN_AGENT_HOME="$fresh_home" \
    VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
    "${common_env[@]}" \
    "$override" \
    bash "$ROOT_DIR/deploy/install-agent.sh" >"$fresh_log" 2>&1; then
    echo "expected deploy installer to reject fresh client id case: $case_name" >&2
    exit 1
  fi
  grep -Fq "$expected_error" "$fresh_log"
  test ! -e "$fresh_home"
  assert_existing_install_unchanged
  ((invalid_client_id_count += 1))
}

run_invalid_client_id \
  empty \
  "VPSMAN_AGENT_CLIENT_ID is required" \
  "VPSMAN_AGENT_CLIENT_ID="
run_invalid_client_id \
  whitespace \
  "VPSMAN_AGENT_CLIENT_ID must contain only ASCII letters, digits, '.', '_', ':', and '-'" \
  "VPSMAN_AGENT_CLIENT_ID=agent id"
client_id_control=$'VPSMAN_AGENT_CLIENT_ID=agent\nid'
run_invalid_client_id \
  control-character \
  "VPSMAN_AGENT_CLIENT_ID must contain only ASCII letters, digits, '.', '_', ':', and '-'" \
  "$client_id_control"
client_id_non_ascii=$'VPSMAN_AGENT_CLIENT_ID=agent-\303\251'
run_invalid_client_id \
  non-ascii \
  "VPSMAN_AGENT_CLIENT_ID must contain only ASCII letters, digits, '.', '_', ':', and '-'" \
  "$client_id_non_ascii"
run_invalid_client_id \
  invalid-punctuation \
  "VPSMAN_AGENT_CLIENT_ID must contain only ASCII letters, digits, '.', '_', ':', and '-'" \
  "VPSMAN_AGENT_CLIENT_ID=agent+id"
printf -v client_id_too_long '%129s' ''
client_id_too_long="${client_id_too_long// /a}"
run_invalid_client_id \
  too-long \
  "VPSMAN_AGENT_CLIENT_ID must not exceed 128 ASCII bytes" \
  "VPSMAN_AGENT_CLIENT_ID=$client_id_too_long"

run_invalid_override \
  relative-agent-home \
  "VPSMAN_AGENT_HOME must be an absolute path using only systemd-safe path characters" \
  "VPSMAN_AGENT_HOME=relative-agent-home"
test ! -e "$SMOKE_TMPDIR/relative-agent-home"

run_invalid_override \
  relative-state-dir \
  "VPSMAN_AGENT_STATE_DIR must be an absolute path using only systemd-safe path characters" \
  "VPSMAN_AGENT_STATE_DIR=relative-state"
test ! -e "$SMOKE_TMPDIR/relative-state"

run_invalid_override \
  config-traversal \
  "VPSMAN_AGENT_CONFIG_DIR must not contain dot or parent-directory traversal segments" \
  "VPSMAN_AGENT_CONFIG_DIR=$agent_home/config/../escape"
test ! -e "$agent_home/escape"

outside_config_dir="$SMOKE_TMPDIR/outside-config"
run_invalid_override \
  config-outside-home \
  "VPSMAN_AGENT_CONFIG_DIR must remain inside VPSMAN_AGENT_HOME" \
  "VPSMAN_AGENT_CONFIG_DIR=$outside_config_dir"
test ! -e "$outside_config_dir"

control_log_dir="${agent_home}/"$'log\nforged'
run_invalid_override \
  log-dir-control-character \
  "VPSMAN_AGENT_LOG_DIR must be an absolute path using only systemd-safe path characters" \
  "VPSMAN_AGENT_LOG_DIR=$control_log_dir"
test ! -e "$control_log_dir"

outside_systemd_dir="$SMOKE_TMPDIR/outside-systemd"
run_invalid_override \
  systemd-dir-outside-home \
  "VPSMAN_AGENT_SYSTEMD_DIR must remain inside VPSMAN_AGENT_HOME" \
  "VPSMAN_AGENT_SYSTEMD_DIR=$outside_systemd_dir"
test ! -e "$outside_systemd_dir"

run_invalid_override \
  service-name-traversal \
  "VPSMAN_AGENT_SERVICE_NAME must be a safe systemd .service unit name" \
  "VPSMAN_AGENT_SERVICE_NAME=../escaped.service"
test ! -e "$agent_home/escaped.service"

run_invalid_override \
  config-symlink-traversal \
  "VPSMAN_AGENT_CONFIG_DIR must not traverse symbolic links" \
  "VPSMAN_AGENT_CONFIG_DIR=$agent_home/config-link"
test ! -e "$symlink_config_target/agent.toml"

run_invalid_override \
  unit-target-symlink \
  "systemd unit path must not traverse symbolic links" \
  "VPSMAN_AGENT_SERVICE_NAME=linked.service"
test ! -e "$symlink_unit_target"

run_invalid_override \
  retry-noncanonical \
  "VPSMAN_GATEWAY_RETRY_SECS must be a canonical integer from 1 through 3600" \
  "VPSMAN_GATEWAY_RETRY_SECS=060"
run_invalid_override \
  retry-out-of-range \
  "VPSMAN_GATEWAY_RETRY_SECS must be a canonical integer from 1 through 3600" \
  "VPSMAN_GATEWAY_RETRY_SECS=3601"
run_invalid_override \
  connect-timeout-noncanonical \
  "VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS must be a canonical integer from 1 through 300" \
  "VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS=+10"
run_invalid_override \
  connect-timeout-out-of-range \
  "VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS must be a canonical integer from 1 through 300" \
  "VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS=301"

if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  "${common_env[@]}" \
  VPSMAN_GATEWAY_ENDPOINTS='primary=127.0.0.1:9443=10,malformed-later-entry' \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/invalid-existing.log" 2>&1; then
  echo "expected deploy installer to reject a malformed later endpoint" >&2
  exit 1
fi
grep -q "endpoint must be label=host:port=priority" \
  "$SMOKE_TMPDIR/invalid-existing.log"
assert_existing_install_unchanged

if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$invalid_fresh_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  "${common_env[@]}" \
  VPSMAN_GATEWAY_ENDPOINTS='ipv6=2001:db8::1:9443=10' \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/invalid-address.log" 2>&1; then
  echo "expected deploy installer to reject unbracketed IPv6" >&2
  exit 1
fi
grep -q "endpoint address must be IPv4:port, hostname:port, or \\[IPv6\\]:port" \
  "$SMOKE_TMPDIR/invalid-address.log"
test ! -e "$invalid_fresh_home"

if env \
  PATH="$no_systemctl_bin" \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/missing-systemctl.log" 2>&1; then
  echo "expected deploy installer to require systemctl before service installation" >&2
  exit 1
fi
grep -q "systemctl is required when VPSMAN_AGENT_ENABLE_SERVICE=1" \
  "$SMOKE_TMPDIR/missing-systemctl.log"
assert_existing_install_unchanged

env \
  PATH="$no_systemctl_bin" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$staged_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/staged-only.log" 2>&1

test -x "$staged_home/bin/vpsman-agent"
if grep -q "$staged_home" "$fake_systemctl_log"; then
  echo "staging-only install must not call systemctl" >&2
  exit 1
fi
grep -q "staging-only install complete; no service was started" "$SMOKE_TMPDIR/staged-only.log"
grep -Fq "start in foreground:" "$SMOKE_TMPDIR/staged-only.log"
grep -Fq "$staged_home/bin/vpsman-agent" "$SMOKE_TMPDIR/staged-only.log"

env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$download_home" \
  VPSMAN_AGENT_BINARY_URL="file://$fake_agent" \
  VPSMAN_AGENT_BINARY_SHA256="$fake_agent_sha" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/custom-url.log" 2>&1

test -x "$download_home/bin/vpsman-agent"

if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$missing_hash_home" \
  VPSMAN_AGENT_BINARY_URL="file://$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/missing-hash.log" 2>&1; then
  echo "expected deploy installer to reject custom URL without sha256" >&2
  exit 1
fi
grep -q "VPSMAN_AGENT_BINARY_SHA256 must be exactly 64 hex characters" \
  "$SMOKE_TMPDIR/missing-hash.log"

if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$SMOKE_TMPDIR/obsolete-env-home" \
  VPSMAN_AGENT_DISPLAY_NAME=obsolete-local-display \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/obsolete-env.log" 2>&1; then
  echo "expected deploy installer to reject runtime config env in bootstrap install" >&2
  exit 1
fi
grep -q "VPSMAN_AGENT_DISPLAY_NAME is server runtime config" \
  "$SMOKE_TMPDIR/obsolete-env.log"

jq -n \
  --argjson invalid_override_cases "$invalid_override_count" \
  --argjson invalid_client_id_cases "$invalid_client_id_count" \
  '{
    deploy_install_agent: "ok",
    invalid_override_cases: $invalid_override_cases,
    invalid_client_id_cases: $invalid_client_id_cases
  }'
