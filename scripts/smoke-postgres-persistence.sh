#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"
source "$ROOT_DIR/scripts/lib-smoke-postgres-backups.sh"

smoke_enter_root
smoke_require_tools curl docker jq python3 shuf timeout

if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsman-api -p vpsman-gateway -p vpsctl -p vpsman-worker
fi

smoke_init_tmpdir "vpsman-postgres-persistence"

pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
container_name="vpsman-postgres-smoke-$(date +%s%N)"
internal_token="postgres-smoke-internal-$(date +%s%N)"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
super_password="postgres-smoke-super-password"
super_salt_hex="01020304"
gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"
api_pid=""
api_log=""
gateway_pid=""
gateway_log=""

cleanup_postgres_smoke() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_postgres_smoke EXIT

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
    VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
    VPSMAN_INTERNAL_TOKEN="$internal_token" \
    VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
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
  smoke_dump_logs "postgres persistence API failed to start" "$SMOKE_TMPDIR"/api-"$label"-*.log
  exit 1
}

stop_api() {
  if [[ -n "$api_pid" ]]; then
    kill "$api_pid" >/dev/null 2>&1 || true
    wait "$api_pid" >/dev/null 2>&1 || true
    api_pid=""
  fi
}

start_gateway() {
  gateway_log="$SMOKE_TMPDIR/gateway.log"
  VPSMAN_GATEWAY_BIND="$gateway_addr" \
  VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
  VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
  VPSMAN_API_URL="$api_url" \
  VPSMAN_INTERNAL_TOKEN="$internal_token" \
  VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
  VPSMAN_GATEWAY_ID=postgres-persistence-gateway \
  RUST_LOG="vpsman_gateway=warn" \
    target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
  gateway_pid="$!"
  smoke_track_pid "$gateway_pid"
  if ! smoke_wait_tcp 127.0.0.1 "$gateway_control_port"; then
    smoke_dump_logs "postgres persistence gateway failed to start" "$gateway_log"
    exit 1
  fi
}

api_get() {
  local path="$1"
  curl -fsS -H "Authorization: Bearer $access_token" "$api_url$path"
}

wait_api_jq() {
  local path="$1"
  local jq_filter="$2"
  local label="$3"
  local body_file="$SMOKE_TMPDIR/wait-$label.json"
  local deadline=$((SECONDS + 10))
  while (( SECONDS < deadline )); do
    if api_get "$path" >"$body_file" && jq -e "$jq_filter" "$body_file" >/dev/null; then
      cat "$body_file"
      return
    fi
    sleep 0.2
  done
  echo "timed out waiting for $label" >&2
  cat "$body_file" >&2 || true
  exit 1
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

api_post_expect_status() {
  local path="$1"
  local json="$2"
  local expected_status="$3"
  local body_file
  local status
  body_file="$SMOKE_TMPDIR/api-post-$(date +%s%N).json"
  status="$(curl -sS \
    -o "$body_file" \
    -w "%{http_code}" \
    -H "Authorization: Bearer $access_token" \
    -H "Content-Type: application/json" \
    -d "$json" \
    "$api_url$path")"
  if [[ "$status" != "$expected_status" ]]; then
    echo "expected HTTP $expected_status from $path, got $status" >&2
    cat "$body_file" >&2 || true
    exit 1
  fi
  cat "$body_file"
}

vpsctl_json() {
  VPSMAN_API_URL="$api_url" \
  VPSMAN_API_TOKEN="$access_token" \
  VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
    target/debug/vpsctl "$@"
}

seed_agent() {
  local client_id="$1"
  local optional_hello_fields=""
  local noise_public_key_json="null"
  if [[ $# -ge 2 && -n "$2" ]]; then
    optional_hello_fields=", \"capabilities\": $2"
  fi
  if [[ $# -ge 3 ]]; then
    noise_public_key_json="\"$3\""
  fi
  curl -fsS \
    -H "Authorization: Bearer $internal_token" \
    -H "Content-Type: application/json" \
    -d "{
      \"gateway_id\": \"postgres-persistence-gateway\",
      \"noise_public_key_hex\": $noise_public_key_json,
      \"hello\": {
        \"client_id\": \"$client_id\",
        \"agent_version\": \"postgres-persistence-smoke\",
        \"os_release\": \"Debian smoke\",
        \"arch\": \"x86_64\"$optional_hello_fields
      }
    }" \
    "$api_url/internal/v1/gateway/agent-hello" >/dev/null
}

seed_telemetry() {
  local client_id="$1"
  local observed_unix="$2"
  local cpu_load="$3"
  local memory_available="$4"
  local disk_available="$5"
  local network_rx="$6"
  local network_tx="$7"
  curl -fsS \
    -H "Authorization: Bearer $internal_token" \
    -H "Content-Type: application/json" \
    -d "{
      \"gateway_id\": \"postgres-persistence-gateway\",
      \"telemetry\": {
        \"client_id\": \"$client_id\",
        \"metrics\": {
          \"observed_unix\": $observed_unix,
          \"hostname\": \"$client_id\",
          \"uptime_secs\": 3600,
          \"cpu\": {
            \"load\": {\"one\": $cpu_load, \"five\": $cpu_load, \"fifteen\": $cpu_load},
            \"cores\": 1
          },
          \"memory\": {
            \"total_bytes\": 268435456,
            \"available_bytes\": $memory_available
          },
          \"disks\": [{\"mountpoint\": \"/\", \"total_bytes\": 10737418240, \"available_bytes\": $disk_available}],
          \"networks\": [{\"interface\": \"eth0\", \"rx_bytes\": $network_rx, \"tx_bytes\": $network_tx}]
        }
      }
    }" \
    "$api_url/internal/v1/gateway/telemetry" >/dev/null
}

