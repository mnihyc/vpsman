#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk base64 cp curl docker grep jq python3 timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-live-configuration-presets"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
container_name="vpsman-live-configuration-presets-$(date +%s%N)"
internal_token="smoke-internal-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
operator_password="configuration-presets-smoke-password"
client_id="configuration-presets-smoke-$(date +%s)"
super_password="smoke-super-password"
super_salt_hex="102132435465768798a9bacbdcedfe0f102132435465768798a9bacbdcedfe0f"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"
patch_proc_root="$SMOKE_TMPDIR/proc-root"
execution_cwd="$SMOKE_TMPDIR/execution-cwd"
execution_env_value="smoke-exec-policy-$(date +%s%N)"
ospf_status_marker="ospf-status-smoke-$(date +%s%N)"
ospf_update_marker="ospf-update-smoke-$(date +%s%N)"
execution_timeout_job_id=""
execution_policy_job_id=""
terminal_reject_job_id=""

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

api_pid=""
api_log=""
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_log="$SMOKE_TMPDIR/agent.log"
agent_config="$SMOKE_TMPDIR/agent.toml"

cleanup_live_configuration_presets_smoke() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_live_configuration_presets_smoke EXIT

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
if ! SMOKE_WAIT_TCP_SECS=90 smoke_wait_tcp 127.0.0.1 "$pg_port"; then
  docker logs "$container_name" >&2 || true
  exit 1
fi

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
  smoke_dump_logs "live configuration preset API failed to start" "$SMOKE_TMPDIR"/api-"$label"-*.log
  exit 1
}

api_get() {
  local path="$1"
  curl -fsS -H "Authorization: Bearer $access_token" "$api_url$path"
}

dump_job_diagnostics() {
  local label="$1"
  local inspected_job_id="$2"
  echo "$label" >&2
  if [[ -n "$inspected_job_id" && "$inspected_job_id" != "null" ]]; then
    echo "job:" >&2
    api_get "/api/v1/jobs/$inspected_job_id" >&2 || true
    echo >&2
    echo "targets:" >&2
    api_get "/api/v1/jobs/$inspected_job_id/targets" >&2 || true
    echo >&2
    echo "outputs:" >&2
    api_get "/api/v1/jobs/$inspected_job_id/outputs" >&2 || true
    echo >&2
  fi
  smoke_dump_logs "$label" "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log"
}

