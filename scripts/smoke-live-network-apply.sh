#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk base64 cmp curl docker grep jq python3 sed sha256sum shuf timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-live-network-apply"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
container_name="vpsman-live-network-apply-$(date +%s%N)"
internal_token="network-apply-internal-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
client_id="network-apply-smoke-$(date +%s)"
peer_client_id="$client_id-peer"
super_password="network-apply-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

api_pid=""
api_log=""
agent_pid=""
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_log="$SMOKE_TMPDIR/agent-bootstrap.log"
agent_config="$SMOKE_TMPDIR/agent.toml"
peer_agent_config="$SMOKE_TMPDIR/peer-agent.toml"
network_plan_file="$SMOKE_TMPDIR/network-apply-plan.json"
adapter_log="$SMOKE_TMPDIR/runtime-adapter.log"
adapter_script="$SMOKE_TMPDIR/runtime-adapter.sh"
adapter_plan_file="$SMOKE_TMPDIR/runtime-adapter-plan.json"
adapter_apply_status_file="$SMOKE_TMPDIR/runtime-adapter-apply-status.json"
adapter_rollback_status_file="$SMOKE_TMPDIR/runtime-adapter-rollback-status.json"
network_root="$SMOKE_TMPDIR/network-root"
ifupdown_file="$network_root/etc/network/interfaces.d/vpsman-tunnels"
bird2_file="$network_root/etc/bird/vpsman-ospf.conf"
expected_ifupdown="$SMOKE_TMPDIR/expected-ifupdown"
expected_bird2="$SMOKE_TMPDIR/expected-bird2"
apply_status_file="$SMOKE_TMPDIR/apply-status.json"
rollback_status_file="$SMOKE_TMPDIR/rollback-status.json"
adapter_plan_id=""
adapter_apply_job_id=""
adapter_rollback_job_id=""
mkdir -p "$(dirname "$ifupdown_file")" "$(dirname "$bird2_file")"
printf '# unmanaged ifupdown\n' >"$expected_ifupdown"
printf '# unmanaged bird2\n' >"$expected_bird2"
cp "$expected_ifupdown" "$ifupdown_file"
cp "$expected_bird2" "$bird2_file"
printf '%s\n' \
  '#!/usr/bin/env sh' \
  'set -eu' \
  'log_file="$1"' \
  'shift' \
  'printf "%s\n" "$*" >>"$log_file"' \
  >"$adapter_script"
chmod +x "$adapter_script"

cleanup_live_network_apply_smoke() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_live_network_apply_smoke EXIT

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

stop_agent() {
  if [[ -n "$agent_pid" ]]; then
    kill "$agent_pid" >/dev/null 2>&1 || true
    wait "$agent_pid" >/dev/null 2>&1 || true
    agent_pid=""
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
  smoke_dump_logs "live network-apply API failed to start" "$SMOKE_TMPDIR"/api-"$label"-*.log
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
      smoke_dump_logs "agent did not become online for live network-apply smoke" \
        "$SMOKE_TMPDIR"/api-*.log "$gateway_log" "$SMOKE_TMPDIR"/agent-*.log
      exit 1
    fi
    status="$(api_get "/api/v1/agents" \
      | jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty')"
    sleep 0.25
  done
}

start_agent() {
  local label="$1"
  agent_log="$SMOKE_TMPDIR/agent-$label.log"
  VPSMAN_AGENT_CONFIG="$agent_config" \
  RUST_LOG="vpsman_agent=warn" \
    target/debug/vpsman-agent run >"$agent_log" 2>&1 &
  agent_pid="$!"
  smoke_track_pid "$agent_pid"
  wait_agent_online
}

status_output_for_job() {
  local job_id="$1"
  local output_file="$2"
  local encoded
  encoded="$(api_get "/api/v1/jobs/$job_id/outputs" \
    | jq -er 'first(.[] | select(.stream == "status" and .done == true and .exit_code == 0) | .data_base64)')"
  printf '%s' "$encoded" | base64 -d >"$output_file"
}