validate_agent_identity() {
  local client_id="$1"
  local public_key_hex="$2"
  curl -fsS \
    -H "Authorization: Bearer $internal_token" \
    -H "Content-Type: application/json" \
    -d "{
      \"client_id\": \"$client_id\",
      \"noise_public_key_hex\": \"$public_key_hex\"
    }" \
    "$api_url/internal/v1/gateway/agent-identity"
}

start_gateway
start_api "first"

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d '{"username":"postgres-smoke","password":"postgres-smoke-password"}' \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
jq -e '.operator.username == "postgres-smoke" and .token_type == "Bearer"' <<<"$auth_json" >/dev/null

first_public_key_hex="$(printf '11%.0s' {1..32})"
second_public_key_hex="$(printf '22%.0s' {1..32})"
third_public_key_hex="$(printf '33%.0s' {1..32})"
revoked_replacement_key_hex="$(printf '44%.0s' {1..32})"
vpsctl_json agent-identity-upsert \
  --client-id pg-agent-a \
  --client-public-key-hex "$first_public_key_hex" \
  --display-name pg-edge-a-direct \
  --tags direct:first,direct:initial,edge,bgp,country:US \
  --confirmed | jq -e '
    .client_id == "pg-agent-a" and
    .display_name == "pg-edge-a-direct" and
    (.tags | sort == ["bgp", "country:US", "direct:first", "direct:initial", "edge"])
  ' >/dev/null
first_stored_key_hex="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT encode(public_key, 'hex') FROM clients WHERE id = 'pg-agent-a'")"
if [[ "$first_stored_key_hex" != "$first_public_key_hex" ]]; then
  echo "expected initial direct identity public key to be stored" >&2
  exit 1
fi
api_get "/api/v1/agents" | jq -e '
  any(.[]; .id == "pg-agent-a" and
    .display_name == "pg-edge-a-direct" and
    (.tags | sort == ["bgp", "country:US", "direct:first", "direct:initial", "edge"]))
' >/dev/null

unprivileged_capabilities='{"privilege_mode":"unprivileged","effective_uid":1000,"can_attempt_privileged_ops":true,"can_manage_runtime_tunnels":false,"can_apply_process_limits":false,"unprivileged_hint":"postgres smoke agent is running without root"}'
seed_agent "pg-agent-a" "" "$first_public_key_hex"
seed_agent "pg-agent-b" "$unprivileged_capabilities"
api_post "/api/v1/agents/pg-agent-b/alias" '{"display_name":"pg-edge-b"}' >/dev/null
vpsctl_json agent-tag --client-id pg-agent-b --tag edge >/dev/null
vpsctl_json agent-tag --client-id pg-agent-b --tag bird2 >/dev/null
api_get "/api/v1/agents" | jq -e '
  any(.[]; .id == "pg-agent-b" and
    .capabilities.privilege_mode == "unprivileged" and
    .capabilities.effective_uid == 1000 and
    .capabilities.can_manage_runtime_tunnels == false and
    .capabilities.can_apply_process_limits == false)
' >/dev/null
telemetry_bucket=$(( (($(date +%s) - 600) / 60) * 60 ))
seed_telemetry "pg-agent-a" "$((telemetry_bucket + 10))" 0.5 134217728 5368709120 1000 2000
seed_telemetry "pg-agent-a" "$((telemetry_bucket + 20))" 1.0 100663296 4294967296 4000 8000
seed_telemetry "pg-agent-a" "$((telemetry_bucket - 7200))" 9.9 67108864 2147483648 9000 18000
vpsctl_json telemetry-rollups --client-id pg-agent-a --bucket-secs 60 --limit 10 | jq -e '
  any(.[]; .client_id == "pg-agent-a" and .bucket_secs == 60 and .sample_count == 2 and
    (.cpu_load_1_avg > 0.74 and .cpu_load_1_avg < 0.76) and
    .cpu_load_1_max == 1 and
    .memory_total_bytes_max == 268435456 and
    .memory_available_bytes_min == 100663296 and
    .disk_total_bytes_max == 10737418240 and
    .disk_available_bytes_avg == 4831838208 and
    .disk_available_bytes_min == 4294967296 and
    .network_rx_bytes_max == 4000 and
    .network_tx_bytes_max == 8000)
' >/dev/null
vpsctl_json telemetry-network-rates --client-id pg-agent-a --interface eth0 --bucket-secs 60 --limit 10 | jq -e '
  any(.[]; .client_id == "pg-agent-a" and .interface == "eth0" and .bucket_secs == 60 and
    .sample_count == 2 and
    .rx_bytes_avg == 2500 and
    .tx_bytes_avg == 5000 and
    .rx_bytes_delta == 0 and
    .tx_bytes_delta == 0 and
    .rx_bps_avg == 0 and
    .tx_bps_avg == 0)
' >/dev/null

vpsctl_json agent-tag --client-id pg-agent-a --tag group:pg-persistent >/dev/null
vpsctl_json agent-tag --client-id pg-agent-a --tag persistent >/dev/null

