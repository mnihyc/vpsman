#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools curl docker grep jq python3 timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-live-runtime-tunnel-sync"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
container_name="vpsman-live-runtime-tunnel-sync-$(date +%s%N)"
internal_token="runtime-tunnel-sync-internal-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
client_id="runtime-tunnel-sync-$(date +%s)"
peer_client_id="$client_id-peer"
operator_password="runtime-tunnel-sync-password"
super_password="smoke-super-password"
super_salt_hex="2031425364758697a8b9cadbecfd0e1f2031425364758697a8b9cadbecfd0e1f"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"
plan_name="runtime-sync-smoke"
interface_name="rtun0"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

api_pid=""
api_log=""
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_log="$SMOKE_TMPDIR/agent-left.log"
peer_agent_log="$SMOKE_TMPDIR/agent-right.log"
agent_config="$SMOKE_TMPDIR/agent-left.toml"
peer_agent_config="$SMOKE_TMPDIR/agent-right.toml"
plan_id=""
config_read_job_id=""
peer_config_read_job_id=""
current_ospf_cost=""
updated_ospf_cost=""

cleanup_runtime_tunnel_sync_smoke() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_runtime_tunnel_sync_smoke EXIT

docker run --rm -d \
  --name "$container_name" \
  -e POSTGRES_DB=vpsman \
  -e POSTGRES_PASSWORD=vpsman \
  -e POSTGRES_USER=vpsman \
  -p "127.0.0.1:$pg_port:5432" \
  postgres:16-alpine >/dev/null

deadline=$((SECONDS + 45))
until docker exec "$container_name" pg_isready -U vpsman -d vpsman >/dev/null 2>&1; do
  if (( SECONDS >= deadline )); then
    echo "timed out waiting for postgres container" >&2
    docker logs "$container_name" >&2 || true
    exit 1
  fi
  sleep 0.25
done
smoke_wait_tcp 127.0.0.1 "$pg_port"

stop_api() {
  if [[ -n "$api_pid" ]]; then
    kill "$api_pid" >/dev/null 2>&1 || true
    wait "$api_pid" >/dev/null 2>&1 || true
    api_pid=""
  fi
}

start_api() {
  local label="$1"
  local attempt=0
  local deadline=$((SECONDS + 45))
  while (( SECONDS < deadline )); do
    attempt=$((attempt + 1))
    api_log="$SMOKE_TMPDIR/api-$label-$attempt.log"
    VPSMAN_API_BIND="127.0.0.1:$api_port" \
    VPSMAN_POSTGRES_URL="$postgres_url" \
    VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
    VPSMAN_INTERNAL_TOKEN="$internal_token" \
    VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
    VPSMAN_PUBLIC_GATEWAY_ENDPOINTS="primary=$gateway_addr=10" \
    VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX="$gateway_public_hex" \
    VPSMAN_BACKUP_OBJECT_STORE_DIR="$SMOKE_TMPDIR/object-store" \
    RUST_LOG="vpsman_api=warn" \
      target/debug/vpsman-api >"$api_log" 2>&1 &
    api_pid="$!"
    smoke_track_pid "$api_pid"

    local http_deadline=$((SECONDS + 8))
    until curl -fsS "$api_url/health" >/dev/null 2>&1; do
      if ! kill -0 "$api_pid" >/dev/null 2>&1; then
        wait "$api_pid" >/dev/null 2>&1 || true
        api_pid=""
        break
      fi
      if (( SECONDS >= http_deadline )); then
        stop_api
        break
      fi
      sleep 0.1
    done
    if curl -fsS "$api_url/health" >/dev/null 2>&1; then
      return
    fi
    sleep 0.5
  done
  smoke_dump_logs "runtime tunnel sync API failed to start" "$SMOKE_TMPDIR"/api-"$label"-*.log
  exit 1
}

api_get() {
  local path="$1"
  curl -fsS -H "Authorization: Bearer $access_token" "$api_url$path"
}

api_post() {
  local path="$1"
  local body="$2"
  curl -fsS \
    -H "content-type: application/json" \
    -H "Authorization: Bearer $access_token" \
    -d "$body" \
    "$api_url$path"
}