assert_job_completed() {
  local job_id="$1"
  local command_type="$2"
  local job_json targets_json
  job_json="$(api_get "/api/v1/jobs/$job_id")"
  targets_json="$(api_get "/api/v1/jobs/$job_id/targets")"
  jq -e --arg command_type "$command_type" '
    .status == "completed" and .command_type == $command_type and .target_count == 1
  ' <<<"$job_json" >/dev/null
  jq -e --arg client "$client_id" '
    length == 1 and .[0].client_id == $client and .[0].status == "completed" and .[0].exit_code == 0
  ' <<<"$targets_json" >/dev/null
}

assert_outputs_redacted() {
  local decoded_outputs job_id
  decoded_outputs="$(
    for job_id in \
      "$apply_job_id" \
      "$rollback_job_id" \
      "${adapter_apply_job_id:-}" \
      "${adapter_rollback_job_id:-}"; do
      [[ -n "$job_id" ]] || continue
      api_get "/api/v1/jobs/$job_id/outputs"
    done | jq -r '.[].data_base64' | while IFS= read -r item; do
      printf '%s' "$item" | base64 -d
      printf '\n'
    done
  )"
  if grep -F \
    -e "$super_password" \
    -e "$super_salt_hex" \
    -e "privilege_assertion" \
    -e "client_private_key_hex" \
    -e "server_public_key_hex" \
    <<<"$decoded_outputs" >/dev/null; then
    echo "network apply/rollback outputs leaked privilege or trust material" >&2
    exit 1
  fi
}

assert_apply_evidence() {
  local backup_path
  assert_job_completed "$apply_job_id" "network_apply"
  status_output_for_job "$apply_job_id" "$apply_status_file"
  jq -e --arg client "$client_id" --arg peer "$peer_client_id" '
    .type == "network_apply"
      and .plan == "live-apply"
      and .interface == "vpsap0"
      and .side == "left"
      and .client_id == $client
      and .peer_client_id == $peer
      and .rollback_available == true
      and ([.applied_files[] | select(
        .changed == true
        and (.backup_path != null)
        and (.sha256_hex | test("^[0-9a-f]{64}$"))
      )] | length == 2)
      and (.validation | length == 0)
      and (.reload | length == 0)
  ' "$apply_status_file" >/dev/null
  while IFS= read -r backup_path; do
    [[ -f "$backup_path" ]]
  done < <(jq -r '.applied_files[].backup_path' "$apply_status_file")
}

assert_rollback_evidence() {
  assert_job_completed "$rollback_job_id" "network_rollback"
  status_output_for_job "$rollback_job_id" "$rollback_status_file"
  jq -e --arg client "$client_id" --arg peer "$peer_client_id" '
    .type == "network_rollback"
      and .plan == "live-apply"
      and .interface == "vpsap0"
      and .side == "left"
      and .client_id == $client
      and .peer_client_id == $peer
      and .changed == true
      and ([.removed_files[] | select(
        .changed == true
        and (.backup_path != null)
        and (.sha256_hex | test("^[0-9a-f]{64}$"))
      )] | length == 2)
      and (.validation | length == 0)
      and (.reload | length == 0)
  ' "$rollback_status_file" >/dev/null
}

assert_audit_evidence() {
  local audits_json
  audits_json="$(api_get "/api/v1/audit?limit=100")"
  jq -e '[.[].action] |
    index("job.dispatch_requested")
    and index("job.target_result")
    and index("network.tunnel_plan_applied")
    and index("network.tunnel_plan_rolled_back")' \
    <<<"$audits_json" >/dev/null
}

