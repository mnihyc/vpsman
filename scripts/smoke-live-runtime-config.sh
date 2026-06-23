#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk base64 cp curl docker grep jq python3 sed shuf timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-live-runtime-config"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
container_name="vpsman-live-runtime-config-$(date +%s%N)"
internal_token="smoke-internal-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
operator_password="runtime-config-smoke-password"
client_id="runtime-config-smoke-$(date +%s)"
runtime_proc_root="$SMOKE_TMPDIR/runtime-proc-root"
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
runtime_patch="$SMOKE_TMPDIR/runtime-config-patch.toml"

cleanup_live_runtime_config_smoke() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_live_runtime_config_smoke EXIT

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
  smoke_dump_logs "live runtime config API failed to start" "$SMOKE_TMPDIR"/api-"$label"-*.log
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
      smoke_dump_logs "agent did not become online for live runtime config smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log"
      exit 1
    fi
    status="$(api_get "/api/v1/agents" \
      | jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty')"
    sleep 0.25
  done
}

assert_runtime_config_sync_persisted() {
  local job_json targets_json outputs_json audits_json decoded_outputs
  job_json="$(api_get "/api/v1/jobs/$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=20")"

  jq -e '.status == "completed" and .command_type == "runtime_config_sync" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e '
    .items[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "runtime_config_sync"
      and .status == "applied"
      and .bootstrap_config_persisted == false
      and .reason == "CLI config patch"
  ' <<<"$outputs_json" >/dev/null
  jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' \
    <<<"$audits_json" >/dev/null

  decoded_outputs="$(
    jq -r '.items[].data_base64' <<<"$outputs_json" | while IFS= read -r item; do
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
    echo "job outputs leaked runtime config secrets or trust anchors" >&2
    exit 1
  fi
}

submit_config_read() {
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
      "id:$client_id" \
      "config_read" \
      '{"type":"config_read"}' \
      30 \
      false \
      true \
      300 \
      "$client_id"
  )"
  read_body="$(jq -nc \
    --arg job_id "$read_job_id" \
    --arg client "$client_id" \
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
  read_json="$(curl -fsS \
    -H 'content-type: application/json' \
    -H "Authorization: Bearer $access_token" \
    -d "$read_body" \
    "$api_url/api/v1/jobs")"
  smoke_assert_job_create_queued "$read_json" 1 >/dev/null
  smoke_wait_api_job_status "$api_url" "$read_job_id" completed 45 >/dev/null
  printf '%s\n' "$read_job_id"
}

assert_runtime_config_visible() {
  local read_job_id outputs_json
  read_job_id="$(submit_config_read)"
  outputs_json="$(api_get "/api/v1/jobs/$read_job_id/outputs")"
  jq -e --arg proc_root "$runtime_proc_root" '
    .items[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "config_read"
      and (.toml | contains("[telemetry]"))
      and (.toml | contains("proc_root = \"" + $proc_root + "\""))
  ' <<<"$outputs_json" >/dev/null
}

start_api "first"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"runtime-config-smoke\",\"password\":\"$operator_password\"}" \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
export VPSMAN_API_TOKEN="$access_token"
jq -e '.operator.username == "runtime-config-smoke" and .token_type == "Bearer"' \
  <<<"$auth_json" >/dev/null

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="runtime-config-smoke-gateway" \
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
  "runtime-config-smoke" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"
mkdir -p "$runtime_proc_root"
cat >"$runtime_patch" <<PATCH
[telemetry]
proc_root = "$runtime_proc_root"
PATCH

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"
wait_agent_online

reject_body="$(jq -nc \
  --arg client "$client_id" \
  --rawfile toml "$runtime_patch" \
  '{
    selector_expression: ("id:" + $client),
    target_client_ids: [$client],
    toml: $toml,
    reason: "smoke reject",
    confirmed: true,
  }')"
reject_json="$SMOKE_TMPDIR/reject.json"
reject_status="$(curl -sS -o "$reject_json" -w "%{http_code}" \
  -H 'content-type: application/json' \
  -H "Authorization: Bearer $access_token" \
  -d "$reject_body" \
  "$api_url/api/v1/runtime-config/patch")"
if [[ "$reject_status" != "403" ]]; then
  echo "expected no-privilege-unlock runtime config patch to return 403, got $reject_status" >&2
  cat "$reject_json" >&2 || true
  exit 1
fi
jq -e '.error == "privilege_assertion_required" and .status == 403' "$reject_json" >/dev/null
if grep -q "$runtime_proc_root" "$agent_config"; then
  echo "runtime config patch changed bootstrap config after no-privilege-unlock rejection" >&2
  exit 1
fi

push_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" config-patch \
    --config-file "$runtime_patch" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --confirmed)"
job_id="$(jq -r '.sync_job_ids[0]' <<<"$push_json")"
jq -e '.target_count == 1 and (.sync_job_ids | length == 1)' <<<"$push_json" >/dev/null
smoke_wait_api_job_status "$api_url" "$job_id" completed 45 >/dev/null

if grep -q "$runtime_proc_root" "$agent_config"; then
  echo "runtime config patch mutated immutable bootstrap config file" >&2
  exit 1
fi

assert_runtime_config_sync_persisted
assert_runtime_config_visible

stop_api
start_api "restart"
api_get "/api/v1/auth/me" | jq -e '.username == "runtime-config-smoke"' >/dev/null
assert_runtime_config_sync_persisted

jq -n \
  --arg client_id "$client_id" \
  --arg job_id "$job_id" \
  --arg proc_root "$runtime_proc_root" \
  '{
    live_runtime_config_smoke: "ok",
    postgres_backed: true,
    auth_session: "persisted",
    api_restart: "verified",
    no_privilege_unlock_rejected: true,
    client_id: $client_id,
    job_id: $job_id,
    runtime_proc_root: $proc_root,
    bootstrap_config_immutable: true
  }'
