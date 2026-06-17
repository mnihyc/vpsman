#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk base64 cp curl docker grep jq python3 timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-live-data-source-config-patch"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
container_name="vpsman-live-ds-patch-$(date +%s%N)"
internal_token="smoke-internal-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
operator_password="data-source-patch-smoke-password"
client_id="data-source-patch-smoke-$(date +%s)"
super_password="smoke-super-password"
super_salt_hex="102132435465768798a9bacbdcedfe0f102132435465768798a9bacbdcedfe0f"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"
patch_proc_root="$SMOKE_TMPDIR/proc-root"
execution_cwd="$SMOKE_TMPDIR/execution-cwd"
execution_env_value="smoke-exec-policy-$(date +%s%N)"
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
rollback_config="$agent_config.rollback"

cleanup_live_data_source_patch_smoke() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_live_data_source_patch_smoke EXIT

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
  smoke_dump_logs "live data-source config patch API failed to start" "$SMOKE_TMPDIR"/api-"$label"-*.log
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
      smoke_dump_logs "agent did not become online for live data-source patch smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$agent_log"
      exit 1
    fi
    status="$(api_get "/api/v1/agents" \
      | jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty')"
    sleep 0.25
  done
}

assert_patch_persisted() {
  local job_json targets_json outputs_json audits_json decoded_outputs
  job_json="$(api_get "/api/v1/jobs/$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  outputs_json="$(api_get "/api/v1/jobs/$job_id/outputs")"
  audits_json="$(api_get "/api/v1/audit?limit=30")"

  jq -e '.status == "completed" and .command_type == "data_source_config_patch" and .target_count == 1' \
    <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
  jq -e --arg config_path "$agent_config" --arg rollback_path "$rollback_config" '
    .[] | select(.stream == "status" and .done == true and .exit_code == 0)
    | (.data_base64 | @base64d | fromjson)
    | .type == "data_source_config_patch"
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
    echo "job outputs leaked data-source patch secrets or trust anchors" >&2
    exit 1
  fi
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
      --timeout-secs 10 \
      --confirmed)"
  execution_policy_job_id="$(jq -r '.job_id' <<<"$shell_json")"
  if ! smoke_assert_job_create_queued "$shell_json" 1 || ! smoke_wait_api_job_status "$api_url" "$execution_policy_job_id" completed 45 >/dev/null; then
    dump_job_diagnostics "command execution policy shell script did not complete" \
      "$execution_policy_job_id"
    exit 1
  fi
  shell_outputs="$(api_get "/api/v1/jobs/$execution_policy_job_id/outputs")"
  shell_stdout="$(
    jq -r '.[] | select(.stream == "stdout") | .data_base64' <<<"$shell_outputs" \
      | base64 -d
  )"
  grep -F "cwd=$execution_cwd" <<<"$shell_stdout" >/dev/null
  grep -F "env=$execution_env_value" <<<"$shell_stdout" >/dev/null
  jq -e --arg cwd "$execution_cwd" '
    .[] | select(.stream == "status" and .done == true and .exit_code == 0)
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
      --timeout-secs 1 \
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
  if [[ "$(jq 'length' <<<"$timeout_outputs")" != "0" ]]; then
    jq -e '
      any(.[]; .stream == "status" and .done == true and .exit_code == 124 and (
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
      --timeout-secs 10 \
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
    .[] | select(.stream == "status" and .done == true and .exit_code == 126)
    | (.data_base64 | @base64d | fromjson)
    | .type == "terminal_open"
      and .status == "rejected"
      and .reason == "execution_pty_policy_disabled"
  ' <<<"$terminal_outputs" >/dev/null
}

start_api "first"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"data-source-patch-smoke\",\"password\":\"$operator_password\"}" \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
export VPSMAN_API_TOKEN="$access_token"
jq -e '.operator.username == "data-source-patch-smoke" and .token_type == "Bearer"' \
  <<<"$auth_json" >/dev/null

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="data-source-patch-smoke-gateway" \
VPSMAN_GATEWAY_SPOOL_DIR="$SMOKE_TMPDIR/gateway-spool" \
RUST_LOG="vpsman_gateway=warn" \
  target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
if ! SMOKE_WAIT_TCP_SECS=90 smoke_wait_tcp 127.0.0.1 "$gateway_port"; then
  smoke_dump_logs "gateway listener did not open for live data-source patch smoke" \
    "$SMOKE_TMPDIR"/api-*.log "$gateway_log"
  exit 1
fi
if ! SMOKE_WAIT_TCP_SECS=90 smoke_wait_tcp 127.0.0.1 "$gateway_control_port"; then
  smoke_dump_logs "gateway control listener did not open for live data-source patch smoke" \
    "$SMOKE_TMPDIR"/api-*.log "$gateway_log"
  exit 1
fi

smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$agent_config" \
  "$client_id" \
  "$client_id" \
  "data-source-patch-smoke" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"
wait_agent_online

mkdir -p "$patch_proc_root" "$execution_cwd"
preset_json="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" data-source-preset-create \
    --domain telemetry_metrics_source \
    --name smoke:custom-proc-root \
    --description "smoke custom proc root" \
    --definition-json "{\"source\":\"linux_procfs\",\"proc_root\":\"$patch_proc_root\"}")"
