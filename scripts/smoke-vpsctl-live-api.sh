#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash cargo curl docker jq python3 shuf timeout

fail() {
  echo "$1" >&2
  if [[ -n "${api_log:-}" && -f "$api_log" ]]; then
    smoke_dump_logs "vpsctl live API smoke failed" "$api_log" "${gateway_log:-}"
  fi
  exit 1
}

on_error() {
  local status=$?
  local line="${1:-unknown}"
  echo "vpsctl live API smoke failed at line $line (exit $status)" >&2
  if [[ -n "${api_log:-}" && -f "$api_log" ]]; then
    smoke_dump_logs "vpsctl live API smoke failed" "$api_log" "${gateway_log:-}"
  fi
  exit "$status"
}
trap 'on_error "$LINENO"' ERR

if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsman-api -p vpsman-gateway -p vpsctl >/dev/null
fi

bin="${VPSMAN_VPSCTL_BIN:-target/debug/vpsctl}"
if [[ ! -x "$bin" ]]; then
  fail "vpsctl binary is not executable: $bin"
fi

smoke_init_tmpdir "vpsman-vpsctl-live-api"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
api_url="http://127.0.0.1:$api_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
container_name="vpsman-vpsctl-live-api-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
internal_token="vpsctl-live-api-internal-$(date +%s%N)"
api_log="$SMOKE_TMPDIR/api.log"
gateway_log="$SMOKE_TMPDIR/gateway.log"
api_pid=""
gateway_pid=""
operator_password="vpsctl-live-api-password"
viewer_password="vpsctl-live-api-viewer-password"
scoped_password="vpsctl-live-api-scoped-password"
super_password="vpsctl-live-api-super-password"
super_salt_hex="1111111111111111111111111111111111111111111111111111111111111111"
cli_agent_a_public_hex="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa1"
cli_agent_b_public_hex="bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb2"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"
gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"

cleanup_vpsctl_live_api() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_vpsctl_live_api EXIT

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
    docker logs "$container_name" >&2 || true
    fail "timed out waiting for Postgres container"
  fi
  sleep 0.25
done
smoke_wait_tcp 127.0.0.1 "$pg_port"

start_api() {
  local attempt=0
  local deadline=$((SECONDS + 45))
  while (( SECONDS < deadline )); do
    attempt=$((attempt + 1))
    api_log="$SMOKE_TMPDIR/api-$attempt.log"
    VPSMAN_API_BIND="127.0.0.1:$api_port" \
    VPSMAN_POSTGRES_URL="$postgres_url" \
    VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
    VPSMAN_INTERNAL_TOKEN="$internal_token" \
    VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
    VPSMAN_BACKUP_OBJECT_STORE_DIR="$SMOKE_TMPDIR/object-store" \
    VPSMAN_DEV_ALLOW_LOOPBACK_WEBHOOKS=1 \
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
        kill "$api_pid" >/dev/null 2>&1 || true
        wait "$api_pid" >/dev/null 2>&1 || true
        api_pid=""
        break
      fi
      sleep 0.1
    done
    if curl -fsS "$api_url/health" >/dev/null 2>&1; then
      return
    fi
    sleep 0.5
  done
  fail "API did not become healthy for vpsctl live API smoke"
}

start_gateway() {
  VPSMAN_GATEWAY_BIND="127.0.0.1:$gateway_port" \
  VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
  VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
  VPSMAN_API_URL="$api_url" \
  VPSMAN_INTERNAL_TOKEN="$internal_token" \
  VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
  VPSMAN_GATEWAY_SPOOL_DIR="$SMOKE_TMPDIR/gateway-spool" \
  RUST_LOG="vpsman_gateway=warn" \
    target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
  gateway_pid="$!"
  smoke_track_pid "$gateway_pid"
  if ! smoke_wait_tcp 127.0.0.1 "$gateway_control_port"; then
    fail "gateway did not become healthy for vpsctl live API smoke"
  fi
}

start_api
start_gateway

vpsctl_public() {
  VPSMAN_API_URL="$api_url" "$bin" "$@"
}

vpsctl_auth() {
  VPSMAN_API_URL="$api_url" \
  VPSMAN_API_TOKEN="$access_token" \
  VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
    "$bin" "$@"
}

require_no_secret() {
  local text="$1"
  local secret="$2"
  local label="$3"
  if [[ "$text" == *"$secret"* ]]; then
    fail "$label leaked secret material"
  fi
}

seed_agent() {
  local client_id="$1"
  local process_incarnation_id="11111111-1111-4111-8111-111111111111"
  local gateway_session_id
  local noise_public_key_hex
  case "$client_id" in
    cli-agent-a)
      gateway_session_id="11111111-1111-4111-8111-11111111111a"
      noise_public_key_hex="$cli_agent_a_public_hex"
      ;;
    cli-agent-b)
      gateway_session_id="11111111-1111-4111-8111-11111111111b"
      noise_public_key_hex="$cli_agent_b_public_hex"
      ;;
    *)
      gateway_session_id="11111111-1111-4111-8111-11111111111f"
      noise_public_key_hex="fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff3"
      ;;
  esac
  local optional_hello_fields=""
  if [[ $# -ge 2 && -n "$2" ]]; then
    optional_hello_fields=", \"capabilities\": $2"
  fi
  curl -fsS \
    -H "Authorization: Bearer $internal_token" \
    -H "Content-Type: application/json" \
    -d "{
      \"gateway_id\": \"vpsctl-live-api-gateway\",
      \"gateway_session_id\": \"$gateway_session_id\",
      \"noise_public_key_hex\": \"$noise_public_key_hex\",
      \"hello\": {
        \"client_id\": \"$client_id\",
        \"process_incarnation_id\": \"$process_incarnation_id\",
        \"agent_version\": \"vpsctl-live-api-smoke\",
        \"os_release\": \"Debian smoke\",
        \"arch\": \"x86_64\"$optional_hello_fields
      }
    }" \
    "$api_url/internal/v1/gateway/agent-hello" >/dev/null
}