assert_adapter_promotion_evidence() {
  local plans_json
  plans_json="$(api_get "/api/v1/tunnel-plans")"
  if ! jq -e \
    --arg plan_id "$adapter_plan_id" \
    --arg adapter_script "$adapter_script" '
      .[] | select(.id == $plan_id)
      | .name == "live-adapter"
        and .plan.runtime_control.manager == "external_managed_adapter"
        and .input.runtime_control.manager == "external_managed_adapter"
        and .plan.runtime_control.startup.argv[0] == $adapter_script
        and .plan.runtime_control.status.argv[0] == $adapter_script
        and .plan.runtime_control.traffic_limit_apply.argv[0] == $adapter_script
        and .plan.runtime_control.traffic_limit.egress_kbps == 10000
        and .plan.runtime_control.traffic_limit.ingress_kbps == 5000
        and .plan.runtime_topology.version == "live-adapter-v1"
        and .plan.runtime_topology.desired_interfaces == ["ovpnlive0"]
        and .plan.touched_files == ["/etc/bird/vpsman-ospf.conf"]
    ' <<<"$plans_json" >/dev/null; then
    echo "unexpected adapter tunnel plan state after promotion" >&2
    printf '%s\n' "$plans_json" >&2
    exit 1
  fi
}

assert_adapter_apply_evidence() {
  assert_job_completed "$adapter_apply_job_id" "network_apply"
  status_output_for_job "$adapter_apply_job_id" "$adapter_apply_status_file"
  jq -e --arg client "$client_id" --arg peer "$peer_client_id" '
    .type == "network_apply"
      and .plan == "live-adapter"
      and .interface == "ovpnlive0"
      and .side == "left"
      and .client_id == $client
      and .peer_client_id == $peer
      and .config_backend == "ifupdown"
      and .rollback_available == true
      and ([.applied_files[] | select(
        .path == "/etc/bird/vpsman-ospf.conf"
        and .changed == true
        and (.backup_path != null)
        and (.sha256_hex | test("^[0-9a-f]{64}$"))
      )] | length == 1)
      and .runtime_reconcile.status == "converged"
      and .runtime_reconcile.manager == "external_managed_adapter"
      and .runtime_reconcile.unprivileged_mutation_policy == "try_external_adapters"
      and .runtime_reconcile.compensation == null
      and ([.runtime_reconcile.commands[].label] | index("runtime_adapter_startup"))
      and ([.runtime_reconcile.commands[].label] | index("runtime_adapter_traffic_limit"))
      and ([.runtime_reconcile.commands[].label] | index("runtime_adapter_status"))
      and (.runtime_reconcile.commands | all(.success == true))
      and .routing_gate.status == "ready"
      and .routing_gate.runtime_status == "converged"
      and (.validation | length == 0)
      and (.reload | length == 0)
  ' "$adapter_apply_status_file" >/dev/null
}

assert_adapter_rollback_evidence() {
  assert_job_completed "$adapter_rollback_job_id" "network_rollback"
  status_output_for_job "$adapter_rollback_job_id" "$adapter_rollback_status_file"
  jq -e --arg client "$client_id" --arg peer "$peer_client_id" '
    .type == "network_rollback"
      and .plan == "live-adapter"
      and .interface == "ovpnlive0"
      and .side == "left"
      and .client_id == $client
      and .peer_client_id == $peer
      and .config_backend == "ifupdown"
      and .changed == true
      and ([.removed_files[] | select(
        .path == "/etc/bird/vpsman-ospf.conf"
        and .changed == true
        and (.backup_path != null)
        and (.sha256_hex | test("^[0-9a-f]{64}$"))
      )] | length == 1)
      and .runtime_remove.status == "removed"
      and .runtime_remove.manager == "external_managed_adapter"
      and .runtime_remove.unprivileged_mutation_policy == "try_external_adapters"
      and ([.runtime_remove.commands[].label] | index("runtime_adapter_stop"))
      and ([.runtime_remove.commands[].label] | index("runtime_adapter_cleanup"))
      and ([.runtime_remove.commands[].label] | index("runtime_adapter_status"))
      and (.runtime_remove.commands | all(.success == true))
      and (.validation | length == 0)
      and (.reload | length == 0)
  ' "$adapter_rollback_status_file" >/dev/null
}

