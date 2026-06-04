#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk base64 cmp curl jq python3 sha256sum shuf stat timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-live-file-push"

api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
internal_token="smoke-internal-$(date +%s%N)"
client_id="file-push-smoke-$(date +%s)"
super_password="smoke-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"
signing_keys="$(target/debug/vpsctl signing-keygen)"
server_signing_private_hex="$(jq -r '.private_key_hex' <<<"$signing_keys")"

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
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
VPSMAN_SERVER_SIGNING_KEY_HEX="$server_signing_private_hex" \
VPSMAN_PUBLIC_GATEWAY_ENDPOINTS="primary=$gateway_addr=10" \
VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX="$gateway_public_hex" \
RUST_LOG="vpsman_api=warn" \
  target/debug/vpsman-api >"$api_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_http "$api_url/health"

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_NOISE_MODE="enrolled_ik" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_GATEWAY_ID="file-push-smoke-gateway" \
RUST_LOG="vpsman_gateway=warn" \
  target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_tcp 127.0.0.1 "$gateway_port"
smoke_wait_tcp 127.0.0.1 "$gateway_control_port"

token_json="$(target/debug/vpsctl --api-url "$api_url" enrollment-token-create \
  --ttl-secs 600 \
  --default-tags file-push-smoke)"
enrollment_token="$(jq -r '.token' <<<"$token_json")"

VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" enroll-config \
    --token "$enrollment_token" \
    --client-id "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --output-file "$agent_config"

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"

deadline=$((SECONDS + 30))
status=""
until [[ "$status" == "connected" ]]; do
  if (( SECONDS >= deadline )); then
    smoke_dump_logs "agent did not connect for live file-push smoke" \
      "$api_log" "$gateway_log" "$agent_log"
    exit 1
  fi
  agents_json="$(curl -fsS "$api_url/api/v1/agents" || printf '[]')"
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
    clients: [$client],
    pools: [],
    tags: [],
    privileged: true,
    destructive: false,
    confirmed: true,
    timeout_secs: 30,
    envelope: null,
    envelopes: {}
  }')"
reject_json="$SMOKE_TMPDIR/reject.json"
reject_status="$(curl -sS -o "$reject_json" -w "%{http_code}" \
  -H 'content-type: application/json' \
  -d "$reject_body" \
  "$api_url/api/v1/jobs")"
if [[ "$reject_status" != "403" ]]; then
  echo "expected no-proof file push to return 403, got $reject_status" >&2
  cat "$reject_json" >&2 || true
  exit 1
fi
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
jq -e '.accepted_targets == 1 and .status == "completed"' <<<"$push_json" >/dev/null

cmp -s "$source_file" "$destination_file"
[[ "$(sha256sum "$destination_file" | awk '{print $1}')" == "$payload_sha" ]]
[[ "$(stat -c '%a' "$destination_file")" == "600" ]]

job_json="$(curl -fsS "$api_url/api/v1/jobs/$job_id")"
targets_json="$(curl -fsS "$api_url/api/v1/jobs/$job_id/targets")"
outputs_json="$(curl -fsS "$api_url/api/v1/jobs/$job_id/outputs")"
audits_json="$(curl -fsS "$api_url/api/v1/audit?limit=20")"

jq -e '.status == "completed" and .command_type == "file_push"' <<<"$job_json" >/dev/null
jq -e --arg client "$client_id" '.[] | select(.client_id == $client and .status == "completed" and .exit_code == 0)' <<<"$targets_json" >/dev/null
jq -e --arg path "$destination_file" --arg sha "$payload_sha" '
  .[] | select(.stream == "status" and .done == true and .exit_code == 0)
  | (.data_base64 | @base64d | fromjson)
  | .type == "file_push" and .path == $path and .sha256_hex == $sha and .atomic == true
' <<<"$outputs_json" >/dev/null
jq -e '[.[].action] | index("job.rejected_authorization_required") and index("job.dispatch_requested") and index("job.target_result")' <<<"$audits_json" >/dev/null

decoded_outputs="$(
  jq -r '.[].data_base64' <<<"$outputs_json" | while IFS= read -r item; do
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
jq -e '.accepted_targets == 1 and .status == "completed"' <<<"$chunked_push_json" >/dev/null

cmp -s "$chunked_source_file" "$chunked_destination_file"
[[ "$(sha256sum "$chunked_destination_file" | awk '{print $1}')" == "$chunked_payload_sha" ]]
[[ "$(stat -c '%a' "$chunked_destination_file")" == "640" ]]

chunked_job_json="$(curl -fsS "$api_url/api/v1/jobs/$chunked_job_id")"
chunked_outputs_json="$(curl -fsS "$api_url/api/v1/jobs/$chunked_job_id/outputs")"
jq -e '.status == "completed" and .command_type == "file_push_chunked"' <<<"$chunked_job_json" >/dev/null
jq -e --arg path "$chunked_destination_file" --arg sha "$chunked_payload_sha" --argjson size "$chunked_payload_size" '
  .[] | select(.stream == "status" and .done == true and .exit_code == 0)
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
    no_proof_rejected: true,
    client_id: $client_id,
    job_id: $job_id,
    chunked_job_id: $chunked_job_id,
    destination: $destination,
    chunked_destination: $chunked_destination,
    sha256_hex: $sha256_hex,
    chunked_sha256_hex: $chunked_sha256_hex,
    checks: ["inline_file_push", "chunked_file_push"]
  }'