seed_tunnel_telemetry() {
  local client_id="$1"
  local endpoint_side="$2"
  local peer_client_id="$3"
  local gateway_session_id
  local observed_unix
  case "$client_id" in
    cli-agent-a) gateway_session_id="11111111-1111-4111-8111-11111111111a" ;;
    cli-agent-b) gateway_session_id="11111111-1111-4111-8111-11111111111b" ;;
    *) gateway_session_id="11111111-1111-4111-8111-11111111111f" ;;
  esac
  observed_unix="$(date +%s)"
  curl -fsS \
    -H "Authorization: Bearer $internal_token" \
    -H "Content-Type: application/json" \
    -d "{
      \"gateway_id\": \"vpsctl-live-api-gateway\",
      \"gateway_session_id\": \"$gateway_session_id\",
      \"process_incarnation_id\": \"11111111-1111-4111-8111-111111111111\",
      \"telemetry_seq\": 1,
      \"telemetry\": {
        \"client_id\": \"$client_id\",
        \"metrics\": {
          \"observed_unix\": $observed_unix,
          \"hostname\": \"$client_id\",
          \"uptime_secs\": 3600,
          \"cpu\": {\"load\": {\"one\": 0.1, \"five\": 0.1, \"fifteen\": 0.1}, \"cores\": 1},
          \"memory\": {\"total_bytes\": 268435456, \"available_bytes\": 134217728},
          \"disks\": [],
          \"networks\": [],
          \"tunnels\": [{
            \"interface\": \"grecli\",
            \"kind\": \"gre\",
            \"ownership_mode\": \"agent_builtin\",
            \"mutation_policy\": \"managed_desired\",
            \"source\": \"approved_runtime_status_telemetry\",
            \"rx_bytes\": 0,
            \"tx_bytes\": 0,
            \"traffic_source\": \"interface_counters\",
            \"traffic_status\": \"missing\",
            \"traffic_reason\": \"grecli_not_found\",
            \"traffic_checked_unix\": $observed_unix,
            \"plan_id\": \"$plan_id\",
            \"plan_name\": \"cli-gre-a-b\",
            \"plan_runtime_manager\": \"agent_builtin\",
            \"endpoint_side\": \"$endpoint_side\",
            \"peer_client_id\": \"$peer_client_id\"
          }]
        }
      }
    }" \
    "$api_url/internal/v1/gateway/telemetry" >/dev/null
}

assign_agent_alias() {
  local client_id="$1"
  local display_name="$2"
  curl -fsS \
    -H "Authorization: Bearer $access_token" \
    -H "Content-Type: application/json" \
    -d "{\"display_name\":\"$display_name\",\"confirmed\":true}" \
    "$api_url/api/v1/agents/$client_id/alias" >/dev/null
}

assign_agent_tags() {
  local client_id="$1"
  shift
  local tag
  for tag in "$@"; do
    vpsctl_auth agent-tag --client-id "$client_id" --tag "$tag" --confirmed >/dev/null
  done
}

health_text="$(vpsctl_public health)"
if [[ "$health_text" != "ok" ]]; then
  fail "vpsctl health returned unexpected body: $health_text"
fi

bootstrap_json="$(VPSMAN_OPERATOR_PASSWORD="$operator_password" \
  vpsctl_public bootstrap --username vpsctl-smoke --password-env VPSMAN_OPERATOR_PASSWORD)"
require_no_secret "$bootstrap_json" "$operator_password" "bootstrap"
access_token="$(jq -r '.access_token' <<<"$bootstrap_json")"
refresh_token="$(jq -r '.refresh_token' <<<"$bootstrap_json")"
jq -e '.operator.username == "vpsctl-smoke" and .token_type == "Bearer"' \
  <<<"$bootstrap_json" >/dev/null

login_json="$(VPSMAN_OPERATOR_PASSWORD="$operator_password" \
  vpsctl_public login --username vpsctl-smoke --password-env VPSMAN_OPERATOR_PASSWORD)"
require_no_secret "$login_json" "$operator_password" "login"
access_token="$(jq -r '.access_token' <<<"$login_json")"
refresh_token="$(jq -r '.refresh_token' <<<"$login_json")"

refresh_json="$(VPSMAN_REFRESH_TOKEN="$refresh_token" \
  vpsctl_public refresh --refresh-token-env VPSMAN_REFRESH_TOKEN)"
access_token="$(jq -r '.access_token' <<<"$refresh_json")"
jq -e '.operator.username == "vpsctl-smoke" and .token_type == "Bearer"' \
  <<<"$refresh_json" >/dev/null

me_json="$(vpsctl_auth me)"
jq -e '.username == "vpsctl-smoke" and .role == "admin" and (.scopes | index("*"))' <<<"$me_json" >/dev/null

viewer_json="$(VPSMAN_API_URL="$api_url" \
  VPSMAN_API_TOKEN="$access_token" \
  VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
  VPSMAN_NEW_OPERATOR_PASSWORD="$viewer_password" \
  "$bin" operator-create \
    --username vpsctl-viewer \
    --role viewer \
    --password-env VPSMAN_NEW_OPERATOR_PASSWORD \
    --confirmed)"
require_no_secret "$viewer_json" "$viewer_password" "operator-create"
jq -e '.username == "vpsctl-viewer" and .role == "viewer" and (.scopes == ["fleet:read"]) and (.id | length) > 0' \
  <<<"$viewer_json" >/dev/null
operators_json="$(vpsctl_auth operators)"
require_no_secret "$operators_json" "$viewer_password" "operators"
jq -e 'length == 2 and any(.[]; .username == "vpsctl-smoke" and .role == "admin" and (.scopes | index("*"))) and any(.[]; .username == "vpsctl-viewer" and .role == "viewer" and (.scopes == ["fleet:read"]))' \
  <<<"$operators_json" >/dev/null
viewer_login_json="$(VPSMAN_API_URL="$api_url" \
  VPSMAN_OPERATOR_PASSWORD="$viewer_password" \
  "$bin" login --username vpsctl-viewer --password-env VPSMAN_OPERATOR_PASSWORD)"
require_no_secret "$viewer_login_json" "$viewer_password" "viewer login"
viewer_access_token="$(jq -r '.access_token' <<<"$viewer_login_json")"
viewer_job_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $viewer_access_token" \
  -H "Content-Type: application/json" \
  -d '{"command":"/bin/true","argv":[],"selector_expression":"id:cli-agent-a","target_client_ids":["cli-agent-a"],"privileged":true,"destructive":false,"confirmed":true,"max_timeout_secs":1}' \
  "$api_url/api/v1/jobs")"
