#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk docker file grep jq shuf stat timeout

agent_bin="target/x86_64-unknown-linux-musl/release/vpsman-agent"
gateway_bin="target/debug/vpsman-gateway"
rss_limit_kib="${VPSMAN_AGENT_RSS_LIMIT_KIB:-15360}"
cpu_limit_percent="${VPSMAN_AGENT_CPU_LIMIT_PERCENT:-2.0}"
threads_limit="${VPSMAN_AGENT_THREADS_LIMIT:-16}"
binary_size_limit_bytes="${VPSMAN_AGENT_BINARY_SIZE_LIMIT_BYTES:-10485760}"

if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsman-gateway
  cargo build -p vpsman-agent --release --target x86_64-unknown-linux-musl
fi
if [[ ! -x "$gateway_bin" ]]; then
  cargo build -p vpsman-gateway
fi
if [[ ! -x "$agent_bin" ]]; then
  cargo build -p vpsman-agent --release --target x86_64-unknown-linux-musl
fi

file "$agent_bin" | grep -Eq 'statically linked|static-pie linked'
binary_size_bytes="$(stat -c '%s' "$agent_bin")"
if (( binary_size_bytes > binary_size_limit_bytes )); then
  echo "agent release binary exceeds size budget: $binary_size_bytes > $binary_size_limit_bytes" >&2
  exit 1
fi

smoke_init_tmpdir "vpsman-agent-resource-budget"

gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
gateway_addr="127.0.0.1:$gateway_port"
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_config="$SMOKE_TMPDIR/agent.toml"
network_root="$SMOKE_TMPDIR/network-root"
container_name="vpsman-agent-resource-$(date +%s%N)"
mkdir -p "$network_root"

cleanup_agent_resource_smoke() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_agent_resource_smoke EXIT

cat >"$agent_config" <<EOF
client_id = "resource-smoke"
display_name = "resource-smoke"
telemetry_light_secs = 5
telemetry_full_secs = 60
tags = ["resource-smoke"]

[noise]
mode = "dev_xx"

[auth]
command_timeout_secs = 30

[network]
root_dir = "$network_root"

[[tcp_endpoints]]
label = "local"
tcp_addr = "$gateway_addr"
priority = 10
EOF

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_NOISE_MODE="dev_xx" \
RUST_LOG="vpsman_gateway=warn" \
  "$gateway_bin" >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_tcp 127.0.0.1 "$gateway_port"
smoke_wait_tcp 127.0.0.1 "$gateway_control_port"

docker run --rm -d \
  --name "$container_name" \
  --network host \
  --memory 128m \
  --cpus 1.0 \
  --pids-limit 64 \
  -e RUST_LOG=vpsman_agent=info \
  -v "$SMOKE_ROOT_DIR:$SMOKE_ROOT_DIR" \
  -w "$SMOKE_ROOT_DIR" \
  alpine:3.20 \
  "$SMOKE_ROOT_DIR/$agent_bin" --config "$agent_config" run >/dev/null

deadline=$((SECONDS + 30))
until docker logs "$container_name" 2>&1 | grep -q "gateway accepted agent"; do
  if ! docker ps -q --filter "name=^/${container_name}$" | grep -q .; then
    smoke_dump_logs "resource-budget agent container exited before connecting" "$gateway_log"
    docker logs "$container_name" >&2 || true
    exit 1
  fi
  if (( SECONDS >= deadline )); then
    smoke_dump_logs "resource-budget agent did not connect" "$gateway_log"
    docker logs "$container_name" >&2 || true
    exit 1
  fi
  sleep 0.25
done

container_cpu_ticks() {
  docker exec "$container_name" cat /proc/1/stat | awk '{print $14 + $15}'
}

read_container_status() {
  docker exec "$container_name" cat /proc/1/status
}

clk_tck="$(getconf CLK_TCK)"
start_ticks="$(container_cpu_ticks)"
start_ns="$(date +%s%N)"
max_rss_kib=0
max_threads=0

for _ in 1 2 3 4 5; do
  status="$(read_container_status)"
  rss_kib="$(awk '$1 == "VmRSS:" {print $2}' <<<"$status")"
  threads="$(awk '$1 == "Threads:" {print $2}' <<<"$status")"
  if [[ -z "$rss_kib" || -z "$threads" ]]; then
    echo "failed to read agent resource status" >&2
    docker logs "$container_name" >&2 || true
    exit 1
  fi
  if (( rss_kib > max_rss_kib )); then
    max_rss_kib="$rss_kib"
  fi
  if (( threads > max_threads )); then
    max_threads="$threads"
  fi
  sleep 1
done

end_ticks="$(container_cpu_ticks)"
end_ns="$(date +%s%N)"
elapsed_ns=$((end_ns - start_ns))
delta_ticks=$((end_ticks - start_ticks))
cpu_percent="$(
  awk -v ticks="$delta_ticks" -v hz="$clk_tck" -v ns="$elapsed_ns" \
    'BEGIN {
      if (ns <= 0 || hz <= 0) {
        printf "0.000"
      } else {
        printf "%.3f", ((ticks / hz) / (ns / 1000000000)) * 100
      }
    }'
)"

