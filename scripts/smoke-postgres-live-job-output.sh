#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"
source "$ROOT_DIR/scripts/lib-smoke-process-supervisor.sh"

smoke_enter_root
smoke_require_tools awk base64 cmp curl docker grep jq ping ps python3 sha256sum shuf stat timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-postgres-live-job-output"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
speed_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
container_name="vpsman-postgres-live-job-output-$(date +%s%N)"
internal_token="postgres-live-job-internal-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
client_id="postgres-live-job-$(date +%s)"
peer_client_id="$client_id-peer"
super_password="postgres-live-job-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

api_pid=""
api_log=""
agent_pid=""
peer_agent_pid=""
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_log="$SMOKE_TMPDIR/agent.log"
peer_agent_log="$SMOKE_TMPDIR/agent-peer.log"
agent_config="$SMOKE_TMPDIR/agent.toml"
peer_agent_config="$SMOKE_TMPDIR/agent-peer.toml"
source_file="$SMOKE_TMPDIR/payload.txt"
destination_dir="$SMOKE_TMPDIR/agent-destination"
destination_file="$destination_dir/pushed.txt"
resumable_upload_file="$SMOKE_TMPDIR/resumable-upload.txt"
resumable_upload_destination="$destination_dir/resumable-uploaded.txt"
resumable_download_source="$destination_dir/resumable-download-source.txt"
resumable_download_file="$SMOKE_TMPDIR/resumable-downloaded.txt"
resumable_upload_events="$SMOKE_TMPDIR/resumable-upload-events.jsonl"
resumable_download_events="$SMOKE_TMPDIR/resumable-download-events.jsonl"
shell_marker="$destination_dir/shell-marker.txt"
shell_script_marker="$destination_dir/shell-script-marker.txt"
large_output_file="$SMOKE_TMPDIR/large-output-artifact.bin"
network_plan_file="$SMOKE_TMPDIR/network-status-plan.json"
agent_supervisor_dir="$SMOKE_TMPDIR/agent-supervisor"
object_store_dir="$SMOKE_TMPDIR/object-store"
mkdir -p "$destination_dir" "$object_store_dir"

payload="vpsman postgres live job output smoke payload $(date +%s%N)"
printf '%s\n' "$payload" >"$source_file"
payload_sha="$(sha256sum "$source_file" | awk '{print $1}')"
shell_payload="vpsman-shell-job-$(date +%s%N)"
pty_payload="vpsman-shell-pty-$(date +%s%N)"
shell_script_payload="vpsman-shell-script-$(date +%s%N)"
live_stream_start="vpsman-live-stream-start-$(date +%s%N)"
live_stream_end="vpsman-live-stream-end-$(date +%s%N)"
large_output_size=4096
large_output_script='i=0; while [ "$i" -lt 4096 ]; do printf A; i=$((i + 1)); done'
file_pull_payload="vpsman-file-pull-$(date +%s%N)"
terminal_payload="vpsman-terminal-live-$(date +%s%N)"
terminal_input_base64="$(printf '%s\n' "$terminal_payload" | base64 | tr -d '\n')"
resumable_upload_payload="vpsman-resumable-upload-$(date +%s%N)"
resumable_download_payload="vpsman-resumable-download-$(date +%s%N)"
printf '%s\n' "$resumable_upload_payload" >"$resumable_upload_file"
printf '%s\n' "$resumable_download_payload" >"$resumable_download_source"
resumable_upload_sha="$(sha256sum "$resumable_upload_file" | awk '{print $1}')"
resumable_download_sha="$(sha256sum "$resumable_download_source" | awk '{print $1}')"
resumable_upload_size="$(stat -c '%s' "$resumable_upload_file")"
resumable_download_size="$(stat -c '%s' "$resumable_download_source")"
supervisor_name="sup-$(date +%s)"
supervisor_payload="vpsman-supervisor-live-$(date +%s%N)"
supervisor_script="while true; do echo $supervisor_payload; sleep 1; done"

cleanup_postgres_live_job_smoke() {
  if [[ -n "${supervisor_payload:-}" ]]; then
    while IFS= read -r leaked_pid; do
      kill "$leaked_pid" >/dev/null 2>&1 || true
    done < <(ps -eo pid=,args= | awk -v needle="$supervisor_payload" 'index($0, needle) {print $1}')
  fi
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_postgres_live_job_smoke EXIT

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
    VPSMAN_BACKUP_OBJECT_STORE_DIR="$object_store_dir" \
    VPSMAN_JOB_OUTPUT_ARTIFACT_MIN_BYTES=2048 \
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
  smoke_dump_logs "postgres live job API failed to start" "$SMOKE_TMPDIR"/api-"$label"-*.log
  exit 1
}

