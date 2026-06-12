#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools curl docker grep jq shuf timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-agent-endpoint-failover"

api_port="$(smoke_free_port)"
pg_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
dead_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
smoke_start_postgres "vpsman-endpoint-failover-postgres" "$pg_port" >/dev/null
postgres_url="$SMOKE_POSTGRES_URL"
gateway_addr="127.0.0.1:$gateway_port"
dead_addr="127.0.0.1:$dead_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
internal_token="smoke-internal-$(date +%s%N)"
client_id="endpoint-failover-smoke-$(date +%s)"
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
  VPSMAN_POSTGRES_URL="$postgres_url" \
  VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
  VPSMAN_INTERNAL_TOKEN="$internal_token" \
  VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
  VPSMAN_PUBLIC_GATEWAY_ENDPOINTS="primary=$gateway_addr=10" \
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX="$gateway_public_hex" \
  VPSMAN_BACKUP_OBJECT_STORE_DIR="$SMOKE_TMPDIR/object-store" \
  RUST_LOG="vpsman_api=warn" \
    target/debug/vpsman-api >"$log_file" 2>&1 &
  api_pid="$!"
  smoke_track_pid "$api_pid"
  smoke_wait_http "$api_url/health"
}

start_api "$api_log"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d '{"username":"endpoint-failover-smoke","password":"endpoint-failover-smoke-password"}' \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="endpoint-failover-smoke-gateway" \
RUST_LOG="vpsman_gateway=warn" \
  target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_tcp 127.0.0.1 "$gateway_port"
smoke_wait_tcp 127.0.0.1 "$gateway_control_port"

kill "$api_pid" >/dev/null 2>&1 || true
wait "$api_pid" >/dev/null 2>&1 || true
start_api "$api_restart_log"

smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$agent_config" \
  "$client_id" \
  "$client_id" \
  "endpoint-failover-smoke" \
  "$gateway_public_hex" \
  "dead=$dead_addr=5,primary=$gateway_addr=10"

grep -q "server_public_key_hex = \"$gateway_public_hex\"" "$agent_config"
grep -q "tcp_addr = \"$dead_addr\"" "$agent_config"
grep -q "tcp_addr = \"$gateway_addr\"" "$agent_config"

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=info" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"

deadline=$((SECONDS + 45))
status=""
until [[ "$status" == "online" ]]; do
  if (( SECONDS >= deadline )); then
    smoke_dump_logs "agent did not become online through static endpoint failover" \
      "$api_log" "$api_restart_log" "$gateway_log" "$agent_log"
    exit 1
  fi
  agents_json="$(curl -fsS -H "Authorization: Bearer $access_token" "$api_url/api/v1/agents" || printf '[]')"
  status="$(jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty' <<<"$agents_json")"
  sleep 0.25
done

grep -q "gateway session failed" "$agent_log"

jq -n \
  --arg client_id "$client_id" \
  --arg dead_addr "$dead_addr" \
  --arg live_addr "$gateway_addr" \
  '{
    agent_endpoint_failover_smoke: "ok",
    client_id: $client_id,
    dead_configured_endpoint: $dead_addr,
    live_configured_endpoint: $live_addr,
    checks: ["static_tcp_endpoints", "api_restart", "dead_tcp_failover"]
  }'
