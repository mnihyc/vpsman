#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools base64 docker jq sha256sum

agent_bin="target/x86_64-unknown-linux-musl/release/vpsman-agent"
if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsman-agent --release --target x86_64-unknown-linux-musl
fi
if [[ ! -x "$agent_bin" ]]; then
  cargo build -p vpsman-agent --release --target x86_64-unknown-linux-musl
fi

smoke_init_tmpdir "vpsman-agent-install-distro"

config_file="$SMOKE_TMPDIR/agent.toml"
cat >"$config_file" <<'TOML'
client_id = "install-distro-smoke"
display_name = "install-distro-smoke"
telemetry_light_secs = 15
telemetry_full_secs = 60
tags = ["install-distro"]

[noise]
mode = "enrolled_ik"
client_private_key_hex = "1111111111111111111111111111111111111111111111111111111111111111"
server_public_key_hex = "2222222222222222222222222222222222222222222222222222222222222222"

[auth]
command_timeout_secs = 30

[[tcp_endpoints]]
label = "local"
tcp_addr = "127.0.0.1:9443"
priority = 10
TOML

config_b64="$(base64 <"$config_file" | tr -d '\n')"
agent_sha="$(sha256sum "$agent_bin" | awk '{print $1}')"

default_images=(
  "ubuntu:18.04"
  "ubuntu:20.04"
  "ubuntu:22.04"
  "ubuntu:24.04"
  "debian:11-slim"
  "debian:12-slim"
)

if [[ -n "${VPSMAN_INSTALL_DISTRO_IMAGES:-}" ]]; then
  IFS=',' read -r -a images <<<"$VPSMAN_INSTALL_DISTRO_IMAGES"
else
  images=("${default_images[@]}")
fi

checked_images=()
for image in "${images[@]}"; do
  safe_name="$(tr ':/' '--' <<<"$image")"
  stage_root="$SMOKE_TMPDIR/root-$safe_name"
  mkdir -p "$stage_root"
  log_path="$stage_root/install.log"
  metrics_path="$stage_root/metrics.json"
  container_name="vpsman-install-${safe_name}-$(date +%s%N)"

  if ! docker run --rm \
    --name "$container_name" \
    --user "$(id -u):$(id -g)" \
    -e VPSMAN_INSTALL_ROOT="$stage_root" \
    -e VPSMAN_AGENT_BINARY="$SMOKE_ROOT_DIR/$agent_bin" \
    -e VPSMAN_AGENT_SHA256_HEX="$agent_sha" \
    -e VPSMAN_AGENT_CONFIG_B64="$config_b64" \
    -e VPSMAN_SKIP_SERVICE_ENABLE=1 \
    -v "$SMOKE_ROOT_DIR:$SMOKE_ROOT_DIR" \
    -w "$SMOKE_ROOT_DIR" \
    "$image" \
    bash -lc '
      set -euo pipefail
      bash scripts/install-agent.sh >"$VPSMAN_INSTALL_ROOT/install.log" 2>&1
      test -x "$VPSMAN_INSTALL_ROOT/opt/vpsman/vpsman-agent"
      test -f "$VPSMAN_INSTALL_ROOT/etc/vpsman/agent.toml"
      test -f "$VPSMAN_INSTALL_ROOT/etc/systemd/system/vpsman-agent.service"
      test -x "$VPSMAN_INSTALL_ROOT/etc/init.d/vpsman-agent"
      test "$(stat -c "%a" "$VPSMAN_INSTALL_ROOT/etc/vpsman/agent.toml")" = "600"
      grep -q "^ExecStart=/opt/vpsman/vpsman-agent --config /etc/vpsman/agent.toml run$" \
        "$VPSMAN_INSTALL_ROOT/etc/systemd/system/vpsman-agent.service"
      grep -q "start-stop-daemon --start" "$VPSMAN_INSTALL_ROOT/etc/init.d/vpsman-agent"
      "$VPSMAN_INSTALL_ROOT/opt/vpsman/vpsman-agent" \
        --config "$VPSMAN_INSTALL_ROOT/etc/vpsman/agent.toml" \
        once >"$VPSMAN_INSTALL_ROOT/metrics.json"
      grep -q "\"observed_unix\"" "$VPSMAN_INSTALL_ROOT/metrics.json"
      grep -q "\"memory\"" "$VPSMAN_INSTALL_ROOT/metrics.json"
      printf "state\n" >"$VPSMAN_INSTALL_ROOT/var/lib/vpsman/state.marker"
      printf "log\n" >"$VPSMAN_INSTALL_ROOT/var/log/vpsman/install.log"
      VPSMAN_UNINSTALL=1 bash scripts/install-agent.sh >"$VPSMAN_INSTALL_ROOT/uninstall-preserve.log" 2>&1
      test ! -e "$VPSMAN_INSTALL_ROOT/opt/vpsman/vpsman-agent"
      test ! -e "$VPSMAN_INSTALL_ROOT/etc/systemd/system/vpsman-agent.service"
      test ! -e "$VPSMAN_INSTALL_ROOT/etc/init.d/vpsman-agent"
      test -f "$VPSMAN_INSTALL_ROOT/etc/vpsman/agent.toml"
      test -f "$VPSMAN_INSTALL_ROOT/var/lib/vpsman/state.marker"
      test -f "$VPSMAN_INSTALL_ROOT/var/log/vpsman/install.log"
      grep -q "agent config preserved" "$VPSMAN_INSTALL_ROOT/uninstall-preserve.log"
      bash scripts/install-agent.sh >"$VPSMAN_INSTALL_ROOT/install-after-uninstall.log" 2>&1
      test -x "$VPSMAN_INSTALL_ROOT/opt/vpsman/vpsman-agent"
      test -f "$VPSMAN_INSTALL_ROOT/etc/systemd/system/vpsman-agent.service"
      test -x "$VPSMAN_INSTALL_ROOT/etc/init.d/vpsman-agent"
      VPSMAN_UNINSTALL=1 VPSMAN_PURGE_CONFIG=1 bash scripts/install-agent.sh >"$VPSMAN_INSTALL_ROOT/uninstall-purge.log" 2>&1
      test ! -e "$VPSMAN_INSTALL_ROOT/opt/vpsman/vpsman-agent"
      test ! -e "$VPSMAN_INSTALL_ROOT/etc/systemd/system/vpsman-agent.service"
      test ! -e "$VPSMAN_INSTALL_ROOT/etc/init.d/vpsman-agent"
      test ! -e "$VPSMAN_INSTALL_ROOT/etc/vpsman/agent.toml"
      test ! -e "$VPSMAN_INSTALL_ROOT/var/lib/vpsman/state.marker"
      test ! -e "$VPSMAN_INSTALL_ROOT/var/log/vpsman/install.log"
      grep -q "agent config, state, and logs purged" "$VPSMAN_INSTALL_ROOT/uninstall-purge.log"
    '; then
    echo "agent install distro smoke failed for $image" >&2
    cat "$log_path" >&2 || true
    cat "$metrics_path" >&2 || true
    exit 1
  fi
  checked_images+=("$image")
done

printf '%s\n' "${checked_images[@]}" >"$SMOKE_TMPDIR/images.txt"
jq -Rn '
  [inputs | select(length > 0)] as $images
  | {
      agent_install_distro_matrix_smoke: "ok",
      images: $images,
      checks: [
        "staged_install",
        "static_agent_executes_once",
        "config_0600",
        "systemd_unit",
        "sysvinit_fallback",
        "staged_uninstall_preserves_config",
        "staged_uninstall_purge_config_state_logs"
      ]
    }
' <"$SMOKE_TMPDIR/images.txt"