wait_agent_online() {
  local status=""
  local deadline=$((SECONDS + 35))
  until [[ "$status" == "online" ]]; do
    if (( SECONDS >= deadline )); then
      smoke_dump_logs "agent did not become online for live configuration preset smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log"
      exit 1
    fi
    status="$(api_get "/api/v1/agents" \
      | jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty')"
    sleep 0.25
  done
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

assert_configuration_preset_runtime_visible() {
  local deadline outputs_json
  deadline=$((SECONDS + 45))
  while (( SECONDS < deadline )); do
    job_id="$(submit_config_read)"
    outputs_json="$(api_get "/api/v1/jobs/$job_id/outputs")"
    if jq -e \
      --arg proc_root "$patch_proc_root" \
      --arg cwd "$execution_cwd" \
      --arg env "$execution_env_value" \
      --arg ospf_status "$ospf_status_marker" \
      --arg ospf_update "$ospf_update_marker" '
        .items[] | select(.stream == "status" and .done == true and .exit_code == 0)
        | (.data_base64 | @base64d | fromjson)
        | .type == "config_read"
          and (.runtime_config.telemetry.proc_root == $proc_root)
          and (.runtime_config.execution.working_directory == $cwd)
          and (.runtime_config.execution.environment_policy == "clean")
          and (.runtime_config.execution.environment_set.VPSMAN_EXEC_POLICY_SMOKE == $env)
          and (.runtime_config.execution.pty_policy == "disabled")
          and (.runtime_config.execution.process_cleanup == "direct_child")
          and (.runtime_config.network.ospf_status_command.argv | index($ospf_status) != null)
          and (.runtime_config.network.ospf_update_command.argv | index($ospf_update) != null)
      ' <<<"$outputs_json" >/dev/null; then
      return
    fi
    sleep 1
  done
  echo "preset-selected runtime config did not become visible through config_read" >&2
  dump_job_diagnostics "last config_read did not include selected configuration presets" "$job_id"
  exit 1
}

assert_execution_policy_applied() {
  local shell_json shell_outputs shell_stdout timeout_json timeout_job_json timeout_outputs
  local timeout_targets terminal_json terminal_outputs terminal_targets

  shell_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" job-shell \
      --script 'printf "cwd=%s\n" "$PWD"; printf "env=%s\n" "$VPSMAN_EXEC_POLICY_SMOKE"; printf "path=%s\n" "${PATH-}"' \
      --clients "$client_id" \
      --super-salt-hex "$super_salt_hex" \
      --max-timeout-secs 10 \
      --confirmed)"
  execution_policy_job_id="$(jq -r '.job_id' <<<"$shell_json")"
  if ! smoke_assert_job_create_queued "$shell_json" 1 || ! smoke_wait_api_job_status "$api_url" "$execution_policy_job_id" completed 45 >/dev/null; then
    dump_job_diagnostics "command execution policy shell script did not complete" \
      "$execution_policy_job_id"
    exit 1
  fi
  shell_outputs="$(api_get "/api/v1/jobs/$execution_policy_job_id/outputs")"
  shell_stdout="$(
    jq -r '.items[] | select(.stream == "stdout") | .data_base64' <<<"$shell_outputs" \
      | base64 -d
  )"
  grep -F "cwd=$execution_cwd" <<<"$shell_stdout" >/dev/null
  grep -F "env=$execution_env_value" <<<"$shell_stdout" >/dev/null
  jq -e --arg cwd "$execution_cwd" '
    .items[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "shell_script"
      and .working_directory == $cwd
      and .environment_policy == "clean"
      and .pty_policy == "disabled"
      and .process_cleanup == "direct_child"
  ' <<<"$shell_outputs" >/dev/null

  timeout_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" job-shell \
      --script 'exec sleep 3' \
      --clients "$client_id" \
      --super-salt-hex "$super_salt_hex" \
      --max-timeout-secs 1 \
      --confirmed)"
  execution_timeout_job_id="$(jq -r '.job_id' <<<"$timeout_json")"
  if ! smoke_assert_job_create_queued "$timeout_json" 1 || ! smoke_wait_api_job_status "$api_url" "$execution_timeout_job_id" terminal 45 >/dev/null; then
    dump_job_diagnostics "direct-child execution policy timeout did not report a terminal timeout" \
      "$execution_timeout_job_id"
    exit 1
  fi
  timeout_job_json="$(api_get "/api/v1/jobs/$execution_timeout_job_id")"
  timeout_targets="$(api_get "/api/v1/jobs/$execution_timeout_job_id/targets")"
  jq -e '.status == "agent_timeout" or .status == "control_timeout"' \
    <<<"$timeout_job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1
    and .[0].client_id == $client
    and (. [0].status == "agent_timeout" or .[0].status == "control_timeout")
  ' <<<"$timeout_targets" >/dev/null
  timeout_outputs="$(api_get "/api/v1/jobs/$execution_timeout_job_id/outputs")"
  if [[ "$(jq '.items | length' <<<"$timeout_outputs")" != "0" ]]; then
    jq -e '
      any(.items[]; .stream == "status" and .done == true and .exit_code == 124 and (
        (.data_base64 | @base64d | fromjson)
        | .type == "command_timeout"
          and .mode == "shell_script"
          and .cleanup.target_kind == "process"
      ))
    ' <<<"$timeout_outputs" >/dev/null
  fi

  terminal_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" terminal-open \
      --argv /bin/sh \
      --clients "$client_id" \
      --super-salt-hex "$super_salt_hex" \
      --max-timeout-secs 10 \
      --confirmed)"
  terminal_reject_job_id="$(jq -r '.job.job_id' <<<"$terminal_json")"
  if ! jq -e '.job.target_count == 1' <<<"$terminal_json" >/dev/null \
    || ! smoke_wait_api_job_status "$api_url" "$terminal_reject_job_id" rejected 45 >/dev/null; then
    dump_job_diagnostics "disabled PTY policy did not reject terminal open" \
      "$terminal_reject_job_id"
    exit 1
  fi
  terminal_targets="$(api_get "/api/v1/jobs/$terminal_reject_job_id/targets")"
  terminal_outputs="$(api_get "/api/v1/jobs/$terminal_reject_job_id/outputs")"
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "rejected" and .[0].exit_code == 126
  ' <<<"$terminal_targets" >/dev/null
  jq -e '
    .items[] | select(.stream == "status" and .done == true and .exit_code == 126)
    | (.data_base64 | @base64d | fromjson)
    | .type == "terminal_open"
      and .status == "rejected"
      and .reason == "execution_pty_policy_disabled"
  ' <<<"$terminal_outputs" >/dev/null
}

start_api "first"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"configuration-presets-smoke\",\"password\":\"$operator_password\"}" \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
export VPSMAN_API_TOKEN="$access_token"
jq -e '.operator.username == "configuration-presets-smoke" and .token_type == "Bearer"' \
  <<<"$auth_json" >/dev/null

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="configuration-presets-smoke-gateway" \
VPSMAN_GATEWAY_SPOOL_DIR="$SMOKE_TMPDIR/gateway-spool" \
RUST_LOG="vpsman_gateway=warn" \
  target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
if ! SMOKE_WAIT_TCP_SECS=90 smoke_wait_tcp 127.0.0.1 "$gateway_port"; then
  smoke_dump_logs "gateway listener did not open for live configuration preset smoke" \
    "$SMOKE_TMPDIR"/api-*.log "$gateway_log"
  exit 1
fi
if ! SMOKE_WAIT_TCP_SECS=90 smoke_wait_tcp 127.0.0.1 "$gateway_control_port"; then
  smoke_dump_logs "gateway control listener did not open for live configuration preset smoke" \
    "$SMOKE_TMPDIR"/api-*.log "$gateway_log"
  exit 1
fi

smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$agent_config" \
  "$client_id" \
  "$client_id" \
  "configuration-presets-smoke" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"

smoke_start_local_agent \
  "$agent_config" \
  "$agent_log" \
  "$SMOKE_TMPDIR/agent-work" \
  "vpsman_agent=warn"
wait_agent_online

mkdir -p "$patch_proc_root" "$execution_cwd"
host_metrics_definition="$(jq -nc \
  --arg proc_root "$patch_proc_root" \
  '{
    source:"linux_procfs",
    proc_root:$proc_root,
    sys_class_net_dir:"/sys/class/net",
    hostname_file:"/etc/hostname",
    os_release_file:"/etc/os-release"
  }')"