stop_api() {
  if [[ -n "$api_pid" ]]; then
    kill "$api_pid" >/dev/null 2>&1 || true
    wait "$api_pid" >/dev/null 2>&1 || true
    api_pid=""
  fi
}

api_get() {
  local path="$1"
  curl -fsS -H "Authorization: Bearer $access_token" "$api_url$path"
}

wait_agent_online() {
  local expected_client_id="$1"
  local status=""
  local deadline=$((SECONDS + 35))
  until [[ "$status" == "online" ]]; do
    if (( SECONDS >= deadline )); then
      smoke_dump_logs "agent did not become online for postgres live job smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log" "$peer_agent_log"
      exit 1
    fi
    status="$(api_get "/api/v1/agents" \
      | jq -r --arg id "$expected_client_id" '.[] | select(.id == $id) | .status // empty')"
    sleep 0.25
  done
}

assert_gateway_session_active() {
  local expected_client="$1"
  local sessions_json
  sessions_json="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" gateway-sessions --limit 20)"
  jq -e \
    --arg client "$expected_client" \
    --arg gateway "postgres-live-job-gateway" '
      any(.[]; .client_id == $client and .gateway_id == $gateway and .status == "active")
    ' <<<"$sessions_json" >/dev/null
}

assert_gateway_sessions_active() {
  assert_gateway_session_active "$client_id"
  assert_gateway_session_active "$peer_client_id"
}

assert_gateway_session_ended() {
  local expected_client="$1"
  local sessions_json agents_json
  local deadline=$((SECONDS + 15))
  until sessions_json="$(api_get "/api/v1/gateway-sessions?limit=20")" \
    && jq -e --arg client "$expected_client" '
      any(.[]; .client_id == $client and .status == "ended" and (.end_reason | type == "string"))
    ' <<<"$sessions_json" >/dev/null; do
    if (( SECONDS >= deadline )); then
      smoke_dump_logs "gateway session did not end for postgres live job smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log" "$peer_agent_log"
      exit 1
    fi
    sleep 0.25
  done
  agents_json="$(api_get "/api/v1/agents")"
  jq -e --arg client "$expected_client" '
    any(.[]; .id == $client and .status == "offline")
  ' <<<"$agents_json" >/dev/null
}

assert_persisted_job_state() {
  local job_json targets_json outputs_json audits_json
  job_json="$(api_get "/api/v1/jobs/$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=20")"

  jq -e '.status == "completed" and .command_type == "file_push" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e --arg path "$destination_file" --arg sha "$payload_sha" '
    .[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "file_push" and .path == $path and .sha256_hex == $sha and .atomic == true
  ' <<<"$outputs_json" >/dev/null
  jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' \
    <<<"$audits_json" >/dev/null

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
}

assert_status_observation() {
  local observations_json
  observations_json="$(api_get "/api/v1/network/observations?limit=20")"
  jq -e --arg job_id "$network_status_job_id" --arg client "$client_id" --arg peer "$peer_client_id" '
    any(.[]; .job_id == $job_id
      and .client_id == $client
      and .kind == "network_status"
      and .plan_name == "postgres-live-status"
      and .interface_name == "pgstat0"
      and .peer_client_id == $peer
      and .healthy == false
      and .metadata.type == "network_status"
      and .metadata.applied == false)
  ' <<<"$observations_json" >/dev/null
}

assert_probe_observation() {
  local observations_json
  observations_json="$(api_get "/api/v1/network/observations?limit=20")"
  jq -e --arg job_id "$network_probe_job_id" --arg client "$client_id" --arg peer "$peer_client_id" '
    any(.[]; .job_id == $job_id
      and .client_id == $client
      and .kind == "network_probe"
      and .plan_name == "postgres-live-status"
      and .interface_name == "pgstat0"
      and .peer_client_id == $peer
      and .target == "127.0.0.3"
      and .healthy == true
      and .latency_avg_ms != null
      and .packet_loss_ratio == 0)
  ' <<<"$observations_json" >/dev/null
}

assert_speed_observations() {
  local observations_json
  observations_json="$(api_get "/api/v1/network/observations?limit=50")"
  jq -e --arg job_id "$network_speed_job_id" --arg left "$client_id" --arg right "$peer_client_id" --arg port "$speed_port" '
    ([
      .[] | select(.job_id == $job_id and .kind == "network_speed_test")
    ] | length == 2)
    and any(.[]; .job_id == $job_id
      and .client_id == $left
      and .peer_client_id == $right
      and .kind == "network_speed_test"
      and .role == "server"
      and .target == ("127.0.0.2:" + $port)
      and .healthy == true
      and .bytes > 0
      and .throughput_mbps > 0)
    and any(.[]; .job_id == $job_id
      and .client_id == $right
      and .peer_client_id == $left
      and .kind == "network_speed_test"
      and .role == "client"
      and .target == ("127.0.0.2:" + $port)
      and .healthy == true
      and .bytes > 0
      and .throughput_mbps > 0)
  ' <<<"$observations_json" >/dev/null
}

assert_network_trends() {
  local trends_json
  trends_json="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" network-trends --limit 50)"
  jq -e --arg client "$client_id" --arg peer "$peer_client_id" '
    any(.[]; .client_id == $client
      and .peer_client_id == $peer
      and .kind == "network_probe"
      and .plan_name == "postgres-live-status"
      and .interface_name == "pgstat0"
      and .sample_count >= 1
      and .healthy_count >= 1
      and .latency_avg_ms != null
      and .latency_min_ms != null
      and .latency_max_ms != null
      and .packet_loss_avg_ratio == 0)
    and any(.[]; .client_id == $peer
      and .peer_client_id == $client
      and .kind == "network_speed_test"
      and .plan_name == "postgres-live-status"
      and .interface_name == "pgstat0"
      and .sample_count >= 1
      and .healthy_count >= 1
      and .throughput_avg_mbps > 0
      and .throughput_max_mbps > 0
      and .bytes_total > 0)
  ' <<<"$trends_json" >/dev/null
}

