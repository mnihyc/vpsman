#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk base64 cp curl docker grep jq python3 sed shuf timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-live-hot-config"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
container_name="vpsman-live-hot-config-$(date +%s%N)"
internal_token="smoke-internal-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
operator_password="hot-config-smoke-password"
client_id="hot-config-smoke-$(date +%s)"
updated_display_name="$client_id-updated"
super_password="smoke-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

api_pid=""
api_log=""
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_log="$SMOKE_TMPDIR/agent.log"
agent_config="$SMOKE_TMPDIR/agent.toml"
hot_config="$SMOKE_TMPDIR/hot-agent.toml"
rollback_config="$agent_config.rollback"

cleanup_live_hot_config_smoke() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_live_hot_config_smoke EXIT

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
  local attempt
  local deadline=$((SECONDS + 45))
  attempt=0
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
  smoke_dump_logs "live hot-config API failed to start" "$SMOKE_TMPDIR"/api-"$label"-*.log
  exit 1
}

api_get() {
  local path="$1"
  curl -fsS -H "Authorization: Bearer $access_token" "$api_url$path"
}

wait_agent_online() {
  local status=""
  local deadline=$((SECONDS + 35))
  until [[ "$status" == "online" ]]; do
    if (( SECONDS >= deadline )); then
      smoke_dump_logs "agent did not become online for live hot-config smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log"
      exit 1
    fi
    status="$(api_get "/api/v1/agents" \
      | jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty')"
    sleep 0.25
  done
}

assert_hot_config_persisted() {
  local job_json targets_json outputs_json audits_json decoded_outputs
  job_json="$(api_get "/api/v1/jobs/$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=20")"

  jq -e '.status == "completed" and .command_type == "hot_config" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e --arg config_path "$agent_config" --arg rollback_path "$rollback_config" '
    .[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "hot_config"
      and .status == "applied"
      and .config_path == $config_path
      and .rollback_path == $rollback_path
  ' <<<"$outputs_json" >/dev/null
  jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' \
    <<<"$audits_json" >/dev/null

  decoded_outputs="$(
    jq -r '.[].data_base64' <<<"$outputs_json" | while IFS= read -r item; do
      printf '%s' "$item" | base64 -d
      printf '\n'
    done
  )"
  if grep -F \
    -e "$super_password" \
    -e "$super_salt_hex" \
    -e "client_private_key_hex" \
    -e "privilege_assertion" \
    -e "server_public_key_hex" \
    <<<"$decoded_outputs" >/dev/null; then
    echo "job outputs leaked hot-config secrets or trust anchors" >&2
    exit 1
  fi
}

start_api "first"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"hot-config-smoke\",\"password\":\"$operator_password\"}" \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
export VPSMAN_API_TOKEN="$access_token"
jq -e '.operator.username == "hot-config-smoke" and .token_type == "Bearer"' \
  <<<"$auth_json" >/dev/null

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="hot-config-smoke-gateway" \
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
  "hot-config-smoke" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"
updated_display_name="$client_id-updated"

cp "$agent_config" "$hot_config"
sed -i \
  -e "s/^display_name = .*/display_name = \"$updated_display_name\"/" \
  -e 's/^telemetry_light_secs = .*/telemetry_light_secs = 10/' \
  -e 's/^telemetry_full_secs = .*/telemetry_full_secs = 30/' \
  "$hot_config"
perl -0pi -e 's/^tags = \[(?:\n(?:    .*\n)*|[^\n]*)\]/tags = ["hot-config-smoke", "updated-live"]/m' \
  "$hot_config"

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"
wait_agent_online

reject_body="$(jq -nc \
  --arg client "$client_id" \
  --rawfile toml "$hot_config" \
  '{
    command: "hot_config",
    operation: {
      type: "hot_config",
      toml: $toml
    },
    selector_expression: ("id:" + $client),
    target_client_ids: [$client],
    privileged: true,
    confirmed: true,
    timeout_secs: 30
  }')"
reject_json="$SMOKE_TMPDIR/reject.json"
reject_status="$(curl -sS -o "$reject_json" -w "%{http_code}" \
  -H 'content-type: application/json' \
  -H "Authorization: Bearer $access_token" \
  -d "$reject_body" \
  "$api_url/api/v1/jobs")"
if [[ "$reject_status" != "403" ]]; then
  echo "expected no-privilege-unlock hot-config to return 403, got $reject_status" >&2
  cat "$reject_json" >&2 || true
  exit 1
fi
jq -e '.error == "privilege_assertion_required" and .status == 403' "$reject_json" >/dev/null
grep -q "display_name = \"$client_id\"" "$agent_config"
if grep -q "$updated_display_name" "$agent_config"; then
  echo "hot config changed after no-privilege-unlock rejection" >&2
  exit 1
fi

push_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" hot-config \
    --config-file "$hot_config" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --force-unprivileged \
    --confirmed)"
job_id="$(jq -r '.job_id' <<<"$push_json")"
smoke_assert_job_create_queued "$push_json" 1
smoke_wait_api_job_status "$api_url" "$job_id" completed 45 >/dev/null

grep -q "display_name = \"$updated_display_name\"" "$agent_config"
grep -q 'telemetry_light_secs = 10' "$agent_config"
grep -q 'telemetry_full_secs = 30' "$agent_config"
grep -q '^tags = \[' "$agent_config"
grep -q '"hot-config-smoke"' "$agent_config"
grep -q '"updated-live"' "$agent_config"
grep -q "display_name = \"$client_id\"" "$rollback_config"
grep -q 'telemetry_light_secs = 15' "$rollback_config"

assert_hot_config_persisted

stop_api
start_api "restart"
api_get "/api/v1/auth/me" | jq -e '.username == "hot-config-smoke"' >/dev/null
assert_hot_config_persisted

jq -n \
  --arg client_id "$client_id" \
  --arg job_id "$job_id" \
  --arg display_name "$updated_display_name" \
  '{
    live_hot_config_smoke: "ok",
    postgres_backed: true,
    auth_session: "persisted",
    api_restart: "verified",
    no_privilege_unlock_rejected: true,
    client_id: $client_id,
    job_id: $job_id,
    display_name: $display_name
  }'