vpsctl_json agent-identity-upsert \
  --client-id pg-agent-a \
  --client-public-key-hex "$second_public_key_hex" \
  --replace-existing-key \
  --confirmed | jq -e '.client_id == "pg-agent-a"' >/dev/null
api_post "/api/v1/agents/pg-agent-a/alias" '{"display_name":"pg-edge-a-rotated"}' >/dev/null
vpsctl_json agent-tag --client-id pg-agent-a --tag rotated >/dev/null
vpsctl_json agent-tag --client-id pg-agent-a --tag os:debian >/dev/null
api_get "/api/v1/agents" | jq -e '
  any(.[]; .id == "pg-agent-a" and
    .display_name == "pg-edge-a-rotated" and
    (.tags | index("edge")) and
    (.tags | index("bgp")) and
    (.tags | index("country:US")) and
    (.tags | index("direct:first")) and
    (.tags | index("direct:initial")) and
    (.tags | index("group:pg-persistent")) and
    (.tags | index("persistent")) and
    (.tags | index("rotated")) and
    (.tags | index("os:debian")))
' >/dev/null
second_stored_key_hex="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT encode(public_key, 'hex') FROM clients WHERE id = 'pg-agent-a'")"
if [[ "$second_stored_key_hex" != "$second_public_key_hex" || "$second_stored_key_hex" == "$first_public_key_hex" ]]; then
  echo "expected direct identity key rotation to update stored public key" >&2
  exit 1
fi
validate_agent_identity pg-agent-a "$first_public_key_hex" | jq -e '.accepted == false' >/dev/null
validate_agent_identity pg-agent-a "$second_public_key_hex" | jq -e '.accepted == true' >/dev/null

vpsctl_json agent-identity-upsert \
  --client-id pg-revoked-agent \
  --client-public-key-hex "$third_public_key_hex" \
  --display-name pg-revoked-agent \
  --tags revoked-test \
  --confirmed | jq -e '.client_id == "pg-revoked-agent"' >/dev/null
validate_agent_identity pg-revoked-agent "$third_public_key_hex" | jq -e '.accepted == true' >/dev/null
vpsctl_json client-key-revoke \
  --client-id pg-revoked-agent \
  --reason postgres-smoke-revoke \
  --confirmed | jq -e '
    .client_id == "pg-revoked-agent" and
    .reason == "postgres-smoke-revoke" and
    (.public_key_sha256_hex | length == 64)
  ' >/dev/null
validate_agent_identity pg-revoked-agent "$third_public_key_hex" | jq -e '.accepted == false' >/dev/null
if vpsctl_json agent-identity-upsert \
  --client-id pg-revoked-agent \
  --client-public-key-hex "$revoked_replacement_key_hex" \
  --replace-existing-key \
  --confirmed >"$SMOKE_TMPDIR/revoked-replace.json" 2>&1; then
  echo "expected revoked direct identity key replacement to fail" >&2
  cat "$SMOKE_TMPDIR/revoked-replace.json" >&2 || true
  exit 1
fi
vpsctl_json key-lifecycle-report | jq -e '
  .current_key_revoked_count == 0 and
  .revocation_count >= 1 and
  any(.clients[]; .client_id == "pg-agent-a" and .current_key_revoked == false) and
  all(.clients[]; .client_id != "pg-revoked-agent")
' >/dev/null
api_get "/api/v1/agents" | jq -e 'all(.[]; .id != "pg-revoked-agent")' >/dev/null
seed_agent "pg-agent-a" "" "$second_public_key_hex"

plan_json="$(api_post "/api/v1/tunnel-plans" '{
  "name": "pg-gre-a-b",
  "interface_name": "gre77",
  "kind": "gre",
  "left_client_id": "pg-agent-a",
  "right_client_id": "pg-agent-b",
  "left_underlay": "203.0.113.77",
  "right_underlay": "203.0.113.78",
  "address_pool_cidr": "10.251.0.0/30",
  "reserved_addresses": [],
  "ipv4_tunnel": {
    "left": "10.251.0.0",
    "right": "10.251.0.1",
    "prefix_len": 31
  },
  "bandwidth": "1000m",
  "latency_ms": 17,
  "packet_loss_ratio": 0,
  "preference": 1.5
}')"
jq -e '.name == "pg-gre-a-b" and .status == "planned" and .plan.mutates_host == false' <<<"$plan_json" >/dev/null

schedule_json="$(vpsctl_json schedule-create \
  --name pg-hourly-uptime \
  --command /usr/bin/uptime \
  --tags edge \
  --cron-expr '* * * * *' \
  --catch-up-policy run_all_limited \
  --catch-up-limit 2 \
  --retry-delay-secs 120 \
  --max-failures 5)"
schedule_id="$(jq -r '.id' <<<"$schedule_json")"
jq -e '.name == "pg-hourly-uptime" and .enabled == true and .command_type == "shell_argv" and .selector_expression == "tag:edge" and (.target_client_ids | sort == ["pg-agent-a","pg-agent-b"]) and .cron_expr == "* * * * *" and .catch_up_policy == "run_all_limited" and .catch_up_limit == 2 and .retry_delay_secs == 120 and .max_failures == 5 and .failure_count == 0' \
  <<<"$schedule_json" >/dev/null
