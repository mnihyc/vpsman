#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools curl grep jq python3 shuf timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-direct-identity-failover"

api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
dead_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
dead_addr="127.0.0.1:$dead_port"
internal_token="smoke-internal-$(date +%s%N)"
client_id="direct-failover-smoke-$(date +%s)"
super_password="smoke-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"
signing_keys="$(target/debug/vpsctl signing-keygen)"
server_signing_private_hex="$(jq -r '.private_key_hex' <<<"$signing_keys")"
server_signing_public_hex="$(jq -r '.public_key_hex' <<<"$signing_keys")"

api_log="$SMOKE_TMPDIR/api.log"
gateway_log="$SMOKE_TMPDIR/gateway.log"
gateway_restart_log="$SMOKE_TMPDIR/gateway-restart.log"
agent_log="$SMOKE_TMPDIR/agent.log"
agent_config="$SMOKE_TMPDIR/agent.toml"
gateway_pid=""

wait_agent_online() {
  local deadline=$((SECONDS + 45))
  local status=""
  until [[ "$status" == "online" ]]; do
    if (( SECONDS >= deadline )); then
      smoke_dump_logs "agent did not become online" \
        "$api_log" "$gateway_log" "$gateway_restart_log" "$agent_log"
      exit 1
    fi
    local agents_json
    agents_json="$(curl -fsS "$api_url/api/v1/agents" || printf '[]')"
    status="$(jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty' <<<"$agents_json")"
    sleep 0.25
  done
}

start_gateway() {
  local log_file="$1"
  VPSMAN_GATEWAY_BIND="$gateway_addr" \
  VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
  VPSMAN_GATEWAY_NOISE_MODE="enrolled_ik" \
  VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
  VPSMAN_API_URL="$api_url" \
  VPSMAN_INTERNAL_TOKEN="$internal_token" \
  VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
  VPSMAN_GATEWAY_ID="direct-failover-smoke-gateway" \
  VPSMAN_GATEWAY_RECONNECT_GRACE_SECS=2 \
  RUST_LOG="vpsman_gateway=info" \
    target/debug/vpsman-gateway >"$log_file" 2>&1 &
  gateway_pid="$!"
  smoke_track_pid "$gateway_pid"
  smoke_wait_tcp 127.0.0.1 "$gateway_port"
  smoke_wait_tcp 127.0.0.1 "$gateway_control_port"
}

VPSMAN_API_BIND="127.0.0.1:$api_port" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
VPSMAN_SERVER_SIGNING_KEY_HEX="$server_signing_private_hex" \
VPSMAN_DEBUG_INTERNAL_TEST_MODE=true \
RUST_LOG="vpsman_api=warn" \
  target/debug/vpsman-api >"$api_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_http "$api_url/health"

start_gateway "$gateway_log"

smoke_register_direct_agent_config \
  "$api_url" \
  "" \
  "$agent_config" \
  "$client_id" \
  "Direct Failover Smoke" \
  "direct-identity,failover" \
  "$gateway_addr" \
  "$gateway_public_hex" \
  "$server_signing_public_hex" \
  30 \
  2 \
  4

cat >>"$agent_config" <<EOF_AGENT

[[tcp_endpoints]]
label = "dead-first"
tcp_addr = "$dead_addr"
priority = 1
EOF_AGENT

grep -q "client_id = \"$client_id\"" "$agent_config"
grep -q "mode = \"enrolled_ik\"" "$agent_config"
grep -q "server_public_key_hex = \"$gateway_public_hex\"" "$agent_config"
grep -q "server_ed25519_public_key_hex = \"$server_signing_public_hex\"" "$agent_config"
grep -q "tcp_addr = \"$dead_addr\"" "$agent_config"
grep -q "tcp_addr = \"$gateway_addr\"" "$agent_config"

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=info" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"

wait_agent_online

grep -q "gateway session failed" "$agent_log"
grep -q "$client_id" "$gateway_log"

kill "$gateway_pid" >/dev/null 2>&1 || true
wait "$gateway_pid" >/dev/null 2>&1 || true
sleep 1
start_gateway "$gateway_restart_log"
wait_agent_online

restart_deadline=$((SECONDS + 30))
until grep -q "$client_id" "$gateway_restart_log"; do
  if (( SECONDS >= restart_deadline )); then
    smoke_dump_logs "agent did not reconnect after gateway restart" \
      "$api_log" "$gateway_log" "$gateway_restart_log" "$agent_log"
    exit 1
  fi
  sleep 0.25
done

jq -n \
  --arg client_id "$client_id" \
  --arg dead_addr "$dead_addr" \
  --arg gateway_addr "$gateway_addr" \
  '{
    direct_identity_gateway_failover_smoke: "ok",
    client_id: $client_id,
    dead_configured_endpoint: $dead_addr,
    live_configured_endpoint: $gateway_addr,
    checks: ["direct_identity_registration", "server_signed_agent_config", "static_dead_endpoint_failover", "gateway_restart_reconnect"]
  }'