preset_id="$(jq -r '.id' <<<"$preset_json")"
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
  target/debug/vpsctl --api-url "$api_url" data-source-preset-create \
    --domain command_execution_policy \
    --name smoke:locked-execution-policy \
    --description "smoke non-default command execution policy" \
    --definition-json "$execution_definition")"
execution_preset_id="$(jq -r '.id' <<<"$execution_preset_json")"

VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" data-source-preset-assign \
    --domain telemetry_metrics_source \
    --preset-id "$preset_id" \
    --clients "$client_id" \
    --confirmed >/dev/null
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" data-source-preset-assign \
    --domain command_execution_policy \
    --preset-id "$execution_preset_id" \
    --clients "$client_id" \
    --confirmed >/dev/null

rendered_patch="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" data-source-hot-config \
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

reject_body="$(jq -nc \
  --arg client "$client_id" \
  --arg toml "$rendered_patch" \
  '{
    command: "data_source_config_patch",
    operation: {
      type: "data_source_config_patch",
      apply_mode: "incremental_patch",
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
  echo "expected no-privilege-unlock data-source config patch to return 403, got $reject_status" >&2
  cat "$reject_json" >&2 || true
  exit 1
fi
jq -e '.error == "privilege_assertion_required" and .status == 403' "$reject_json" >/dev/null
if grep -q "$patch_proc_root" "$agent_config"; then
  echo "data-source patch changed config after no-privilege-unlock rejection" >&2
  exit 1
fi

push_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" data-source-hot-config-apply \
    --client-id "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --force-unprivileged \
    --confirmed)"
job_id="$(jq -r '.job_id' <<<"$push_json")"
if ! smoke_assert_job_create_queued "$push_json" 1 || ! smoke_wait_api_job_status "$api_url" "$job_id" completed 45 >/dev/null; then
  echo "expected privilege-unlocked data-source patch to complete; got:" >&2
  echo "$push_json" >&2
  dump_job_diagnostics "privilege-unlocked data-source patch did not complete" "$job_id"
  exit 1
fi

grep -q "proc_root = \"$patch_proc_root\"" "$agent_config"
grep -q "working_directory = \"$execution_cwd\"" "$agent_config"
grep -q 'environment_policy = "clean"' "$agent_config"
grep -q "$execution_env_value" "$agent_config"
grep -q 'pty_policy = "disabled"' "$agent_config"
grep -q 'process_cleanup = "direct_child"' "$agent_config"
grep -q "display_name = \"$client_id\"" "$agent_config"
if grep -q "$patch_proc_root" "$rollback_config"; then
  echo "data-source patch rollback captured patched proc_root instead of original config" >&2
  exit 1
fi
assert_patch_persisted
assert_execution_policy_applied

stop_api
start_api "restart"
api_get "/api/v1/auth/me" | jq -e '.username == "data-source-patch-smoke"' >/dev/null
api_get "/api/v1/data-source-presets" | jq -e --arg preset_id "$preset_id" '
  any(.[]; .id == $preset_id and .domain == "telemetry_metrics_source" and .name == "smoke:custom-proc-root")
' >/dev/null
api_get "/api/v1/data-source-presets" | jq -e --arg preset_id "$execution_preset_id" '
  any(.[]; .id == $preset_id and .domain == "command_execution_policy" and .name == "smoke:locked-execution-policy")
' >/dev/null
api_get "/api/v1/data-source-assignments?client_id=$client_id" | jq -e --arg preset_id "$preset_id" '
  any(.[]; .preset_id == $preset_id and .domain == "telemetry_metrics_source")
' >/dev/null
api_get "/api/v1/data-source-assignments?client_id=$client_id" | jq -e --arg preset_id "$execution_preset_id" '
  any(.[]; .preset_id == $preset_id and .domain == "command_execution_policy")
' >/dev/null
assert_patch_persisted

jq -n \
  --arg client_id "$client_id" \
  --arg job_id "$job_id" \
  --arg preset_id "$preset_id" \
  --arg execution_preset_id "$execution_preset_id" \
  --arg execution_policy_job_id "$execution_policy_job_id" \
  --arg execution_timeout_job_id "$execution_timeout_job_id" \
  --arg terminal_reject_job_id "$terminal_reject_job_id" \
  --arg proc_root "$patch_proc_root" \
  --arg execution_cwd "$execution_cwd" \
  '{
    live_data_source_config_patch_smoke: "ok",
    postgres_backed: true,
    auth_session: "persisted",
    api_restart: "verified",
    no_privilege_unlock_rejected: true,
    command_execution_policy_fields: "verified",
    client_id: $client_id,
    preset_id: $preset_id,
    execution_preset_id: $execution_preset_id,
    job_id: $job_id,
    execution_policy_job_id: $execution_policy_job_id,
    execution_timeout_job_id: $execution_timeout_job_id,
    terminal_reject_job_id: $terminal_reject_job_id,
    proc_root: $proc_root,
    execution_cwd: $execution_cwd
  }'