docker exec "$container_name" psql -U vpsman -d vpsman -v ON_ERROR_STOP=1 -c "UPDATE schedules SET next_run_at = now() - interval '2 minutes' WHERE id = '$schedule_id'" >/dev/null
notification_channel_id="11111111-1111-4111-8111-111111111177"
queued_notification_id="11111111-1111-4111-8111-111111111178"
old_notification_id="11111111-1111-4111-8111-111111111179"
contended_notification_id="11111111-1111-4111-8111-111111111180"
docker exec -i "$container_name" psql -U vpsman -d vpsman -v ON_ERROR_STOP=1 >/dev/null <<SQL
INSERT INTO fleet_alert_notification_channels (
  id,
  name,
  scope_kind,
  scope_value,
  min_severity,
  categories,
  operator_states,
  delivery_kind,
  target,
  cooldown_secs,
  enabled,
  notes
)
VALUES (
  '$notification_channel_id',
  'pg-worker-custom',
  'global',
  NULL,
  'warning',
  '["source_readiness"]'::jsonb,
  '["open"]'::jsonb,
  'custom_pager',
  'adapter:custom-pager',
  0,
  TRUE,
  'postgres persistence smoke'
)
ON CONFLICT (name) DO NOTHING;

INSERT INTO fleet_alert_notification_deliveries (
  id,
  channel_id,
  channel_name,
  alert_id,
  alert_severity,
  alert_category,
  status,
  delivery_kind,
  target,
  dedupe_key,
  payload,
  error,
  cooldown_until_unix,
  attempt_count,
  last_attempt_at,
  created_at,
  delivered_at
)
VALUES (
  '$queued_notification_id',
  '$notification_channel_id',
  'pg-worker-custom',
  'source_readiness:server:object_store',
  'warning',
  'source_readiness',
  'queued',
  'custom_pager',
  'adapter:custom-pager',
  'pg-worker-custom-dedupe',
  '{"schema":"vpsman.fleet_alert.notification.v1","alert":{"id":"source_readiness:server:object_store"}}'::jsonb,
  NULL,
  0,
  0,
  NULL,
  now(),
  NULL
);

INSERT INTO fleet_alert_notification_deliveries (
  id,
  channel_id,
  channel_name,
  alert_id,
  alert_severity,
  alert_category,
  status,
  delivery_kind,
  target,
  dedupe_key,
  payload,
  error,
  cooldown_until_unix,
  attempt_count,
  last_attempt_at,
  created_at,
  delivered_at
)
VALUES (
  '$old_notification_id',
  '$notification_channel_id',
  'pg-worker-custom',
  'source_readiness:server:old',
  'warning',
  'source_readiness',
  'delivered',
  'audit_log',
  'audit:fleet',
  'pg-worker-old-dedupe',
  '{"schema":"vpsman.fleet_alert.notification.v1","alert":{"id":"source_readiness:server:old"}}'::jsonb,
  NULL,
  0,
  1,
  now() - interval '120 days',
  now() - interval '120 days',
  now() - interval '120 days'
);
SQL
VPSMAN_POSTGRES_URL="$postgres_url" \
VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
  target/debug/vpsman-worker --once --worker-id pg-postgres-smoke --notification-retention-days 30 >"$SMOKE_TMPDIR/worker-once.log" 2>&1
scheduled_run_count="0"
scheduled_run_job_id=""
deadline=$((SECONDS + 10))
while (( SECONDS < deadline )); do
  scheduled_run_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM jobs WHERE source_schedule_id::text = '$schedule_id' AND command_type LIKE 'scheduled%' AND status IN ('queued','running')")"
  scheduled_run_job_id="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT id FROM jobs WHERE source_schedule_id::text = '$schedule_id' AND command_type LIKE 'scheduled%' AND status IN ('queued','running') ORDER BY created_at DESC, id DESC LIMIT 1")"
  if [[ "$scheduled_run_count" == "2" && -n "$scheduled_run_job_id" ]]; then
    break
  fi
  sleep 0.2
done
if [[ -z "$scheduled_run_job_id" ]]; then
  echo "scheduled run job was not materialized" >&2
  cat "$SMOKE_TMPDIR/worker-once.log" >&2 || true
  docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT now() AS db_now, count(*) AS due_count FROM schedules WHERE enabled = TRUE AND next_run_at <= now()" >&2 || true
  docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT id, name, enabled, next_run_at, catch_up_policy, catch_up_limit, failure_count, last_error FROM schedules" >&2 || true
  docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT task_name, owner, lease_expires_at, updated_at FROM worker_leases" >&2 || true
  docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT id, command_type, status, target_count, source_schedule_id FROM jobs ORDER BY created_at DESC LIMIT 10" >&2 || true
  docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT action, metadata FROM audit_logs ORDER BY created_at DESC LIMIT 10" >&2 || true
  exit 1
fi
if [[ "$scheduled_run_count" != "2" ]]; then
  echo "expected run_all_limited schedule catch-up to materialize two active run jobs" >&2
  docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT id, command_type, status, target_count, source_schedule_id, completed_at FROM jobs ORDER BY created_at DESC LIMIT 10" >&2 || true
  exit 1
fi
api_get "/api/v1/schedules" | jq -e --arg schedule_id "$schedule_id" '
  any(.[]; .id == $schedule_id and .catch_up_policy == "run_all_limited" and .catch_up_limit == 2 and .failure_count == 0 and .last_error == null)
