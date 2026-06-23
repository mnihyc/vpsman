#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk bash chmod grep jq sha256sum
smoke_init_tmpdir "vpsman-deploy-install-agent"

fake_bin_dir="$SMOKE_TMPDIR/bin"
fake_systemctl_log="$SMOKE_TMPDIR/systemctl.log"
fake_agent="$SMOKE_TMPDIR/vpsman-agent"
agent_home="$SMOKE_TMPDIR/agent-home"
staged_home="$SMOKE_TMPDIR/staged-home"
download_home="$SMOKE_TMPDIR/download-home"
missing_hash_home="$SMOKE_TMPDIR/missing-hash-home"

mkdir -p "$fake_bin_dir"
cat >"$fake_bin_dir/systemctl" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"${VPSMAN_FAKE_SYSTEMCTL_LOG:?}"
SH
chmod 0755 "$fake_bin_dir/systemctl"

cat >"$fake_agent" <<'SH'
#!/usr/bin/env sh
echo vpsman-agent-deploy-smoke
SH
chmod 0755 "$fake_agent"
fake_agent_sha="$(sha256sum "$fake_agent" | awk '{print $1}')"

common_env=(
  VPSMAN_INSTALL_MODE=user
  VPSMAN_AGENT_CLIENT_ID=deploy-smoke-a
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=1111111111111111111111111111111111111111111111111111111111111111
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=2222222222222222222222222222222222222222222222222222222222222222
  VPSMAN_GATEWAY_ENDPOINTS=primary=127.0.0.1:9443=10
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
grep -q -- "--user link $agent_home/systemd/vpsman-agent.service" "$fake_systemctl_log"
grep -q -- "--user enable --now vpsman-agent.service" "$fake_systemctl_log"
grep -q "installed and enabled direct gateway agent" "$SMOKE_TMPDIR/default-start.log"

env \
  PATH="$fake_bin_dir:$PATH" \
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
grep -q "staging-only installs" "$SMOKE_TMPDIR/staged-only.log"

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

jq -n '{deploy_install_agent: "ok"}'