viewer_job_status="${viewer_job_response##*$'\n'}"
viewer_job_body="${viewer_job_response%$'\n'*}"
if [[ "$viewer_job_status" != "403" ]]; then
  fail "viewer job creation returned HTTP $viewer_job_status: $viewer_job_body"
fi
jq -e '.error == "operator_role_insufficient"' <<<"$viewer_job_body" >/dev/null
operator_sessions_json="$(vpsctl_auth operator-sessions --limit 10)"
viewer_session_id="$(jq -r 'map(select(.operator_username == "vpsctl-viewer" and .revoked == false)) | first | .id' <<<"$operator_sessions_json")"
if [[ -z "$viewer_session_id" || "$viewer_session_id" == "null" ]]; then
  fail "operator-sessions did not include active viewer session: $operator_sessions_json"
fi
revoked_session_json="$(vpsctl_auth operator-session-revoke --session-id "$viewer_session_id" --confirmed)"
jq -e --arg session_id "$viewer_session_id" '.id == $session_id and .revoked == true' \
  <<<"$revoked_session_json" >/dev/null
viewer_me_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $viewer_access_token" \
  "$api_url/api/v1/auth/me")"
viewer_me_status="${viewer_me_response##*$'\n'}"
viewer_me_body="${viewer_me_response%$'\n'*}"
if [[ "$viewer_me_status" != "401" ]]; then
  fail "revoked viewer session returned HTTP $viewer_me_status: $viewer_me_body"
fi
jq -e '.error == "invalid_bearer_token"' <<<"$viewer_me_body" >/dev/null

scoped_json="$(VPSMAN_API_URL="$api_url" \
  VPSMAN_API_TOKEN="$access_token" \
  VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
  VPSMAN_NEW_OPERATOR_PASSWORD="$scoped_password" \
  "$bin" operator-create \
    --username vpsctl-fleet-reader \
    --role operator \
    --scopes fleet:read \
    --password-env VPSMAN_NEW_OPERATOR_PASSWORD \
    --confirmed)"
require_no_secret "$scoped_json" "$scoped_password" "operator-create scoped"
jq -e '.username == "vpsctl-fleet-reader" and .role == "operator" and (.scopes == ["fleet:read"])' \
  <<<"$scoped_json" >/dev/null
scoped_login_json="$(VPSMAN_API_URL="$api_url" \
  VPSMAN_OPERATOR_PASSWORD="$scoped_password" \
  "$bin" login --username vpsctl-fleet-reader --password-env VPSMAN_OPERATOR_PASSWORD)"
require_no_secret "$scoped_login_json" "$scoped_password" "scoped login"
scoped_access_token="$(jq -r '.access_token' <<<"$scoped_login_json")"
scoped_summary_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  "$api_url/api/v1/fleet/summary")"
scoped_summary_status="${scoped_summary_response##*$'\n'}"
scoped_summary_body="${scoped_summary_response%$'\n'*}"
if [[ "$scoped_summary_status" != "200" ]]; then
  fail "fleet:read scoped summary returned HTTP $scoped_summary_status: $scoped_summary_body"
fi
scoped_alerts_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  "$api_url/api/v1/fleet-alerts?limit=5")"
scoped_alerts_status="${scoped_alerts_response##*$'\n'}"
scoped_alerts_body="${scoped_alerts_response%$'\n'*}"
if [[ "$scoped_alerts_status" != "403" ]]; then
  fail "fleet:read-only scoped fleet-alerts returned HTTP $scoped_alerts_status: $scoped_alerts_body"
fi
jq -e '.error == "operator_scope_insufficient"' <<<"$scoped_alerts_body" >/dev/null
scoped_tag_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  -H "Content-Type: application/json" \
  -d '{"name":"scope-denied-tag"}' \
  "$api_url/api/v1/tags")"
scoped_tag_status="${scoped_tag_response##*$'\n'}"
scoped_tag_body="${scoped_tag_response%$'\n'*}"
if [[ "$scoped_tag_status" != "403" ]]; then
  fail "fleet:read scoped tag create returned HTTP $scoped_tag_status: $scoped_tag_body"
fi
jq -e '.error == "operator_scope_insufficient"' <<<"$scoped_tag_body" >/dev/null
scoped_job_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  -H "Content-Type: application/json" \
  -d '{"command":"/bin/true","argv":[],"selector_expression":"id:cli-agent-a","target_client_ids":["cli-agent-a"],"privileged":true,"destructive":false,"confirmed":true,"max_timeout_secs":1}' \
  "$api_url/api/v1/jobs")"
scoped_job_status="${scoped_job_response##*$'\n'}"
scoped_job_body="${scoped_job_response%$'\n'*}"
if [[ "$scoped_job_status" != "403" ]]; then
  fail "fleet:read scoped job creation returned HTTP $scoped_job_status: $scoped_job_body"
fi
jq -e '.error == "operator_scope_insufficient"' <<<"$scoped_job_body" >/dev/null

identity_keys="$(vpsctl_public noise-keygen)"
identity_public_hex="$(jq -r '.public_key_hex' <<<"$identity_keys")"
identity_json="$(vpsctl_auth agent-identity-upsert \
  --client-id cli-direct-agent \
  --client-public-key-hex "$identity_public_hex" \
  --display-name cli-direct-agent \
  --tags edge,bgp \
  --confirmed)"
jq -e \
  '.identity.client_id == "cli-direct-agent" and
   .identity.display_name == "cli-direct-agent" and
   (.identity.tags | sort == ["bgp", "edge"]) and
   any(.post_commit[]; .operation == "job_terminal_reconciliation" and (.status == "completed" or .status == "failed"))' \
  <<<"$identity_json" >/dev/null
key_report_json="$(vpsctl_auth key-lifecycle-report)"
jq -e '.direct_identity_client_count >= 1' <<<"$key_report_json" >/dev/null

root_capabilities='{"privilege_mode":"root","effective_uid":0,"can_attempt_privileged_ops":true,"can_manage_runtime_tunnels":true,"can_apply_process_limits":true}'
unprivileged_capabilities='{"privilege_mode":"unprivileged","effective_uid":1000,"can_attempt_privileged_ops":true,"can_manage_runtime_tunnels":false,"can_apply_process_limits":false,"unprivileged_hint":"vpsctl smoke agent is running without root"}'
vpsctl_auth agent-identity-upsert \
  --client-id cli-agent-a \
  --client-public-key-hex "$cli_agent_a_public_hex" \
  --display-name cli-agent-a \
  --confirmed >/dev/null