assert_ospf_recommendations() {
  local recommendations_json
  recommendations_json="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" network-ospf-recommendations --limit 50)"
  jq -e --arg client "$client_id" --arg peer "$peer_client_id" '
    any(.[]; .plan_name == "postgres-live-status"
      and .interface_name == "pgstat0"
      and .left_client_id == $client
      and .right_client_id == $peer
      and .configured_bandwidth == "100m"
      and (.effective_bandwidth == "10m" or .effective_bandwidth == "100m")
      and .confidence == "measured"
      and .latency_avg_ms != null
      and .throughput_avg_mbps != null
      and .sample_count >= 2
      and .recommended_ospf_cost >= 5)
  ' <<<"$recommendations_json" >/dev/null
}

assert_ospf_update_plans() {
  local update_plans_json
  update_plans_json="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" network-ospf-update-plans --limit 50)"
  jq -e --arg client "$client_id" --arg peer "$peer_client_id" '
    any(.[]; .plan_name == "postgres-live-status"
      and .interface_name == "pgstat0"
      and .left_client_id == $client
      and .right_client_id == $peer
      and .mutation_mode == "reviewed_plan_only"
      and .privilege_required == (.cost_delta != 0)
      and .bird2_file == "/etc/bird/vpsman-ospf.conf"
      and (.status == "noop" or .status == "review_required" or .status == "review_degraded")
      and .evidence.sample_count >= 2
      and (.proposed_left_bird2_interface_snippet | contains("cost ")))
  ' <<<"$update_plans_json" >/dev/null
}

assert_shell_job_output() {
  local check_marker="${1:-1}"
  local job_json targets_json outputs_json audits_json decoded_stdout marker_text
  job_json="$(api_get "/api/v1/jobs/$shell_job_id")"
  targets_json="$(api_get "/api/v1/jobs/$shell_job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$shell_job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=30")"

  jq -e '.status == "completed" and .command_type == "shell_argv" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e '.[] | select(.stream == "status" and .done == true and .exit_code == 0)' \
    <<<"$outputs_json" >/dev/null
  decoded_stdout="$(jq -r '.[] | select(.stream == "stdout") | .data_base64' <<<"$outputs_json" | base64 -d)"
  [[ "$decoded_stdout" == "$shell_payload" ]]
  if [[ "$check_marker" == "1" ]]; then
    marker_text="$(cat "$shell_marker")"
    [[ "$marker_text" == "$shell_payload" ]]
  fi
  jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' \
    <<<"$audits_json" >/dev/null
}

