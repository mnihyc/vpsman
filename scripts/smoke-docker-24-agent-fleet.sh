#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash curl docker google-chrome jq python3 shuf timeout
smoke_build_binaries
if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsman-worker >/dev/null
fi
smoke_init_tmpdir "vpsman-docker-24-agent-fleet"

agent_count="${VPSMAN_DOCKER_FLEET_AGENT_COUNT:-24}"
if ((agent_count < 20)); then
  smoke_fail "VPSMAN_DOCKER_FLEET_AGENT_COUNT must be at least 20"
fi
long_running_secs="${VPSMAN_DOCKER_FLEET_LONG_RUNNING_SECS:-0}"
simulate_api_backlog="${VPSMAN_DOCKER_FLEET_SIMULATE_API_BACKLOG:-0}"
gateway_command_output_ttl_secs="${VPSMAN_DOCKER_FLEET_COMMAND_OUTPUT_TTL_SECS:-86400}"
bulk_max_timeout_secs=45
bulk_follow_max_polls=$((240 + agent_count * 8))
rollup_bucket_secs=60
alert_notification_dispatch_limit=200

run_id="docker-fleet-$(date +%s%N)"
label_key="vpsman.smoke.run"
pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
frontend_port="$(smoke_free_port)"
api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
internal_token="docker-fleet-internal-$(date +%s%N)"
operator_username="docker-fleet-admin"
operator_password="docker-fleet-password-$(date +%s%N)"
super_password="docker-fleet-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"
object_store_dir="$SMOKE_TMPDIR/object-store"
screenshot_dir="$ROOT_DIR/tmp/docker-24-agent-fleet-$run_id"
runtime_image="${VPSMAN_DOCKER_FLEET_RUNTIME_IMAGE:-ubuntu:24.04}"
extended_review="${VPSMAN_DOCKER_FLEET_EXTENDED_REVIEW:-1}"
mkdir -p "$object_store_dir" "$screenshot_dir"

pg_container="vpsman-$run_id-postgres"
api_container="vpsman-$run_id-api"
gateway_container="vpsman-$run_id-gateway"

smoke_fleet_log() {
  printf '[docker-fleet:%s] %s\n' "$(date -u +%H:%M:%S)" "$*" >&2
}

smoke_fleet_log "starting run_id=$run_id agents=$agent_count long_running_secs=$long_running_secs simulate_api_backlog=$simulate_api_backlog extended_review=$extended_review"

cleanup_docker_fleet_smoke() {
  docker ps -aq --filter "label=$label_key=$run_id" | xargs -r docker rm -f >/dev/null 2>&1 || true
  if [[ -n "${SMOKE_TMPDIR:-}" && -d "$SMOKE_TMPDIR/object-store" ]]; then
    docker run --rm \
      -v "$SMOKE_TMPDIR:$SMOKE_TMPDIR" \
      -w "$SMOKE_TMPDIR" \
      "$runtime_image" \
      sh -c 'rm -rf object-store' >/dev/null 2>&1 || true
  fi
  if [[ -n "${SMOKE_TMPDIR:-}" && -d "$SMOKE_TMPDIR" ]]; then
    docker run --rm \
      -v "$SMOKE_TMPDIR:$SMOKE_TMPDIR" \
      -w "$SMOKE_TMPDIR" \
      "$runtime_image" \
      sh -c "chown -R $(id -u):$(id -g) . >/dev/null 2>&1 || true" \
      >/dev/null 2>&1 || true
  fi
  smoke_cleanup
}
trap cleanup_docker_fleet_smoke EXIT

dump_docker_logs() {
  local title="$1"
  local container
  echo "$title" >&2
  while IFS= read -r container; do
    [[ -n "$container" ]] || continue
    echo "--- docker logs: $container ---" >&2
    docker logs "$container" >&2 || true
  done < <(docker ps -a --filter "label=$label_key=$run_id" --format '{{.Names}}' | sort)
  docker ps -a --filter "label=$label_key=$run_id" >&2 || true
}

docker_fleet_smoke_err() {
  local line="$1"
  local status="$2"
  trap - ERR
  dump_docker_logs "Docker fleet smoke failed at line $line with status $status"
  exit "$status"
}
trap 'docker_fleet_smoke_err "$LINENO" "$?"' ERR

api_get() {
  local path="$1"
  curl -fsS -H "Authorization: Bearer $access_token" "$api_url$path"
}

api_post() {
  local path="$1"
  local json="$2"
  curl -fsS \
    -H "Authorization: Bearer $access_token" \
    -H "Content-Type: application/json" \
    -d "$json" \
    "$api_url$path"
}

api_put() {
  local path="$1"
  local json="$2"
  curl -fsS \
    -X PUT \
    -H "Authorization: Bearer $access_token" \
    -H "Content-Type: application/json" \
    -d "$json" \
    "$api_url$path"
}

vpsctl_json() {
  VPSMAN_API_URL="$api_url" \
  VPSMAN_API_TOKEN="$access_token" \
  VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
    target/debug/vpsctl "$@"
}

docker run -d \
  --name "$pg_container" \
  --label "$label_key=$run_id" \
  -e POSTGRES_DB=vpsman \
  -e POSTGRES_PASSWORD=vpsman \
  -e POSTGRES_USER=vpsman \
  -p "127.0.0.1:$pg_port:5432" \
  postgres:16-alpine >/dev/null

smoke_fleet_log "waiting for postgres on 127.0.0.1:$pg_port"
deadline=$((SECONDS + 45))
until docker exec "$pg_container" psql -U vpsman -d vpsman -tAc 'select 1' >/dev/null 2>&1; do
  if ((SECONDS >= deadline)); then
    dump_docker_logs "timed out waiting for postgres"
    exit 1
  fi
  sleep 0.25
