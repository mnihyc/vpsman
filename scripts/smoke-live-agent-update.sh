#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk base64 chmod cmp curl docker grep jq openssl python3 sed sha256sum shuf stat timeout
smoke_build_binaries
if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsman-agent --release
fi
smoke_init_tmpdir "vpsman-live-agent-update"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
artifact_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
artifact_url="https://localhost:$artifact_port/vpsman-agent-new"
container_name="vpsman-live-agent-update-$(date +%s%N)"
internal_token="agent-update-internal-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
client_id="agent-update-smoke-$(date +%s)"
super_password="agent-update-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"
signing_keys="$(target/debug/vpsctl signing-keygen)"
server_signing_private_hex="$(jq -r '.private_key_hex' <<<"$signing_keys")"

api_pid=""
api_log=""
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_pid=""
agent_log="$SMOKE_TMPDIR/agent-initial.log"
https_log="$SMOKE_TMPDIR/update-https.log"
agent_config="$SMOKE_TMPDIR/agent.toml"
agent_bin="$SMOKE_TMPDIR/vpsman-agent"
staged_agent="$SMOKE_TMPDIR/vpsman-agent.next"
rollback_agent="$SMOKE_TMPDIR/vpsman-agent.rollback"
activation_marker="$SMOKE_TMPDIR/vpsman-agent.activated.json"
artifact_dir="$SMOKE_TMPDIR/artifacts"
artifact_file="$artifact_dir/vpsman-agent-new"
server_key="$SMOKE_TMPDIR/update-server.key"
server_csr="$SMOKE_TMPDIR/update-server.csr"
server_cert="$SMOKE_TMPDIR/update-server.crt"
server_ext="$SMOKE_TMPDIR/update-server.ext"
ca_key="$SMOKE_TMPDIR/update-ca.key"
ca_cert="$SMOKE_TMPDIR/update-ca.crt"
update_status_file="$SMOKE_TMPDIR/update-status.json"
mkdir -p "$artifact_dir"

cp target/debug/vpsman-agent "$agent_bin"
chmod 0755 "$agent_bin"
release_agent_bin="${VPSMAN_AGENT_UPDATE_ARTIFACT_BIN:-target/release/vpsman-agent}"
if [[ ! -x "$release_agent_bin" && -x target/x86_64-unknown-linux-musl/release/vpsman-agent ]]; then
  release_agent_bin="target/x86_64-unknown-linux-musl/release/vpsman-agent"
fi
if [[ ! -x "$release_agent_bin" ]]; then
  echo "missing release agent artifact: $release_agent_bin" >&2
  exit 1
fi
cp "$release_agent_bin" "$artifact_file"
printf '\n# vpsman staged update artifact %s\n' "$(date +%s%N)" >>"$artifact_file"
chmod 0755 "$artifact_file"
artifact_sha="$(sha256sum "$artifact_file" | awk '{print $1}')"
update_signing_seed_hex="7777777777777777777777777777777777777777777777777777777777777777"
update_signature_json="$(target/debug/vpsctl agent-update-signature \
  --artifact-file "$artifact_file" \
  --signing-seed-hex "$update_signing_seed_hex")"
artifact_signature_hex="$(jq -r '.artifact_signature_hex' <<<"$update_signature_json")"
artifact_signing_key_hex="$(jq -r '.artifact_signing_key_hex' <<<"$update_signature_json")"
jq -e --arg sha "$artifact_sha" '.artifact_sha256_hex == $sha' \
  <<<"$update_signature_json" >/dev/null

cleanup_live_agent_update_smoke() {
  local kept_tmpdir="${SMOKE_TMPDIR:-}"
  if [[ "${VPSMAN_SMOKE_KEEP_TMP:-0}" == "1" ]]; then
    SMOKE_TMPDIR=""
  fi
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
  if [[ "${VPSMAN_SMOKE_KEEP_TMP:-0}" == "1" && -n "$kept_tmpdir" ]]; then
    echo "kept smoke tmpdir: $kept_tmpdir" >&2
  fi
}
trap cleanup_live_agent_update_smoke EXIT

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$ca_key" \
  -out "$ca_cert" \
  -subj '/CN=vpsman update smoke CA' \
  -addext 'basicConstraints=critical,CA:TRUE' \
  -addext 'keyUsage=critical,keyCertSign,cRLSign' \
  -days 1 >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes \
  -keyout "$server_key" \
  -out "$server_csr" \
  -subj '/CN=localhost' >/dev/null 2>&1
