start_api "first"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d '{"username":"postgres-live-job","password":"postgres-live-job-password"}' \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
export VPSMAN_API_TOKEN="$access_token"
jq -e '.operator.username == "postgres-live-job" and .token_type == "Bearer"' \
  <<<"$auth_json" >/dev/null

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="postgres-live-job-gateway" \
VPSMAN_GATEWAY_SPOOL_DIR="$SMOKE_TMPDIR/gateway-spool" \
VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS="${VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS:-86400}" \
RUST_LOG="vpsman_gateway=warn" \
  target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_tcp 127.0.0.1 "$gateway_port"
smoke_wait_tcp 127.0.0.1 "$gateway_control_port"

client_id="postgres-live-job-a"
peer_client_id="postgres-live-job-b"
smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$agent_config" \
  "$client_id" \
  "$client_id" \
  "postgres-live-job" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"
smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$peer_agent_config" \
  "$peer_client_id" \
  "$peer_client_id" \
  "postgres-live-job" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"

VPSMAN_AGENT_CONFIG="$agent_config" \
VPSMAN_SUPERVISOR_DIR="$agent_supervisor_dir" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
agent_pid="$!"
smoke_track_pid "$agent_pid"
VPSMAN_AGENT_CONFIG="$peer_agent_config" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$peer_agent_log" 2>&1 &
peer_agent_pid="$!"
smoke_track_pid "$peer_agent_pid"
wait_agent_online "$client_id"
wait_agent_online "$peer_client_id"
assert_gateway_sessions_active

target/debug/vpsctl --api-url "$api_url" tunnel-plan \
  --name postgres-live-status \
  --interface-name pgstat0 \
  --kind gre \
  --left-client-id "$client_id" \
  --right-client-id "$peer_client_id" \
  --left-underlay 127.0.0.1 \
  --right-underlay 127.0.0.1 \
  --address-pool-cidr 127.0.0.0/29 \
  --reserved-addresses 127.0.0.0,127.0.0.1 \
  --left-tunnel-ipv4 127.0.0.2 \
  --right-tunnel-ipv4 127.0.0.3 \
  --bandwidth 100m \
  --latency-ms 5 >"$network_plan_file"
VPSMAN_API_TOKEN="$access_token" \
target/debug/vpsctl --api-url "$api_url" tunnel-plan \
  --name postgres-live-status \
  --interface-name pgstat0 \
  --kind gre \
  --left-client-id "$client_id" \
  --right-client-id "$peer_client_id" \
  --left-underlay 127.0.0.1 \
  --right-underlay 127.0.0.1 \
  --address-pool-cidr 127.0.0.0/29 \
  --reserved-addresses 127.0.0.0,127.0.0.1 \
  --left-tunnel-ipv4 127.0.0.2 \
  --right-tunnel-ipv4 127.0.0.3 \
  --bandwidth 100m \
  --latency-ms 5 \
  --save >/dev/null

shell_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" job-create \
    --command /bin/sh \
    --argv "/bin/sh,-c,printf '%s' '$shell_payload' | tee '$shell_marker'" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
shell_job_id="$(jq -r '.job_id' <<<"$shell_json")"
smoke_assert_job_create_queued "$shell_json" 1
smoke_wait_api_job_status "$api_url" "$shell_job_id" completed 45 >/dev/null
assert_shell_job_output

shell_pty_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" job-create \
    --command /bin/sh \
    --argv "/bin/sh,-lc,test -t 1 && printf '%s' '$pty_payload'" \
    --pty \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
shell_pty_job_id="$(jq -r '.job_id' <<<"$shell_pty_json")"
smoke_assert_job_create_queued "$shell_pty_json" 1
smoke_wait_api_job_status "$api_url" "$shell_pty_job_id" completed 45 >/dev/null
assert_shell_pty_job_output

shell_script_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" job-shell \
    --script "printf '%s' '$shell_script_payload' | tee '$shell_script_marker'" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
shell_script_job_id="$(jq -r '.job_id' <<<"$shell_script_json")"
smoke_assert_job_create_queued "$shell_script_json" 1
smoke_wait_api_job_status "$api_url" "$shell_script_job_id" completed 45 >/dev/null
assert_shell_script_job_output
assert_job_follow_output