done
smoke_wait_tcp 127.0.0.1 "$pg_port"
smoke_fleet_log "postgres is ready"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

docker run -d \
  --name "$api_container" \
  --network host \
  --user "$(id -u):$(id -g)" \
  --label "$label_key=$run_id" \
  -e VPSMAN_API_BIND="127.0.0.1:$api_port" \
  -e VPSMAN_POSTGRES_URL="$postgres_url" \
  -e VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
  -e VPSMAN_INTERNAL_TOKEN="$internal_token" \
  -e VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
  -e VPSMAN_PUBLIC_GATEWAY_ENDPOINTS="primary=$gateway_addr=10" \
  -e VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX="$gateway_public_hex" \
  -e VPSMAN_BACKUP_OBJECT_STORE_DIR="$object_store_dir" \
  -e VPSMAN_SUITE_CONFIG="$VPSMAN_SUITE_CONFIG" \
  -e VPSMAN_ENROLLMENT_TELEMETRY_LIGHT_SECS=2 \
  -e VPSMAN_ENROLLMENT_TELEMETRY_FULL_SECS=4 \
  -e VPSMAN_ENROLLMENT_DEFAULT_COUNTRY="" \
  -e RUST_LOG=vpsman_api=warn \
  -v "$ROOT_DIR:$ROOT_DIR" \
  -w "$ROOT_DIR" \
  "$runtime_image" \
  "$ROOT_DIR/target/debug/vpsman-api" >/dev/null

smoke_fleet_log "waiting for API health on $api_url"
if ! smoke_wait_http "$api_url/health"; then
  dump_docker_logs "API did not become healthy"
  exit 1
fi
smoke_fleet_log "API is healthy; bootstrapping operator"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"$operator_username\",\"password\":\"$operator_password\"}" \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
jq -e '.operator.username == "docker-fleet-admin" and .operator.role == "admin"' <<<"$auth_json" >/dev/null
smoke_fleet_log "operator bootstrap succeeded"

docker run -d \
  --name "$gateway_container" \
  --network host \
  --label "$label_key=$run_id" \
  -e VPSMAN_GATEWAY_BIND="$gateway_addr" \
  -e VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
  -e VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
  -e VPSMAN_API_URL="$api_url" \
  -e VPSMAN_INTERNAL_TOKEN="$internal_token" \
  -e VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
  -e VPSMAN_GATEWAY_ID=docker-fleet-gateway \
  -e VPSMAN_SUITE_CONFIG="$VPSMAN_SUITE_CONFIG" \
  -e VPSMAN_GATEWAY_SPOOL_DIR="$SMOKE_TMPDIR/gateway-spool" \
  -e VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS="$gateway_command_output_ttl_secs" \
  -e RUST_LOG=vpsman_gateway=warn \
  -v "$ROOT_DIR:$ROOT_DIR" \
  -w "$ROOT_DIR" \
  "$runtime_image" \
  "$ROOT_DIR/target/debug/vpsman-gateway" >/dev/null

smoke_fleet_log "waiting for gateway data/control listeners"
if ! smoke_wait_tcp 127.0.0.1 "$gateway_port"; then
  dump_docker_logs "gateway agent listener did not start"
  exit 1
fi
if ! smoke_wait_tcp 127.0.0.1 "$gateway_control_port"; then
  dump_docker_logs "gateway control listener did not start"
  exit 1
fi
smoke_fleet_log "gateway listeners are ready"

providers=(alpha beta gamma)
countries=(US DE SG NL)
roles=(edge core backup batch)
provider_alpha_count=0
country_us_count=0
provider_alpha_country_us_count=0
role_edge_count=0
for ((i = 1; i <= agent_count; i += 1)); do
  index=$((i - 1))
  provider="${providers[$((index % ${#providers[@]}))]}"
  country="${countries[$((index % ${#countries[@]}))]}"
  role="${roles[$((index % ${#roles[@]}))]}"
  if [[ "$provider" == "alpha" ]]; then
    provider_alpha_count=$((provider_alpha_count + 1))
  fi
  if [[ "$country" == "US" ]]; then
    country_us_count=$((country_us_count + 1))
  fi
  if [[ "$provider" == "alpha" && "$country" == "US" ]]; then
    provider_alpha_country_us_count=$((provider_alpha_country_us_count + 1))
  fi
  if [[ "$role" == "edge" ]]; then
    role_edge_count=$((role_edge_count + 1))
  fi
done
first_client_id=""
second_client_id=""

smoke_fleet_log "starting $agent_count agent containers"
for ((i = 1; i <= agent_count; i += 1)); do
  index=$((i - 1))
  provider="${providers[$((index % ${#providers[@]}))]}"
  country="${countries[$((index % ${#countries[@]}))]}"
  role="${roles[$((index % ${#roles[@]}))]}"
  logical_client_id="$(printf 'docker-fleet-%02d' "$i")"
  display_name="$(printf 'df-%s-%s-%02d' "$provider" "$country" "$i")"
  tag_csv="provider:$provider,country:$country,role:$role,audit:docker-fleet,bulk-target"

  agent_dir="$SMOKE_TMPDIR/$logical_client_id"
  agent_config="$agent_dir/agent.toml"
  mkdir -p "$agent_dir/state"
  smoke_create_direct_agent_config \
    "$api_url" \
    "$access_token" \
    "$agent_config" \
    "$logical_client_id" \
    "$display_name" \
    "$tag_csv" \
    "$gateway_public_hex" \
    "primary=$gateway_addr=10"
  enrolled_client_id="$logical_client_id"
  [[ -n "$first_client_id" ]] || first_client_id="$enrolled_client_id"
  if [[ -z "$second_client_id" && "$enrolled_client_id" != "$first_client_id" ]]; then
    second_client_id="$enrolled_client_id"
  fi

  docker run -d \
    --name "vpsman-$run_id-agent-$i" \
    --network host \
    --label "$label_key=$run_id" \
    --memory 128m \
    --cpus 0.5 \
    --pids-limit 96 \
    -e RUST_LOG=vpsman_agent=warn \
    -e VPSMAN_AGENT_STATE_DIR="$agent_dir/state" \
    -e VPSMAN_SUPERVISOR_DIR="$agent_dir/supervisor" \
    -v "$ROOT_DIR:$ROOT_DIR" \
    -w "$ROOT_DIR" \
    "$runtime_image" \
    "$ROOT_DIR/target/debug/vpsman-agent" --config "$agent_config" run >/dev/null
