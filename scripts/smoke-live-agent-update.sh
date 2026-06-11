#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk base64 chmod cmp curl docker grep jq openssl sed sha256sum stat
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
super_password="agent-update-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

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
printf '\n# vpsman direct update artifact %s\n' "$(date +%s%N)" >>"$artifact_file"
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

wait_job_terminal() {
  local job_id="$1"
  local deadline=$((SECONDS + 75))
  local status=""
  while true; do
    local job_json
    job_json="$(api_get "/api/v1/jobs/$job_id")"
    status="$(jq -r '.status' <<<"$job_json")"
    case "$status" in
      completed|partially_completed|failed|timed_out|dispatch_failed|degraded_unprivileged|rejected_authorization_required)
        printf '%s' "$job_json"
        return
        ;;
    esac
    if (( SECONDS >= deadline )); then
      api_get "/api/v1/jobs/$job_id/targets" >&2 || true
      api_get "/api/v1/jobs/$job_id/outputs" >&2 || true
      smoke_dump_logs "job did not reach a terminal status during live agent-update smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$SMOKE_TMPDIR"/agent-*.log "$https_log"
      exit 1
    fi
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

assert_single_target_completed() {
  local job_id="$1"
  local command_type="$2"
  local job_json targets_json
  job_json="$(wait_job_terminal "$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  jq -e --arg command_type "$command_type" '
    .status == "completed" and .command_type == $command_type and .target_count == 1
  ' <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
}