host_metrics_preset_json="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" config-preset-create \
    --behavior host_metrics \
    --name smoke:custom-proc-root \
    --description "smoke custom proc root" \
    --definition-json "$host_metrics_definition")"
host_metrics_preset_id="$(jq -r '.id' <<<"$host_metrics_preset_json")"
execution_definition="$(jq -nc \
  --arg cwd "$execution_cwd" \
  --arg env "$execution_env_value" \
  '{
    shell_script_argv: ["/bin/sh", "-lc"],
    working_directory: $cwd,
    environment_policy: "clean",
    environment_keep: ["PATH"],
    environment_set: {VPSMAN_EXEC_POLICY_SMOKE: $env},
    pty_policy: "disabled",
    process_cleanup: "direct_child"
  }')"
execution_preset_json="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" config-preset-create \
    --behavior command_execution \
    --name smoke:locked-execution-policy \
    --description "smoke non-default command execution policy" \
    --definition-json "$execution_definition")"
execution_preset_id="$(jq -r '.id' <<<"$execution_preset_json")"
ospf_definition="$(jq -nc \
  --arg status_marker "$ospf_status_marker" \
  --arg update_marker "$ospf_update_marker" \
  '{
    contract_version: 2,
    status_command: {
      argv: ["/bin/sh", "-c", "printf \"100\\n\"", $status_marker, "{plan_id}", "{interface}", "{endpoint_side}"],
      max_timeout_secs: 5,
      max_output_bytes: 4096
    },
    update_command: {
      argv: ["/bin/sh", "-c", "printf \"cost updated\\n\"", $update_marker, "{plan_id}", "{interface}", "{endpoint_side}", "{desired_cost}"],
      max_timeout_secs: 5,
      max_output_bytes: 4096
    }
  }')"
ospf_preset_json="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" config-preset-create \
    --behavior ospf_update_command \
    --name smoke:ospf-updater \
    --description "smoke bounded OSPF updater commands" \
    --definition-json "$ospf_definition")"
ospf_preset_id="$(jq -r '.id' <<<"$ospf_preset_json")"

host_metrics_assignment_preview="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" config-source-set \
    --behavior host_metrics \
    --preset-id "$host_metrics_preset_id" \
    --clients "$client_id")"