vpsctl_auth agent-identity-upsert \
  --client-id cli-agent-b \
  --client-public-key-hex "$cli_agent_b_public_hex" \
  --display-name cli-agent-b \
  --confirmed >/dev/null
seed_agent "cli-agent-a" "$root_capabilities"
seed_agent "cli-agent-b" "$unprivileged_capabilities"
assign_agent_alias "cli-agent-a" "cli-edge-a"
assign_agent_alias "cli-agent-b" "cli-edge-b"
assign_agent_tags "cli-agent-a" edge bgp
assign_agent_tags "cli-agent-b" edge routing

summary_json="$(vpsctl_auth summary)"
jq -e '.total >= 2 and .online == 2' <<<"$summary_json" >/dev/null
agents_json="$(vpsctl_auth agents)"
jq -e '
  (map(select(.status == "online")) | length) == 2 and
  any(.[]; .id == "cli-agent-a" and .status == "online" and .capabilities.privilege_mode == "root") and
  any(.[]; .id == "cli-agent-b" and .status == "online" and .capabilities.privilege_mode == "unprivileged" and .capabilities.can_apply_process_limits == false)
' \
  <<<"$agents_json" >/dev/null
configuration_sources_json="$(vpsctl_auth config-sources)"
jq -e '
  map(select(.client_id == "cli-agent-a" or .client_id == "cli-agent-b")) as $live |
  ($live | length) == 12 and
  ([ $live[].behavior ] | unique | sort) ==
    ["command_execution", "host_metrics", "latency_probe", "ospf_update_command", "process_inventory", "user_sessions"] and
  all($live[];
    .effective_preset_kind == "system" and
    .selection_origin == "system_default" and
    .override_updated_at == null and
    .runtime_sync.state == "unknown" and
    (if .behavior == "ospf_update_command"
     then .readiness.state == "unconfigured" and .readiness.evidence.command_configured == false
     else .readiness.state == "unverified"
     end))
' \
  <<<"$configuration_sources_json" >/dev/null
plan_json="$(vpsctl_auth tunnel-plan \
  --name cli-gre-a-b \
  --interface-name grecli \
  --kind gre \
  --left-client-id cli-agent-a \
  --right-client-id cli-agent-b \
  --left-remote-underlay 203.0.113.201 \
  --right-remote-underlay 203.0.113.202 \
  --address-pool-cidr 10.252.0.0/30 \
  --left-tunnel-ipv4-cidr 10.252.0.0/31 \
  --right-tunnel-ipv4-cidr 10.252.0.1/31 \
  --bandwidth-mbps 100 \
  --save \
  --confirmed)"
jq -e '.plan.name == "cli-gre-a-b" and .plan.revision == 1 and .plan.enabled == false and .plan.plan.kind == "gre" and (.plan.plan | has("mutates_host") | not) and .plan.plan.recommended_ospf_cost == null and .sync == []' \
  <<<"$plan_json" >/dev/null
plan_id="$(jq -r '.plan.id' <<<"$plan_json")"
plan_revision="$(jq -r '.plan.revision' <<<"$plan_json")"
enabled_plan_json="$(vpsctl_auth tunnel-plan-enable \
  --plan-id "$plan_id" \
  --expected-revision "$plan_revision" \
  --confirmed)"
jq -e '
  .plan.enabled == true and
  .plan.revision == 2 and
  ([.sync[].client_id] | sort == ["cli-agent-a", "cli-agent-b"]) and
  all(.sync[]; .status == "queued" and .job_id != null and .error == null)
' <<<"$enabled_plan_json" >/dev/null
plan_revision="$(jq -r '.plan.revision' <<<"$enabled_plan_json")"
seed_tunnel_telemetry cli-agent-a left cli-agent-b
seed_tunnel_telemetry cli-agent-b right cli-agent-a
network_status_seed_job_json="$(vpsctl_auth tunnel-status \
  --plan-id "$plan_id" \
  --side left \
  --max-timeout-secs 5)"
network_status_seed_job_id="$(jq -r '.job_id' <<<"$network_status_seed_job_json")"
network_status_seed_payload_hash="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT payload_hash FROM jobs WHERE id = '$network_status_seed_job_id'")"
network_status_seed_data="$(python3 -c 'import json,sys; print(json.dumps(list(json.dumps({
  "type": "network_status",
  "plan": "cli-gre-a-b",
  "interface": "grecli",
  "peer_client_id": "cli-agent-b",
  "scope": "declared_plan_only",
  "runtime": {
    "manager": "agent_builtin",
    "summary": {
      "healthy": False,
      "state": "declared_interface_missing",
      "message": "Declared GRE interface is missing"
    },
    "interface": {"exists": False, "operstate": None},
    "desired_interfaces": [{"interface": "grecli", "exists": False}],
    "declared_stale_interfaces": [],
    "adapter": None
  }
}).encode())))')"
curl -fsS \
  -H "Authorization: Bearer $internal_token" \
  -H "Content-Type: application/json" \
  -d "{
    \"gateway_id\": \"vpsctl-live-api-gateway\",
    \"gateway_session_id\": \"11111111-1111-4111-8111-11111111111a\",
    \"process_incarnation_id\": \"11111111-1111-4111-8111-111111111111\",
    \"client_id\": \"cli-agent-a\",
    \"job_id\": \"$network_status_seed_job_id\",
    \"payload_hash\": \"$network_status_seed_payload_hash\",
    \"seq\": 0,
    \"received_unix\": $(date +%s),
    \"output\": {
      \"job_id\": \"$network_status_seed_job_id\",
      \"stream\": \"status\",
      \"data\": $network_status_seed_data,
      \"exit_code\": 0,
      \"done\": true
    }
  }" \
  "$api_url/internal/v1/gateway/command-output" >/dev/null
fleet_alerts_json="$(vpsctl_auth fleet-alerts --limit 20)"
jq -e '
  any(.[]; .category == "network" and .severity == "warning" and .status == "tunnel_traffic_degraded" and .client_id == "cli-agent-a") and
  any(.[]; .category == "network" and .severity == "warning" and .status == "tunnel_traffic_degraded" and .client_id == "cli-agent-b")