' >/dev/null
worker_lease_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM worker_leases WHERE task_name IN ('schedules','alert_notifications') AND owner = 'pg-postgres-smoke' AND lease_expires_at > now()")"
if [[ "$worker_lease_count" != "2" ]]; then
  echo "expected active worker lease rows for scheduler and alert singleton tasks" >&2
  docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT task_name, owner, lease_expires_at FROM worker_leases ORDER BY task_name" >&2 || true
  exit 1
fi
api_get "/api/v1/jobs/$scheduled_run_job_id" | jq -e --arg job_id "$scheduled_run_job_id" '
  .id == $job_id and (.command_type | startswith("scheduled_")) and (.status == "queued" or .status == "running")
' >/dev/null
api_get "/api/v1/jobs/$scheduled_run_job_id/targets" | jq -e '
  length == 2 and
  (map(.client_id) | sort == ["pg-agent-a","pg-agent-b"]) and
  all(.[]; (.status == "queued" or .status == "dispatching") and .completed_at == null)
' >/dev/null
notification_failed_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM fleet_alert_notification_deliveries WHERE id = '$queued_notification_id' AND status = 'failed' AND attempt_count = 1 AND error LIKE '%not configured%'")"
if [[ "$notification_failed_count" != "1" ]]; then
  echo "expected worker to fail unsupported queued notification exactly once" >&2
  exit 1
fi
old_notification_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM fleet_alert_notification_deliveries WHERE id = '$old_notification_id'")"
if [[ "$old_notification_count" != "0" ]]; then
  echo "expected worker notification retention pruning to delete old delivered notification" >&2
  exit 1
fi
notification_audit_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM audit_logs WHERE action IN ('fleet.alert_notification_deliveries_worker_processed', 'fleet.alert_notification_deliveries_pruned')")"
if [[ "$notification_audit_count" -lt "2" ]]; then
  echo "expected worker notification process and prune audits" >&2
  exit 1
fi
docker exec -i "$container_name" psql -U vpsman -d vpsman -v ON_ERROR_STOP=1 >/dev/null <<SQL
INSERT INTO fleet_alert_notification_deliveries (
  id,
  channel_id,
  channel_name,
  alert_id,
  alert_severity,
  alert_category,
  status,
  delivery_kind,
  target,
  dedupe_key,
  payload,
  error,
  cooldown_until_unix,
  attempt_count,
  last_attempt_at,
  created_at,
  delivered_at
)
VALUES (
  '$contended_notification_id',
  '$notification_channel_id',
  'pg-worker-custom',
  'source_readiness:server:contended',
  'warning',
  'source_readiness',
  'queued',
  'custom_pager',
  'adapter:custom-pager',
  'pg-worker-contended-dedupe',
  '{"schema":"vpsman.fleet_alert.notification.v1","alert":{"id":"source_readiness:server:contended"}}'::jsonb,
  NULL,
  0,
  0,
  NULL,
  now(),
  NULL
);
SQL
VPSMAN_POSTGRES_URL="$postgres_url" \
VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
  target/debug/vpsman-worker --once --worker-id pg-competing-worker --notification-retention-days 30 >/dev/null
contended_notification_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM fleet_alert_notification_deliveries WHERE id = '$contended_notification_id' AND status = 'queued' AND attempt_count = 0")"
if [[ "$contended_notification_count" != "1" ]]; then
  echo "expected competing worker to leave queued notification untouched while lease is active" >&2
  docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT task_name, owner, lease_expires_at FROM worker_leases ORDER BY task_name" >&2 || true
  docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT id, status, attempt_count, error FROM fleet_alert_notification_deliveries WHERE id = '$contended_notification_id'" >&2 || true
  exit 1
fi
backup_json="$(vpsctl_json backup-request \
  --client-id pg-agent-a \
  --paths /etc/hostname \
  --include-config \
  --note "postgres persistence backup" \
  --confirmed)"
backup_id="$(jq -r '.id' <<<"$backup_json")"
jq -e '.client_id == "pg-agent-a" and .status == "requested_metadata_only" and .include_config == true and .command_scope == "client:pg-agent-a" and .artifact_id == null' \
  <<<"$backup_json" >/dev/null

backup_policy_recipient="cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
backup_policy_json="$(vpsctl_json backup-policy-upsert \
  --name pg-nightly-backup \
  --paths /etc/hostname \
  --include-config \
  --recipient-public-key-hex "$backup_policy_recipient" \
  --clients pg-agent-a \
  --tags persistent \
  --cron-expr '0 * * * *' \
  --retention-days 45 \
  --keep-last 12 \
  --rotation-generation keyring/v2 \
  --confirmed)"
backup_policy_schedule_id="$(jq -r '.schedule_id' <<<"$backup_policy_json")"
jq -e --arg recipient "$backup_policy_recipient" '
  .name == "pg-nightly-backup" and
  .enabled == true and
  .selector_expression == "id:pg-agent-a || tag:persistent" and
  .cron_expr == "0 * * * *" and
  .paths == ["/etc/hostname"] and
  .include_config == true and
  .recipient_public_key_hex == $recipient and
  .retention_days == 45 and
  .keep_last == 12 and
  .rotation_generation == "keyring/v2"
' <<<"$backup_policy_json" >/dev/null

artifact_json="$(vpsctl_json backup-artifact-record \
  --backup-request-id "$backup_id" \
  --object-key backups/pg-agent-a/postgres-persistence.cbor.zst.age \
  --sha256-hex aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
  --size-bytes 4096 \
  --confirmed)"