host_metrics_assignment_hash="$(jq -er '
  .preview_hash | select(type == "string" and length > 0)
' <<<"$host_metrics_assignment_preview")"
VPSMAN_API_TOKEN="$access_token" \
VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
  target/debug/vpsctl --api-url "$api_url" config-source-set \
    --behavior host_metrics \
    --preset-id "$host_metrics_preset_id" \
    --clients "$client_id" \
    --preview-hash "$host_metrics_assignment_hash" \
    --confirmed >/dev/null
execution_assignment_preview="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" config-source-set \
    --behavior command_execution \
    --preset-id "$execution_preset_id" \
    --clients "$client_id")"
execution_assignment_hash="$(jq -er '
  .preview_hash | select(type == "string" and length > 0)
' <<<"$execution_assignment_preview")"
VPSMAN_API_TOKEN="$access_token" \
VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
  target/debug/vpsctl --api-url "$api_url" config-source-set \
    --behavior command_execution \
    --preset-id "$execution_preset_id" \
    --clients "$client_id" \
    --preview-hash "$execution_assignment_hash" \
    --confirmed >/dev/null
ospf_assignment_preview="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" config-source-set \
    --behavior ospf_update_command \
    --preset-id "$ospf_preset_id" \
    --clients "$client_id")"
ospf_assignment_hash="$(jq -er '
  .preview_hash | select(type == "string" and length > 0)
' <<<"$ospf_assignment_preview")"
VPSMAN_API_TOKEN="$access_token" \
VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
  target/debug/vpsctl --api-url "$api_url" config-source-set \
    --behavior ospf_update_command \
    --preset-id "$ospf_preset_id" \
    --clients "$client_id" \
    --preview-hash "$ospf_assignment_hash" \
    --confirmed >/dev/null

rendered_patch="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" config-render \
    --client-id "$client_id" \
    --format toml)"
grep -q '\[telemetry\]' <<<"$rendered_patch"
grep -q "proc_root = \"$patch_proc_root\"" <<<"$rendered_patch"
grep -q '\[execution\]' <<<"$rendered_patch"
grep -q "working_directory = \"$execution_cwd\"" <<<"$rendered_patch"
grep -q 'environment_policy = "clean"' <<<"$rendered_patch"
grep -q "$execution_env_value" <<<"$rendered_patch"
grep -q 'pty_policy = "disabled"' <<<"$rendered_patch"
grep -q 'process_cleanup = "direct_child"' <<<"$rendered_patch"
grep -q "$ospf_status_marker" <<<"$rendered_patch"
grep -q "$ospf_update_marker" <<<"$rendered_patch"

if grep -q "$patch_proc_root" "$agent_config"; then
  echo "configuration preset override mutated immutable bootstrap config file" >&2
  exit 1
fi
assert_configuration_preset_runtime_visible
assert_execution_policy_applied

stop_api
start_api "restart"
api_get "/api/v1/auth/me" | jq -e '.username == "configuration-presets-smoke"' >/dev/null
api_get "/api/v1/configuration-presets?behavior=host_metrics" | jq -e --arg preset_id "$host_metrics_preset_id" '
  any(.[]; .id == $preset_id and .behavior == "host_metrics" and .kind == "custom" and .name == "smoke:custom-proc-root")
' >/dev/null
api_get "/api/v1/configuration-presets?behavior=command_execution" | jq -e --arg preset_id "$execution_preset_id" '
  any(.[]; .id == $preset_id and .behavior == "command_execution" and .kind == "custom" and .name == "smoke:locked-execution-policy")
' >/dev/null
api_get "/api/v1/configuration-presets?behavior=ospf_update_command" | jq -e --arg preset_id "$ospf_preset_id" '
  any(.[]; .id == $preset_id and .behavior == "ospf_update_command" and .kind == "custom" and .name == "smoke:ospf-updater")
' >/dev/null
api_get "/api/v1/configuration-sources?client_id=$client_id" | jq -e --arg preset_id "$host_metrics_preset_id" '
  any(.[]; .effective_preset_id == $preset_id and .behavior == "host_metrics" and .selection_origin == "explicit_override")
' >/dev/null
api_get "/api/v1/configuration-sources?client_id=$client_id" | jq -e --arg preset_id "$execution_preset_id" '
  any(.[]; .effective_preset_id == $preset_id and .behavior == "command_execution" and .selection_origin == "explicit_override")