printf '%s\n' \
  'basicConstraints=critical,CA:FALSE' \
  'keyUsage=critical,digitalSignature,keyEncipherment' \
  'extendedKeyUsage=serverAuth' \
  'subjectAltName=DNS:localhost' >"$server_ext"
openssl x509 -req \
  -in "$server_csr" \
  -CA "$ca_cert" \
  -CAkey "$ca_key" \
  -CAcreateserial \
  -out "$server_cert" \
  -days 1 \
  -extfile "$server_ext" >/dev/null 2>&1

(
  cd "$artifact_dir"
  openssl s_server -quiet -WWW \
    -accept "127.0.0.1:$artifact_port" \
    -cert "$server_cert" \
    -key "$server_key"
) >"$https_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_tcp 127.0.0.1 "$artifact_port"
curl -kfsS "$artifact_url" >/dev/null

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
    VPSMAN_INTERNAL_TOKEN="$internal_token" \
    VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
    VPSMAN_SERVER_SIGNING_KEY_HEX="$server_signing_private_hex" \
    VPSMAN_PUBLIC_GATEWAY_ENDPOINTS="primary=$gateway_addr=10" \
    VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX="$gateway_public_hex" \
    RUST_LOG="${VPSMAN_SMOKE_API_RUST_LOG:-vpsman_api=warn}" \
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
  smoke_dump_logs "live agent-update API failed to start" "$SMOKE_TMPDIR"/api-"$label"-*.log
  exit 1
}

stop_agent() {
  if [[ -n "$agent_pid" ]]; then
    kill "$agent_pid" >/dev/null 2>&1 || true
    wait "$agent_pid" >/dev/null 2>&1 || true
    agent_pid=""
  fi
}

start_agent() {
  local label="$1"
  agent_log="$SMOKE_TMPDIR/agent-$label.log"
  VPSMAN_AGENT_CONFIG="$agent_config" \
  VPSMAN_UPDATE_ROOT_CERT_PEM="$ca_cert" \
  RUST_LOG="${VPSMAN_SMOKE_AGENT_RUST_LOG:-vpsman_agent=warn}" \
    "$agent_bin" run >"$agent_log" 2>&1 &
  agent_pid="$!"
  smoke_track_pid "$agent_pid"
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
      smoke_dump_logs "agent did not become online for live agent-update smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$SMOKE_TMPDIR"/agent-*.log "$https_log"
      exit 1
    fi
    status="$(api_get "/api/v1/agents" \
      | jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty')"
    sleep 0.25
  done
}

status_output_for_job() {
  local job_id="$1"
  local encoded
  encoded="$(api_get "/api/v1/jobs/$job_id/outputs" \
    | jq -er 'first(.[] | select(.stream == "status" and .done == true and .exit_code == 0) | .data_base64)')"
  printf '%s' "$encoded" | base64 -d >"$update_status_file"
}