assert_shell_pty_job_output() {
  local job_json targets_json outputs_json decoded_pty
  job_json="$(api_get "/api/v1/jobs/$shell_pty_job_id")"
  targets_json="$(api_get "/api/v1/jobs/$shell_pty_job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$shell_pty_job_id/outputs")"

  jq -e '.status == "completed" and .command_type == "shell_pty" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e '
    .[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "shell_pty" and .pty == true
  ' <<<"$outputs_json" >/dev/null
  decoded_pty="$(jq -r '.[] | select(.stream == "pty") | .data_base64' <<<"$outputs_json" | base64 -d)"
  [[ "$decoded_pty" == "$pty_payload" ]]
}

assert_shell_script_job_output() {
  local check_marker="${1:-1}"
  local job_json targets_json outputs_json audits_json decoded_stdout marker_text
  job_json="$(api_get "/api/v1/jobs/$shell_script_job_id")"
  targets_json="$(api_get "/api/v1/jobs/$shell_script_job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$shell_script_job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=30")"

  jq -e '.status == "completed" and .command_type == "shell_script" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e '
    .[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "shell_script" and .shell == "/bin/sh"
  ' <<<"$outputs_json" >/dev/null
  decoded_stdout="$(jq -r '.[] | select(.stream == "stdout") | .data_base64' <<<"$outputs_json" | base64 -d)"
  [[ "$decoded_stdout" == "$shell_script_payload" ]]
  if [[ "$check_marker" == "1" ]]; then
    marker_text="$(cat "$shell_script_marker")"
    [[ "$marker_text" == "$shell_script_payload" ]]
  fi
  jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' \
    <<<"$audits_json" >/dev/null
}

assert_job_follow_output() {
  local cli_follow json_follow vty_follow
  cli_follow="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" job-follow \
      --job-id "$shell_job_id" \
      --interval-ms 100 \
      --max-polls 3)"
  grep -F "[$client_id stdout" <<<"$cli_follow" | grep -F "$shell_payload" >/dev/null
  grep -F "status=completed" <<<"$cli_follow" >/dev/null

  json_follow="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" job-follow \
      --job-id "$shell_pty_job_id" \
      --interval-ms 100 \
      --max-polls 3 \
      --json)"
  jq -s -e --arg client "$client_id" --arg job "$shell_pty_job_id" --arg payload "$pty_payload" '
    any(.[]; .event == "job_output"
      and .client_id == $client
      and .stream == "pty"
      and (.data_base64 | @base64d) == $payload)
    and any(.[]; .event == "job_follow_complete"
      and .job_id == $job
      and .status == "completed"
      and .outputs >= 1)
  ' <<<"$json_follow" >/dev/null

  vty_follow="$(printf 'job-follow %s --interval-ms 100 --max-polls 3\nexit\n' "$shell_script_job_id" \
    | VPSMAN_API_TOKEN="$access_token" target/debug/vpsctl --api-url "$api_url" vty)"
  grep -F "[$client_id stdout" <<<"$vty_follow" | grep -F "$shell_script_payload" >/dev/null
  grep -F "status=completed" <<<"$vty_follow" >/dev/null
}