artifact_id="$(jq -r '.id' <<<"$artifact_json")"
jq -e '.client_id == "pg-agent-a" and .object_key == "backups/pg-agent-a/postgres-persistence.cbor.zst.age" and .encrypted == true and .size_bytes == 4096' \
  <<<"$artifact_json" >/dev/null
api_get "/api/v1/backup-artifacts?limit=10" | jq -e --arg artifact_id "$artifact_id" '
  any(.[]; .id == $artifact_id and .client_id == "pg-agent-a" and .sha256_hex == "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
' >/dev/null

smoke_postgres_backup_policy_prune_evidence

restore_json="$(vpsctl_json restore-plan \
  --source-backup-request-id "$backup_id" \
  --target-client-id pg-agent-b \
  --paths /etc/hostname \
  --include-config \
  --destination-root /restore \
  --note "postgres persistence restore" \
  --confirmed)"
restore_id="$(jq -r '.id' <<<"$restore_json")"
jq -e --arg backup_id "$backup_id" '.source_backup_request_id == $backup_id and .source_client_id == "pg-agent-a" and .target_client_id == "pg-agent-b" and .status == "planned_metadata_only" and .destination_root == "/restore" and .command_scope == "client:pg-agent-b"' \
  <<<"$restore_json" >/dev/null

degraded_update_json="$(vpsctl_json agent-update \
  --artifact-url https://updates.example/vpsman-agent \
  --sha256-hex bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb \
  --clients pg-agent-b \
  --confirmed)"
degraded_update_job_id="$(jq -r '.job_id' <<<"$degraded_update_json")"
jq -e '(has("accepted" + "_targets") | not) and .target_count == 1 and .target_counts.total == 1 and .target_counts.skipped == 1 and .status == "partial_success"' \
  <<<"$degraded_update_json" >/dev/null
wait_api_jq "/api/v1/jobs/$degraded_update_job_id/targets" '
  length == 1 and .[0].client_id == "pg-agent-b" and .[0].status == "skipped" and .[0].completed_at != null
' "degraded-update-targets" >/dev/null
wait_api_jq "/api/v1/jobs/$degraded_update_job_id/outputs" '
  length == 1 and
  (.[0].data_base64 | @base64d | fromjson | .reason == "target_agent_lacks_agent_update_capability")
' "degraded-update-outputs" >/dev/null

degraded_process_json="$(vpsctl_json process-start \
  --name pg-limited-worker \
  --argv /usr/bin/sleep \
  --argv 60 \
  --memory-max-bytes 1048576 \
  --clients pg-agent-b \
  --confirmed)"
degraded_process_job_id="$(jq -r '.job_id' <<<"$degraded_process_json")"
jq -e '(has("accepted" + "_targets") | not) and .target_count == 1 and .target_counts.total == 1 and .target_counts.skipped == 1 and .status == "partial_success"' \
  <<<"$degraded_process_json" >/dev/null
wait_api_jq "/api/v1/jobs/$degraded_process_job_id/targets" '
  length == 1 and .[0].client_id == "pg-agent-b" and .[0].status == "skipped" and .[0].completed_at != null
' "degraded-process-targets" >/dev/null
wait_api_jq "/api/v1/jobs/$degraded_process_job_id/outputs" '
  length == 1 and
  (.[0].data_base64 | @base64d | fromjson | .reason == "target_agent_lacks_process_limit_capability")
' "degraded-process-outputs" >/dev/null

rejected_job_json="$(api_post_expect_status "/api/v1/jobs" '{
  "selector_expression": "id:pg-agent-a || tag:edge",
  "target_client_ids": ["pg-agent-a"],
  "command": "uptime",
  "argv": ["/usr/bin/uptime"],
  "operation": null,
  "timeout_secs": 5,
  "privileged": true,
  "confirmed": true
}' "403")"
jq -e '.error == "privilege_assertion_required" and .status == 403' \
  <<<"$rejected_job_json" >/dev/null

command_template_request="$(jq -n '{
  name: "pg-smoke-tag-uptime",
  scope_kind: "tag",
  scope_value: "edge",
  operation: {
    type: "shell",
    argv: ["/usr/bin/uptime"],
    pty: false
  },
  defaults: {
    timeout_secs: 15,
    confirmed: true
  },
  confirmed: true
}')"
command_template_json="$(api_post "/api/v1/command-templates" "$command_template_request")"
command_template_id="$(jq -r '.id' <<<"$command_template_json")"
jq -e '.name == "pg-smoke-tag-uptime" and .scope_kind == "tag" and .scope_value == "edge" and .command_type == "shell_argv" and .operation.type == "shell"' \
  <<<"$command_template_json" >/dev/null
api_get "/api/v1/command-templates?limit=20&scope_kind=tag&scope_value=edge" | jq -e --arg template_id "$command_template_id" '
  any(.[]; .id == $template_id and .name == "pg-smoke-tag-uptime" and .command_type == "shell_argv")
' >/dev/null

rejected_job_payload="$(jq -n '{
  selector_expression: "id:pg-agent-a",
  target_client_ids: ["pg-agent-a"],
  command: "idempotent-uptime",
  argv: ["/usr/bin/uptime"],
  operation: null,
  timeout_secs: 5,
  privileged: true,
  confirmed: true
}')"
rejected_job_first="$(api_post_expect_status "/api/v1/jobs" "$rejected_job_payload" "403")"
jq -e '.error == "privilege_assertion_required" and .status == 403' \
  <<<"$rejected_job_first" >/dev/null
