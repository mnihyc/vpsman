#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash curl docker google-chrome jq python3 shuf timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-docker-24-agent-fault-fuzz"

agent_count="${VPSMAN_DOCKER_FLEET_AGENT_COUNT:-24}"
if ((agent_count < 20)); then
  smoke_fail "VPSMAN_DOCKER_FLEET_AGENT_COUNT must be at least 20"
fi
gateway_command_output_ttl_secs="${VPSMAN_DOCKER_FLEET_COMMAND_OUTPUT_TTL_SECS:-86400}"

rollup_bucket_secs=60
run_id="docker-fault-fuzz-$(date +%s%N)"
label_key="vpsman.smoke.run"
pg_port="$(smoke_free_port)"
api_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
proxy_port="$(smoke_free_port)"
frontend_port="$(smoke_free_port)"
api_url="http://127.0.0.1:$api_port"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
postgres_url="postgres://vpsman:vpsman@127.0.0.1:$pg_port/vpsman"
internal_token="docker-fault-fuzz-internal-$(date +%s%N)"
operator_username="docker-fleet-admin"
operator_password="docker-fleet-password-$(date +%s%N)"
super_password="docker-fault-fuzz-super-password"
super_salt_hex="0102030405060708090a0b0c0d0e0f100102030405060708090a0b0c0d0e0f10"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"
object_store_dir="$SMOKE_TMPDIR/object-store"
screenshot_dir="$ROOT_DIR/tmp/docker-24-agent-fault-fuzz-$run_id"
runtime_image="${VPSMAN_DOCKER_FLEET_RUNTIME_IMAGE:-ubuntu:24.04}"
proxy_log="$SMOKE_TMPDIR/network-proxy.log"
proxy_stats="$SMOKE_TMPDIR/network-proxy-stats.json"
mkdir -p "$object_store_dir" "$screenshot_dir"

pg_container="vpsman-$run_id-postgres"
api_container="vpsman-$run_id-api"
gateway_container="vpsman-$run_id-gateway"