done
smoke_fleet_log "agent containers started; waiting for all agents online"

deadline=$((SECONDS + 90))
online_count=0
until [[ "$online_count" == "$agent_count" ]]; do
  if ((SECONDS >= deadline)); then
    dump_docker_logs "not all Docker fleet agents online"
    api_get "/api/v1/agents" >&2 || true
    exit 1
  fi
  online_count="$(api_get "/api/v1/fleet/summary" | jq -r '.online')"
  sleep 0.5
done
smoke_fleet_log "all agents online ($online_count/$agent_count)"

smoke_fleet_log "waiting for telemetry rollups from all agents"
deadline=$((SECONDS + 60))
telemetry_rollup_count=0
telemetry_ready_client_count=0
until ((telemetry_ready_client_count == agent_count)); do
  if ((SECONDS >= deadline)); then
    dump_docker_logs "not enough per-minute telemetry summaries arrived"
    docker exec "$pg_container" psql -U vpsman -d vpsman -c "SELECT client_id, bucket_start, sample_count FROM telemetry_rollups ORDER BY client_id, bucket_start DESC" >&2 || true
    exit 1
  fi
  telemetry_rollup_count="$(docker exec "$pg_container" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM telemetry_rollups WHERE bucket_secs = $rollup_bucket_secs")"
  telemetry_ready_client_count="$(docker exec "$pg_container" psql -U vpsman -d vpsman -tAc "SELECT count(DISTINCT client_id) FROM telemetry_rollups WHERE bucket_secs = $rollup_bucket_secs AND sample_count >= 2")"
  sleep 1
done
telemetry_rollup_count="$(docker exec "$pg_container" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM telemetry_rollups WHERE bucket_secs = $rollup_bucket_secs")"
smoke_fleet_log "telemetry rollups ready: clients=$telemetry_ready_client_count rows=$telemetry_rollup_count"

smoke_fleet_log "waiting for network-rate telemetry from all agents"
deadline=$((SECONDS + 60))
telemetry_network_rate_ready_client_count=0
until ((telemetry_network_rate_ready_client_count >= agent_count)); do
  if ((SECONDS >= deadline)); then
    dump_docker_logs "not enough per-client telemetry network rates arrived through the API"
    api_get "/api/v1/telemetry/network-rates?limit=5000&bucket_secs=$rollup_bucket_secs" >&2 || true
    docker exec "$pg_container" psql -U vpsman -d vpsman -c "SELECT client_id, interface, bucket_start, sample_count FROM telemetry_network_rates ORDER BY client_id, interface, bucket_start DESC" >&2 || true
    exit 1
  fi
  telemetry_network_rate_ready_client_count="$(api_get "/api/v1/telemetry/network-rates?limit=5000&bucket_secs=$rollup_bucket_secs" | jq -r '
    [.[] | select(.sample_count >= 2 and (.interface | length) > 0) | .client_id] | unique | length
  ')"
  sleep 1
done
smoke_fleet_log "network-rate telemetry ready: clients=$telemetry_network_rate_ready_client_count"

smoke_fleet_log "validating fleet inventory, tags, dashboard, and system dashboard APIs"
agents_json="$(api_get "/api/v1/agents")"
jq -e \
  --argjson expected "$agent_count" \
  --argjson alpha_count "$provider_alpha_count" \
  --argjson us_count "$country_us_count" '
  length == $expected and
  all(.[]; .status == "online") and
  ([.[] | select(.tags | index("provider:alpha"))] | length == $alpha_count) and
  ([.[] | select(.tags | index("country:US"))] | length == $us_count) and
  ([.[] | select(.tags | index("audit:docker-fleet"))] | length == $expected)
' <<<"$agents_json" >/dev/null

api_get "/api/v1/fleet/summary" | jq -e --argjson expected "$agent_count" '
  .total == $expected and .online == $expected
' >/dev/null

api_get "/api/v1/tags" | jq -e \
  --argjson expected "$agent_count" \
  --argjson alpha_count "$provider_alpha_count" \
  --argjson us_count "$country_us_count" '
  any(.[]; .name == "provider:alpha" and (.clients | length) == $alpha_count) and
  any(.[]; .name == "country:US" and (.clients | length) == $us_count) and
  any(.[]; .name == "audit:docker-fleet" and (.clients | length) == $expected)
' >/dev/null

api_get "/api/v1/telemetry/rollups?limit=5000&bucket_secs=$rollup_bucket_secs" | jq -e --argjson expected "$agent_count" --argjson bucket "$rollup_bucket_secs" '
  ([.[] | select(.bucket_secs == $bucket and .sample_count >= 2) | .client_id] | unique | length) >= $expected and
  all(.[] | select(.bucket_secs == $bucket and .sample_count >= 2); .memory_total_bytes_max > 0 and .disk_total_bytes_max >= 0)
' >/dev/null

api_get "/api/v1/telemetry/network-rates?limit=5000&bucket_secs=$rollup_bucket_secs" | jq -e --argjson expected "$agent_count" --argjson bucket "$rollup_bucket_secs" '
  ([.[] | select(.bucket_secs == $bucket and .sample_count >= 2 and (.interface | length) > 0) | .client_id] | unique | length) >= $expected