rejected_job_second="$(api_post_expect_status "/api/v1/jobs" "$rejected_job_payload" "403")"
jq -e '.error == "privilege_assertion_required" and .status == 403' \
  <<<"$rejected_job_second" >/dev/null

audit_json="$(api_get "/api/v1/audit?limit=200")"
jq -e '
  any(.[]; .action == "agent_identity.upserted" and .target == "client:pg-agent-a") and
  any(.[]; .action == "client_key.revoked" and .target == "client:pg-revoked-agent") and
  any(.[]; .action == "network.tunnel_plan_created") and
  any(.[]; .action == "schedule.created") and
  any(.[]; .action == "fleet.alert_notification_deliveries_worker_processed") and
  any(.[]; .action == "fleet.alert_notification_deliveries_pruned") and
  any(.[]; .action == "backup.requested_metadata_only") and
  any(.[]; .action == "backup.artifact_metadata_recorded") and
  any(.[]; .action == "backup_policy.retention_pruned") and
  any(.[]; .action == "restore.planned_metadata_only") and
  any(.[]; .action == "command_template.upserted")
' <<<"$audit_json" >/dev/null || {
  jq -r '.[].action' <<<"$audit_json" | sort | uniq -c >&2
  exit 1
}

stop_api
start_api "restart"

api_get "/api/v1/auth/me" | jq -e '.username == "postgres-smoke"' >/dev/null
api_get "/api/v1/fleet/summary" | jq -e '.total == 2 and .online == 2' >/dev/null
api_get "/api/v1/agents" | jq -e '
  any(.[]; .id == "pg-agent-a" and .display_name == "pg-edge-a-rotated" and (.tags | index("persistent")) and (.tags | index("rotated")) and (.tags | index("os:debian"))) and
  any(.[]; .id == "pg-agent-b" and (.tags | index("bird2")) and .capabilities.privilege_mode == "unprivileged" and .capabilities.can_apply_process_limits == false)
' >/dev/null
persisted_rotated_key_hex="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT encode(public_key, 'hex') FROM clients WHERE id = 'pg-agent-a'")"
if [[ "$persisted_rotated_key_hex" != "$second_public_key_hex" ]]; then
  echo "expected rotated public key to persist across API restart" >&2
  exit 1
fi
api_get "/api/v1/key-lifecycle/report" | jq -e '
  .current_key_revoked_count == 0 and
  .revocation_count >= 1 and
  any(.clients[]; .client_id == "pg-agent-a" and .current_key_revoked == false)
' >/dev/null
api_get "/api/v1/telemetry/rollups?client_id=pg-agent-a&bucket_secs=60&limit=10" | jq -e '
  any(.[]; .client_id == "pg-agent-a" and .sample_count == 2 and
    .memory_available_bytes_avg == 117440512 and
    .disk_available_bytes_avg == 4831838208 and
    .network_rx_bytes_max == 4000 and
    .network_tx_bytes_max == 8000)
' >/dev/null
api_get "/api/v1/telemetry/network-rates?client_id=pg-agent-a&interface=eth0&bucket_secs=60&limit=10" | jq -e '
  any(.[]; .client_id == "pg-agent-a" and .interface == "eth0" and .sample_count == 2 and
    .rx_bytes_avg == 2500 and
    .tx_bytes_avg == 5000 and
    .rx_bytes_delta == 0 and
    .tx_bytes_delta == 0 and
    .rx_bps_avg == 0 and
    .tx_bps_avg == 0)
' >/dev/null
api_post "/api/v1/bulk/resolve" '{"selector_expression":"tag:edge"}' \
  | jq -e '.target_count == 2 and (.targets | map(.id) | sort == ["pg-agent-a","pg-agent-b"])' >/dev/null
api_get "/api/v1/tunnel-plans" | jq -e '
  any(.[]; .name == "pg-gre-a-b" and .status == "planned" and .plan.mutates_host == false)
' >/dev/null
api_get "/api/v1/schedules" | jq -e --arg schedule_id "$schedule_id" '
  any(.[]; .id == $schedule_id and .name == "pg-hourly-uptime" and .enabled == true and .command_type == "shell_argv" and .selector_expression == "tag:edge" and (.target_client_ids | sort == ["pg-agent-a","pg-agent-b"]))
' >/dev/null
api_get "/api/v1/backups?limit=10" | jq -e --arg backup_id "$backup_id" --arg artifact_id "$artifact_id" '
  any(.[]; .id == $backup_id and .client_id == "pg-agent-a" and .status == "artifact_metadata_recorded" and .include_config == true and .command_scope == "client:pg-agent-a" and .artifact_id == $artifact_id)
' >/dev/null
api_get "/api/v1/backup-artifacts?limit=10" | jq -e --arg artifact_id "$artifact_id" '
  any(.[]; .id == $artifact_id and .client_id == "pg-agent-a" and .object_key == "backups/pg-agent-a/postgres-persistence.cbor.zst.age" and .encrypted == true and .size_bytes == 4096)