assert_adapter_log_evidence() {
  grep -F 'start ovpnlive0' "$adapter_log" >/dev/null
  grep -F 'shape ovpnlive0 10000 5000' "$adapter_log" >/dev/null
  grep -F 'status live-adapter ovpnlive0' "$adapter_log" >/dev/null
  grep -F 'stop ovpnlive0' "$adapter_log" >/dev/null
  grep -F 'cleanup ovpnlive0' "$adapter_log" >/dev/null
}

assert_adapter_tunnel_plan_after_apply() {
  local plans_json
  plans_json="$(api_get "/api/v1/tunnel-plans")"
  if ! jq -e \
    --arg plan_id "$adapter_plan_id" \
    --arg apply_job_id "$adapter_apply_job_id" '
      .[] | select(.id == $plan_id)
      | .name == "live-adapter"
        and .left_status == "applied"
        and .right_status == "planned"
        and .status == "partially_applied"
        and .last_apply_job_id == $apply_job_id
        and .last_rollback_job_id == null
    ' <<<"$plans_json" >/dev/null; then
    echo "unexpected adapter tunnel plan state after apply" >&2
    printf '%s\n' "$plans_json" >&2
    exit 1
  fi
}

assert_adapter_tunnel_plan_after_rollback() {
  local plans_json
  plans_json="$(api_get "/api/v1/tunnel-plans")"
  if ! jq -e \
    --arg plan_id "$adapter_plan_id" \
    --arg apply_job_id "$adapter_apply_job_id" \
    --arg rollback_job_id "$adapter_rollback_job_id" '
      .[] | select(.id == $plan_id)
      | .name == "live-adapter"
        and .left_status == "rolled_back"
        and .right_status == "planned"
        and .status == "partially_rolled_back"
        and .last_apply_job_id == $apply_job_id
        and .last_rollback_job_id == $rollback_job_id
    ' <<<"$plans_json" >/dev/null; then
    echo "unexpected adapter tunnel plan state after rollback" >&2
    printf '%s\n' "$plans_json" >&2
    exit 1
  fi
}

assert_tunnel_plan_after_apply() {
  local plans_json
  plans_json="$(api_get "/api/v1/tunnel-plans")"
  if ! jq -e \
    --arg apply_job_id "$apply_job_id" '
      .[] | select(.name == "live-apply")
      | .left_status == "applied"
        and .right_status == "planned"
        and .status == "partially_applied"
        and .last_apply_job_id == $apply_job_id
        and .last_rollback_job_id == null
    ' <<<"$plans_json" >/dev/null; then
    echo "unexpected tunnel plan state after apply" >&2
    printf '%s\n' "$plans_json" >&2
    exit 1
  fi
}

assert_tunnel_plan_after_rollback() {
  local plans_json
  plans_json="$(api_get "/api/v1/tunnel-plans")"
  if ! jq -e \
    --arg apply_job_id "$apply_job_id" \
    --arg rollback_job_id "$rollback_job_id" '
      .[] | select(.name == "live-apply")
      | .left_status == "rolled_back"
        and .right_status == "planned"
        and .status == "partially_rolled_back"
        and .last_apply_job_id == $apply_job_id
        and .last_rollback_job_id == $rollback_job_id
    ' <<<"$plans_json" >/dev/null; then
    echo "unexpected tunnel plan state after rollback" >&2
    printf '%s\n' "$plans_json" >&2
    exit 1
  fi
}