assert_update_persisted() {
  local expected_rollout_status="$1"
  local expected_target_status="$2"
  local job_json targets_json outputs_json audits_json rollouts_json cli_rollouts_json decoded_outputs
  job_json="$(api_get "/api/v1/jobs/$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=30")"
  rollouts_json="$(api_get "/api/v1/agent-update-rollouts?limit=10")"
  cli_rollouts_json="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" agent-update-rollouts --limit 10)"

  jq -e '.status == "completed" and .command_type == "agent_update" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e \
    --arg job "$job_id" \
    --arg client "$client_id" \
    --arg sha "$artifact_sha" \
    --arg expected_rollout_status "$expected_rollout_status" \
    --arg expected_target_status "$expected_target_status" '
    any(.[]; .job_id == $job
      and .status == $expected_rollout_status
      and .artifact_sha256_hex == $sha
      and .artifact_signature_provided == true
      and .activation_policy == "manual_staging_only"
      and .target_count == 1
      and .completed_count == 1
      and .failed_count == 0
      and .pending_count == 0
      and (.artifact_signing_key_sha256_hex | type == "string")
      and (.artifact_signing_key_sha256_hex != "")
      and (.artifact_signing_key_sha256_hex != $sha)
      and (.targets | length == 1)
      and .targets[0].client_id == $client
      and .targets[0].status == $expected_target_status
      and .targets[0].exit_code == 0)
  ' <<<"$rollouts_json" >/dev/null
  jq -e \
    --arg job "$job_id" \
    --arg expected_rollout_status "$expected_rollout_status" \
    'any(.[]; .job_id == $job and .status == $expected_rollout_status)' \
    <<<"$cli_rollouts_json" >/dev/null
  if grep -F \
    -e "$artifact_url" \
    -e "$artifact_signature_hex" \
    -e "$artifact_signing_key_hex" \
    <<<"$rollouts_json" >/dev/null; then
    echo "agent-update rollout records leaked artifact URL, signature, or signing public key" >&2
    exit 1
  fi
  status_output_for_job "$job_id"
  jq -e --arg sha "$artifact_sha" --arg tmpdir "$SMOKE_TMPDIR" '
    .type == "agent_update"
      and .status == "staged"
      and .sha256_hex == $sha
      and (.staged_path | startswith($tmpdir))
      and (.rollback_path | startswith($tmpdir))
      and (.staged_path | endswith(".next"))
      and (.rollback_path | endswith(".rollback"))
      and .activation == "manual_restart_required"
      and .signature == "verified"
      and .size_bytes > 0
      and (.artifact_url == null)
  ' "$update_status_file" >/dev/null
  jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' \
    <<<"$audits_json" >/dev/null

  decoded_outputs="$(
    jq -r '.[].data_base64' <<<"$outputs_json" | while IFS= read -r item; do
      printf '%s' "$item" | base64 -d
      printf '\n'
    done
  )"
  if grep -F \
    -e "$artifact_url" \
    -e "$super_password" \
    -e "$super_salt_hex" \
    -e "$artifact_signature_hex" \
    -e "$artifact_signing_key_hex" \
    -e "privilege_assertion" \
    -e "client_private_key_hex" \
    -e "server_public_key_hex" \
    -e "server_ed25519_public_key_hex" \
    <<<"$decoded_outputs" >/dev/null; then
    echo "agent-update outputs leaked artifact URL, privilege material, or trust anchors" >&2
    exit 1
  fi
}

wait_update_heartbeat_verified() {
  local deadline=$((SECONDS + 35))
  until api_get "/api/v1/agent-update-rollouts?limit=10" | jq -e \
    --arg job "$job_id" \
    --arg client "$client_id" \
    --arg sha "$artifact_sha" '
    any(.[]; .job_id == $job
      and .status == "heartbeat_verified"
      and .artifact_sha256_hex == $sha
      and (.targets | length == 1)
      and .targets[0].client_id == $client
      and .targets[0].status == "heartbeat_verified"
      and .targets[0].exit_code == 0)
  ' >/dev/null; do
    if (( SECONDS >= deadline )); then
      smoke_dump_logs "agent update heartbeat was not recorded after activated restart" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$SMOKE_TMPDIR"/agent-*.log "$https_log"
      api_get "/api/v1/agent-update-rollouts?limit=10" >&2 || true
      exit 1
    fi
    sleep 0.25
  done

  api_get "/api/v1/audit?limit=50" | jq -e \
    --arg activate_job "$activate_job_id" \
    --arg client "$client_id" \
    --arg sha "$artifact_sha" '
    any(.[]; .action == "agent_update.heartbeat_verified"
      and .target == ("client:" + $client)
      and .metadata.activation_job_id == $activate_job
      and .metadata.artifact_sha256_hex == $sha
      and .metadata.heartbeat == "post_restart_activation_marker")
  ' >/dev/null
}

assert_update_activation_pending() {
  api_get "/api/v1/agent-update-rollouts?limit=10" | jq -e \
    --arg job "$job_id" \
    --arg client "$client_id" \
    --arg sha "$artifact_sha" '
    any(.[]; .job_id == $job
      and .status == "activation_pending_restart"
      and .artifact_sha256_hex == $sha
      and .completed_count == 0
      and .pending_count == 1
      and (.targets | length == 1)
      and .targets[0].client_id == $client
      and .targets[0].status == "activation_pending_restart"
      and .targets[0].exit_code == 0)
  ' >/dev/null

  api_get "/api/v1/audit?limit=50" | jq -e \
    --arg activate_job "$activate_job_id" \
    --arg client "$client_id" \
    --arg sha "$artifact_sha" '
    any(.[]; .action == "agent_update.activation_pending_restart"
      and .target == ("client:" + $client)
      and .metadata.activation_job_id == $activate_job
      and .metadata.artifact_sha256_hex == $sha
      and .metadata.status == "activation_pending_restart")
  ' >/dev/null
}