' >/dev/null

api_post "/api/v1/bulk/resolve" '{"selector_expression":"provider:alpha"}' \
  | jq -e --argjson alpha_count "$provider_alpha_count" '.target_count == $alpha_count' >/dev/null
api_post "/api/v1/bulk/resolve" '{"selector_expression":"provider:alpha && country:US"}' \
  | jq -e --argjson alpha_us_count "$provider_alpha_country_us_count" '.target_count == $alpha_us_count' >/dev/null

dashboard_json="$(api_get "/api/v1/dashboard/overview?window=1h&group_by=providers")"
jq -e \
  --argjson expected "$agent_count" \
  --argjson alpha_count "$provider_alpha_count" '
  .summary.total == $expected and
  .summary.online == $expected and
  .resources.sampled_clients >= 20 and
  .resources.cpu_load_avg != null and
  .resources.memory_used_ratio != null and
  .resources.disk_free_ratio != null and
  (.network.points | length) >= 1 and
  (.network.traffic_points | length) >= 1 and
  (.network.traffic_series | length) > 0 and
  (.network.top_clients | length) > 0 and
  all(.network.top_clients[]; (.interfaces | length) > 0) and
  all(.network.traffic_series[]; (.points | length) >= 1 and (.interfaces | length) > 0) and
  any(.available_filters.providers[]; .value == "alpha" and .count == $alpha_count) and
  any(.available_filters.windows[]; .value == "all") and
  any(.label_clusters[]; .label == "provider:alpha" and .total == $alpha_count)
' <<<"$dashboard_json" >/dev/null