' >/dev/null
api_get "/api/v1/backup-policies" | jq -e --arg schedule_id "$backup_policy_schedule_id" --arg recipient "$backup_policy_recipient" '
  any(.[]; .schedule_id == $schedule_id and .name == "pg-nightly-backup" and .recipient_public_key_hex == $recipient and .retention_days == 45 and .keep_last == 12 and .rotation_generation == "keyring/v2")
' >/dev/null
api_get "/api/v1/backup-policies" | jq -e --arg schedule_id "$backup_prune_policy_schedule_id" '
  any(.[]; .schedule_id == $schedule_id and .name == "pg-prune-policy" and .retention_days == 1 and .keep_last == 1 and .rotation_generation == "prune/v1")
' >/dev/null
api_get "/api/v1/backups?limit=20" | jq -e \
  --arg old_a "$prune_old_a_request_id" \
  --arg old_b "$prune_old_b_request_id" \
  --arg retained "$prune_retained_request_id" \
  --arg retained_artifact "$prune_retained_artifact_id" \
  --arg schedule_id "$backup_prune_policy_schedule_id" '
    any(.[]; .id == $old_a and .source_schedule_id == $schedule_id and .artifact_id == null and .status == "requested_metadata_only") and
    any(.[]; .id == $old_b and .source_schedule_id == $schedule_id and .artifact_id == null and .status == "requested_metadata_only") and
    any(.[]; .id == $retained and .source_schedule_id == $schedule_id and .artifact_id == $retained_artifact and .status == "artifact_metadata_recorded")
  ' >/dev/null
api_get "/api/v1/backup-artifacts?limit=20" | jq -e \
  --arg old_a "$prune_old_a_artifact_id" \
  --arg old_b "$prune_old_b_artifact_id" \
  --arg retained "$prune_retained_artifact_id" '
    (all(.[]; .id != $old_a and .id != $old_b)) and
    any(.[]; .id == $retained and .object_key == "backups/pg-agent-a/policy-prune-retained.cbor.zst.age")
  ' >/dev/null
api_get "/api/v1/restore-plans?limit=10" | jq -e --arg restore_id "$restore_id" --arg backup_id "$backup_id" '
  any(.[]; .id == $restore_id and .source_backup_request_id == $backup_id and .source_client_id == "pg-agent-a" and .target_client_id == "pg-agent-b" and .status == "planned_metadata_only" and .destination_root == "/restore" and .command_scope == "client:pg-agent-b")
' >/dev/null
api_get "/api/v1/jobs/$scheduled_run_job_id" | jq -e --arg job_id "$scheduled_run_job_id" '
  .id == $job_id and (.command_type | startswith("scheduled_")) and (.status == "queued" or .status == "running")
' >/dev/null
api_get "/api/v1/jobs/$scheduled_run_job_id/targets" | jq -e '
  length == 2 and
  (map(.client_id) | sort == ["pg-agent-a","pg-agent-b"]) and
  all(.[]; (.status == "queued" or .status == "dispatching") and .completed_at == null)
' >/dev/null
wait_api_jq "/api/v1/jobs/$degraded_update_job_id/targets" '
  length == 1 and .[0].client_id == "pg-agent-b" and .[0].status == "skipped" and .[0].completed_at != null
' "degraded-update-targets-restart" >/dev/null
wait_api_jq "/api/v1/jobs/$degraded_update_job_id/outputs" '
  length == 1 and
  (.[0].data_base64 | @base64d | fromjson | .reason == "target_agent_lacks_agent_update_capability")
' "degraded-update-outputs-restart" >/dev/null
wait_api_jq "/api/v1/jobs/$degraded_process_job_id/targets" '
  length == 1 and .[0].client_id == "pg-agent-b" and .[0].status == "skipped" and .[0].completed_at != null
' "degraded-process-targets-restart" >/dev/null
wait_api_jq "/api/v1/jobs/$degraded_process_job_id/outputs" '
  length == 1 and
  (.[0].data_base64 | @base64d | fromjson | .reason == "target_agent_lacks_process_limit_capability")
' "degraded-process-outputs-restart" >/dev/null
audit_json="$(api_get "/api/v1/audit?limit=200")"
jq -e '
  any(.[]; .action == "agent_identity.upserted" and .target == "client:pg-agent-a") and
  any(.[]; .action == "network.tunnel_plan_created") and
  any(.[]; .action == "schedule.created") and
  any(.[]; .action == "backup_policy.upserted") and
  any(.[]; .action == "backup_policy.retention_pruned") and
  any(.[]; .action == "backup.requested_metadata_only") and
  any(.[]; .action == "backup.artifact_metadata_recorded") and
  any(.[]; .action == "restore.planned_metadata_only") and
  any(.[]; .action == "command_template.upserted")
' <<<"$audit_json" >/dev/null || {
  jq -r '.[].action' <<<"$audit_json" | sort | uniq -c >&2
  exit 1
}

jq -n \
  --arg api_url "$api_url" \
  '{
    postgres_persistence_smoke: "ok",
    api_url: $api_url,
    checks: ["auth_session", "agents", "direct_identity_key_rotation", "client_key_revocation", "telemetry_minute_rollups", "telemetry_minute_network_rates", "tag_bulk", "tunnel_plan", "schedule", "backup_policy", "backup_policy_retention_prune", "worker_leases", "alert_notification_worker", "missing_privilege_rejection", "capability_degraded_update", "capability_degraded_process_limit", "backup_artifact_metadata", "backup_restore_metadata", "audit", "api_restart"]
  }'