assert_live_streaming_job_output() {
  local job_json targets_json outputs_json decoded_stdout
  job_json="$(api_get "/api/v1/jobs/$live_stream_job_id")"
  targets_json="$(api_get "/api/v1/jobs/$live_stream_job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$live_stream_job_id/outputs")"

  jq -e '.status == "completed" and .command_type == "shell_script" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e '
    .[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "shell_script"
  ' <<<"$outputs_json" >/dev/null
  decoded_stdout="$(jq -r '.[] | select(.stream == "stdout") | .data_base64' <<<"$outputs_json" | base64 -d)"
  [[ "$decoded_stdout" == "$live_stream_start$live_stream_end" ]]
}

assert_large_output_artifact() {
  local outputs_json artifact_json seq object_key object_path expected_hash downloaded_hash object_hash size_bytes
  outputs_json="$(api_get "/api/v1/jobs/$large_output_job_id/outputs")"
  artifact_json="$(jq -c '
    .[] | select(.stream == "stdout" and .storage == "object_store")
    | select(.artifact_object_key != null and .artifact_sha256_hex != null and .artifact_size_bytes >= 4096)
  ' <<<"$outputs_json" | head -n 1)"
  if [[ -z "$artifact_json" ]]; then
    echo "large job output did not externalize stdout to object store" >&2
    jq . <<<"$outputs_json" >&2 || true
    exit 1
  fi
  seq="$(jq -r '.seq' <<<"$artifact_json")"
  object_key="$(jq -r '.artifact_object_key' <<<"$artifact_json")"
  expected_hash="$(jq -r '.artifact_sha256_hex' <<<"$artifact_json")"
  size_bytes="$(jq -r '.artifact_size_bytes' <<<"$artifact_json")"
  object_path="$object_store_dir/$object_key"

  [[ "$size_bytes" == "$large_output_size" ]]
  [[ "$expected_hash" =~ ^[0-9a-f]{64}$ ]]
  [[ -f "$object_path" ]]
  [[ "$(stat -c '%s' "$object_path")" == "$large_output_size" ]]
  object_hash="$(sha256sum "$object_path" | awk '{print $1}')"
  [[ "$object_hash" == "$expected_hash" ]]

  rm -f "$large_output_file"
  VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" job-output-artifact \
      --job-id "$large_output_job_id" \
      --client-id "$client_id" \
      --seq "$seq" \
      --output-file "$large_output_file" >/dev/null
  [[ "$(stat -c '%s' "$large_output_file")" == "$large_output_size" ]]
  downloaded_hash="$(sha256sum "$large_output_file" | awk '{print $1}')"
  [[ "$downloaded_hash" == "$expected_hash" ]]
}

assert_timed_out_shell_job() {
  local job_json targets_json outputs_json audits_json
  job_json="$(api_get "/api/v1/jobs/$timeout_job_id")"
  targets_json="$(api_get "/api/v1/jobs/$timeout_job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$timeout_job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=200")"

  jq -e '.status == "timed_out" and .command_type == "shell_argv" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "timed_out" and .[0].exit_code == 124
  ' <<<"$targets_json" >/dev/null
  jq -e '
    .[] | select(.stream == "status" and .done == true and .exit_code == 124)
    | (.data_base64 | @base64d | fromjson)
    | .type == "command_timeout" and .timeout_secs == 1
  ' <<<"$outputs_json" >/dev/null
  jq -e --arg job_id "$timeout_job_id" '
    any(.[]; .action == "job.target_result"
      and .metadata.job_id == $job_id
      and .metadata.status == "timed_out"
      and .metadata.exit_code == 124)
  ' <<<"$audits_json" >/dev/null
}

assert_file_pull_output() {
  local job_json targets_json outputs_json status_json pulled_bytes
  job_json="$(api_get "/api/v1/jobs/$file_pull_job_id")"
  targets_json="$(api_get "/api/v1/jobs/$file_pull_job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$file_pull_job_id/outputs")"

  jq -e '.status == "completed" and .command_type == "file_pull" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  pulled_bytes="$(jq -r '.[] | select(.stream == "stdout") | .data_base64' <<<"$outputs_json" | base64 -d)"
  [[ "$pulled_bytes" == "$file_pull_payload" ]]
  status_json="$(jq -r '.[] | select(.stream == "status" and .done == true and .exit_code == 0) | .data_base64' <<<"$outputs_json" | base64 -d)"
  jq -e --arg path "$shell_marker" --argjson size "${#file_pull_payload}" '
    .type == "file_pull" and .path == $path and .size_bytes == $size
  ' <<<"$status_json" >/dev/null
}

assert_user_sessions_no_privilege_unlock_rejection() {
  local reject_body reject_json reject_status
  reject_body="$(jq -n \
    --arg client "$client_id" \
    '{
      command: "user_sessions",
      argv: [],
      operation: {type: "user_sessions"},
      selector_expression: ("id:" + $client),
      target_client_ids: [$client],
      privileged: true,
      confirmed: true,
      timeout_secs: 10
    }')"
  reject_json="$SMOKE_TMPDIR/user-sessions-reject.json"
  reject_status="$(curl -sS -o "$reject_json" -w "%{http_code}" \
    -H "Authorization: Bearer $access_token" \
    -H 'content-type: application/json' \
    -d "$reject_body" \
    "$api_url/api/v1/jobs")"
  if [[ "$reject_status" != "403" ]]; then
    echo "expected no-privilege-unlock user-sessions to return 403, got $reject_status" >&2
    cat "$reject_json" >&2 || true
    exit 1
  fi
  jq -e '.error == "privilege_assertion_required" and .status == 403' \
    "$reject_json" >/dev/null
}

assert_user_sessions_output() {
  local job_json targets_json outputs_json audits_json decoded_outputs
  job_json="$(api_get "/api/v1/jobs/$user_sessions_job_id")"
  targets_json="$(api_get "/api/v1/jobs/$user_sessions_job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$user_sessions_job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=30")"

  jq -e '.status == "completed" and .command_type == "user_sessions" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e '
    .[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "user_sessions" and (.source == "/usr/bin/w" or .source == "/usr/bin/who")
  ' <<<"$outputs_json" >/dev/null
  jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' \
    <<<"$audits_json" >/dev/null

  decoded_outputs="$(
    jq -r '.[].data_base64' <<<"$outputs_json" | while IFS= read -r item; do
      printf '%s' "$item" | base64 -d
      printf '\n'
    done
  )"
  if grep -Fq "$super_password" <<<"$decoded_outputs"; then
    echo "user-sessions outputs leaked super password" >&2
    exit 1
  fi
}