system_dashboard_json="$(api_get "/api/v1/system/dashboard?window=1h&chart_points=120")"
jq -e '
  .current.db_pool.max_connections >= 1 and
  .current.db_pool.open_connections >= 1 and
  .current.dispatch.active_jobs >= 0 and
  .current.dispatch.queue_depth >= 0 and
  .current.targets.active >= 0 and
  .current.targets.deadline_expired_active >= 0 and
  .current.cancellations.acked >= 0 and
  .current.gateway_events.status == "live" and
  ((.current.gateway_events.queued_events // 0) >= (.current.gateway_events.delivered_events // 0)) and
  (.current.gateway_events.retry_attempts // 0) >= 0 and
  any(.series[]; .metric == "db_pool.in_use_connections")
' <<<"$system_dashboard_json" >/dev/null

api_get "/api/v1/dashboard/overview?window=all&group_by=providers" \
  | jq -e --argjson expected "$agent_count" '
    .window == "all" and
    .time_range.mode == "all" and
    .summary.total == $expected and
    (.network.traffic_series | length) > 0
  ' >/dev/null

api_get "/api/v1/dashboard/overview?window=1h&scope_kind=country&scope_value=US&group_by=providers" \
  | jq -e \
    --argjson us_count "$country_us_count" \
    --argjson alpha_us_count "$provider_alpha_country_us_count" '
    .scope.label == "country:US" and
    .scope.matched_clients == $us_count and
    any(.label_clusters[]; .label == "provider:alpha" and .total == $alpha_us_count)
  ' >/dev/null
smoke_fleet_log "fleet/dashboard API validation passed"

smoke_fleet_log "validating alert notification, network plan, backup metadata, and preferences workflows"
alert_policy_json="$(vpsctl_json alert-policy upsert \
  --name docker-edge-resource-alerts \
  --selector 'tag:role:edge' \
  --rule 'cpu.load_1 >= 0.5' \
  --severity warning \
  --notes docker-fleet-live-review \
  --confirmed)"
jq -e '
  .name == "docker-edge-resource-alerts" and
  .selector_expression == "tag:role:edge" and
  .enabled == true and
  (.rules | length) == 1 and
  .rules[0].condition_expression == "cpu.load_1 >= 0.5"
' <<<"$alert_policy_json" >/dev/null

alert_notification_channel_json="$(vpsctl_json fleet-alert-notification-channel-upsert \
  --name docker-resource-webhook \
  --scope-kind global \
  --min-severity warning \
  --categories resource \
  --operator-states open \
  --delivery-kind webhook \
  --target http://127.0.0.1:9/vpsman/docker-resource \
  --cooldown-secs 600 \
  --notes docker-fleet-live-review \
  --confirmed)"
alert_notification_channel_id="$(jq -r '.id' <<<"$alert_notification_channel_json")"
jq -e '
  .name == "docker-resource-webhook" and
  .scope_kind == "global" and
  .min_severity == "warning" and
  .categories == ["resource"] and
  .operator_states == ["open"] and
  .delivery_kind == "webhook" and
  .enabled == true
' <<<"$alert_notification_channel_json" >/dev/null

alert_notification_custom_channel_json="$(vpsctl_json fleet-alert-notification-channel-upsert \
  --name docker-resource-backup-webhook \
  --scope-kind global \
  --min-severity warning \
  --categories resource \
  --operator-states open \
  --delivery-kind webhook \
  --target http://127.0.0.1:9/vpsman/docker-resource-backup \
  --cooldown-secs 600 \
  --notes docker-fleet-live-review-custom \
  --confirmed)"
jq -e '
  .name == "docker-resource-backup-webhook" and
  .delivery_kind == "webhook" and
  .enabled == true
' <<<"$alert_notification_custom_channel_json" >/dev/null

alert_notification_dry_run_json="$(vpsctl_json fleet-alert-notification-dispatch \
  --category resource \
  --include-muted \
  --dry-run \
  --limit "$alert_notification_dispatch_limit")"
jq -e '
  length >= 1 and
  any(.[]; .channel_name == "docker-resource-webhook" and .status == "matched_dry_run")
' <<<"$alert_notification_dry_run_json" >/dev/null
alert_notification_dispatch_preview_hash="$(jq -r '.[0].review_preview_hash // empty' <<<"$alert_notification_dry_run_json")"
if [[ ! "$alert_notification_dispatch_preview_hash" =~ ^[0-9a-f]{64}$ ]]; then
  echo "fleet alert notification dry run did not return a valid preview hash" >&2
  exit 1
fi

alert_notification_dispatch_err="$SMOKE_TMPDIR/alert-notification-dispatch.err"
trap - ERR
set +e
alert_notification_dispatch_json="$(vpsctl_json fleet-alert-notification-dispatch \
  --category resource \
  --include-muted \
  --preview-hash "$alert_notification_dispatch_preview_hash" \
  --confirmed \
  --limit "$alert_notification_dispatch_limit" 2>"$alert_notification_dispatch_err")"
alert_notification_dispatch_status="$?"
set -e
trap 'docker_fleet_smoke_err "$LINENO" "$?"' ERR
if [[ "$alert_notification_dispatch_status" != "0" ]]; then
  alert_notification_retry_dry_run_json="$(vpsctl_json fleet-alert-notification-dispatch \
    --category resource \
    --include-muted \
    --dry-run \
    --limit "$alert_notification_dispatch_limit" || printf '[]')"
  echo "fleet alert notification dispatch failed after preview" >&2
  echo "first_preview_hash=$alert_notification_dispatch_preview_hash" >&2
  echo "retry_preview_hash=$(jq -r '.[0].review_preview_hash // empty' <<<"$alert_notification_retry_dry_run_json")" >&2
  echo "first_preview_summary:" >&2
  jq -c 'group_by(.alert_category, .alert_severity, .channel_name, .status) | map({alert_category: .[0].alert_category, alert_severity: .[0].alert_severity, channel_name: .[0].channel_name, status: .[0].status, count: length})' <<<"$alert_notification_dry_run_json" >&2 || true
  echo "retry_preview_summary:" >&2
  jq -c 'group_by(.alert_category, .alert_severity, .channel_name, .status) | map({alert_category: .[0].alert_category, alert_severity: .[0].alert_severity, channel_name: .[0].channel_name, status: .[0].status, count: length})' <<<"$alert_notification_retry_dry_run_json" >&2 || true
  cat "$alert_notification_dispatch_err" >&2 || true
  exit "$alert_notification_dispatch_status"
fi
jq -e --arg channel_id "$alert_notification_channel_id" '
  length >= 1 and
  any(.[]; .channel_id == $channel_id and .status == "queued" and .delivery_kind == "webhook")
' <<<"$alert_notification_dispatch_json" >/dev/null

schedule_json="$(vpsctl_json schedule-create \
  --name docker-provider-alpha-hourly \
  --command /bin/true \
  --tags provider:alpha \
  --cron-expr '0 * * * *' \
  --disabled \
  --catch-up-policy skip_missed \
  --retry-delay-secs 120 \
  --max-failures 5 \
  --confirmed)"
jq -e --argjson alpha_count "$provider_alpha_count" '
  .name == "docker-provider-alpha-hourly" and
  .enabled == false and
  .selector_expression == "provider:alpha" and
  (.target_client_ids | length) == $alpha_count and
  .cron_expr == "0 * * * *"
' \
  <<<"$schedule_json" >/dev/null

api_post "/api/v1/tunnel-plans" "$(jq -n \
  --arg left "$first_client_id" \
  --arg right "$second_client_id" \
  '{
  "name": "docker-fleet-gre",
  "interface_name": "gre24",
  "kind": "gre",
  "runtime_control": {
    "manager": "external_observed"
  },
  "left_client_id": $left,
  "right_client_id": $right,
  "left_remote_underlay": "203.0.113.12",
  "left_local_underlay": null,
  "right_remote_underlay": "203.0.113.11",
  "right_local_underlay": null,
  "address_pool_cidr": "10.254.24.0/30",
  "reserved_addresses": [],
  "ipv4_tunnel": {
    "left": "10.254.24.0",
    "right": "10.254.24.1",
    "prefix_len": 31
  },
  "latency_primary_family": "ipv4",
  "bandwidth_mbps": 1000,
  "enabled": true,
  "confirmed": true
}')" | jq -e '
  .plan.name == "docker-fleet-gre"
    and .plan.enabled == true
    and .plan.revision == 1
    and .plan.plan.runtime_control.manager == "external_observed"
    and .plan.plan.ospf == null
    and .plan.recommended_ospf_cost == null
    and (.sync | length) == 2
    and all(.sync[]; .status == "queued")
' >/dev/null

backup_json="$(vpsctl_json backup-request \
  --client-id "$first_client_id" \
  --paths /etc/hostname \
  --include-config \
  --note "docker fleet audit metadata request" \
  --confirmed)"
jq -e --arg client "$first_client_id" '
  .client_id == $client and .status == "requested_metadata_only" and .include_config == true
' <<<"$backup_json" >/dev/null

api_put "/api/v1/auth/preferences" '{
  "vps_name_display_mode": "name_id_suffix",
  "timezone": "UTC",
  "language": "en",
  "sidebar_subpanel_default": "all"
}' | jq -e '.preferences.timezone == "UTC" and .preferences.sidebar_subpanel_default == "all"' >/dev/null
smoke_fleet_log "pre-job workflow validation passed"

smoke_fleet_log "dispatching provider:alpha bulk job ($provider_alpha_count targets)"
job_json="$(vpsctl_json job-shell \
  --script 'printf "docker-bulk-ok\n"' \
  --tags provider:alpha \
  --max-timeout-secs "$bulk_max_timeout_secs" \
  --confirmed)"
job_id="$(jq -r '.job_id' <<<"$job_json")"
jq -e --argjson alpha_count "$provider_alpha_count" '
  .target_counts.total == $alpha_count and
  .target_counts.skipped == 0 and
  (.target_counts.rejected + .target_counts.failed + .target_counts.agent_lost + .target_counts.agent_timeout + .target_counts.control_timeout + .target_counts.canceled) == 0
' <<<"$job_json" >/dev/null
smoke_assert_job_create_queued "$job_json" "$provider_alpha_count"

smoke_fleet_log "following bulk job $job_id"
trap - ERR
set +e
vpsctl_json job-follow --job-id "$job_id" --interval-ms 250 --max-polls "$bulk_follow_max_polls" --json >"$SMOKE_TMPDIR/job-follow.jsonl"
job_follow_status="$?"
set -e
trap 'docker_fleet_smoke_err "$LINENO" "$?"' ERR
if [[ "$job_follow_status" != "0" ]]; then
  echo "bulk job follow failed:" >&2
  api_get "/api/v1/jobs/$job_id" | jq . >&2 || true
  echo "bulk job target status counts:" >&2
  api_get "/api/v1/jobs/$job_id/targets" \
    | jq 'group_by(.status) | map({status: .[0].status, count: length, clients: map(.client_id)})' >&2 || true
  echo "bulk job output summary:" >&2
  api_get "/api/v1/jobs/$job_id/outputs" \
    | jq '.items | group_by(.client_id) | map({client_id: .[0].client_id, streams: map(.stream), done: map(select(.done)) | length})' >&2 || true
  echo "bulk job follow tail:" >&2
  tail -n 20 "$SMOKE_TMPDIR/job-follow.jsonl" >&2 || true
  dump_docker_logs "bulk job follow failed"
  exit "$job_follow_status"
fi
if ! api_get "/api/v1/jobs/$job_id" | jq -e \
  --argjson alpha_count "$provider_alpha_count" '
  .status == "completed" and .target_count == $alpha_count
' >/dev/null; then
  echo "bulk job did not finish completed:" >&2
  api_get "/api/v1/jobs/$job_id" | jq . >&2 || true
  echo "bulk job target status counts:" >&2
  api_get "/api/v1/jobs/$job_id/targets" \
    | jq 'group_by(.status) | map({status: .[0].status, count: length, clients: map(.client_id)})' >&2 || true
  echo "bulk job output summary:" >&2
  api_get "/api/v1/jobs/$job_id/outputs" \
    | jq '.items | group_by(.client_id) | map({client_id: .[0].client_id, streams: map(.stream), done: map(select(.done)) | length})' >&2 || true
  echo "bulk job follow tail:" >&2
  tail -n 20 "$SMOKE_TMPDIR/job-follow.jsonl" >&2 || true
  dump_docker_logs "bulk job did not finish completed"
  exit 1
fi
if ! api_get "/api/v1/jobs/$job_id/targets" | jq -e \
  --argjson alpha_count "$provider_alpha_count" '
  length == $alpha_count and all(.[]; .status == "completed" and .exit_code == 0)
' >/dev/null; then
  echo "bulk job targets were not all successful:" >&2
  api_get "/api/v1/jobs/$job_id/targets" | jq . >&2 || true
  dump_docker_logs "bulk job targets were not all successful"
  exit 1
fi
if ! api_get "/api/v1/jobs/$job_id/outputs" | jq -e \
  --argjson alpha_count "$provider_alpha_count" '
  ([.items[] | select(.stream == "stdout") | .data_base64 | @base64d] | map(select(. == "docker-bulk-ok\n")) | length) == $alpha_count
' >/dev/null; then
  echo "bulk job outputs were incomplete:" >&2
  api_get "/api/v1/jobs/$job_id/outputs" | jq . >&2 || true
  dump_docker_logs "bulk job outputs were incomplete"
  exit 1
fi
smoke_fleet_log "bulk job $job_id completed with all $provider_alpha_count targets"

long_job_id=""
if ((long_running_secs > 0)); then
  long_timeout=$((long_running_secs + 120))
  smoke_fleet_log "dispatching long-running fleet job (${long_running_secs}s command; $agent_count targets)"
  long_job_json="$(vpsctl_json job-shell \
    --script "printf 'docker-long-start\n'; sleep $long_running_secs; printf 'docker-long-done\n'" \
    --tags bulk-target \
    --max-timeout-secs "$long_timeout" \
    --confirmed)"
  long_job_id="$(jq -r '.job_id' <<<"$long_job_json")"
  smoke_assert_job_create_queued "$long_job_json" "$agent_count"
  sleep 2
  if [[ "$simulate_api_backlog" == "1" ]]; then
    smoke_fleet_log "pausing API container for backlog simulation"
    docker pause "$api_container" >/dev/null
    sleep 6
    docker unpause "$api_container" >/dev/null
    if ! smoke_wait_http "$api_url/health"; then
      dump_docker_logs "API did not recover after backlog pause"
      exit 1
    fi
    smoke_fleet_log "API recovered after backlog simulation"
  fi
  smoke_fleet_log "following long-running job $long_job_id; heartbeat every 30s"
  trap - ERR
  set +e
  vpsctl_json job-follow --job-id "$long_job_id" --interval-ms 500 --max-polls "$((long_timeout * 2))" --json >"$SMOKE_TMPDIR/long-job-follow.jsonl" &
  long_follow_pid="$!"
  long_follow_start="$SECONDS"
  while kill -0 "$long_follow_pid" >/dev/null 2>&1; do
    sleep 30
    if kill -0 "$long_follow_pid" >/dev/null 2>&1; then
      elapsed=$((SECONDS - long_follow_start))
      long_job_snapshot="$(api_get "/api/v1/jobs/$long_job_id" 2>/dev/null || true)"
      long_job_status="$(jq -r '.status // "unavailable"' <<<"$long_job_snapshot" 2>/dev/null || echo unavailable)"
      target_summary="$(api_get "/api/v1/jobs/$long_job_id/targets" 2>/dev/null \
        | jq -r 'group_by(.status) | map("\(.[0].status)=\(length)") | join(",")' 2>/dev/null \
        || echo "targets=unavailable")"
      smoke_fleet_log "long-running job heartbeat elapsed=${elapsed}s status=$long_job_status targets=$target_summary"
    fi
  done
  wait "$long_follow_pid"
  long_follow_status="$?"
  set -e
  trap 'docker_fleet_smoke_err "$LINENO" "$?"' ERR
  if [[ "$long_follow_status" != "0" ]]; then
    echo "long-running job follow failed:" >&2
    tail -n 20 "$SMOKE_TMPDIR/long-job-follow.jsonl" >&2 || true
    exit "$long_follow_status"
  fi
  long_job_status_json="$(api_get "/api/v1/jobs/$long_job_id")"
  if ! jq -e --argjson expected "$agent_count" '
    .status == "completed" and .target_count == $expected
  ' <<<"$long_job_status_json" >/dev/null; then
    echo "long-running job did not finish completed:" >&2
    jq . <<<"$long_job_status_json" >&2 || true
    echo "long-running target status counts:" >&2
    api_get "/api/v1/jobs/$long_job_id/targets" \
      | jq 'group_by(.status) | map({status: .[0].status, count: length, clients: map(.client_id)})' >&2 || true
    echo "long-running output summary:" >&2
    api_get "/api/v1/jobs/$long_job_id/outputs" \
      | jq '.items | group_by(.client_id) | map({client_id: .[0].client_id, streams: map(.stream), done: map(select(.done)) | length})' >&2 || true
    echo "long-running job follow tail:" >&2
    tail -n 20 "$SMOKE_TMPDIR/long-job-follow.jsonl" >&2 || true
    exit 1
  fi
  api_get "/api/v1/jobs/$long_job_id/targets" | jq -e --argjson expected "$agent_count" '
    length == $expected and all(.[]; .status == "completed" and .exit_code == 0)
  ' >/dev/null
  api_get "/api/v1/jobs/$long_job_id/outputs" | jq -e --argjson expected "$agent_count" '
    ([.items[] | select(.stream == "stdout") | .data_base64 | @base64d] | map(select(. == "docker-long-done\n")) | length) == $expected
  ' >/dev/null
  api_get "/api/v1/system/dashboard?window=1h&chart_points=120" | jq -e --argjson expected "$agent_count" '
    .current.dispatch.total_dispatch_attempts >= $expected and
    .current.targets.deadline_expired_active == 0 and
    .current.gateway_events.status == "live" and
    any(.series[]; .metric == "dispatch.queue_depth")
  ' >/dev/null
  smoke_fleet_log "long-running job $long_job_id completed with all $agent_count targets"
fi

smoke_fleet_log "validating gateway sessions and audit trail"
gateway_session_limit=$((agent_count * 4))
api_get "/api/v1/gateway-sessions?limit=$gateway_session_limit" | jq -e --argjson expected "$agent_count" '
  ([.[] | select(.gateway_id == "docker-fleet-gateway" and .status == "active") | .client_id] | unique | length) == $expected
' >/dev/null

api_get "/api/v1/audit?limit=100" | jq -e '
  any(.[]; .action == "job.dispatch_requested") and
  any(.[]; .action == "network.tunnel_plan_created") and
  any(.[]; .action == "backup.requested_metadata_only")
' >/dev/null

smoke_fleet_log "uploading cleanup source artifact and validating cleanup preview"
cleanup_expression='artifact.domain = "file_transfer_source"'
cleanup_source="$SMOKE_TMPDIR/docker-fleet-q2-capacity-reconciliation.csv"
printf 'account,provider,country,role,instance_count\nacme-network,alpha,US,edge,2\nacme-network,alpha,DE,core,2\nrun,%s,total,%s\n' "$run_id" "$agent_count" >"$cleanup_source"
cleanup_source_json="$(vpsctl_json file-transfer-source-upload \
  --source "$cleanup_source" \
  --name docker-fleet-q2-capacity-reconciliation.csv \
  --confirmed)"
cleanup_source_artifact_id="$(jq -r '.id' <<<"$cleanup_source_json")"
cleanup_source_object_key="$(jq -r '.object_key' <<<"$cleanup_source_json")"
jq -e '
  .name == "docker-fleet-q2-capacity-reconciliation.csv" and
  .size_bytes > 0 and
  (.object_key | startswith("file-transfer-sources/")) and
  (.sha256_hex | length) == 64
' <<<"$cleanup_source_json" >/dev/null
cleanup_preview_json="$(vpsctl_json artifact-cleanup-preview --expression "$cleanup_expression" --domains file_transfer)"
jq -e --arg expression "$cleanup_expression" '
  .expression == $expression and
  (.domains == ["file_transfer"]) and
  .matched_count >= 1 and
  .matched_bytes > 0 and
  (.preview_hash | length) == 64
' <<<"$cleanup_preview_json" >/dev/null

frontend_test_host="${VPSMAN_FRONTEND_TEST_HOST:-localhost}"
smoke_fleet_log "building frontend production assets for live Docker fleet UI smoke"
env \
  VPSMAN_API_PROXY="$api_url" \
  VPSMAN_FRONTEND_SMOKE_ROOT="$ROOT_DIR" \
  bash -ic 'cd "$VPSMAN_FRONTEND_SMOKE_ROOT/frontend" && npm run build'

smoke_fleet_log "running live Docker fleet UI smoke (desktop and mobile sequentially)"
if ! env \
  VPSMAN_API_PROXY="$api_url" \
  VPSMAN_FRONTEND_SMOKE_ROOT="$ROOT_DIR" \
  VPSMAN_FRONTEND_TEST_HOST="$frontend_test_host" \
  VPSMAN_FRONTEND_TEST_PORT="$frontend_port" \
  VPSMAN_FRONTEND_TEST_SERVER_COMMAND="npm run preview -- --host $frontend_test_host --port $frontend_port" \
  VPSMAN_DOCKER_FLEET_UI_SMOKE=1 \
  VPSMAN_DOCKER_FLEET_EXPECTED_TOTAL="$agent_count" \
  VPSMAN_DOCKER_FLEET_PROVIDER_ALPHA_COUNT="$provider_alpha_count" \
  VPSMAN_DOCKER_FLEET_COUNTRY_US_COUNT="$country_us_count" \
  VPSMAN_DOCKER_FLEET_PROVIDER_ALPHA_COUNTRY_US_COUNT="$provider_alpha_country_us_count" \
  VPSMAN_DOCKER_FLEET_ROLE_EDGE_COUNT="$role_edge_count" \
  VPSMAN_DOCKER_FLEET_CLEANUP_EXPRESSION="$cleanup_expression" \
  VPSMAN_DOCKER_FLEET_EXTENDED_REVIEW="$extended_review" \
  VPSMAN_DOCKER_FLEET_USERNAME="$operator_username" \
  VPSMAN_DOCKER_FLEET_PASSWORD="$operator_password" \
  VPSMAN_DOCKER_FLEET_SCREENSHOT_DIR="$screenshot_dir" \
  bash -ic 'cd "$VPSMAN_FRONTEND_SMOKE_ROOT/frontend" && npm run test:ui -- tests/live-docker-fleet.spec.ts --project desktop-chrome --project mobile-chrome --workers=1'; then
  dump_docker_logs "live Docker fleet UI smoke failed"
  exit 1
fi
smoke_fleet_log "live Docker fleet UI smoke passed"

smoke_fleet_log "running artifact cleanup worker validation"
cleanup_worker_log="$SMOKE_TMPDIR/artifact-cleanup-worker.log"
if ! env \
  VPSMAN_POSTGRES_URL="$postgres_url" \
  VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
  VPSMAN_BACKUP_OBJECT_STORE_DIR="$object_store_dir" \
  target/debug/vpsman-worker --once --worker-id docker-fleet-artifact-cleanup --worker-lease-secs 60 \
  >"$cleanup_worker_log" 2>&1; then
  echo "artifact cleanup worker failed" >&2
  cat "$cleanup_worker_log" >&2 || true
  dump_docker_logs "artifact cleanup worker failed"
  exit 1
fi
api_get "/api/v1/server-jobs?limit=20" | jq -e '
  any(.[]; .job_type == "artifact_cleanup" and .status == "completed" and .deleted_count >= 1)
' >/dev/null
cleanup_artifact_status="$(docker exec "$pg_container" psql -U vpsman -d vpsman -tAc "SELECT status FROM server_artifacts WHERE object_key = '$cleanup_source_object_key'")"
if [[ "$cleanup_artifact_status" != "deleted" ]]; then
  echo "expected server artifact to be marked deleted after cleanup, got: $cleanup_artifact_status" >&2
  docker exec "$pg_container" psql -U vpsman -d vpsman -c "SELECT id, domain, object_key, status, deleted_at FROM server_artifacts ORDER BY created_at DESC LIMIT 10" >&2 || true
  exit 1
fi
cleanup_source_count="$(docker exec "$pg_container" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM file_transfer_source_artifacts WHERE id = '$cleanup_source_artifact_id'")"
if [[ "$cleanup_source_count" != "0" ]]; then
  echo "expected file transfer source artifact row to be removed by cleanup" >&2
  exit 1