assert_update_activation_failed() {
  local failed_stage_job_id="$1"
  local failed_rollout_id="$2"
  local failed_activate_job_id="$3"
  local failed_job_json failed_targets_json failed_outputs_json failed_rollouts_json failed_audits_json
  local decoded_failed_outputs

  failed_job_json="$(api_get "/api/v1/jobs/$failed_activate_job_id")"
  failed_targets_json="$(api_get "/api/v1/jobs/$failed_activate_job_id/targets")"
  failed_outputs_json="$(api_get "/api/v1/jobs/$failed_activate_job_id/outputs")"
  failed_rollouts_json="$(api_get "/api/v1/agent-update-rollouts?limit=20")"
  failed_audits_json="$(api_get "/api/v1/audit?limit=80")"

  jq -e '.status == "failed" and .command_type == "agent_update_activate" and .target_count == 1' \
    <<<"$failed_job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1
      and .[0].client_id == $client
      and .[0].status == "failed"
      and .[0].exit_code == 127
  ' <<<"$failed_targets_json" >/dev/null
  jq -e \
    --arg rollout "$failed_rollout_id" \
    --arg stage_job "$failed_stage_job_id" \
    --arg client "$client_id" \
    --arg sha "$artifact_sha" '
    any(.[]; .id == $rollout
      and .job_id == $stage_job
      and .status == "activation_failed"
      and .artifact_sha256_hex == $sha
      and .completed_count == 0
      and .failed_count == 1
      and .pending_count == 0
      and (.targets | length == 1)
      and .targets[0].client_id == $client
      and .targets[0].status == "activation_failed"
      and .targets[0].exit_code == 127)
  ' <<<"$failed_rollouts_json" >/dev/null
  jq -e \
    --arg activate_job "$failed_activate_job_id" \
    --arg client "$client_id" \
    --arg sha "$artifact_sha" '
    any(.[]; .action == "agent_update.activation_failed"
      and .target == ("client:" + $client)
      and .metadata.activation_job_id == $activate_job
      and .metadata.artifact_sha256_hex == $sha
      and .metadata.status == "activation_failed"
      and .metadata.activation_outcome_status == "failed"
      and .metadata.exit_code == 127
      and .metadata.rollback_recommended == true)
  ' <<<"$failed_audits_json" >/dev/null

  decoded_failed_outputs="$(
    jq -r '.[].data_base64' <<<"$failed_outputs_json" | while IFS= read -r item; do
      printf '%s' "$item" | base64 -d
      printf '\n'
    done
  )"
  if ! grep -F "failed to read staged update" <<<"$decoded_failed_outputs" >/dev/null; then
    echo "failed activation output did not explain missing staged artifact" >&2
    printf '%s\n' "$decoded_failed_outputs" >&2
    exit 1
  fi
  if grep -F \
    -e "$artifact_url" \
    -e "$super_password" \
    -e "$super_salt_hex" \
    -e "$artifact_signature_hex" \
    -e "$artifact_signing_key_hex" \
    -e "privilege_assertion" \
    -e "client_private_key_hex" \
    -e "server_public_key_hex" \
    -e "server_ed25519_public_key_hex" \
    <<<"$decoded_failed_outputs" >/dev/null; then
    echo "failed activation outputs leaked artifact URL, privilege material, or trust anchors" >&2
    exit 1
  fi
}