' <<<"$fleet_alerts_json" >/dev/null
fleet_alerts_filtered_json="$(vpsctl_auth fleet-alerts --client-id cli-agent-a --severity warning --limit 10)"
jq -e '
  length >= 1 and all(.[]; .client_id == "cli-agent-a" and .severity == "warning")
' <<<"$fleet_alerts_filtered_json" >/dev/null
alert_state_target_id="$(jq -r 'map(select(.category == "network" and .status == "tunnel_traffic_degraded" and .client_id == "cli-agent-a"))[0].id' <<<"$fleet_alerts_json")"
if [[ -z "$alert_state_target_id" || "$alert_state_target_id" == "null" ]]; then
  fail "failed to select network alert for state update"
fi
alert_state_json="$(vpsctl_auth fleet-alert-state-update \
  --alert-id "$alert_state_target_id" \
  --action mute \
  --muted-for-secs 600 \
  --reason live-smoke \
  --confirmed)"
jq -e '
  .alert_id == "'"$alert_state_target_id"'" and
  .state == "muted" and
  .muted_until_unix != null and
  .reason == "live-smoke"
' <<<"$alert_state_json" >/dev/null
alert_states_json="$(vpsctl_auth fleet-alert-states --state muted --limit 20)"
jq -e 'any(.[]; .alert_id == "'"$alert_state_target_id"'" and .state == "muted")' <<<"$alert_states_json" >/dev/null
muted_alerts_json="$(vpsctl_auth fleet-alerts --operator-state muted --include-muted --limit 20)"
jq -e '
  any(.[]; .id == "'"$alert_state_target_id"'" and .operator_state == "muted" and .state_reason == "live-smoke")
' <<<"$muted_alerts_json" >/dev/null
default_alerts_after_mute_json="$(vpsctl_auth fleet-alerts --limit 100)"
jq -e 'all(.[]; .id != "'"$alert_state_target_id"'")' <<<"$default_alerts_after_mute_json" >/dev/null
alert_export_json="$(vpsctl_auth fleet-alert-export --operator-state muted --include-muted --limit 20)"
jq -e '
  (.generated_at | type == "string") and
  any(.alerts[]; .id == "'"$alert_state_target_id"'" and .operator_state == "muted")
' <<<"$alert_export_json" >/dev/null
scoped_alert_states_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  "$api_url/api/v1/fleet-alert-states?limit=5")"
scoped_alert_states_status="${scoped_alert_states_response##*$'\n'}"
scoped_alert_states_body="${scoped_alert_states_response%$'\n'*}"
if [[ "$scoped_alert_states_status" != "200" ]]; then
  fail "fleet:read scoped fleet-alert-states returned HTTP $scoped_alert_states_status: $scoped_alert_states_body"
fi
scoped_alert_export_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  "$api_url/api/v1/fleet-alerts/export?limit=5&include_muted=true")"
scoped_alert_export_status="${scoped_alert_export_response##*$'\n'}"
scoped_alert_export_body="${scoped_alert_export_response%$'\n'*}"
if [[ "$scoped_alert_export_status" != "403" ]]; then
  fail "fleet:read-only scoped fleet-alert export returned HTTP $scoped_alert_export_status: $scoped_alert_export_body"
fi
jq -e '.error == "operator_scope_insufficient"' <<<"$scoped_alert_export_body" >/dev/null
scoped_alert_state_write_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  -H "Content-Type: application/json" \
  -d '{"alert_id":"agent_status:agent:denied","action":"acknowledge","confirmed":true}' \
  "$api_url/api/v1/fleet-alert-states")"
scoped_alert_state_write_status="${scoped_alert_state_write_response##*$'\n'}"
scoped_alert_state_write_body="${scoped_alert_state_write_response%$'\n'*}"
if [[ "$scoped_alert_state_write_status" != "403" ]]; then
  fail "fleet:read scoped fleet-alert-state write returned HTTP $scoped_alert_state_write_status: $scoped_alert_state_write_body"
fi
jq -e '.error == "operator_scope_insufficient"' <<<"$scoped_alert_state_write_body" >/dev/null
alert_state_clear_json="$(vpsctl_auth fleet-alert-state-update \
  --alert-id "$alert_state_target_id" \
  --action clear \
  --reason live-smoke-clear \
  --confirmed)"
jq -e '.alert_id == "'"$alert_state_target_id"'" and .state == "open"' <<<"$alert_state_clear_json" >/dev/null
alert_policy_json="$(vpsctl_auth alert-policy upsert \
  --name edge-resource-alerts \
  --selector 'tag:edge' \
  --rule 'cpu.load_1 >= 1.5' \
  --severity warning \
  --notes live-smoke \
  --confirmed)"
jq -e '
  .name == "edge-resource-alerts" and
  .selector_expression == "tag:edge" and
  .enabled == true and
  (.rules | length) == 1 and
  .rules[0].condition_expression == "cpu.load_1 >= 1.5"
' <<<"$alert_policy_json" >/dev/null
alert_policies_json="$(vpsctl_auth alert-policies list --selector 'tag:edge' --enabled true --limit 20)"
jq -e 'length == 1 and .[0].name == "edge-resource-alerts"' <<<"$alert_policies_json" >/dev/null
scoped_alert_policies_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  "$api_url/api/v1/fleet-alert-policies?limit=5")"
scoped_alert_policies_status="${scoped_alert_policies_response##*$'\n'}"
scoped_alert_policies_body="${scoped_alert_policies_response%$'\n'*}"
if [[ "$scoped_alert_policies_status" != "200" ]]; then
  fail "fleet:read fleet-alert-policies returned HTTP $scoped_alert_policies_status: $scoped_alert_policies_body"
fi
scoped_alert_policy_write_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  -H "Content-Type: application/json" \
  -d '{"name":"denied-alert-policy","enabled":true,"selector_expression":"tag:edge","rules":[{"name":"cpu","enabled":true,"traffic_selector":null,"condition_expression":"cpu.load_1 >= 1.0","window_secs":0,"severity":"warning"}],"confirmed":true}' \
  "$api_url/api/v1/fleet-alert-policies")"