assert_terminal_job_completed() {
  local current_job_id="$1"
  local expected_command="$2"
  local job_json targets_json
  job_json="$(api_get "/api/v1/jobs/$current_job_id")"
  targets_json="$(api_get "/api/v1/jobs/$current_job_id/targets")"

  jq -e --arg command "$expected_command" '
    .status == "completed" and .command_type == $command and .target_count == 1
  ' <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
}

assert_terminal_session_workflow() {
  local open_outputs input_outputs attach_outputs resize_outputs poll_outputs close_outputs sessions_json vty_sessions decoded_pty

  assert_terminal_job_completed "$terminal_open_job_id" "terminal_open"
  assert_terminal_job_completed "$terminal_resize_job_id" "terminal_resize"
  assert_terminal_job_completed "$terminal_input_job_id" "terminal_input"
  assert_terminal_job_completed "$terminal_attach_job_id" "terminal_open"
  assert_terminal_job_completed "$terminal_poll_job_id" "terminal_poll"
  assert_terminal_job_completed "$terminal_close_job_id" "terminal_close"

  open_outputs="$(api_get "/api/v1/jobs/$terminal_open_job_id/outputs")"
  input_outputs="$(api_get "/api/v1/jobs/$terminal_input_job_id/outputs")"
  attach_outputs="$(api_get "/api/v1/jobs/$terminal_attach_job_id/outputs")"
  resize_outputs="$(api_get "/api/v1/jobs/$terminal_resize_job_id/outputs")"
  poll_outputs="$(api_get "/api/v1/jobs/$terminal_poll_job_id/outputs")"
  close_outputs="$(api_get "/api/v1/jobs/$terminal_close_job_id/outputs")"

  jq -e --arg sid "$terminal_session_id" '
    any(.[]; .stream == "status" and .done == true and .exit_code == 0 and (
      (.data_base64 | @base64d | fromjson)
      | .type == "terminal_open"
        and .status == "opened"
        and .session_id == $sid
        and .argv == ["/bin/sh", "-lc", "read line; printf '\''got:%s\\n'\'' \"$line\""]
        and .cols == 96
        and .rows == 28
        and .idle_timeout_secs == 300
        and .flow_window_bytes == 32768
        and .output_retained_bytes >= 0
        and .output_dropped_bytes == 0
        and .output_replay_truncated == false
    ))
  ' <<<"$open_outputs" >/dev/null
  jq -e --arg sid "$terminal_session_id" '
    any(.[]; .stream == "status" and .done == true and .exit_code == 0 and (
      (.data_base64 | @base64d | fromjson)
      | .type == "terminal_resize"
        and .status == "resized"
        and .session_id == $sid
        and .cols == 100
        and .rows == 30
    ))
  ' <<<"$resize_outputs" >/dev/null
  jq -e --arg sid "$terminal_session_id" '
    any(.[]; .stream == "status" and .done == true and .exit_code == 0 and (
      (.data_base64 | @base64d | fromjson)
      | .type == "terminal_input"
        and .status == "accepted"
        and .session_id == $sid
        and .input_seq == 1
        and .written_bytes > 0
        and .output_next_seq >= .output_first_seq
        and .output_retained_bytes > 0
        and .output_dropped_bytes == 0
        and .output_replay_truncated == false
    ))
  ' <<<"$input_outputs" >/dev/null
  decoded_pty="$(jq -r '.[] | select(.stream == "pty") | .data_base64' <<<"$input_outputs" | base64 -d)"
  [[ "$decoded_pty" == *"got:$terminal_payload"* ]]
  decoded_pty="$(jq -r '.[] | select(.stream == "pty") | .data_base64' <<<"$attach_outputs" | base64 -d)"
  [[ "$decoded_pty" == *"got:$terminal_payload"* ]]
  jq -e --arg sid "$terminal_session_id" '
    any(.[]; .stream == "status" and .done == true and .exit_code == 0 and (
      (.data_base64 | @base64d | fromjson)
      | .type == "terminal_open"
        and .status == "attached"
        and .session_id == $sid
        and .output_first_seq == 1
        and .output_retained_bytes > 0
        and .output_dropped_bytes == 0
        and .output_replay_truncated == false
    ))
  ' <<<"$attach_outputs" >/dev/null
  decoded_pty="$(jq -r '.[] | select(.stream == "pty") | .data_base64' <<<"$poll_outputs" | base64 -d)"
  [[ "$decoded_pty" == *"got:$terminal_payload"* ]]
  jq -e --arg sid "$terminal_session_id" '
    any(.[]; .stream == "status" and .done == true and .exit_code == 0 and (
      (.data_base64 | @base64d | fromjson)
      | .type == "terminal_poll"
        and .status == "polled"
        and .session_id == $sid
        and .replay_from_seq == 1
        and .output_first_seq == 1
        and .output_retained_bytes > 0
        and .output_dropped_bytes == 0
        and .output_replay_truncated == false
    ))
  ' <<<"$poll_outputs" >/dev/null
  jq -e --arg sid "$terminal_session_id" '
    any(.[]; .stream == "status" and .done == true and .exit_code == 0 and (
      (.data_base64 | @base64d | fromjson)
      | .type == "terminal_close"
        and .status == "closed"
        and .session_id == $sid
        and .reason == "live-terminal-smoke"
    ))
  ' <<<"$close_outputs" >/dev/null

  sessions_json="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" terminal-sessions \
      --client-id "$client_id" \
      --session-id "$terminal_session_id" \
      --limit 5)"
  jq -e --arg client "$client_id" --arg sid "$terminal_session_id" '
    length == 1
    and .[0].client_id == $client
    and .[0].session_id == $sid
    and .[0].state == "closed"
    and .[0].last_status == "closed"
    and .[0].argv == ["/bin/sh", "-lc", "read line; printf '\''got:%s\\n'\'' \"$line\""]
    and .[0].cols == 100
    and .[0].rows == 30
    and .[0].idle_timeout_secs == 300
    and .[0].flow_window_bytes == 32768
    and .[0].last_input_seq == 1
    and .[0].output_retained_bytes > 0
    and .[0].output_dropped_bytes == 0
    and .[0].output_replay_truncated == false
    and .[0].close_reason == "live-terminal-smoke"
    and .[0].last_event == "terminal_close"
    and .[0].last_command_type == "terminal_close"
  ' <<<"$sessions_json" >/dev/null

  vty_sessions="$(printf 'terminal-sessions --client-id %s --session-id %s\nexit\n' \
    "$client_id" "$terminal_session_id" \
    | VPSMAN_API_TOKEN="$access_token" target/debug/vpsctl --api-url "$api_url" vty)"
  grep -F "$terminal_session_id" <<<"$vty_sessions" >/dev/null
  grep -F '"state":"closed"' <<<"$vty_sessions" >/dev/null
}