wait_agent_online() {
  local id="$1"
  local status=""
  local deadline=$((SECONDS + 35))
  until [[ "$status" == "online" ]]; do
    if (( SECONDS >= deadline )); then
      smoke_dump_logs "agent $id did not become online for runtime tunnel sync smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log" "$peer_agent_log"
      exit 1
    fi
    status="$(api_get "/api/v1/agents" \
      | jq -r --arg id "$id" '.[] | select(.id == $id) | .status // empty')"
    sleep 0.25
  done
}

submit_config_read() {
  local id="$1"
  local read_job_id read_body read_json privilege_assertion
  read_job_id="$(python3 - <<'PY'
import uuid
print(uuid.uuid4())
PY
)"
  privilege_assertion="$(
    smoke_job_privilege_assertion \
      "$super_password" \
      "$super_salt_hex" \
      "id:$id" \
      "config_read" \
      '{"type":"config_read"}' \
      30 \
      false \
      true \
      300 \
      "$id"
  )"
  read_body="$(jq -nc \
    --arg job_id "$read_job_id" \
    --arg client "$id" \
    --argjson privilege_assertion "$privilege_assertion" \
    '{
      job_id: $job_id,
      command: "config_read",
      operation: {type: "config_read"},
      selector_expression: ("id:" + $client),
      target_client_ids: [$client],
      privileged: true,
      destructive: false,
      confirmed: false,
      force_unprivileged: false,
      max_timeout_secs: 30,
      privilege_assertion: $privilege_assertion
    }')"
  read_json="$(api_post "/api/v1/jobs" "$read_body")"
  smoke_assert_job_create_queued "$read_json" 1 >/dev/null
  smoke_wait_api_job_status "$api_url" "$read_job_id" completed 45 >/dev/null
  printf '%s\n' "$read_job_id"
}