' >/dev/null
api_get "/api/v1/configuration-sources?client_id=$client_id" | jq -e --arg preset_id "$ospf_preset_id" '
  any(.[]; .effective_preset_id == $preset_id and .behavior == "ospf_update_command" and .selection_origin == "explicit_override" and .readiness.state != "unconfigured")
' >/dev/null
assert_configuration_preset_runtime_visible

for behavior in host_metrics command_execution ospf_update_command; do
  reset_preview="$(VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" config-source-reset \
      --behavior "$behavior" \
      --clients "$client_id")"
  reset_preview_hash="$(jq -er '
    .preview_hash | select(type == "string" and length > 0)
  ' <<<"$reset_preview")"
  VPSMAN_API_TOKEN="$access_token" \
  VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
    target/debug/vpsctl --api-url "$api_url" config-source-reset \
      --behavior "$behavior" \
      --clients "$client_id" \
      --preview-hash "$reset_preview_hash" \
      --confirmed >/dev/null
done
api_get "/api/v1/configuration-sources?client_id=$client_id" | jq -e '
  [.[] | select(.behavior == "host_metrics" or .behavior == "command_execution" or .behavior == "ospf_update_command")] as $reset |
  ($reset | length) == 3 and
  all($reset[];
    .effective_preset_kind == "system" and
    .selection_origin == "system_default" and
    .override_updated_at == null) and
  any($reset[]; .behavior == "ospf_update_command" and .readiness.state == "unconfigured")
' >/dev/null
default_rendered_config="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" config-render \
    --client-id "$client_id" \
    --format toml)"
if grep -Fq "$patch_proc_root" <<<"$default_rendered_config"; then
  echo "reset retained the custom host metrics override" >&2
  exit 1
fi
grep -Fq 'proc_root = "/proc"' <<<"$default_rendered_config"
grep -Fq 'environment_policy = "inherit"' <<<"$default_rendered_config"
grep -Fq 'pty_policy = "native_pty"' <<<"$default_rendered_config"
if grep -Fq "$ospf_status_marker" <<<"$default_rendered_config" \
  || grep -Fq "$ospf_update_marker" <<<"$default_rendered_config"; then
  echo "reset retained the custom OSPF updater override" >&2
  exit 1
fi

for preset_id in "$host_metrics_preset_id" "$execution_preset_id" "$ospf_preset_id"; do
  VPSMAN_API_TOKEN="$access_token" \
    target/debug/vpsctl --api-url "$api_url" config-preset-delete \
      --preset-id "$preset_id" \
      --confirmed
done
api_get "/api/v1/configuration-presets" | jq -e \
  --arg host_metrics_preset_id "$host_metrics_preset_id" \
  --arg execution_preset_id "$execution_preset_id" \
  --arg ospf_preset_id "$ospf_preset_id" '
    all(.[];
      .id != $host_metrics_preset_id and
      .id != $execution_preset_id and
      .id != $ospf_preset_id)
  ' >/dev/null

jq -n \
  --arg client_id "$client_id" \
  --arg job_id "$job_id" \
  --arg host_metrics_preset_id "$host_metrics_preset_id" \
  --arg execution_preset_id "$execution_preset_id" \
  --arg ospf_preset_id "$ospf_preset_id" \
  --arg execution_policy_job_id "$execution_policy_job_id" \
  --arg execution_timeout_job_id "$execution_timeout_job_id" \
  --arg terminal_reject_job_id "$terminal_reject_job_id" \
  --arg proc_root "$patch_proc_root" \
  --arg execution_cwd "$execution_cwd" \
  '{
    live_configuration_presets_smoke: "ok",
    postgres_backed: true,
    auth_session: "persisted",
    api_restart: "verified",
    reset_to_system_default: "verified",
    unused_custom_preset_delete: "verified",
    command_execution_policy_fields: "verified",
    ospf_update_command_lifecycle: "verified",
    client_id: $client_id,
    host_metrics_preset_id: $host_metrics_preset_id,
    execution_preset_id: $execution_preset_id,
    ospf_preset_id: $ospf_preset_id,
    config_read_job_id: $job_id,
    execution_policy_job_id: $execution_policy_job_id,
    execution_timeout_job_id: $execution_timeout_job_id,
    terminal_reject_job_id: $terminal_reject_job_id,
    proc_root: $proc_root,
    execution_cwd: $execution_cwd
  }'