dispatch_direct_rollback_for_failed_activation() {
  local failed_rollout_id="$1"
  local failed_activate_job_id="$2"
  local failed_rollback_sha="$3"
  local rollback_json rollback_job_id

  rollback_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" agent-update-rollout-rollback \
      --rollout-id "$failed_rollout_id" \
      --rollback-sha256-hex "$failed_rollback_sha" \
      --clients "$client_id" \
      --super-salt-hex "$super_salt_hex" \
      --privilege-ttl-secs 600 \
      --timeout-secs 10 \
      --force-unprivileged \
      --confirmed)"
  rollback_job_id="$(jq -r '.job_id' <<<"$rollback_json")"
  jq -e '.accepted_targets == 1 and .status == "completed"' <<<"$rollback_json" >/dev/null

  api_get "/api/v1/jobs/$rollback_job_id" | jq -e \
    '.status == "completed" and .command_type == "agent_update_rollback" and .target_count == 1' \
    >/dev/null
  api_get "/api/v1/jobs/$rollback_job_id/targets" | jq -e --arg client "$client_id" '
    length == 1
      and .[0].client_id == $client
      and .[0].status == "completed"
      and .[0].exit_code == 0
  ' >/dev/null
  api_get "/api/v1/audit?limit=100" | jq -e \
    --arg rollback_job "$rollback_job_id" \
    --arg activate_job "$failed_activate_job_id" \
    --arg client "$client_id" \
    --arg sha "$failed_rollback_sha" '
    any(.[]; .action == "agent_update.rollback_completed"
      and .target == ("client:" + $client)
      and .metadata.rollback_job_id == $rollback_job
      and .metadata.rollback_sha256_hex == $sha
      and .metadata.previous_status == "activation_failed"
      and .metadata.status == "rolled_back")
    and any(.[]; .action == "agent_update.activation_failed"
      and .target == ("client:" + $client)
      and .metadata.activation_job_id == $activate_job
      and .metadata.rollback_recommended == true)
  ' >/dev/null

  printf '%s' "$rollback_job_id"
}

start_api "first"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d '{"username":"agent-update-smoke","password":"agent-update-smoke-password"}' \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
jq -e '.operator.username == "agent-update-smoke" and .token_type == "Bearer"' \
  <<<"$auth_json" >/dev/null

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_NOISE_MODE="enrolled_ik" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="agent-update-smoke-gateway" \
RUST_LOG="vpsman_gateway=warn" \
  target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_tcp 127.0.0.1 "$gateway_port"
smoke_wait_tcp 127.0.0.1 "$gateway_control_port"

token_json="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" enrollment-token-create \
    --ttl-secs 600 \
    --default-tags agent-update-smoke)"
enrollment_token="$(jq -r '.token' <<<"$token_json")"

target/debug/vpsctl --api-url "$api_url" enroll-config \
  --token "$enrollment_token" \
  --output-file "$agent_config"
client_id="$(smoke_agent_config_client_id "$agent_config")"
if [[ -z "$client_id" ]]; then
  smoke_fail "enroll-config did not write client_id for live agent-update smoke"
fi
if grep -q '^trusted_artifact_signing_key_hex = ' "$agent_config"; then
  sed -i "s/^trusted_artifact_signing_key_hex = .*/trusted_artifact_signing_key_hex = \"$artifact_signing_key_hex\"/" "$agent_config"
elif grep -q '^\[update\]' "$agent_config"; then
  sed -i "/^\[update\]/a trusted_artifact_signing_key_hex = \"$artifact_signing_key_hex\"" "$agent_config"
else
  cat >>"$agent_config" <<TOML

[update]
trusted_artifact_signing_key_hex = "$artifact_signing_key_hex"
TOML
fi

start_agent "initial"
wait_agent_online

reject_body="$(jq -nc \
  --arg client "$client_id" \
  --arg artifact_url "$artifact_url" \
  --arg sha "$artifact_sha" \
  '{
    command: "agent_update",
    operation: {
      type: "agent_update",
      artifact_url: $artifact_url,
      sha256_hex: $sha
    },
    selector_expression: ("id:" + $client),
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
  echo "expected no-privilege-unlock agent-update to return 403, got $reject_status" >&2
  cat "$reject_json" >&2 || true
  exit 1
fi
jq -e '.error == "privilege_assertion_required" and .status == 403' "$reject_json" >/dev/null
if [[ -e "$staged_agent" || -e "$rollback_agent" ]]; then
  echo "agent update staged files after no-privilege-unlock rejection" >&2
  exit 1
fi

update_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" agent-update \
    --artifact-url "$artifact_url" \
    --sha256-hex "$artifact_sha" \
    --artifact-signature-hex "$artifact_signature_hex" \
    --artifact-signing-key-hex "$artifact_signing_key_hex" \
    --clients "$client_id" \
    --canary-count 1 \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --force-unprivileged \
    --confirmed)"
job_id="$(jq -r '.job_id' <<<"$update_json")"
initial_stage_job_id="$job_id"
if ! jq -e '.accepted_targets == 1 and .status == "completed"' <<<"$update_json" >/dev/null; then
  echo "agent update job did not complete" >&2
  api_get "/api/v1/jobs/$job_id/outputs" >&2 || true
  smoke_dump_logs "live agent-update staging failure logs" \
    "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$SMOKE_TMPDIR"/agent-*.log "$https_log"
  exit 1
fi