assert_resumable_event_streams() {
  jq -s -e \
    --arg sid "$resumable_upload_session_id" \
    --arg client "$client_id" \
    --arg path "$resumable_upload_destination" \
    --arg sha "$resumable_upload_sha" \
    --argjson size "$resumable_upload_size" '
      any(.[]; .event == "file_transfer_upload_ready"
        and .session_id == $sid
        and .path == $path
        and .size_bytes == $size
        and .sha256_hex == $sha
        and .chunk_size_bytes == 16
        and .rate_limit_kbps == 0
        and .multi_target_policy == "same-offset"
        and .resume_token_generated == true
        and (.resume_token | type == "string"))
      and any(.[]; .event == "file_transfer_upload_started"
        and .session_id == $sid
        and .next_offset == 0
        and .multi_target_policy == "same-offset"
        and .target_offsets[$client] == 0
        and .accepted_targets == 1)
      and ([.[] | select(.event == "file_transfer_upload_chunk" and .session_id == $sid and .multi_target_policy == "same-offset" and (.target_offsets[$client] | type == "number"))] | length >= 2)
      and any(.[]; .event == "file_transfer_upload_complete"
        and .session_id == $sid
        and .path == $path
        and .size_bytes == $size
        and .sha256_hex == $sha
        and .multi_target_policy == "same-offset"
        and .target_offsets[$client] == $size
        and .accepted_targets == 1)
    ' "$resumable_upload_events" >/dev/null

  jq -s -e \
    --arg sid "$resumable_download_session_id" \
    --arg path "$resumable_download_source" \
    --arg destination "$resumable_download_file" \
    --arg sha "$resumable_download_sha" \
    --argjson size "$resumable_download_size" '
      any(.[]; .event == "file_transfer_download_ready"
        and .session_id == $sid
        and .path == $path
        and .destination == $destination
        and .chunk_size_bytes == 16
        and .rate_limit_kbps == 0
        and .multi_target_policy == "single-target"
        and .resume_token_generated == true
        and (.resume_token | type == "string"))
      and any(.[]; .event == "file_transfer_download_started"
        and .session_id == $sid
        and .size_bytes == $size
        and .sha256_hex == $sha
        and .next_offset == 0
        and .multi_target_policy == "single-target")
      and ([.[] | select(.event == "file_transfer_download_chunk" and .session_id == $sid and .multi_target_policy == "single-target")] | length >= 2)
      and any(.[]; .event == "file_transfer_download_complete"
        and .session_id == $sid
        and .path == $path
        and .destination == $destination
        and .size_bytes == $size
        and .sha256_hex == $sha
        and .multi_target_policy == "single-target")
    ' "$resumable_download_events" >/dev/null
}