live_stream_output="$SMOKE_TMPDIR/live-stream-job.json"
VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" job-shell \
    --script "printf '%s' '$live_stream_start'; sleep 5; printf '%s' '$live_stream_end'" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed >"$live_stream_output" &
live_stream_pid="$!"
live_stream_job_id=""
deadline=$((SECONDS + 15))
until [[ -n "$live_stream_job_id" ]]; do
  if (( SECONDS >= deadline )); then
    smoke_dump_logs "live streaming job creation response was not written" \
      "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log"
    exit 1
  fi
  if [[ -s "$live_stream_output" ]]; then
    live_stream_job_id="$(jq -r '.job_id // empty' "$live_stream_output" 2>/dev/null || true)"
  fi
  sleep 0.2
done
deadline=$((SECONDS + 4))
until outputs_json="$(api_get "/api/v1/jobs/$live_stream_job_id/outputs")" \
  && jq -e --arg start "$live_stream_start" '
    ([.[] | select(.stream == "stdout") | .data_base64 | @base64d] | join(""))
    | contains($start)
  ' <<<"$outputs_json" >/dev/null; do
  if (( SECONDS >= deadline )); then
    smoke_dump_logs "live streaming stdout was not retained before job completion" \
      "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log"
    exit 1
  fi
  sleep 0.2
done
api_get "/api/v1/jobs/$live_stream_job_id" | jq -e '.status == "running"' >/dev/null
wait "$live_stream_pid"
jq -e --arg job_id "$live_stream_job_id" '
  .job_id == $job_id and .target_count == 1
' "$live_stream_output" >/dev/null
smoke_wait_api_job_status "$api_url" "$live_stream_job_id" completed 45 >/dev/null
assert_live_streaming_job_output

large_output_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" job-shell \
    --script "$large_output_script" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
large_output_job_id="$(jq -r '.job_id' <<<"$large_output_json")"
smoke_assert_job_create_queued "$large_output_json" 1
smoke_wait_api_job_status "$api_url" "$large_output_job_id" completed 45 >/dev/null
assert_large_output_artifact

timeout_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" job-create \
    --command /bin/sh \
    --argv "/bin/sh,-c,sleep 2" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 1 \
    --confirmed)"
timeout_job_id="$(jq -r '.job_id' <<<"$timeout_json")"
smoke_assert_job_create_queued "$timeout_json" 1
smoke_wait_api_job_status "$api_url" "$timeout_job_id" agent_timeout 45 >/dev/null
assert_agent_timeout_shell_job

printf '%s' "$file_pull_payload" >"$shell_marker"
file_pull_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" file-pull \
    --path "$shell_marker" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
file_pull_job_id="$(jq -r '.job_id' <<<"$file_pull_json")"
smoke_assert_job_create_queued "$file_pull_json" 1
smoke_wait_api_job_status "$api_url" "$file_pull_job_id" completed 45 >/dev/null
assert_file_pull_output

terminal_open_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" terminal-open \
    --argv "/bin/sh,-lc,read line; printf 'got:%s\\n' \"\$line\"" \
    --cols 96 \
    --rows 28 \
    --idle-timeout-secs 300 \
    --flow-window-bytes 32768 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
terminal_session_id="$(jq -r '.terminal_session_id' <<<"$terminal_open_json")"
terminal_open_job_id="$(jq -r '.job.job_id' <<<"$terminal_open_json")"
jq -e '
  (.terminal_session_id | length == 36)
  and .job.target_count == 1
' <<<"$terminal_open_json" >/dev/null
smoke_wait_api_job_status "$api_url" "$terminal_open_job_id" completed 45 >/dev/null

terminal_resize_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" terminal-resize \
    --session-id "$terminal_session_id" \
    --cols 100 \
    --rows 30 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
terminal_resize_job_id="$(jq -r '.job_id' <<<"$terminal_resize_json")"
smoke_assert_job_create_queued "$terminal_resize_json" 1
smoke_wait_api_job_status "$api_url" "$terminal_resize_job_id" completed 45 >/dev/null

terminal_input_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" terminal-input \
    --session-id "$terminal_session_id" \
    --input-seq 1 \
    --data-base64 "$terminal_input_base64" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
terminal_input_job_id="$(jq -r '.job_id' <<<"$terminal_input_json")"
smoke_assert_job_create_queued "$terminal_input_json" 1
smoke_wait_api_job_status "$api_url" "$terminal_input_job_id" completed 45 >/dev/null