scoped_alert_policy_write_status="${scoped_alert_policy_write_response##*$'\n'}"
scoped_alert_policy_write_body="${scoped_alert_policy_write_response%$'\n'*}"
if [[ "$scoped_alert_policy_write_status" != "403" ]]; then
  fail "fleet:read fleet-alert-policy write returned HTTP $scoped_alert_policy_write_status: $scoped_alert_policy_write_body"
fi
jq -e '.error == "operator_scope_insufficient"' <<<"$scoped_alert_policy_write_body" >/dev/null
alert_notification_channel_json="$(vpsctl_auth fleet-alert-notification-channel-upsert \
  --name network-webhook \
  --scope-kind global \
  --min-severity warning \
  --categories network \
  --operator-states open \
  --delivery-kind webhook \
  --target http://127.0.0.1:9/vpsman/network \
  --cooldown-secs 600 \
  --notes live-smoke \
  --confirmed)"
alert_notification_channel_id="$(jq -r '.id' <<<"$alert_notification_channel_json")"
jq -e '
  .name == "network-webhook" and
  .scope_kind == "global" and
  .min_severity == "warning" and
  .categories == ["network"] and
  .operator_states == ["open"] and
  .delivery_kind == "webhook" and
  .enabled == true
' <<<"$alert_notification_channel_json" >/dev/null
alert_notification_channels_json="$(vpsctl_auth fleet-alert-notification-channels --delivery-kind webhook --limit 20)"
jq -e 'any(.[]; .name == "network-webhook")' <<<"$alert_notification_channels_json" >/dev/null
alert_notification_backup_channel_json="$(vpsctl_auth fleet-alert-notification-channel-upsert \
  --name network-backup-webhook \
  --scope-kind global \
  --min-severity warning \
  --categories network \
  --operator-states open \
  --delivery-kind webhook \
  --target http://127.0.0.1:9/vpsman/network-backup \
  --cooldown-secs 600 \
  --notes live-smoke-backup \
  --confirmed)"
alert_notification_backup_channel_id="$(jq -r '.id' <<<"$alert_notification_backup_channel_json")"
jq -e '
  .name == "network-backup-webhook" and
  .delivery_kind == "webhook" and
  .enabled == true
' <<<"$alert_notification_backup_channel_json" >/dev/null
alert_notification_dry_run_json="$(vpsctl_auth fleet-alert-notification-dispatch \
  --category network \
  --include-muted \
  --dry-run \
  --limit 20)"
jq -e '
  length >= 1 and
  all(.[]; .status == "matched_dry_run") and
  any(.[]; .channel_name == "network-webhook" and .alert_category == "network")
' <<<"$alert_notification_dry_run_json" >/dev/null
alert_notification_dispatch_preview_hash="$(jq -r '.[0].review_preview_hash // empty' <<<"$alert_notification_dry_run_json")"
if [[ ! "$alert_notification_dispatch_preview_hash" =~ ^[0-9a-f]{64}$ ]]; then
  fail "fleet-alert-notification-dispatch dry run did not return a valid preview hash"
fi
alert_notification_dispatch_json="$(vpsctl_auth fleet-alert-notification-dispatch \
  --category network \
  --include-muted \
  --preview-hash "$alert_notification_dispatch_preview_hash" \
  --confirmed \
  --limit 20)"
jq -e '
  length >= 1 and
  any(.[]; .channel_name == "network-webhook" and .status == "queued" and .delivery_kind == "webhook")
' <<<"$alert_notification_dispatch_json" >/dev/null
jq -e '
  any(.[]; .channel_name == "network-backup-webhook" and .status == "queued" and .delivery_kind == "webhook")
' <<<"$alert_notification_dispatch_json" >/dev/null
alert_notifications_json="$(vpsctl_auth fleet-alert-notifications --status queued --limit 20)"
jq -e '
  any(.[]; .channel_id == "'"$alert_notification_channel_id"'" and .status == "queued")
' <<<"$alert_notifications_json" >/dev/null
alert_notification_process_dry_run_json="$(vpsctl_auth fleet-alert-notification-process \
  --status queued \
  --delivery-kind webhook \
  --dry-run \
  --limit 20)"
jq -e '
  length >= 1 and
  all(.[]; .status == "delivery_dry_run") and
  any(.[]; .channel_id == "'"$alert_notification_backup_channel_id"'")
' <<<"$alert_notification_process_dry_run_json" >/dev/null
alert_notification_process_preview_hash="$(jq -r '.[0].review_preview_hash // empty' <<<"$alert_notification_process_dry_run_json")"
if [[ ! "$alert_notification_process_preview_hash" =~ ^[0-9a-f]{64}$ ]]; then
  fail "fleet-alert-notification-process dry run did not return a valid preview hash"
fi
alert_notification_process_json="$(vpsctl_auth fleet-alert-notification-process \
  --status queued \
  --delivery-kind webhook \
  --preview-hash "$alert_notification_process_preview_hash" \
  --confirmed \
  --limit 20)"
jq -e '
  length >= 1 and
  any(.[]; .channel_id == "'"$alert_notification_backup_channel_id"'" and .status == "failed" and .attempt_count == 1 and (.error | type == "string" and length > 0))
' <<<"$alert_notification_process_json" >/dev/null
alert_notification_failed_json="$(vpsctl_auth fleet-alert-notifications --status failed --limit 20)"
jq -e '
  any(.[]; .channel_id == "'"$alert_notification_backup_channel_id"'" and .status == "failed" and .attempt_count == 1)
' <<<"$alert_notification_failed_json" >/dev/null
alert_notification_duplicate_json="$(vpsctl_auth fleet-alert-notification-dispatch \
  --category network \
  --include-muted \
  --preview-hash "$alert_notification_dispatch_preview_hash" \
  --confirmed \
  --limit 20)"
jq -e 'length == 0' <<<"$alert_notification_duplicate_json" >/dev/null
scoped_alert_notification_channels_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  "$api_url/api/v1/fleet-alert-notification-channels?limit=5")"
scoped_alert_notification_channels_status="${scoped_alert_notification_channels_response##*$'\n'}"
scoped_alert_notification_channels_body="${scoped_alert_notification_channels_response%$'\n'*}"
if [[ "$scoped_alert_notification_channels_status" != "403" ]]; then
  fail "fleet:read scoped fleet-alert-notification-channels returned HTTP $scoped_alert_notification_channels_status: $scoped_alert_notification_channels_body"