assert_resumable_transfer_inventory() {
  local upload_sessions download_sessions vty_transfers

  cmp -s "$resumable_upload_file" "$resumable_upload_destination"
  cmp -s "$resumable_download_source" "$resumable_download_file"
  [[ "$(sha256sum "$resumable_upload_destination" | awk '{print $1}')" == "$resumable_upload_sha" ]]
  [[ "$(sha256sum "$resumable_download_file" | awk '{print $1}')" == "$resumable_download_sha" ]]
  if grep -Fq "$super_password" "$resumable_upload_events" "$resumable_download_events"; then
    echo "resumable transfer events leaked super password" >&2
    exit 1
  fi

  upload_sessions="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" file-transfers \
      --client-id "$client_id" \
      --session-id "$resumable_upload_session_id" \
      --limit 5)"
  jq -e \
    --arg client "$client_id" \
    --arg sid "$resumable_upload_session_id" \
    --arg path "$resumable_upload_destination" \
    --arg sha "$resumable_upload_sha" \
    --argjson size "$resumable_upload_size" '
      length == 1
      and .[0].client_id == $client
      and .[0].session_id == $sid
      and .[0].direction == "upload"
      and .[0].status == "completed"
      and .[0].path == $path
      and .[0].size_bytes == $size
      and .[0].progress_bytes == $size
      and .[0].progress_ratio == 1
      and .[0].sha256_hex == $sha
      and .[0].chunk_size_bytes == 16
      and .[0].last_chunk_size_bytes > 0
      and .[0].rate_limit_kbps == 0
      and .[0].resumed == false
      and .[0].last_event == "file_transfer_commit"
      and .[0].last_command_type == "file_transfer_commit"
    ' <<<"$upload_sessions" >/dev/null

  download_sessions="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" file-transfers \
      --client-id "$client_id" \
      --session-id "$resumable_download_session_id" \
      --limit 5)"
  jq -e \
    --arg client "$client_id" \
    --arg sid "$resumable_download_session_id" \
    --arg path "$resumable_download_source" \
    --arg sha "$resumable_download_sha" \
    --argjson size "$resumable_download_size" '
      length == 1
      and .[0].client_id == $client
      and .[0].session_id == $sid
      and .[0].direction == "download"
      and .[0].status == "completed"
      and .[0].path == $path
      and .[0].size_bytes == $size
      and .[0].progress_bytes == $size
      and .[0].progress_ratio == 1
      and .[0].sha256_hex == $sha
      and .[0].chunk_size_bytes == 16
      and .[0].last_chunk_size_bytes > 0
      and .[0].rate_limit_kbps == 0
      and .[0].resumed == false
      and .[0].last_event == "file_transfer_download_chunk"
      and .[0].last_command_type == "file_transfer_download_chunk"
    ' <<<"$download_sessions" >/dev/null

  vty_transfers="$(printf 'file-transfers --client-id %s --session-id %s\nfile-transfers --client-id %s --session-id %s\nexit\n' \
    "$client_id" "$resumable_upload_session_id" "$client_id" "$resumable_download_session_id" \
    | VPSMAN_API_TOKEN="$access_token" target/debug/vpsctl --api-url "$api_url" vty)"
  grep -F "$resumable_upload_session_id" <<<"$vty_transfers" >/dev/null
  grep -F "$resumable_download_session_id" <<<"$vty_transfers" >/dev/null
  grep -F '"status":"completed"' <<<"$vty_transfers" >/dev/null
}

assert_resumable_transfer_workflow() {
  assert_resumable_event_streams
  assert_resumable_transfer_inventory
}

source "$ROOT_DIR/scripts/lib-smoke-postgres-live-job-workflow.sh"
