#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk base64 cmp curl docker jq python3 sha256sum shuf stat timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-live-file-push"

api_port="$(smoke_free_port)"
pg_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
smoke_start_postgres "vpsman-file-push-postgres" "$pg_port" >/dev/null
postgres_url="$SMOKE_POSTGRES_URL"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
internal_token="smoke-internal-$(date +%s%N)"
client_id="file-push-smoke-$(date +%s)"
super_password="smoke-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

api_log="$SMOKE_TMPDIR/api.log"
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_log="$SMOKE_TMPDIR/agent.log"
agent_config="$SMOKE_TMPDIR/agent.toml"
source_file="$SMOKE_TMPDIR/payload.txt"
chunked_source_file="$SMOKE_TMPDIR/chunked-payload.bin"
destination_dir="$SMOKE_TMPDIR/agent-destination"
destination_file="$destination_dir/pushed.txt"
chunked_destination_file="$destination_dir/pushed-chunked.bin"
mkdir -p "$destination_dir"

payload="vpsman live file-push smoke payload $(date +%s%N)"
printf '%s\n' "$payload" >"$source_file"
payload_sha="$(sha256sum "$source_file" | awk '{print $1}')"
payload_b64="$(base64 -w0 "$source_file")"
payload_size="$(stat -c '%s' "$source_file")"
python3 - "$chunked_source_file" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_bytes((b"vpsman-chunked-file-push-" * 50000)[:1048593])
PY
chunked_payload_sha="$(sha256sum "$chunked_source_file" | awk '{print $1}')"
chunked_payload_size="$(stat -c '%s' "$chunked_source_file")"

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
smoke_track_pid "$!"
smoke_wait_http "$api_url/health"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d '{"username":"file-push-smoke","password":"file-push-smoke-password"}' \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
export VPSMAN_API_TOKEN="$access_token"

api_auth_get() {
  curl -fsS -H "Authorization: Bearer $access_token" "$api_url$1"
}

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="file-push-smoke-gateway" \
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
  "file-push-smoke" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"

deadline=$((SECONDS + 30))
status=""
until [[ "$status" == "online" ]]; do
  if (( SECONDS >= deadline )); then
    smoke_dump_logs "agent did not become online for live file-push smoke" \
      "$api_log" "$gateway_log" "$agent_log"
    exit 1
  fi
  agents_json="$(api_auth_get "/api/v1/agents" || printf '[]')"
  status="$(jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty' <<<"$agents_json")"
  sleep 0.25
done

reject_body="$(jq -nc \
  --arg client "$client_id" \
  --arg path "$destination_file" \
  --arg sha "$payload_sha" \
  --arg data "$payload_b64" \
  --argjson size "$payload_size" \
  '{
    command: "file_push",
    operation: {
      type: "file_push",
      path: $path,
      mode: 384,
      size_bytes: $size,
      sha256_hex: $sha,
      data_base64: $data
    },
    selector_expression: ("id:" + $client),
    target_client_ids: [$client],
    privileged: true,
    confirmed: true,
    max_timeout_secs: 30
  }')"
reject_json="$SMOKE_TMPDIR/reject.json"
reject_status="$(curl -sS -o "$reject_json" -w "%{http_code}" \
  -H "Authorization: Bearer $access_token" \
  -H 'content-type: application/json' \
  -d "$reject_body" \
  "$api_url/api/v1/jobs")"
if [[ "$reject_status" != "403" ]]; then
  echo "expected no-privilege-unlock file push to return 403, got $reject_status" >&2
  cat "$reject_json" >&2 || true
  exit 1
fi
jq -e '.error == "privilege_assertion_required" and .status == 403' "$reject_json" >/dev/null
[[ ! -e "$destination_file" ]]

push_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" file-push \
    --source "$source_file" \
    --path "$destination_file" \
    --mode 0600 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --confirmed)"
job_id="$(jq -r '.job_id' <<<"$push_json")"
smoke_assert_job_create_queued "$push_json" 1
smoke_wait_api_job_status "$api_url" "$job_id" completed 45 >/dev/null

