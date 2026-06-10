#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools curl grep jq python3 sed shuf timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-discovery-failover"

api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
dead_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
dead_addr="127.0.0.1:$dead_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
discovery_url="$api_url/.well-known/vpsman/endpoints.json"
internal_token="smoke-internal-$(date +%s%N)"
client_id="discovery-smoke-$(date +%s)"
super_password="smoke-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

api_log="$SMOKE_TMPDIR/api.log"
api_restart_log="$SMOKE_TMPDIR/api-restart.log"
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_log="$SMOKE_TMPDIR/agent.log"
agent_config="$SMOKE_TMPDIR/agent.toml"
api_pid=""

start_api() {
  local log_file="$1"
  VPSMAN_API_BIND="127.0.0.1:$api_port" \
  VPSMAN_INTERNAL_TOKEN="$internal_token" \
  VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
  VPSMAN_PUBLIC_GATEWAY_ENDPOINTS="primary=$gateway_addr=10" \
VPSMAN_DISCOVERY_URL="$discovery_url" \
VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX="$gateway_public_hex" \
VPSMAN_DEBUG_INTERNAL_TEST_MODE=true \
RUST_LOG="vpsman_api=warn" \
    target/debug/vpsman-api >"$log_file" 2>&1 &
  api_pid="$!"
  smoke_track_pid "$api_pid"
  smoke_wait_http "$api_url/health"
}

start_api "$api_log"

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_NOISE_MODE="enrolled_ik" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="discovery-smoke-gateway" \
RUST_LOG="vpsman_gateway=warn" \
  target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_tcp 127.0.0.1 "$gateway_port"
smoke_wait_tcp 127.0.0.1 "$gateway_control_port"

kill "$api_pid" >/dev/null 2>&1 || true
wait "$api_pid" >/dev/null 2>&1 || true
start_api "$api_restart_log"

token_json="$(target/debug/vpsctl --api-url "$api_url" enrollment-token-create \
  --ttl-secs 600 \
  --default-tags discovery-smoke)"
enrollment_token="$(jq -r '.token' <<<"$token_json")"

target/debug/vpsctl --api-url "$api_url" enroll-config \
  --token "$enrollment_token" \
  --output-file "$agent_config"
client_id="$(smoke_agent_config_client_id "$agent_config")"
if [[ -z "$client_id" ]]; then
  smoke_fail "enroll-config did not write client_id for discovery failover smoke"
fi

sed -i "0,/tcp_addr = .*/s//tcp_addr = \"$dead_addr\"/" "$agent_config"
grep -q "discovery_url = \"$discovery_url\"" "$agent_config"
grep -q "server_public_key_hex = \"$gateway_public_hex\"" "$agent_config"
grep -q "tcp_addr = \"$dead_addr\"" "$agent_config"

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=info" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"

deadline=$((SECONDS + 45))
status=""
until [[ "$status" == "online" ]]; do
  if (( SECONDS >= deadline )); then
    smoke_dump_logs "agent did not become online through discovery failover" \
      "$api_log" "$api_restart_log" "$gateway_log" "$agent_log"
    exit 1
  fi
  agents_json="$(curl -fsS "$api_url/api/v1/agents" || printf '[]')"
  status="$(jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty' <<<"$agents_json")"
  sleep 0.25
done

discovery_doc="$(curl -fsS "$discovery_url")"
jq -e \
  --arg gateway_addr "$gateway_addr" \
  '.version == 1 and .endpoints[0].tcp_addr == $gateway_addr' \
  <<<"$discovery_doc" >/dev/null

grep -q "gateway session failed" "$agent_log"
grep -q "refreshed discovery endpoint candidates" "$agent_log"

jq -n \
  --arg client_id "$client_id" \
  --arg dead_addr "$dead_addr" \
  --arg discovered_addr "$gateway_addr" \
  '{
    discovery_failover_smoke: "ok",
    client_id: $client_id,
    dead_configured_endpoint: $dead_addr,
    discovered_endpoint: $discovered_addr,
    checks: ["https_or_localhost_discovery", "api_restart", "dead_tcp_failover"]
  }'