terminal_attach_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" terminal-open \
    --session-id "$terminal_session_id" \
    --argv "/bin/sh,-lc,read line; printf 'got:%s\\n' \"\$line\"" \
    --replay-from-seq 1 \
    --cols 96 \
    --rows 28 \
    --idle-timeout-secs 300 \
    --flow-window-bytes 32768 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
terminal_attach_job_id="$(jq -r '.job.job_id' <<<"$terminal_attach_json")"
jq -e '
  .terminal_session_id == "'"$terminal_session_id"'"
  and .job.target_count == 1
' <<<"$terminal_attach_json" >/dev/null
smoke_wait_api_job_status "$api_url" "$terminal_attach_job_id" completed 45 >/dev/null

terminal_poll_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" terminal-poll \
    --session-id "$terminal_session_id" \
    --replay-from-seq 1 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
terminal_poll_job_id="$(jq -r '.job_id' <<<"$terminal_poll_json")"
smoke_assert_job_create_queued "$terminal_poll_json" 1
smoke_wait_api_job_status "$api_url" "$terminal_poll_job_id" completed 45 >/dev/null

terminal_close_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" terminal-close \
    --session-id "$terminal_session_id" \
    --reason live-terminal-smoke \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
terminal_close_job_id="$(jq -r '.job_id' <<<"$terminal_close_json")"
smoke_assert_job_create_queued "$terminal_close_json" 1
smoke_wait_api_job_status "$api_url" "$terminal_close_job_id" completed 45 >/dev/null
assert_terminal_session_workflow

VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" file-transfer-upload \
    --source "$resumable_upload_file" \
    --path "$resumable_upload_destination" \
    --mode 0600 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --chunk-size-bytes 16 \
    --poll-interval-ms 100 \
    --max-polls 120 \
    --timeout-secs 10 \
    --confirmed >"$resumable_upload_events"
resumable_upload_session_id="$(
  jq -r 'select(.event == "file_transfer_upload_ready") | .session_id' \
    "$resumable_upload_events" | head -n 1
)"
[[ "$resumable_upload_session_id" =~ ^[0-9a-fA-F-]{36}$ ]]

VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" file-transfer-download \
    --path "$resumable_download_source" \
    --destination "$resumable_download_file" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --chunk-size-bytes 16 \
    --poll-interval-ms 100 \
    --max-polls 120 \
    --timeout-secs 10 \
    --confirmed >"$resumable_download_events"
resumable_download_session_id="$(
  jq -r 'select(.event == "file_transfer_download_ready") | .session_id' \
    "$resumable_download_events" | head -n 1
)"
[[ "$resumable_download_session_id" =~ ^[0-9a-fA-F-]{36}$ ]]
assert_resumable_transfer_workflow

assert_user_sessions_no_privilege_unlock_rejection
user_sessions_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" user-sessions \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
user_sessions_job_id="$(jq -r '.job_id' <<<"$user_sessions_json")"
smoke_assert_job_create_queued "$user_sessions_json" 1
smoke_wait_api_job_status "$api_url" "$user_sessions_job_id" completed 45 >/dev/null
assert_user_sessions_output

process_start_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" process-start \
    --name "$supervisor_name" \
    --argv /bin/sh \
    --argv=-c \
    --argv="$supervisor_script" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
process_start_job_id="$(jq -r '.job_id' <<<"$process_start_json")"
smoke_assert_job_create_queued "$process_start_json" 1
smoke_wait_api_job_status "$api_url" "$process_start_job_id" completed 45 >/dev/null
assert_supervisor_status_job "$process_start_job_id" "process_start" "process_start" "running"
wait_for_supervisor_log

process_status_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" process-status \
    --name "$supervisor_name" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
process_status_job_id="$(jq -r '.job_id' <<<"$process_status_json")"
smoke_assert_job_create_queued "$process_status_json" 1
smoke_wait_api_job_status "$api_url" "$process_status_job_id" completed 45 >/dev/null
assert_supervisor_snapshot_job "$process_status_job_id"

process_logs_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" process-logs \
    --name "$supervisor_name" \
    --max-bytes 4096 \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
process_logs_job_id="$(jq -r '.job_id' <<<"$process_logs_json")"
smoke_assert_job_create_queued "$process_logs_json" 1
smoke_wait_api_job_status "$api_url" "$process_logs_job_id" completed 45 >/dev/null
assert_supervisor_logs_job "$process_logs_job_id"