cmp -s "$source_file" "$destination_file"
[[ "$(sha256sum "$destination_file" | awk '{print $1}')" == "$payload_sha" ]]
[[ "$(stat -c '%a' "$destination_file")" == "600" ]]

job_json="$(api_auth_get "/api/v1/jobs/$job_id")"
targets_json="$(api_auth_get "/api/v1/jobs/$job_id/targets")"
outputs_json="$(api_auth_get "/api/v1/jobs/$job_id/outputs")"
audits_json="$(api_auth_get "/api/v1/audit?limit=20")"

jq -e '.status == "completed" and .command_type == "file_push"' <<<"$job_json" >/dev/null
jq -e --arg client "$client_id" '.[] | select(.client_id == $client and .status == "completed" and .exit_code == 0)' <<<"$targets_json" >/dev/null
jq -e --arg path "$destination_file" --arg sha "$payload_sha" '
  .items[] | select(.stream == "status" and .done == true and .exit_code == 0)
  | (.data_base64 | @base64d | fromjson)
  | .type == "file_push" and .path == $path and .sha256_hex == $sha and .atomic == true
' <<<"$outputs_json" >/dev/null
jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' <<<"$audits_json" >/dev/null

decoded_outputs="$(
  jq -r '.items[].data_base64' <<<"$outputs_json" | while IFS= read -r item; do
    printf '%s' "$item" | base64 -d
    printf '\n'
  done
)"
if grep -Fq "$payload" <<<"$decoded_outputs"; then
  echo "job outputs leaked raw file-push payload" >&2
  exit 1
fi

chunked_push_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" file-push \
    --source "$chunked_source_file" \
    --path "$chunked_destination_file" \
    --mode 0640 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --confirmed)"
chunked_job_id="$(jq -r '.job_id' <<<"$chunked_push_json")"
smoke_assert_job_create_queued "$chunked_push_json" 1
smoke_wait_api_job_status "$api_url" "$chunked_job_id" completed 45 >/dev/null

cmp -s "$chunked_source_file" "$chunked_destination_file"
[[ "$(sha256sum "$chunked_destination_file" | awk '{print $1}')" == "$chunked_payload_sha" ]]
[[ "$(stat -c '%a' "$chunked_destination_file")" == "640" ]]

chunked_job_json="$(api_auth_get "/api/v1/jobs/$chunked_job_id")"
chunked_outputs_json="$(api_auth_get "/api/v1/jobs/$chunked_job_id/outputs")"
jq -e '.status == "completed" and .command_type == "file_push_chunked"' <<<"$chunked_job_json" >/dev/null
jq -e --arg path "$chunked_destination_file" --arg sha "$chunked_payload_sha" --argjson size "$chunked_payload_size" '
  .items[] | select(.stream == "status" and .done == true and .exit_code == 0)
  | (.data_base64 | @base64d | fromjson)
  | .type == "file_push_chunked"
    and .path == $path
    and .sha256_hex == $sha
    and .size_bytes == $size
    and .atomic == true
    and .chunk_count > 1
' <<<"$chunked_outputs_json" >/dev/null

jq -n \
  --arg client_id "$client_id" \
  --arg job_id "$job_id" \
  --arg chunked_job_id "$chunked_job_id" \
  --arg destination "$destination_file" \
  --arg chunked_destination "$chunked_destination_file" \
  --arg sha256_hex "$payload_sha" \
  --arg chunked_sha256_hex "$chunked_payload_sha" \
  '{
    live_file_push_smoke: "ok",
    no_privilege_unlock_rejected: true,
    client_id: $client_id,
    job_id: $job_id,
    chunked_job_id: $chunked_job_id,
    destination: $destination,
    chunked_destination: $chunked_destination,
    sha256_hex: $sha256_hex,
    chunked_sha256_hex: $chunked_sha256_hex,
    checks: ["inline_file_push", "chunked_file_push"]
  }'