wait_update_heartbeat_verified() {
  local activation_job_id="$1"
  local deadline=$((SECONDS + 35))
  until api_get "/api/v1/audit?limit=80" | jq -e \
    --arg client "$client_id" \
    --arg activation_job "$activation_job_id" \
    --arg sha "$artifact_sha" '
      any(.[]; .action == "agent_update.heartbeat_verified"
        and .target == ("client:" + $client)
        and .metadata.activation_job_id == $activation_job
        and .metadata.artifact_sha256_hex == $sha)
    ' >/dev/null; do
    if (( SECONDS >= deadline )); then
      api_get "/api/v1/audit?limit=80" >&2 || true
      smoke_dump_logs "agent update heartbeat was not recorded" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$SMOKE_TMPDIR"/agent-*.log
      exit 1
    fi
    sleep 0.5
  done
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

smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$agent_config" \
  "$client_id" \
  "$client_id" \
  "agent-update-smoke" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"
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
  echo "expected no-privilege-unlock agent-update to return 403, got $reject_status" >&2
  cat "$reject_json" >&2 || true
  exit 1
fi
jq -e '.error == "privilege_assertion_required" and .status == 403' "$reject_json" >/dev/null
if [[ -e "$staged_agent" || -e "$rollback_agent" ]]; then
  echo "agent update wrote local files after no-privilege-unlock rejection" >&2
  exit 1
fi

stage_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" agent-update \
    --artifact-url "$artifact_url" \
    --sha256-hex "$artifact_sha" \
    --artifact-signature-hex "$artifact_signature_hex" \
    --artifact-signing-key-hex "$artifact_signing_key_hex" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 30 \
    --force-unprivileged \
    --confirmed)"
stage_job_id="$(jq -r '.job_id' <<<"$stage_json")"
assert_single_target_completed "$stage_job_id" "agent_update"
status_output_for_job "$stage_job_id"
jq -e --arg sha "$artifact_sha" '
  .type == "agent_update"
  and .status == "staged"
  and .sha256_hex == $sha
  and .signature.status == "verified"
  and .activation == "manual_restart_required"
  and (.staged_path | endswith(".next"))
  and (.rollback_path | endswith(".rollback"))
' "$update_status_file" >/dev/null
reported_staged_agent="$(jq -r '.staged_path' "$update_status_file")"
reported_rollback_agent="$(jq -r '.rollback_path' "$update_status_file")"
rollback_sha="$(sha256sum "$reported_rollback_agent" | awk '{print $1}')"
cmp -s "$artifact_file" "$reported_staged_agent"
cmp -s "$agent_bin" "$reported_rollback_agent"
[[ "$(stat -c '%a' "$reported_staged_agent")" == "755" ]]

activate_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" agent-update-activate \
    --staged-sha256-hex "$artifact_sha" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 30 \
    --force-unprivileged \
    --confirmed)"
activate_job_id="$(jq -r '.job_id' <<<"$activate_json")"
assert_single_target_completed "$activate_job_id" "agent_update_activate"
status_output_for_job "$activate_job_id"
jq -e --arg sha "$artifact_sha" '
  .type == "agent_update_activation"
  and .status == "activated_pending_restart"
  and .sha256_hex == $sha
  and .restart == "manual_restart_required"
' "$update_status_file" >/dev/null
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
api_get "/api/v1/audit?limit=80" | jq -e \
  --arg activation_job "$activate_job_id" \
  --arg client "$client_id" \
  --arg sha "$artifact_sha" '
  any(.[]; .action == "agent_update.activation_completed"
    and .target == ("client:" + $client)
    and .metadata.activation_job_id == $activation_job
    and .metadata.artifact_sha256_hex == $sha)
' >/dev/null

stop_agent
sleep 1
start_agent "activated"
wait_agent_online
wait_update_heartbeat_verified "$activate_job_id"

rollback_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" agent-update-rollback \
    --rollback-sha256-hex "$rollback_sha" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 30 \
    --force-unprivileged \
    --confirmed)"
rollback_job_id="$(jq -r '.job_id' <<<"$rollback_json")"
assert_single_target_completed "$rollback_job_id" "agent_update_rollback"
status_output_for_job "$rollback_job_id"
jq -e --arg sha "$rollback_sha" '
  .type == "agent_update_rollback"
  and .status == "rolled_back_pending_restart"
  and .rollback_sha256_hex == $sha
' "$update_status_file" >/dev/null
cmp -s "$agent_bin" "$reported_rollback_agent"
if [[ -e "$activation_marker" ]]; then
  echo "agent update rollback did not remove restart heartbeat marker" >&2
  exit 1
fi
api_get "/api/v1/audit?limit=100" | jq -e \
  --arg rollback_job "$rollback_job_id" \
  --arg client "$client_id" \
  --arg sha "$rollback_sha" '
  any(.[]; .action == "agent_update.rollback_completed"
    and .target == ("client:" + $client)
    and .metadata.rollback_job_id == $rollback_job
    and .metadata.rollback_sha256_hex == $sha
    and .metadata.status == "rolled_back")
' >/dev/null

stop_api
start_api "restart"
api_get "/api/v1/auth/me" | jq -e '.username == "agent-update-smoke"' >/dev/null
api_get "/api/v1/jobs/$stage_job_id" | jq -e '.status == "completed"' >/dev/null
api_get "/api/v1/jobs/$activate_job_id" | jq -e '.status == "completed"' >/dev/null
api_get "/api/v1/jobs/$rollback_job_id" | jq -e '.status == "completed"' >/dev/null

if grep -F "agent update heartbeat timeout reconciliation failed" "$SMOKE_TMPDIR"/api-*.log >/dev/null 2>&1; then
  smoke_dump_logs "agent update heartbeat timeout reconciler failed during live smoke" \
    "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$SMOKE_TMPDIR"/agent-*.log "$https_log"
  exit 1
fi

jq -n \
  --arg client_id "$client_id" \
  --arg stage_job_id "$stage_job_id" \
  --arg activate_job_id "$activate_job_id" \
  --arg rollback_job_id "$rollback_job_id" \
  --arg sha256_hex "$artifact_sha" \
  '{
    live_agent_update_smoke: "ok",
    postgres_backed: true,
    auth_session: "persisted",
    api_restart: "verified",
    no_privilege_unlock_rejected: true,
    https_artifact: "trusted_private_root",
    signed_artifact: "verified_ed25519",
    direct_agent_update_job_flow: "stage_activate_restart_rollback",
    heartbeat: "verified_after_restart",
    rollback: "rolled_back_recorded",
    client_id: $client_id,
    stage_job_id: $stage_job_id,
    activate_job_id: $activate_job_id,
    rollback_job_id: $rollback_job_id,
    sha256_hex: $sha256_hex
  }'