if (( max_rss_kib > rss_limit_kib )); then
  echo "agent idle RSS exceeds budget: ${max_rss_kib}KiB > ${rss_limit_kib}KiB" >&2
  docker logs "$container_name" >&2 || true
  exit 1
fi
if (( max_threads > threads_limit )); then
  echo "agent thread count exceeds budget: $max_threads > $threads_limit" >&2
  docker logs "$container_name" >&2 || true
  exit 1
fi
if ! awk -v actual="$cpu_percent" -v limit="$cpu_limit_percent" 'BEGIN { exit(actual <= limit ? 0 : 1) }'; then
  echo "agent idle CPU exceeds budget: ${cpu_percent}% > ${cpu_limit_percent}%" >&2
  docker logs "$container_name" >&2 || true
  exit 1
fi

post_start_ticks="$(container_cpu_ticks)"
post_start_ns="$(date +%s%N)"
post_workload_max_rss_kib="$max_rss_kib"
post_workload_max_threads="$max_threads"

for _ in 1 2 3; do
  sleep 2
  status="$(read_container_status)"
  rss_kib="$(awk '$1 == "VmRSS:" {print $2}' <<<"$status")"
  threads="$(awk '$1 == "Threads:" {print $2}' <<<"$status")"
  if [[ -z "$rss_kib" || -z "$threads" ]]; then
    echo "failed to read post-telemetry agent resource status" >&2
    docker logs "$container_name" >&2 || true
    exit 1
  fi
  if (( rss_kib > post_workload_max_rss_kib )); then
    post_workload_max_rss_kib="$rss_kib"
  fi
  if (( threads > post_workload_max_threads )); then
    post_workload_max_threads="$threads"
  fi
done

post_end_ticks="$(container_cpu_ticks)"
post_end_ns="$(date +%s%N)"
post_elapsed_ns=$((post_end_ns - post_start_ns))
post_delta_ticks=$((post_end_ticks - post_start_ticks))
post_workload_cpu_percent="$(
  awk -v ticks="$post_delta_ticks" -v hz="$clk_tck" -v ns="$post_elapsed_ns" \
    'BEGIN {
      if (ns <= 0 || hz <= 0) {
        printf "0.000"
      } else {
        printf "%.3f", ((ticks / hz) / (ns / 1000000000)) * 100
      }
    }'
)"

if (( post_workload_max_rss_kib > rss_limit_kib )); then
  echo "agent post-telemetry RSS exceeds budget: ${post_workload_max_rss_kib}KiB > ${rss_limit_kib}KiB" >&2
  docker logs "$container_name" >&2 || true
  exit 1
fi
if (( post_workload_max_threads > threads_limit )); then
  echo "agent post-telemetry thread count exceeds budget: $post_workload_max_threads > $threads_limit" >&2
  docker logs "$container_name" >&2 || true
  exit 1
fi
if ! awk -v actual="$post_workload_cpu_percent" -v limit="$cpu_limit_percent" 'BEGIN { exit(actual <= limit ? 0 : 1) }'; then
  echo "agent post-telemetry CPU exceeds budget: ${post_workload_cpu_percent}% > ${cpu_limit_percent}%" >&2
  docker logs "$container_name" >&2 || true
  exit 1
fi

jq -n \
  --argjson binary_size_bytes "$binary_size_bytes" \
  --argjson binary_size_limit_bytes "$binary_size_limit_bytes" \
  --argjson max_rss_kib "$max_rss_kib" \
  --argjson rss_limit_kib "$rss_limit_kib" \
  --argjson max_threads "$max_threads" \
  --argjson threads_limit "$threads_limit" \
  --argjson cpu_percent "$cpu_percent" \
  --argjson cpu_limit_percent "$cpu_limit_percent" \
  --argjson post_workload_max_rss_kib "$post_workload_max_rss_kib" \
  --argjson post_workload_max_threads "$post_workload_max_threads" \
  --argjson post_workload_cpu_percent "$post_workload_cpu_percent" \
  '{
    agent_resource_budget_smoke: "ok",
    checks: [
      "release_static_musl_binary",
      "connected_noise_session",
      "constrained_128m_container",
      "idle_rss_budget",
      "idle_cpu_budget",
      "thread_budget",
      "repeated_constrained_resource_sampling",
      "post_telemetry_rss_budget",
      "post_telemetry_cpu_budget",
      "post_telemetry_thread_budget"
    ],
    binary_size_bytes: $binary_size_bytes,
    binary_size_limit_bytes: $binary_size_limit_bytes,
    max_rss_kib: $max_rss_kib,
    rss_limit_kib: $rss_limit_kib,
    max_threads: $max_threads,
    threads_limit: $threads_limit,
    cpu_percent: $cpu_percent,
    cpu_limit_percent: $cpu_limit_percent,
    post_workload_max_rss_kib: $post_workload_max_rss_kib,
    post_workload_max_threads: $post_workload_max_threads,
    post_workload_cpu_percent: $post_workload_cpu_percent
  }'