config_read_toml() {
  local id="$1"
  local read_job_id outputs_json
  read_job_id="$(submit_config_read "$id")"
  if [[ "$id" == "$client_id" ]]; then
    config_read_job_id="$read_job_id"
    printf '%s\n' "$read_job_id" >"$SMOKE_TMPDIR/config-read-left.job"
  else
    peer_config_read_job_id="$read_job_id"
    printf '%s\n' "$read_job_id" >"$SMOKE_TMPDIR/config-read-right.job"
  fi
  outputs_json="$(api_get "/api/v1/jobs/$read_job_id/outputs")"
  jq -r '
    .items[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | select(.type == "config_read")
    | .toml
  ' <<<"$outputs_json"
}

wait_config_contains_plan() {
  local id="$1"
  local expected_cost="$2"
  local deadline=$((SECONDS + 45))
  local toml_text=""
  while (( SECONDS < deadline )); do
    toml_text="$(config_read_toml "$id")"
    if grep -F "plan_id = \"$plan_id\"" <<<"$toml_text" >/dev/null \
      && grep -F "interface_name = \"$interface_name\"" <<<"$toml_text" >/dev/null \
      && grep -F "recommended_ospf_cost = $expected_cost" <<<"$toml_text" >/dev/null; then
      return
    fi
    sleep 1
  done
  echo "runtime tunnel plan $plan_id did not become visible for $id" >&2
  printf '%s\n' "$toml_text" >&2
  echo "--- recent jobs ---" >&2
  api_get "/api/v1/jobs?limit=200" >&2 || true
  echo "--- runtime config sync outputs ---" >&2
  api_get "/api/v1/jobs?limit=200" \
    | jq -r '.[] | select(.command_type == "runtime_config_sync") | .id' \
    | while IFS= read -r sync_job_id; do
        [[ -n "$sync_job_id" ]] || continue
        echo "--- outputs for $sync_job_id ---" >&2
        api_get "/api/v1/jobs/$sync_job_id/outputs" >&2 || true
      done || true
  exit 1
}

wait_config_omits_plan() {
  local id="$1"
  local deadline=$((SECONDS + 45))
  local toml_text=""
  while (( SECONDS < deadline )); do
    toml_text="$(config_read_toml "$id")"
    if ! grep -F "plan_id = \"$plan_id\"" <<<"$toml_text" >/dev/null; then
      return
    fi
    sleep 1
  done
  echo "disabled runtime tunnel plan $plan_id remained visible for $id" >&2
  printf '%s\n' "$toml_text" >&2
  exit 1
}

start_api "first"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"runtime-tunnel-sync\",\"password\":\"$operator_password\"}" \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
export VPSMAN_API_TOKEN="$access_token"
jq -e '.operator.username == "runtime-tunnel-sync" and .token_type == "Bearer"' \
  <<<"$auth_json" >/dev/null

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="runtime-tunnel-sync-gateway" \
VPSMAN_GATEWAY_SPOOL_DIR="$SMOKE_TMPDIR/gateway-spool" \
RUST_LOG="vpsman_gateway=warn" \
  target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_tcp 127.0.0.1 "$gateway_port"
smoke_wait_tcp 127.0.0.1 "$gateway_control_port"

smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$agent_config" \
  "$client_id" \
  "$client_id" \
  "runtime-tunnel-sync-left" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"
smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$peer_agent_config" \
  "$peer_client_id" \
  "$peer_client_id" \
  "runtime-tunnel-sync-right" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"
VPSMAN_AGENT_CONFIG="$peer_agent_config" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$peer_agent_log" 2>&1 &
smoke_track_pid "$!"
wait_agent_online "$client_id"
wait_agent_online "$peer_client_id"

plan_json="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-plan \
    --name "$plan_name" \
    --interface-name "$interface_name" \
    --kind gre \
    --runtime-manager external_observed \
    --left-client-id "$client_id" \
    --right-client-id "$peer_client_id" \
    --left-underlay 198.51.100.10 \
    --right-underlay 198.51.100.11 \
    --left-tunnel-ipv4-cidr 10.255.70.0/31 \
    --right-tunnel-ipv4-cidr 10.255.70.1/31 \
    --bandwidth 100m \
    --latency-ms 3 \
    --save \
    --enabled \
    --confirmed)"
plan_id="$(jq -r '.id' <<<"$plan_json")"
jq -e \
  --arg id "$plan_id" \
  --arg left "$client_id" \
  --arg right "$peer_client_id" '
    .id == $id
      and .enabled == true
      and .left_client_id == $left
      and .right_client_id == $right
      and .plan.runtime_control.manager == "external_observed"
  ' <<<"$plan_json" >/dev/null

current_ospf_cost="$(jq -r '.recommended_ospf_cost' <<<"$plan_json")"
updated_ospf_cost="$((current_ospf_cost + 15))"
wait_config_contains_plan "$client_id" "$current_ospf_cost"
wait_config_contains_plan "$peer_client_id" "$current_ospf_cost"

updated_plan_json="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-ospf-cost-update \
    --plan-id "$plan_id" \
    --current-ospf-cost "$current_ospf_cost" \
    --recommended-ospf-cost "$updated_ospf_cost" \
    --confirmed)"
jq -e --argjson updated "$updated_ospf_cost" '
  .recommended_ospf_cost == $updated and .plan.recommended_ospf_cost == $updated
' \
  <<<"$updated_plan_json" >/dev/null
wait_config_contains_plan "$client_id" "$updated_ospf_cost"
wait_config_contains_plan "$peer_client_id" "$updated_ospf_cost"

disabled_json="$(api_post \
  "/api/v1/tunnel-plans/$plan_id/disable" \
  '{"confirmed": true}')"
jq -e '.enabled == false' <<<"$disabled_json" >/dev/null
wait_config_omits_plan "$client_id"
wait_config_omits_plan "$peer_client_id"

jobs_json="$(api_get "/api/v1/jobs?limit=50")"
jq -e '
  [ .[] | select(.command_type == "runtime_config_sync") ] | length >= 4
' <<<"$jobs_json" >/dev/null

config_read_job_id="$(cat "$SMOKE_TMPDIR/config-read-left.job")"
peer_config_read_job_id="$(cat "$SMOKE_TMPDIR/config-read-right.job")"

jq -n \
  --arg client_id "$client_id" \
  --arg peer_client_id "$peer_client_id" \
  --arg plan_id "$plan_id" \
  --argjson current_ospf_cost "$current_ospf_cost" \
  --argjson updated_ospf_cost "$updated_ospf_cost" \
  --arg config_read_job_id "$config_read_job_id" \
  --arg peer_config_read_job_id "$peer_config_read_job_id" \
  '{
    live_runtime_tunnel_sync_smoke: "ok",
    client_id: $client_id,
    peer_client_id: $peer_client_id,
    plan_id: $plan_id,
    current_ospf_cost: $current_ospf_cost,
    updated_ospf_cost: $updated_ospf_cost,
    config_read_job_id: $config_read_job_id,
    peer_config_read_job_id: $peer_config_read_job_id,
    create_update_disable_pushed_runtime_config_sync: true
  }'