fi
jq -e '.error == "operator_scope_insufficient"' <<<"$scoped_alert_notification_channels_body" >/dev/null
scoped_alert_notifications_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  "$api_url/api/v1/fleet-alert-notifications?limit=5")"
scoped_alert_notifications_status="${scoped_alert_notifications_response##*$'\n'}"
scoped_alert_notifications_body="${scoped_alert_notifications_response%$'\n'*}"
if [[ "$scoped_alert_notifications_status" != "403" ]]; then
  fail "fleet:read scoped fleet-alert-notifications returned HTTP $scoped_alert_notifications_status: $scoped_alert_notifications_body"
fi
jq -e '.error == "operator_scope_insufficient"' <<<"$scoped_alert_notifications_body" >/dev/null
scoped_alert_notification_write_response="$(curl -sS -w '\n%{http_code}' \
  -H "Authorization: Bearer $scoped_access_token" \
  -H "Content-Type: application/json" \
  -d '{"name":"denied-alert-notification","scope_kind":"global","delivery_kind":"webhook","target":"http://127.0.0.1:9/denied","confirmed":true}' \
  "$api_url/api/v1/fleet-alert-notification-channels")"
scoped_alert_notification_write_status="${scoped_alert_notification_write_response##*$'\n'}"
scoped_alert_notification_write_body="${scoped_alert_notification_write_response%$'\n'*}"
if [[ "$scoped_alert_notification_write_status" != "403" ]]; then
  fail "fleet:read scoped fleet-alert-notification write returned HTTP $scoped_alert_notification_write_status: $scoped_alert_notification_write_body"
fi
jq -e '.error == "operator_scope_insufficient"' <<<"$scoped_alert_notification_write_body" >/dev/null
process_presets_json="$(vpsctl_auth config-presets --behavior process_inventory)"
jq -e '
  any(.[]; .name == "Linux process inventory" and .kind == "system" and .is_default == true)
' <<<"$process_presets_json" >/dev/null

tag_json="$(vpsctl_auth tag-create --name cli-live-tag --confirmed)"
jq -e '.name == "cli-live-tag"' <<<"$tag_json" >/dev/null
vpsctl_auth agent-tag --client-id cli-agent-b --tag cli-live-tag --confirmed >/dev/null

bulk_json="$(vpsctl_auth bulk-resolve \
  --clients cli-agent-a \
  --tags cli-live-tag)"
jq -e '.target_count == 2 and (.targets | length == 2) and any(.targets[]; .id == "cli-agent-a") and any(.targets[]; .id == "cli-agent-b")' \
  <<<"$bulk_json" >/dev/null

plans_json="$(vpsctl_auth tunnel-plans)"
jq -e 'length == 1 and .[0].name == "cli-gre-a-b" and .[0].enabled == true and .[0].revision == 2' <<<"$plans_json" >/dev/null
updated_plan_json="$(vpsctl_auth tunnel-plan \
  --name cli-gre-a-b \
  --interface-name grecli \
  --kind gre \
  --left-client-id cli-agent-a \
  --right-client-id cli-agent-b \
  --left-remote-underlay 203.0.113.201 \
  --right-remote-underlay 203.0.113.202 \
  --left-tunnel-ipv4-cidr 10.252.0.0/31 \
  --right-tunnel-ipv4-cidr 10.252.0.1/31 \
  --bandwidth-mbps 250 \
  --save \
  --update-plan-id "$plan_id" \
  --expected-revision "$plan_revision" \
  --confirmed)"
jq -e '
  .plan.revision == 3 and
  .plan.enabled == true and
  .plan.plan.bandwidth_mbps == 250 and
  ([.sync[].client_id] | sort == ["cli-agent-a", "cli-agent-b"]) and
  all(.sync[]; .status == "queued" and .job_id != null and .error == null)
' <<<"$updated_plan_json" >/dev/null
if stale_plan_output="$(vpsctl_auth tunnel-plan \
  --name cli-gre-a-b \
  --interface-name grecli \
  --kind gre \
  --left-client-id cli-agent-a \
  --right-client-id cli-agent-b \
  --left-remote-underlay 203.0.113.201 \
  --right-remote-underlay 203.0.113.202 \
  --left-tunnel-ipv4-cidr 10.252.0.0/31 \
  --right-tunnel-ipv4-cidr 10.252.0.1/31 \
  --bandwidth-mbps 500 \
  --save \
  --update-plan-id "$plan_id" \
  --expected-revision "$plan_revision" \
  --confirmed 2>&1)"; then
  fail "tunnel-plan update accepted a stale declaration revision"
fi
[[ "$stale_plan_output" == *"Tunnel plan snapshot stale"* && "$stale_plan_output" == *"Refresh current state"* ]] \
  || fail "stale tunnel plan update did not explain the stale snapshot and recovery: $stale_plan_output"

plan_revision="$(jq -r '.plan.revision' <<<"$updated_plan_json")"
disabled_plan_json="$(vpsctl_auth tunnel-plan-disable \
  --plan-id "$plan_id" \
  --expected-revision "$plan_revision" \
  --confirmed)"
jq -e '
  .plan.enabled == false and
  .plan.revision == 4 and
  ([.sync[].client_id] | sort == ["cli-agent-a", "cli-agent-b"]) and
  all(.sync[]; .status == "queued" and .job_id != null and .error == null)
' <<<"$disabled_plan_json" >/dev/null
plan_revision="$(jq -r '.plan.revision' <<<"$disabled_plan_json")"
reenabled_plan_json="$(vpsctl_auth tunnel-plan-enable \
  --plan-id "$plan_id" \
  --expected-revision "$plan_revision" \
  --confirmed)"
jq -e '
  .plan.enabled == true and
  .plan.revision == 5 and
  ([.sync[].client_id] | sort == ["cli-agent-a", "cli-agent-b"]) and
  all(.sync[]; .status == "queued" and .job_id != null and .error == null)
' <<<"$reenabled_plan_json" >/dev/null
plan_revision="$(jq -r '.plan.revision' <<<"$reenabled_plan_json")"
deleted_plan_json="$(vpsctl_auth tunnel-plan-delete \
  --plan-id "$plan_id" \
  --expected-revision "$plan_revision" \
  --confirmed)"