cleanup_fault_fuzz_smoke() {
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

dump_docker_logs() {
  local title="$1"
  local container
  echo "$title" >&2
  while IFS= read -r container; do
    [[ -n "$container" ]] || continue
    echo "--- docker logs: $container ---" >&2
    docker logs "$container" >&2 || true
  done < <(docker ps -a --filter "label=$label_key=$run_id" --format '{{.Names}}' | sort)
  if [[ -f "$proxy_log" ]]; then
    echo "--- proxy log: $proxy_log ---" >&2
    cat "$proxy_log" >&2 || true
  fi
  docker ps -a --filter "label=$label_key=$run_id" >&2 || true
}

on_error() {
  local status=$?
  local line="${1:-unknown}"
  dump_docker_logs "24-agent fault fuzz failed at line $line (exit $status)"
  exit "$status"
}

trap cleanup_fault_fuzz_smoke EXIT
trap 'on_error "$LINENO"' ERR

api_get() {
  local path="$1"
  curl -fsS -H "Authorization: Bearer $access_token" "$api_url$path"
}

vpsctl_json() {
  VPSMAN_API_URL="$api_url" \
  VPSMAN_API_TOKEN="$access_token" \
  VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
    target/debug/vpsctl "$@"
}

agent_name() {
  printf 'vpsman-%s-agent-%s' "$run_id" "$1"
}

agent_dir() {
  printf '%s/docker-fault-agent-%02d' "$SMOKE_TMPDIR" "$1"
}

agent_id() {
  printf 'docker-fleet-%02d' "$1"
}

agent_status() {
  local client_id="$1"
  api_get "/api/v1/agents" | jq -r --arg client_id "$client_id" '
    map(select(.id == $client_id))[0].status // "missing"
  '
}

wait_online_count() {
  local expected="$1"
  local max_timeout_secs="$2"
  local label="$3"
  local deadline=$((SECONDS + max_timeout_secs))
  local online
  until [[ "${online:-}" == "$expected" ]]; do
    if ((SECONDS >= deadline)); then
      dump_docker_logs "timed out waiting for $label online count"
      api_get "/api/v1/fleet/summary" >&2 || true
      api_get "/api/v1/agents" >&2 || true
      exit 1
    fi
    online="$(api_get "/api/v1/fleet/summary" | jq -r '.online')"
    sleep 0.5
  done
}

wait_clients_status() {
  local expected="$1"
  shift
  local max_timeout_secs="$1"
  shift
  local deadline=$((SECONDS + max_timeout_secs))
  local client status all_ready
  while true; do
    all_ready=1
    for client in "$@"; do
      status="$(agent_status "$client")"
      if [[ "$status" != "$expected" ]]; then
        all_ready=0
        break
      fi
    done
    if [[ "$all_ready" -eq 1 ]]; then
      return 0
    fi
    if ((SECONDS >= deadline)); then
      api_get "/api/v1/agents" >&2 || true
      smoke_fail "timed out waiting for clients to become $expected: $*"
    fi
    sleep 0.5
  done
}

wait_active_gateway_sessions() {
  local expected="$1"
  local max_timeout_secs="$2"
  local deadline=$((SECONDS + max_timeout_secs))
  local active
  until [[ "${active:-}" == "$expected" ]]; do
    if ((SECONDS >= deadline)); then
      api_get "/api/v1/gateway-sessions?limit=200" >&2 || true
      smoke_fail "timed out waiting for $expected active gateway sessions"
    fi
    active="$(api_get "/api/v1/gateway-sessions?limit=200" | jq -r '
      [.[] | select(.gateway_id == "docker-fault-fuzz-gateway" and .status == "active")] | length
    ')"
    sleep 0.5
  done
}

wait_telemetry_ready() {
  local expected="$1"
  local max_timeout_secs="$2"
  local deadline=$((SECONDS + max_timeout_secs))
  local ready
  until (( ${ready:-0} >= expected )); do
    if ((SECONDS >= deadline)); then
      docker exec "$pg_container" psql -U vpsman -d vpsman -c \
        "SELECT client_id, bucket_start, sample_count FROM telemetry_rollups ORDER BY client_id, bucket_start DESC" >&2 || true
      smoke_fail "timed out waiting for telemetry from at least $expected agents"
    fi
    ready="$(docker exec "$pg_container" psql -U vpsman -d vpsman -tAc \
      "SELECT count(DISTINCT client_id) FROM telemetry_rollups WHERE bucket_secs = $rollup_bucket_secs AND sample_count >= 2")"
    sleep 1
  done
}

start_agent_container() {
  local index="$1"
  local name
  name="$(agent_name "$index")"
  local dir
  dir="$(agent_dir "$index")"
  docker run -d \
    --name "$name" \
    --network host \
    --label "$label_key=$run_id" \
    --memory 128m \
    --cpus 0.5 \
    --pids-limit 96 \
    -e RUST_LOG=vpsman_agent=warn \
    -e VPSMAN_SUPERVISOR_DIR="$dir/supervisor" \
    -v "$ROOT_DIR:$ROOT_DIR" \
    -w "$ROOT_DIR" \
    "$runtime_image" \
    "$ROOT_DIR/target/debug/vpsman-agent" --config "$dir/agent.toml" run >/dev/null
}

write_network_proxy() {
  local proxy_script="$1"
  cat >"$proxy_script" <<'PY'
import argparse
import asyncio
import json
from pathlib import Path


def parse_args():
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen-host", default="127.0.0.1")
    parser.add_argument("--listen-port", type=int, required=True)
    parser.add_argument("--target-host", default="127.0.0.1")
    parser.add_argument("--target-port", type=int, required=True)
    parser.add_argument("--latency-ms", type=int, default=0)
    parser.add_argument("--drop-accept-every", type=int, default=0)
    parser.add_argument("--close-after-secs", type=float, default=0.0)
    parser.add_argument("--close-first", type=int, default=0)
    parser.add_argument("--stats", required=True)
    return parser.parse_args()


args = parse_args()
stats = {
    "accepted": 0,
    "connected": 0,
    "dropped_connections": 0,
    "forced_closes": 0,
    "upstream_failures": 0,
    "closed": 0,
}
stats_path = Path(args.stats)


def write_stats():
    stats_path.write_text(json.dumps(stats, sort_keys=True), encoding="utf-8")


async def close_later(conn_id, *writers):
    await asyncio.sleep(args.close_after_secs)
    stats["forced_closes"] += 1
    write_stats()
    print(f"FORCED_CLOSE conn={conn_id}", flush=True)
    for writer in writers:
        writer.close()


async def relay(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            if args.latency_ms > 0:
                await asyncio.sleep(args.latency_ms / 1000)
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.IncompleteReadError):
        pass
    finally:
        writer.close()


async def handle_client(client_reader, client_writer):
    stats["accepted"] += 1
    conn_id = stats["accepted"]
    write_stats()
    peer = client_writer.get_extra_info("peername")
    print(f"ACCEPT conn={conn_id} peer={peer}", flush=True)

    if args.drop_accept_every > 0 and conn_id % args.drop_accept_every == 0:
        stats["dropped_connections"] += 1
        write_stats()
        print(f"DROP_CONNECT conn={conn_id}", flush=True)
        client_writer.close()
        await client_writer.wait_closed()
        return

    try:
        upstream_reader, upstream_writer = await asyncio.open_connection(
            args.target_host,
            args.target_port,
        )
    except OSError as exc:
        stats["upstream_failures"] += 1
        write_stats()
        print(f"UPSTREAM_FAIL conn={conn_id} error={exc}", flush=True)
        client_writer.close()
        await client_writer.wait_closed()
        return

    stats["connected"] += 1
    write_stats()
    if args.close_after_secs > 0 and (args.close_first == 0 or conn_id <= args.close_first):
        asyncio.create_task(close_later(conn_id, client_writer, upstream_writer))

    await asyncio.gather(
        relay(client_reader, upstream_writer),
        relay(upstream_reader, client_writer),
        return_exceptions=True,
    )
    stats["closed"] += 1
    write_stats()
    print(f"CLOSED conn={conn_id}", flush=True)


async def main():
    write_stats()
    server = await asyncio.start_server(handle_client, args.listen_host, args.listen_port)
    print(
        f"LISTEN {args.listen_host}:{args.listen_port} -> {args.target_host}:{args.target_port}",
        flush=True,
    )
    async with server:
        await server.serve_forever()


asyncio.run(main())
PY
}

docker run -d \
  --name "$pg_container" \
  --label "$label_key=$run_id" \
  -e POSTGRES_DB=vpsman \
  -e POSTGRES_PASSWORD=vpsman \
  -e POSTGRES_USER=vpsman \
  -p "127.0.0.1:$pg_port:5432" \
  postgres:16-alpine >/dev/null

deadline=$((SECONDS + 45))
until docker exec "$pg_container" psql -U vpsman -d vpsman -tAc 'select 1' >/dev/null 2>&1; do
  if ((SECONDS >= deadline)); then
    dump_docker_logs "timed out waiting for postgres"
    exit 1
  fi
  sleep 0.25
done
smoke_wait_tcp 127.0.0.1 "$pg_port"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"

docker run -d \
  --name "$api_container" \
  --network host \
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

if ! smoke_wait_http "$api_url/health"; then
  dump_docker_logs "API did not become healthy"
  exit 1
fi

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"$operator_username\",\"password\":\"$operator_password\"}" \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
jq -e '.operator.username == "docker-fleet-admin" and .operator.role == "admin"' <<<"$auth_json" >/dev/null

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
  -e VPSMAN_GATEWAY_ID=docker-fault-fuzz-gateway \
  -e VPSMAN_GATEWAY_RECONNECT_GRACE_SECS=2 \
  -e VPSMAN_SUITE_CONFIG="$VPSMAN_SUITE_CONFIG" \
  -e VPSMAN_GATEWAY_SPOOL_DIR="$SMOKE_TMPDIR/gateway-spool" \
  -e VPSMAN_GATEWAY_COMMAND_OUTPUT_EVENT_TTL_SECS="$gateway_command_output_ttl_secs" \
  -e RUST_LOG=vpsman_gateway=warn \
  -v "$ROOT_DIR:$ROOT_DIR" \
  -w "$ROOT_DIR" \
  "$runtime_image" \
  "$ROOT_DIR/target/debug/vpsman-gateway" >/dev/null

if ! smoke_wait_tcp 127.0.0.1 "$gateway_port"; then
  dump_docker_logs "gateway agent listener did not start"
  exit 1
fi
if ! smoke_wait_tcp 127.0.0.1 "$gateway_control_port"; then
  dump_docker_logs "gateway control listener did not start"
  exit 1
fi

proxy_script="$SMOKE_TMPDIR/network_proxy.py"
write_network_proxy "$proxy_script"
python3 "$proxy_script" \
  --listen-host 127.0.0.1 \
  --listen-port "$proxy_port" \
  --target-host 127.0.0.1 \
  --target-port "$gateway_port" \
  --latency-ms 120 \
  --drop-accept-every 5 \
  --close-after-secs 4 \
  --close-first 18 \
  --stats "$proxy_stats" >"$proxy_log" 2>&1 &
smoke_track_pid "$!"
if ! smoke_wait_tcp 127.0.0.1 "$proxy_port"; then
  dump_docker_logs "fault proxy did not start"
  exit 1
fi

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
for ((i = 1; i <= agent_count; i += 1)); do
  index=$((i - 1))
  provider="${providers[$((index % ${#providers[@]}))]}"
  country="${countries[$((index % ${#countries[@]}))]}"
  role="${roles[$((index % ${#roles[@]}))]}"
  logical_client_id="$(agent_id "$i")"
  display_name="$(printf 'df-%s-%s-%02d' "$provider" "$country" "$i")"
  if ((i > agent_count / 2)); then
    endpoint_csv="lossy=127.0.0.1:$proxy_port=10"
    network_tag="network:lossy-proxy"
  else
    endpoint_csv="primary=$gateway_addr=10"
    network_tag="network:direct"
  fi
  tag_csv="provider:$provider,country:$country,role:$role,audit:docker-fuzz,bulk-target,$network_tag"

  dir="$(agent_dir "$i")"
  mkdir -p "$dir/state"
  smoke_create_direct_agent_config \
    "$api_url" \
    "$access_token" \
    "$dir/agent.toml" \
    "$logical_client_id" \
    "$display_name" \
    "$tag_csv" \
    "$gateway_public_hex" \
    "$endpoint_csv" \
    90
  start_agent_container "$i"
done

wait_online_count "$agent_count" 150 "initial 24-agent fault-fuzz fleet"
wait_active_gateway_sessions "$agent_count" 60
wait_telemetry_ready 20 90

disconnect_indexes=(2 6)
disconnect_clients=()
for index in "${disconnect_indexes[@]}"; do
  disconnect_clients+=("$(agent_id "$index")")
  docker stop "$(agent_name "$index")" >/dev/null
done
wait_clients_status disconnected 45 "${disconnect_clients[@]}"
sleep 3

expected_partial_completed=$((agent_count - ${#disconnect_indexes[@]}))
partial_job_json="$(vpsctl_json job-shell \
  --script 'printf "fault-fuzz-partial-ok\n"' \
  --tags audit:docker-fuzz \
  --max-timeout-secs 45 \
  --confirmed)"
partial_job_id="$(jq -r '.job_id' <<<"$partial_job_json")"
smoke_assert_job_create_queued "$partial_job_json" "$agent_count"
partial_final="$(smoke_wait_api_job_status "$api_url" "$partial_job_id" "terminal" 120 "$access_token")"
jq -e --argjson expected "$agent_count" '
  .status == "partial_success" and .target_count == $expected
' <<<"$partial_final" >/dev/null
api_get "/api/v1/jobs/$partial_job_id/targets" | jq -e \
  --argjson expected "$agent_count" \
  --argjson completed "$expected_partial_completed" \
  --argjson control_timeout "${#disconnect_indexes[@]}" '
    length == $expected and
    ([.[] | select(.status == "completed")] | length) == $completed and
    ([.[] | select(.status == "control_timeout" and (.message | contains("agent_not_online")))] | length) == $control_timeout
  ' >/dev/null

for index in 9 10 11 12; do
  docker restart "$(agent_name "$index")" >/dev/null
done
for index in 13 14; do
  docker rm -f "$(agent_name "$index")" >/dev/null
  start_agent_container "$index"
done
for index in "${disconnect_indexes[@]}"; do
  docker start "$(agent_name "$index")" >/dev/null
done

wait_online_count "$agent_count" 180 "recovered fault-fuzz fleet"
wait_active_gateway_sessions "$agent_count" 90
wait_telemetry_ready 20 90

api_get "/api/v1/agents" | jq -e --argjson expected "$agent_count" '
  length == $expected and
  ([.[].id] | unique | length) == $expected and
  all(.[]; .status == "online") and
  ([.[] | select(.tags | index("network:lossy-proxy"))] | length) >= 10
' >/dev/null

api_get "/api/v1/gateway-sessions?limit=200" | jq -e --argjson expected "$agent_count" '
  ([.[] | select(.gateway_id == "docker-fault-fuzz-gateway" and .status == "active")] | length) == $expected and
  ([.[] | select(.status == "ended")] | length) >= 6
' >/dev/null

if [[ ! -f "$proxy_stats" ]]; then
  smoke_fail "fault proxy did not write stats"
fi
jq -e '
  .accepted >= 12 and
  .connected >= 10 and
  .dropped_connections >= 1 and
  .forced_closes >= 1
' "$proxy_stats" >/dev/null

full_job_json="$(vpsctl_json job-shell \
  --script 'printf "fault-fuzz-recovered-ok\n"' \
  --tags audit:docker-fuzz \
  --max-timeout-secs 60 \
  --confirmed)"
full_job_id="$(jq -r '.job_id' <<<"$full_job_json")"
smoke_assert_job_create_queued "$full_job_json" "$agent_count"
full_final="$(smoke_wait_api_job_status "$api_url" "$full_job_id" "terminal" 180 "$access_token")"
jq -e --argjson expected "$agent_count" '
  .status == "completed" and .target_count == $expected
' <<<"$full_final" >/dev/null
api_get "/api/v1/jobs/$full_job_id/targets" | jq -e --argjson expected "$agent_count" '
  length == $expected and all(.[]; .status == "completed" and .exit_code == 0)
' >/dev/null
api_get "/api/v1/jobs/$full_job_id/outputs" | jq -e --argjson expected "$agent_count" '
  ([.items[] | select(.stream == "stdout") | .data_base64 | @base64d] | map(select(. == "fault-fuzz-recovered-ok\n")) | length) == $expected
' >/dev/null

tunnel_plan_json="$(vpsctl_json tunnel-plan \
  --name docker-fault-fuzz-gre \
  --interface-name greff \
  --kind gre \
  --left-client-id "$(agent_id 1)" \
  --right-client-id "$(agent_id 2)" \
  --left-underlay 203.0.113.41 \
  --right-underlay 203.0.113.42 \
  --address-pool-cidr 10.254.25.0/30 \
  --left-tunnel-ipv4-cidr 10.254.25.0/31 \
  --right-tunnel-ipv4-cidr 10.254.25.1/31 \
  --bandwidth 1000m \
  --latency-ms 18 \
  --save \
  --enabled \
  --confirmed)"
jq -e '.name == "docker-fault-fuzz-gre" and .status == "planned"' <<<"$tunnel_plan_json" >/dev/null

alert_policy_json="$(vpsctl_json alert-policy upsert \
  --name docker-edge-resource-alerts \
  --selector 'tag:role:edge' \
  --rule 'cpu.load_1 >= 0.5' \
  --severity warning \
  --notes docker-fault-fuzz-live-review \
  --confirmed)"
jq -e '
  .name == "docker-edge-resource-alerts" and
  .selector_expression == "tag:role:edge" and
  .enabled == true and
  (.rules | length) == 1
' <<<"$alert_policy_json" >/dev/null

alert_notification_channel_json="$(vpsctl_json fleet-alert-notification-channel-upsert \
  --name docker-resource-audit \
  --scope-kind global \
  --min-severity warning \
  --categories resource \
  --operator-states open \
  --delivery-kind audit_log \
  --target audit:fleet \
  --cooldown-secs 600 \
  --notes docker-fault-fuzz-live-review \
  --confirmed)"
alert_notification_channel_id="$(jq -r '.id' <<<"$alert_notification_channel_json")"
jq -e '
  .name == "docker-resource-audit" and
  .scope_kind == "global" and
  .delivery_kind == "audit_log" and
  .enabled == true
' <<<"$alert_notification_channel_json" >/dev/null

alert_notification_custom_channel_json="$(vpsctl_json fleet-alert-notification-channel-upsert \
  --name docker-resource-pager \
  --scope-kind global \
  --min-severity warning \
  --categories resource \
  --operator-states open \
  --delivery-kind custom_pager \
  --target adapter:docker-pager \
  --cooldown-secs 600 \
  --notes docker-fault-fuzz-live-review-custom \
  --confirmed)"
jq -e '
  .name == "docker-resource-pager" and
  .delivery_kind == "custom_pager" and
  .enabled == true
' <<<"$alert_notification_custom_channel_json" >/dev/null

alert_notification_dispatch_json="$(vpsctl_json fleet-alert-notification-dispatch \
  --category resource \
  --include-muted \
  --confirmed \
  --limit 50)"
jq -e --arg channel_id "$alert_notification_channel_id" '
  length >= 1 and
  any(.[]; .channel_id == $channel_id and .status == "delivered")
' <<<"$alert_notification_dispatch_json" >/dev/null

cleanup_expression='artifact.domain = "file_transfer_source"'
cleanup_source="$SMOKE_TMPDIR/docker-fleet-q2-capacity-reconciliation.csv"
printf 'account,provider,country,role,instance_count\nacme-network,alpha,US,edge,2\nacme-network,alpha,DE,core,2\nrun,%s,total,%s\n' "$run_id" "$agent_count" >"$cleanup_source"
cleanup_source_json="$(vpsctl_json file-transfer-source-upload \
  --source "$cleanup_source" \
  --name docker-fleet-q2-capacity-reconciliation.csv \
  --confirmed)"
jq -e '
  .name == "docker-fleet-q2-capacity-reconciliation.csv" and
  .size_bytes > 0 and
  (.object_key | startswith("file-transfer-sources/")) and
  (.sha256_hex | length) == 64
' <<<"$cleanup_source_json" >/dev/null

if ! env \
  VPSMAN_API_PROXY="$api_url" \
  VPSMAN_FRONTEND_SMOKE_ROOT="$ROOT_DIR" \
  VPSMAN_FRONTEND_TEST_PORT="$frontend_port" \
  VPSMAN_DOCKER_FLEET_UI_SMOKE=1 \
  VPSMAN_DOCKER_FLEET_EXPECTED_TOTAL="$agent_count" \
  VPSMAN_DOCKER_FLEET_PROVIDER_ALPHA_COUNT="$provider_alpha_count" \
  VPSMAN_DOCKER_FLEET_COUNTRY_US_COUNT="$country_us_count" \
  VPSMAN_DOCKER_FLEET_PROVIDER_ALPHA_COUNTRY_US_COUNT="$provider_alpha_country_us_count" \
  VPSMAN_DOCKER_FLEET_ROLE_EDGE_COUNT="$role_edge_count" \
  VPSMAN_DOCKER_FLEET_CLEANUP_EXPRESSION="$cleanup_expression" \
  VPSMAN_DOCKER_FLEET_USERNAME="$operator_username" \
  VPSMAN_DOCKER_FLEET_PASSWORD="$operator_password" \
  VPSMAN_DOCKER_FLEET_SCREENSHOT_DIR="$screenshot_dir" \
  bash -ic 'cd "$VPSMAN_FRONTEND_SMOKE_ROOT/frontend" && npm run test:ui -- tests/live-docker-fleet.spec.ts --project desktop-chrome --project mobile-chrome'; then
  dump_docker_logs "post-fault live Docker fleet UI smoke failed"
  exit 1
fi

telemetry_rollup_count="$(docker exec "$pg_container" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM telemetry_rollups WHERE bucket_secs = $rollup_bucket_secs")"
jq -n \
  --arg api_url "$api_url" \
  --arg runtime_image "$runtime_image" \
  --arg screenshot_dir "$screenshot_dir" \
  --arg partial_job_id "$partial_job_id" \
  --arg full_job_id "$full_job_id" \
  --argjson agent_count "$agent_count" \
  --argjson telemetry_rollups "$telemetry_rollup_count" \
  --slurpfile proxy "$proxy_stats" \
  '{
    docker_24_agent_fault_fuzz_smoke: "ok",
    api_url: $api_url,
    runtime_image: $runtime_image,
    agent_count: $agent_count,
    telemetry_rollups: $telemetry_rollups,
    partial_job_id: $partial_job_id,
    recovered_job_id: $full_job_id,
    network_proxy: $proxy[0],
    screenshot_dir: $screenshot_dir,
    checks: [
      "twenty_plus_enrolled_agents_online",
      "lossy_proxy_latency_and_connection_drops",
      "disconnected_subset_partial_dispatch_accounting",
      "container_reboot_recovery",
      "same_identity_snapshot_recreate_recovery",
      "no_duplicate_agent_identity_after_recovery",
      "gateway_session_churn_records_active_and_ended_sessions",
      "post_fault_full_fleet_dispatch_outputs",
      "post_fault_desktop_mobile_live_ui_all_subpanels"
    ]
  }'
