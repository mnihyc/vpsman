#!/usr/bin/env bash

wait_for_supervisor_log() {
  local deadline=$((SECONDS + 10))
  local log_file="$agent_supervisor_dir/logs/$supervisor_name.stdout.log"
  until [[ -f "$log_file" ]] && grep -Fq "$supervisor_payload" "$log_file"; do
    if (( SECONDS >= deadline )); then
      smoke_dump_logs "supervisor log did not contain expected payload" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log" "$log_file"
      exit 1
    fi
    sleep 0.25
  done
}

assert_no_supervisor_process_leaked() {
  if ps -eo args= | grep -F "$supervisor_payload" | grep -v grep >/dev/null; then
    echo "supervised process is still running after process-stop" >&2
    ps -eo pid=,args= | grep -F "$supervisor_payload" >&2 || true
    exit 1
  fi
}

assert_supervisor_status_job() {
  local job_id="$1"
  local command_type="$2"
  local event_type="$3"
  local expected_status="$4"
  local job_json targets_json outputs_json status_json audits_json
  job_json="$(api_get "/api/v1/jobs/$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=50")"

  jq -e --arg command_type "$command_type" '
    .status == "succeeded" and .command_type == $command_type and .target_count == 1
  ' <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "succeeded" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  status_json="$(
    jq -r '.[] | select(.stream == "status" and .done == true and .exit_code == 0) | .data_base64' \
      <<<"$outputs_json" | base64 -d
  )"
  jq -e \
    --arg type "$event_type" \
    --arg name "$supervisor_name" \
    --arg expected_status "$expected_status" '
      .type == $type
      and .name == $name
      and (.stdout_log | contains($name))
      and (.stderr_log | contains($name))
      and (
        if $expected_status == "stopped_or_requested" then
          (.status == "stopped" or .status == "stop_requested")
        else
          .status == $expected_status
        end
      )
    ' <<<"$status_json" >/dev/null
  jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' \
    <<<"$audits_json" >/dev/null
}

assert_supervisor_snapshot_job() {
  local job_id="$1"
  local job_json targets_json outputs_json snapshot_json
  job_json="$(api_get "/api/v1/jobs/$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$job_id/outputs")"

  jq -e '.status == "succeeded" and .command_type == "process_status" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "succeeded" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  snapshot_json="$(jq -r '.[] | select(.stream == "stdout") | .data_base64' <<<"$outputs_json" | base64 -d)"
  jq -e --arg name "$supervisor_name" '
    .type == "process_status"
    and any(.processes[]; .name == $name and .status == "running" and (.pid | type == "number"))
  ' <<<"$snapshot_json" >/dev/null
}

assert_supervisor_logs_job() {
  local job_id="$1"
  local job_json targets_json outputs_json decoded_stdout status_json
  job_json="$(api_get "/api/v1/jobs/$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$job_id/outputs")"

  jq -e '.status == "succeeded" and .command_type == "process_logs" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "succeeded" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  decoded_stdout="$(jq -r '.[] | select(.stream == "stdout") | .data_base64' <<<"$outputs_json" | base64 -d)"
  if [[ "$decoded_stdout" != *"$supervisor_payload"* ]]; then
    echo "process logs output did not include supervisor payload" >&2
    echo "$decoded_stdout" >&2
    exit 1
  fi
  status_json="$(
    jq -r '.[] | select(.stream == "status" and .done == true and .exit_code == 0) | .data_base64' \
      <<<"$outputs_json" | base64 -d
  )"
  jq -e --arg name "$supervisor_name" '
    .type == "process_logs" and .name == $name and .max_bytes == 4096
  ' <<<"$status_json" >/dev/null
  if grep -Fq "$super_password" <<<"$decoded_stdout$status_json"; then
    echo "process supervisor outputs leaked super password" >&2
    exit 1
  fi
}

assert_supervisor_inventory() {
  local expected_job_id="$1"
  local expected_source="$2"
  local expected_status="$3"
  local inventory_json
  inventory_json="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" process-supervisor-inventory --limit 20)"

  jq -e \
    --arg client "$client_id" \
    --arg name "$supervisor_name" \
    --arg job_id "$expected_job_id" \
    --arg source "$expected_source" \
    --arg expected_status "$expected_status" '
      any(.[]; 
        .client_id == $client
        and .name == $name
        and .source_job_id == $job_id
        and .source_command_type == $source
        and (.stdout_log | contains($name))
        and (.stderr_log | contains($name))
        and (
          if $expected_status == "stopped_or_requested" then
            (.status == "stopped" or .status == "stop_requested")
          else
            .status == $expected_status
          end
        )
      )
    ' <<<"$inventory_json" >/dev/null
  if grep -Fq "$super_password" <<<"$inventory_json"; then
    echo "process supervisor inventory leaked super password" >&2
    exit 1
  fi
}