assert_update_persisted "staged" "completed"
rollout_id="$(api_get "/api/v1/agent-update-rollouts?limit=10" \
  | jq -er --arg job "$job_id" 'first(.[] | select(.job_id == $job) | .id)')"
reported_staged_agent="$(jq -r '.staged_path' "$update_status_file")"
reported_rollback_agent="$(jq -r '.rollback_path' "$update_status_file")"
rollback_sha="$(sha256sum "$reported_rollback_agent" | awk '{print $1}')"
if ! cmp -s "$artifact_file" "$reported_staged_agent"; then
  echo "staged update artifact does not match source artifact" >&2
  ls -la "$SMOKE_TMPDIR" >&2 || true
  exit 1
fi
if ! cmp -s "$agent_bin" "$reported_rollback_agent"; then
  echo "rollback update artifact does not match running agent copy" >&2
  ls -la "$SMOKE_TMPDIR" >&2 || true
  exit 1
fi
[[ "$(stat -c '%a' "$reported_staged_agent")" == "755" ]]

activate_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" agent-update-rollout-activate \
    --rollout-id "$rollout_id" \
    --batch-size 1 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --force-unprivileged \
    --confirmed)"
activate_job_id="$(jq -r '.job_id' <<<"$activate_json")"
initial_activate_job_id="$activate_job_id"
jq -e '.accepted_targets == 1 and .status == "completed"' <<<"$activate_json" >/dev/null
cmp -s "$artifact_file" "$agent_bin"
if [[ ! -f "$activation_marker" ]]; then
  echo "agent update activation did not write restart heartbeat marker" >&2
  exit 1
fi
jq -e --arg job "$activate_job_id" --arg sha "$artifact_sha" '
  .activation_job_id == $job
    and .sha256_hex == $sha
    and .marker_unix > 0
' "$activation_marker" >/dev/null
if [[ -e "$staged_agent" ]]; then
  echo "agent update activation did not remove staged artifact" >&2
  exit 1
fi
activate_outputs_json="$(api_get "/api/v1/jobs/$activate_job_id/outputs")"
jq -e --arg sha "$artifact_sha" '
  .[] | select(.stream == "status" and .done == true and .exit_code == 0)
  | (.data_base64 | @base64d | fromjson)
  | .type == "agent_update_activation" and .status == "activated_pending_restart" and .sha256_hex == $sha
' <<<"$activate_outputs_json" >/dev/null
assert_update_activation_pending

stop_agent
sleep 1
start_agent "activated"
wait_agent_online
wait_update_heartbeat_verified
assert_update_persisted "heartbeat_verified" "heartbeat_verified"

rollback_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" agent-update-rollout-rollback \
    --rollout-id "$rollout_id" \
    --rollback-sha256-hex "$rollback_sha" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --force-unprivileged \
    --confirmed)"
rollback_job_id="$(jq -r '.job_id' <<<"$rollback_json")"
initial_rollback_job_id="$rollback_job_id"
jq -e '.accepted_targets == 1 and .status == "completed"' <<<"$rollback_json" >/dev/null
cmp -s "$agent_bin" "$reported_rollback_agent"
if [[ -e "$activation_marker" ]]; then
  echo "agent update rollback did not remove restart heartbeat marker" >&2
  exit 1
fi
rollback_outputs_json="$(api_get "/api/v1/jobs/$rollback_job_id/outputs")"
jq -e --arg sha "$rollback_sha" '
  .[] | select(.stream == "status" and .done == true and .exit_code == 0)
  | (.data_base64 | @base64d | fromjson)
  | .type == "agent_update_rollback" and .status == "rolled_back_pending_restart" and .rollback_sha256_hex == $sha
' <<<"$rollback_outputs_json" >/dev/null
api_get "/api/v1/audit?limit=60" | jq -e \
  --arg rollback_job "$rollback_job_id" \
  --arg client "$client_id" \
  --arg sha "$rollback_sha" '
  any(.[]; .action == "agent_update.rollback_completed"
    and .target == ("client:" + $client)
    and .metadata.rollback_job_id == $rollback_job
    and .metadata.rollback_sha256_hex == $sha
    and .metadata.previous_status == "heartbeat_verified"
    and .metadata.status == "rolled_back")
' >/dev/null

stop_api
start_api "restart"
api_get "/api/v1/auth/me" | jq -e '.username == "agent-update-smoke"' >/dev/null
assert_update_persisted "rolled_back" "rolled_back"