assert_managed_blocks_removed() {
  grep -q '# unmanaged ifupdown' "$ifupdown_file"
  grep -q '# unmanaged bird2' "$bird2_file"
  if grep -F -e 'vpsman-managed' -e 'vpsman tunnel live-apply' "$ifupdown_file" >/dev/null; then
    echo "ifupdown managed block remained after rollback" >&2
    exit 1
  fi
  if grep -F -e 'vpsman-managed' -e 'vpsman GRE tunnel live-apply' "$bird2_file" >/dev/null; then
    echo "bird2 managed block remained after rollback" >&2
    exit 1
  fi
}

start_api "first"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d '{"username":"network-apply-smoke","password":"network-apply-smoke-password"}' \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
export VPSMAN_API_TOKEN="$access_token"
jq -e '.operator.username == "network-apply-smoke" and .token_type == "Bearer"' \
  <<<"$auth_json" >/dev/null

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="network-apply-smoke-gateway" \
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
  "bgp,network-apply-smoke" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"
smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$peer_agent_config" \
  "$peer_client_id" \
  "$peer_client_id" \
  "bgp,network-apply-smoke" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"

enable_network_apply_config() {
  local config_path="$1"
  if grep -q '^apply_enabled = ' "$config_path"; then
    sed -i \
      -e 's/^apply_enabled = .*/apply_enabled = true/' \
      -e "s|^root_dir = .*|root_dir = \"$network_root\"|" \
      -e 's/^validate_enabled = .*/validate_enabled = false/' \
      -e 's/^reload_enabled = .*/reload_enabled = false/' \
      "$config_path"
  else
    cat >>"$config_path" <<TOML

[network]
apply_enabled = true
root_dir = "$network_root"
validate_enabled = false
reload_enabled = false
TOML
  fi
}

enable_network_apply_config "$agent_config"
enable_network_apply_config "$peer_agent_config"
grep -q 'apply_enabled = true' "$agent_config"
grep -q "root_dir = \"$network_root\"" "$agent_config"

target/debug/vpsctl --api-url "$api_url" tunnel-plan \
  --name live-apply \
  --interface-name vpsap0 \
  --kind gre \
  --left-client-id "$client_id" \
  --right-client-id "$peer_client_id" \
  --left-underlay 192.0.2.10 \
  --right-underlay 192.0.2.20 \
  --address-pool-cidr 10.255.9.0/30 \
  --left-tunnel-ipv4 10.255.9.0 \
  --right-tunnel-ipv4 10.255.9.1 \
  --bandwidth 100m \
  --latency-ms 5 >"$network_plan_file"

VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-plan \
    --name live-apply \
    --interface-name vpsap0 \
    --kind gre \
    --left-client-id "$client_id" \
    --right-client-id "$peer_client_id" \
    --left-underlay 192.0.2.10 \
    --right-underlay 192.0.2.20 \
    --address-pool-cidr 10.255.9.0/30 \
    --left-tunnel-ipv4 10.255.9.0 \
    --right-tunnel-ipv4 10.255.9.1 \
    --bandwidth 100m \
    --latency-ms 5 \
    --save \
  | jq -e '
      .name == "live-apply"
      and .left_status == "planned"
      and .right_status == "planned"
      and .status == "planned"
    ' >/dev/null

ifupdown_snippet="$(jq -r '.ifupdown_snippet' "$network_plan_file")"
bird2_snippet="$(jq -r '.bird2_interface_snippet' "$network_plan_file")"
ifupdown_sha="$(printf '%s' "$ifupdown_snippet" | sha256sum | awk '{print $1}')"
bird2_sha="$(printf '%s' "$bird2_snippet" | sha256sum | awk '{print $1}')"

start_agent "managed-files"