jq -e '
  .plan.enabled == false and
  .plan.deleted_reason == "operator_retired" and
  .plan.deleted_at != null and
  .plan.revision == 6 and
  .plan.left_runtime_config.desired == "absent" and
  .plan.right_runtime_config.desired == "absent" and
  .plan.left_runtime_config.status == "queued" and
  .plan.right_runtime_config.status == "queued" and
  ([.sync[].client_id] | sort == ["cli-agent-a", "cli-agent-b"]) and
  all(.sync[]; .status == "queued" and .job_id != null and .error == null)
' \
  <<<"$deleted_plan_json" >/dev/null
jq -e 'length == 0' <<<"$(vpsctl_auth tunnel-plans)" >/dev/null

schedule_json="$(vpsctl_auth schedule-create \
  --name cli-hourly-uptime \
  --command /usr/bin/uptime \
  --tags edge \
  --cron-expr '0 * * * *' \
  --catch-up-policy run_once \
  --retry-delay-secs 120 \
  --max-failures 7 \
  --confirmed)"
jq -e '.name == "cli-hourly-uptime" and .enabled == true and .command_type == "shell_argv" and .selector_expression == "tag:edge" and (.target_client_ids | index("cli-agent-a")) and .cron_expr == "0 * * * *" and .catch_up_policy == "run_once" and .retry_delay_secs == 120 and .max_failures == 7' \
  <<<"$schedule_json" >/dev/null
schedules_json="$(vpsctl_auth schedules)"
jq -e 'length == 1 and .[0].name == "cli-hourly-uptime" and .[0].catch_up_policy == "run_once" and .[0].failure_count == 0 and (.[0].target_client_ids | index("cli-agent-a"))' <<<"$schedules_json" >/dev/null

backup_json="$(vpsctl_auth backup-request \
  --client-id cli-agent-a \
  --paths /etc/hostname \
  --include-config \
  --note "vpsctl live api backup" \
  --confirmed)"
require_no_secret "$backup_json" "$super_password" "backup-request"
backup_id="$(jq -r '.id' <<<"$backup_json")"
jq -e '.client_id == "cli-agent-a" and .status == "requested_metadata_only" and .command_scope == "client:cli-agent-a" and .include_config == true' \
  <<<"$backup_json" >/dev/null

restore_json="$(vpsctl_auth restore-plan \
  --source-backup-request-id "$backup_id" \
  --target-client-id cli-agent-b \
  --note "vpsctl live api restore" \
  --confirmed)"
require_no_secret "$restore_json" "$super_password" "restore-plan"
restore_root="/var/lib/vpsman/restores/$backup_id/cli-agent-b"
jq -e --arg backup_id "$backup_id" --arg restore_root "$restore_root" '.source_backup_request_id == $backup_id and .source_client_id == "cli-agent-a" and .target_client_id == "cli-agent-b" and .status == "planned_metadata_only" and .destination_root == $restore_root and .command_scope == "client:cli-agent-b"' \
  <<<"$restore_json" >/dev/null

backups_json="$(vpsctl_auth backups --limit 10)"
jq -e --arg backup_id "$backup_id" 'any(.[]; .id == $backup_id and .client_id == "cli-agent-a")' \
  <<<"$backups_json" >/dev/null
restores_json="$(vpsctl_auth restore-plans --limit 10)"
jq -e --arg backup_id "$backup_id" 'any(.[]; .source_backup_request_id == $backup_id and .target_client_id == "cli-agent-b")' \
  <<<"$restores_json" >/dev/null

audit_json="$(vpsctl_auth audit --limit 100)"
jq -e 'any(.[]; .action == "operator.created") and any(.[]; .action == "operator_session.revoked") and any(.[]; .action == "backup.requested_metadata_only") and any(.[]; .action == "restore.planned_metadata_only") and any(.[]; .action == "network.tunnel_plan_created") and any(.[]; .action == "network.tunnel_plan_deleted" and .metadata.was_enabled == true)' \
  <<<"$audit_json" >/dev/null

history_retention_json="$(vpsctl_auth history-retention)"
jq -e 'any(.[]; .domain == "audit_logs" and .built_in_default == true) and any(.[]; .domain == "backup_artifacts")' \
  <<<"$history_retention_json" >/dev/null
history_policy_json="$(vpsctl_auth history-retention-upsert \
  --domain audit_logs \
  --retention-days 90 \
  --prune-limit 250 \
  --metadata-only false \
  --export-enabled true \
  --confirmed)"
jq -e '.domain == "audit_logs" and .retention_days == 90 and .prune_limit == 250 and .built_in_default == false' \
  <<<"$history_policy_json" >/dev/null
history_prune_json="$(vpsctl_auth history-retention-prune --domain audit_logs --dry-run)"
jq -e '.dry_run == true and any(.domains[]; .domain == "audit_logs" and .status == "dry_run")' \
  <<<"$history_prune_json" >/dev/null
history_export_json="$(vpsctl_auth history-export --domains audit_logs,backup_artifacts,topology_history --limit 10)"
jq -e '(.domains | index("audit_logs")) and (.domains | index("backup_artifacts")) and (.data.audit_logs | type == "array") and (.data.topology_history.graph.nodes | type == "array")' \
  <<<"$history_export_json" >/dev/null

jq -n \
  --arg api_url "$api_url" \
  --arg backup_id "$backup_id" \
  '{
    vpsctl_live_api_smoke: "ok",
    api_url: $api_url,
    backup_request_id: $backup_id,
    checks: [
      "health",
      "bootstrap_login_refresh_me",
      "operator_create_list_viewer_denial",
      "operator_session_revoke",
      "operator_scope_enforcement",
      "fleet_alerts",
      "fleet_alert_states",
      "fleet_alert_policies",
      "fleet_alert_notifications",
      "direct_agent_identity",
      "agents_summary",
      "configuration_source_system_inheritance",
      "system_configuration_presets",
      "tag_bulk",
      "tunnel_plan_save_enable_update_disable_delete",
      "schedule_create_list",
      "backup_request_restore_plan",
      "audit_visibility",
      "history_retention_export",
      "no_plaintext_password_in_cli_outputs"
    ]
  }'