failed_stage_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" agent-update \
    --artifact-url "$artifact_url" \
    --sha256-hex "$artifact_sha" \
    --artifact-signature-hex "$artifact_signature_hex" \
    --artifact-signing-key-hex "$artifact_signing_key_hex" \
    --clients "$client_id" \
    --canary-count 1 \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --force-unprivileged \
    --confirmed)"
failed_stage_job_id="$(jq -r '.job_id' <<<"$failed_stage_json")"
if ! jq -e '.accepted_targets == 1 and .status == "completed"' \
  <<<"$failed_stage_json" >/dev/null; then
  echo "second agent update staging job did not complete" >&2
  api_get "/api/v1/jobs/$failed_stage_job_id/outputs" >&2 || true
  smoke_dump_logs "live agent-update failure-scenario staging logs" \
    "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$SMOKE_TMPDIR"/agent-*.log "$https_log"
  exit 1
fi
job_id="$failed_stage_job_id"
assert_update_persisted "staged" "completed"
failed_rollout_id="$(api_get "/api/v1/agent-update-rollouts?limit=20" \
  | jq -er --arg job "$failed_stage_job_id" 'first(.[] | select(.job_id == $job) | .id)')"
failed_staged_agent="$(jq -r '.staged_path' "$update_status_file")"
failed_rollback_agent="$(jq -r '.rollback_path' "$update_status_file")"
failed_rollback_sha="$(sha256sum "$failed_rollback_agent" | awk '{print $1}')"
[[ -f "$failed_staged_agent" ]]
[[ -f "$failed_rollback_agent" ]]

rm -f "$failed_staged_agent"
failed_activate_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" agent-update-rollout-activate \
    --rollout-id "$failed_rollout_id" \
    --batch-size 1 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --force-unprivileged \
    --confirmed)"
failed_activate_job_id="$(jq -r '.job_id' <<<"$failed_activate_json")"
jq -e '.accepted_targets == 1 and .status == "failed"' \
  <<<"$failed_activate_json" >/dev/null
if [[ -e "$activation_marker" ]]; then
  echo "failed activation unexpectedly wrote restart heartbeat marker" >&2
  exit 1
fi
assert_update_activation_failed \
  "$failed_stage_job_id" \
  "$failed_rollout_id" \
  "$failed_activate_job_id"
failed_recovery_rollback_job_id="$(
  dispatch_direct_rollback_for_failed_activation \
    "$failed_rollout_id" \
    "$failed_activate_job_id" \
    "$failed_rollback_sha"
)"
cmp -s "$agent_bin" "$failed_rollback_agent"

if grep -F "agent update heartbeat timeout reconciliation failed" "$SMOKE_TMPDIR"/api-*.log >/dev/null 2>&1; then
  smoke_dump_logs "agent update heartbeat timeout reconciler failed during live smoke" \
    "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$SMOKE_TMPDIR"/agent-*.log "$https_log"
  exit 1
fi

jq -n \
  --arg client_id "$client_id" \
  --arg job_id "$initial_stage_job_id" \
  --arg activate_job_id "$initial_activate_job_id" \
  --arg rollback_job_id "$initial_rollback_job_id" \
  --arg failed_stage_job_id "$failed_stage_job_id" \
  --arg failed_activate_job_id "$failed_activate_job_id" \
  --arg failed_recovery_rollback_job_id "$failed_recovery_rollback_job_id" \
  --arg sha256_hex "$artifact_sha" \
  '{
    live_agent_update_smoke: "ok",
    postgres_backed: true,
    auth_session: "persisted",
    api_restart: "verified",
    no_privilege_unlock_rejected: true,
    https_artifact: "trusted_private_root",
    signed_artifact: "verified_ed25519",
    rollout_record: "operator_canary_activation_rollback_and_activation_failure_recovery",
    heartbeat: "verified_after_restart",
    rollback: "rolled_back_recorded",
    activation_failure: "activation_failed_recorded",
    failed_activation_rollback: "direct_dispatch_completed",
    client_id: $client_id,
    job_id: $job_id,
    activate_job_id: $activate_job_id,
    rollback_job_id: $rollback_job_id,
    failed_stage_job_id: $failed_stage_job_id,
    failed_activate_job_id: $failed_activate_job_id,
    failed_recovery_rollback_job_id: $failed_recovery_rollback_job_id,
    sha256_hex: $sha256_hex
  }'