process_restart_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" process-restart \
    --name "$supervisor_name" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
process_restart_job_id="$(jq -r '.job_id' <<<"$process_restart_json")"
smoke_assert_job_create_queued "$process_restart_json" 1
smoke_wait_api_job_status "$api_url" "$process_restart_job_id" completed 45 >/dev/null
assert_supervisor_status_job "$process_restart_job_id" "process_restart" "process_restart" "running"
wait_for_supervisor_log

process_stop_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" process-stop \
    --name "$supervisor_name" \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --confirmed)"
process_stop_job_id="$(jq -r '.job_id' <<<"$process_stop_json")"
smoke_assert_job_create_queued "$process_stop_json" 1
smoke_wait_api_job_status "$api_url" "$process_stop_job_id" completed 45 >/dev/null
assert_supervisor_status_job "$process_stop_job_id" "process_stop" "process_stop" "stopped_or_requested"
assert_supervisor_inventory "$process_stop_job_id" "process_stop" "stopped_or_requested"
assert_no_supervisor_process_leaked

push_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
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

assert_persisted_job_state

network_status_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-status \
    --plan-file "$network_plan_file" \
    --side left \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10)"
network_status_job_id="$(jq -r '.job_id' <<<"$network_status_json")"
smoke_assert_job_create_queued "$network_status_json" 1
smoke_wait_api_job_status "$api_url" "$network_status_job_id" completed 45 >/dev/null
assert_status_observation

network_probe_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-probe \
    --plan-file "$network_plan_file" \
    --side left \
    --count 2 \
    --interval-ms 200 \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10)"
network_probe_job_id="$(jq -r '.job_id' <<<"$network_probe_json")"
smoke_assert_job_create_queued "$network_probe_json" 1
smoke_wait_api_job_status "$api_url" "$network_probe_job_id" completed 45 >/dev/null
assert_probe_observation

network_speed_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-speed-test \
    --plan-file "$network_plan_file" \
    --server-side left \
    --duration-secs 1 \
    --max-bytes 16384 \
    --rate-limit-kbps 512 \
    --port "$speed_port" \
    --connect-timeout-ms 3000 \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10)"
network_speed_job_id="$(jq -r '.job_id' <<<"$network_speed_json")"
smoke_assert_job_create_queued "$network_speed_json" 2
smoke_wait_api_job_status "$api_url" "$network_speed_job_id" completed 45 >/dev/null
assert_speed_observations
assert_network_trends
assert_ospf_recommendations
assert_ospf_update_plans
kill "$peer_agent_pid" >/dev/null 2>&1 || true
wait "$peer_agent_pid" >/dev/null 2>&1 || true
assert_gateway_session_ended "$peer_client_id"

stop_api
start_api "restart"

api_get "/api/v1/auth/me" | jq -e '.username == "postgres-live-job"' >/dev/null
assert_gateway_session_active "$client_id"
assert_gateway_session_ended "$peer_client_id"
assert_shell_job_output 0
assert_shell_pty_job_output
assert_shell_script_job_output 0
assert_job_follow_output
assert_live_streaming_job_output
assert_large_output_artifact
assert_agent_timeout_shell_job
assert_file_pull_output
assert_terminal_session_workflow
assert_resumable_transfer_workflow
assert_user_sessions_output
assert_supervisor_status_job "$process_start_job_id" "process_start" "process_start" "running"
assert_supervisor_snapshot_job "$process_status_job_id"
assert_supervisor_logs_job "$process_logs_job_id"
assert_supervisor_status_job "$process_restart_job_id" "process_restart" "process_restart" "running"
assert_supervisor_status_job "$process_stop_job_id" "process_stop" "process_stop" "stopped_or_requested"
assert_supervisor_inventory "$process_stop_job_id" "process_stop" "stopped_or_requested"
assert_persisted_job_state
assert_status_observation
assert_probe_observation
assert_speed_observations
assert_network_trends
assert_ospf_recommendations
assert_ospf_update_plans