reject_body="$(jq -nc \
  --arg client "$client_id" \
  --slurpfile plan "$network_plan_file" \
  --arg ifupdown_sha "$ifupdown_sha" \
  --arg bird2_sha "$bird2_sha" \
  '{
    command: "network_apply",
    operation: {
      type: "network_apply",
      plan: $plan[0],
      side: "left",
      ifupdown_sha256_hex: $ifupdown_sha,
      bird2_sha256_hex: $bird2_sha
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
  echo "expected no-privilege-unlock network apply to return 403, got $reject_status" >&2
  cat "$reject_json" >&2 || true
  exit 1
fi
jq -e '.error == "privilege_assertion_required" and .status == 403' "$reject_json" >/dev/null
cmp -s "$expected_ifupdown" "$ifupdown_file"
cmp -s "$expected_bird2" "$bird2_file"

apply_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-apply \
    --plan-file "$network_plan_file" \
    --side left \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --force-unprivileged \
    --confirmed)"
apply_job_id="$(jq -r '.job_id' <<<"$apply_json")"
smoke_assert_job_create_queued "$apply_json" 1
smoke_wait_api_job_status "$api_url" "$apply_job_id" completed 45 >/dev/null

grep -q "# vpsman-managed ifupdown begin $client_id $peer_client_id live-apply vpsap0" "$ifupdown_file"
grep -q "# vpsman-managed bird2 begin $client_id $peer_client_id live-apply vpsap0" "$bird2_file"
grep -q '# unmanaged ifupdown' "$ifupdown_file"
grep -q '# unmanaged bird2' "$bird2_file"
assert_apply_evidence
assert_tunnel_plan_after_apply

rollback_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-rollback \
    --plan-file "$network_plan_file" \
    --side left \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --force-unprivileged \
    --confirmed)"
rollback_job_id="$(jq -r '.job_id' <<<"$rollback_json")"
smoke_assert_job_create_queued "$rollback_json" 1
smoke_wait_api_job_status "$api_url" "$rollback_job_id" completed 45 >/dev/null

assert_managed_blocks_removed
assert_rollback_evidence
assert_tunnel_plan_after_rollback
assert_outputs_redacted
assert_audit_evidence

stop_agent
if grep -q '^runtime_reconcile_enabled = ' "$agent_config"; then
  sed -i \
    -e 's/^runtime_reconcile_enabled = .*/runtime_reconcile_enabled = true/' \
    -e 's/^runtime_unprivileged_mutation_policy = .*/runtime_unprivileged_mutation_policy = "try_external_adapters"/' \
    "$agent_config"
else
  cat >>"$agent_config" <<TOML
runtime_reconcile_enabled = true
runtime_unprivileged_mutation_policy = "try_external_adapters"
TOML
fi
grep -q 'runtime_reconcile_enabled = true' "$agent_config"
grep -q 'runtime_unprivileged_mutation_policy = "try_external_adapters"' "$agent_config"
start_agent "runtime-adapter"

observed_json="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-plan \
    --name live-observed \
    --interface-name ovpnlive0 \
    --kind openvpn \
    --runtime-manager external_observed \
    --left-client-id "$client_id" \
    --right-client-id "$peer_client_id" \
    --left-underlay 192.0.2.10 \
    --right-underlay 192.0.2.20 \
    --address-pool-cidr 10.255.10.0/30 \
    --left-tunnel-ipv4 10.255.10.0 \
    --right-tunnel-ipv4 10.255.10.1 \
    --bandwidth 100m \
    --latency-ms 5 \
    --save)"
adapter_plan_id="$(jq -r '.id' <<<"$observed_json")"
jq -e '
  .name == "live-observed"
    and .plan.runtime_control.manager == "external_observed"
    and .left_status == "planned"
    and .status == "planned"
' <<<"$observed_json" >/dev/null