fi
if [[ -e "$object_store_dir/$cleanup_source_object_key" ]]; then
  echo "expected cleanup worker to remove local object store payload $cleanup_source_object_key" >&2
  find "$object_store_dir" -maxdepth 3 -type f -print >&2 || true
  exit 1
fi

jq -n \
  --arg api_url "$api_url" \
  --arg runtime_image "$runtime_image" \
  --arg screenshot_dir "$screenshot_dir" \
  --arg extended_review "$extended_review" \
  --argjson agent_count "$agent_count" \
  --argjson telemetry_rollups "$telemetry_rollup_count" \
  --arg job_id "$job_id" \
  --arg long_job_id "$long_job_id" \
  '{
    docker_24_agent_fleet_smoke: "ok",
    api_url: $api_url,
    runtime_image: $runtime_image,
    agent_count: $agent_count,
    telemetry_rollups: $telemetry_rollups,
    bulk_job_id: $job_id,
    long_running_job_id: ($long_job_id | if length > 0 then . else null end),
    screenshot_dir: $screenshot_dir,
    extended_review_screenshots: ($extended_review == "1"),
    checks: ([
      "docker_postgres_api_gateway",
      "twenty_plus_docker_agents_online",
      "operator_auth_and_preferences",
      "tag_registry_and_bulk_resolve_any_all",
      "dashboard_scope_filter_and_group_by",
      "system_dashboard_queue_pool_cancel_gateway_counters",
      "telemetry_rollups_network_speed_and_traffic",
      "durable_bulk_job_dispatch_outputs",
      "schedule_registry",
      "topology_plan_create",
      "backup_metadata_request",
      "server_artifact_cleanup_cli_ui_worker",
      "gateway_session_inventory",
      "audit_visibility",
      "desktop_mobile_live_ui_layout",
      "live_extended_review_action_screenshots",
      "grid_multiselect_expand_context_column_controls",
      "sidebar_preferences_and_dashboard_customization"
    ] + (if ($long_job_id | length) > 0 then ["long_running_bulk_job_with_api_backlog"] else [] end))
  }'