jq -n \
  --arg api_url "$api_url" \
  --arg client_id "$client_id" \
  --arg peer_client_id "$peer_client_id" \
  --arg shell_job_id "$shell_job_id" \
  --arg shell_pty_job_id "$shell_pty_job_id" \
  --arg shell_script_job_id "$shell_script_job_id" \
  --arg live_stream_job_id "$live_stream_job_id" \
  --arg large_output_job_id "$large_output_job_id" \
  --arg timeout_job_id "$timeout_job_id" \
  --arg file_pull_job_id "$file_pull_job_id" \
  --arg terminal_session_id "$terminal_session_id" \
  --arg terminal_open_job_id "$terminal_open_job_id" \
  --arg terminal_resize_job_id "$terminal_resize_job_id" \
  --arg terminal_input_job_id "$terminal_input_job_id" \
  --arg terminal_attach_job_id "$terminal_attach_job_id" \
  --arg terminal_poll_job_id "$terminal_poll_job_id" \
  --arg terminal_close_job_id "$terminal_close_job_id" \
  --arg resumable_upload_session_id "$resumable_upload_session_id" \
  --arg resumable_download_session_id "$resumable_download_session_id" \
  --arg user_sessions_job_id "$user_sessions_job_id" \
  --arg process_start_job_id "$process_start_job_id" \
  --arg process_status_job_id "$process_status_job_id" \
  --arg process_logs_job_id "$process_logs_job_id" \
  --arg process_restart_job_id "$process_restart_job_id" \
  --arg process_stop_job_id "$process_stop_job_id" \
  --arg job_id "$job_id" \
  --arg network_status_job_id "$network_status_job_id" \
  --arg network_probe_job_id "$network_probe_job_id" \
  --arg network_speed_job_id "$network_speed_job_id" \
  --arg destination "$destination_file" \
  --arg sha256_hex "$payload_sha" \
  '{
    postgres_live_job_output_smoke: "ok",
    api_url: $api_url,
    client_id: $client_id,
    peer_client_id: $peer_client_id,
    shell_job_id: $shell_job_id,
    shell_pty_job_id: $shell_pty_job_id,
    shell_script_job_id: $shell_script_job_id,
    live_stream_job_id: $live_stream_job_id,
    large_output_job_id: $large_output_job_id,
    timeout_job_id: $timeout_job_id,
    file_pull_job_id: $file_pull_job_id,
    terminal_session_id: $terminal_session_id,
    terminal_open_job_id: $terminal_open_job_id,
    terminal_resize_job_id: $terminal_resize_job_id,
    terminal_input_job_id: $terminal_input_job_id,
    terminal_attach_job_id: $terminal_attach_job_id,
    terminal_poll_job_id: $terminal_poll_job_id,
    terminal_close_job_id: $terminal_close_job_id,
    resumable_upload_session_id: $resumable_upload_session_id,
    resumable_download_session_id: $resumable_download_session_id,
    user_sessions_job_id: $user_sessions_job_id,
    process_start_job_id: $process_start_job_id,
    process_status_job_id: $process_status_job_id,
    process_logs_job_id: $process_logs_job_id,
    process_restart_job_id: $process_restart_job_id,
    process_stop_job_id: $process_stop_job_id,
    job_id: $job_id,
    network_status_job_id: $network_status_job_id,
    network_probe_job_id: $network_probe_job_id,
    network_speed_job_id: $network_speed_job_id,
    destination: $destination,
    sha256_hex: $sha256_hex,
    checks: ["auth_session", "enrollment", "agent_noise_connect", "gateway_session_lifecycle", "privilege_unlocked_shell_job", "privilege_unlocked_shell_pty_job", "privilege_unlocked_shell_script_job", "job_output_follow_cli", "job_output_follow_vty", "live_shell_output_streaming", "large_job_output_artifact_retention", "agent_timeout_shell_job", "privilege_unlocked_file_pull", "job_target_status_archive_download", "terminal_session_lifecycle", "terminal_session_poll_output", "terminal_session_inventory", "resumable_file_transfer_upload", "resumable_file_transfer_download", "file_transfer_session_inventory", "no_privilege_unlock_user_sessions_rejected", "privilege_unlocked_user_sessions", "privilege_unlocked_process_start", "privilege_unlocked_process_status", "privilege_unlocked_process_logs", "privilege_unlocked_process_restart", "privilege_unlocked_process_stop", "process_supervisor_inventory", "privilege_unlocked_file_push", "job_target_output_audit", "network_status_observation", "network_probe_observation", "network_speed_observations", "network_observation_trends", "network_ospf_recommendations", "network_ospf_update_plans", "api_restart"]
  }'