promoted_json="$(VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-promote-adapter \
    --plan-id "$adapter_plan_id" \
    --name live-adapter \
    --runtime-startup-argv "$adapter_script,$adapter_log,start,{interface},{local_address},{remote_address}" \
    --runtime-status-argv "$adapter_script,$adapter_log,status,{plan},{interface}" \
    --runtime-stop-argv "$adapter_script,$adapter_log,stop,{interface}" \
    --runtime-cleanup-argv "$adapter_script,$adapter_log,cleanup,{interface}" \
    --runtime-traffic-limit-argv "$adapter_script,$adapter_log,shape,{interface},{egress_kbps},{ingress_kbps}" \
    --traffic-egress-kbps 10000 \
    --traffic-ingress-kbps 5000 \
    --traffic-burst-kb 64 \
    --topology-version live-adapter-v1 \
    --topology-desired-interfaces ovpnlive0 \
    --confirmed)"
jq -e --arg plan_id "$adapter_plan_id" '
  .id == $plan_id
    and .name == "live-adapter"
    and .plan.runtime_control.manager == "external_managed_adapter"
    and .plan.runtime_control.status.argv[0] != null
' <<<"$promoted_json" >/dev/null
jq '.plan' <<<"$promoted_json" >"$adapter_plan_file"
assert_adapter_promotion_evidence

adapter_apply_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-apply \
    --plan-file "$adapter_plan_file" \
    --side left \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --force-unprivileged \
    --confirmed)"
adapter_apply_job_id="$(jq -r '.job_id' <<<"$adapter_apply_json")"
smoke_assert_job_create_queued "$adapter_apply_json" 1
smoke_wait_api_job_status "$api_url" "$adapter_apply_job_id" completed 45 >/dev/null
assert_adapter_apply_evidence
assert_adapter_tunnel_plan_after_apply

adapter_rollback_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
VPSMAN_API_TOKEN="$access_token" \
  target/debug/vpsctl --api-url "$api_url" tunnel-rollback \
    --plan-file "$adapter_plan_file" \
    --side left \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 10 \
    --force-unprivileged \
    --confirmed)"
adapter_rollback_job_id="$(jq -r '.job_id' <<<"$adapter_rollback_json")"
smoke_assert_job_create_queued "$adapter_rollback_json" 1
smoke_wait_api_job_status "$api_url" "$adapter_rollback_job_id" completed 45 >/dev/null
assert_adapter_rollback_evidence
assert_adapter_tunnel_plan_after_rollback
assert_adapter_log_evidence
assert_outputs_redacted
assert_audit_evidence

stop_api
start_api "restart"
api_get "/api/v1/auth/me" | jq -e '.username == "network-apply-smoke"' >/dev/null
assert_apply_evidence
assert_rollback_evidence
assert_tunnel_plan_after_rollback
assert_adapter_promotion_evidence
assert_adapter_apply_evidence
assert_adapter_rollback_evidence
assert_adapter_tunnel_plan_after_rollback
assert_adapter_log_evidence
assert_outputs_redacted
assert_audit_evidence

jq -n \
  --arg client_id "$client_id" \
  --arg peer_client_id "$peer_client_id" \
  --arg apply_job_id "$apply_job_id" \
  --arg rollback_job_id "$rollback_job_id" \
  --arg adapter_plan_id "$adapter_plan_id" \
  --arg adapter_apply_job_id "$adapter_apply_job_id" \
  --arg adapter_rollback_job_id "$adapter_rollback_job_id" \
  '{
    live_network_apply_smoke: "ok",
    postgres_backed: true,
    auth_session: "persisted",
    api_restart: "verified",
    no_privilege_unlock_rejected: true,
    tunnel_plan_endpoint_state: "verified",
    external_adapter_runtime: "verified",
    adapter_promotion: "verified",
    unprivileged_adapter_policy: "try_external_adapters",
    sandboxed_root: true,
    client_id: $client_id,
    peer_client_id: $peer_client_id,
    apply_job_id: $apply_job_id,
    rollback_job_id: $rollback_job_id,
    adapter_plan_id: $adapter_plan_id,
    adapter_apply_job_id: $adapter_apply_job_id,
    adapter_rollback_job_id: $adapter_rollback_job_id
  }'
