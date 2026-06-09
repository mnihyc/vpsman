#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk docker file grep jq python3 shuf stat timeout

agent_bin="target/x86_64-unknown-linux-musl/release/vpsman-agent"
gateway_bin="target/debug/vpsman-gateway"
latency_ms="${VPSMAN_AGENT_RECONNECT_LATENCY_MS:-150}"
drop_first_connections="${VPSMAN_AGENT_RECONNECT_DROP_FIRST_CONNECTIONS:-1}"
deadline_secs="${VPSMAN_AGENT_RECONNECT_DEADLINE_SECS:-60}"
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
if ((binary_size_bytes > binary_size_limit_bytes)); then
  echo "agent release binary exceeds size budget: $binary_size_bytes > $binary_size_limit_bytes" >&2
  exit 1
fi

smoke_init_tmpdir "vpsman-agent-reconnect-churn"

gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"
proxy_port="$(smoke_free_port)"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_addr="127.0.0.1:$gateway_control_port"
proxy_addr="127.0.0.1:$proxy_port"
internal_token="agent-reconnect-internal-token-$(date +%s%N)"
gateway_log="$SMOKE_TMPDIR/gateway.log"
proxy_log="$SMOKE_TMPDIR/proxy.log"
proxy_stats="$SMOKE_TMPDIR/proxy-stats.json"
proxy_script="$SMOKE_TMPDIR/latency_drop_proxy.py"
agent_config="$SMOKE_TMPDIR/agent.toml"
network_root="$SMOKE_TMPDIR/network-root"
container_name="vpsman-agent-reconnect-$(date +%s%N)"
mkdir -p "$network_root"

cleanup_agent_reconnect_smoke() {
  smoke_cleanup
  docker rm -f "$container_name" >/dev/null 2>&1 || true
}
trap cleanup_agent_reconnect_smoke EXIT

cat >"$proxy_script" <<'PY'
#!/usr/bin/env python3
import json
import os
import select
import socket
import sys
import threading
import time

listen_host = sys.argv[1]
listen_port = int(sys.argv[2])
target_host = sys.argv[3]
target_port = int(sys.argv[4])
latency = int(sys.argv[5]) / 1000.0
drop_first = int(sys.argv[6])
stats_path = sys.argv[7]

lock = threading.Lock()
stats = {
    "accepted_connections": 0,
    "dropped_connections": 0,
    "proxied_connections": 0,
    "latency_ms": int(latency * 1000),
}

def write_stats():
    tmp = f"{stats_path}.tmp"
    with open(tmp, "w", encoding="utf-8") as handle:
        json.dump(stats, handle, sort_keys=True)
    os.replace(tmp, stats_path)

def bump(key):
    with lock:
        stats[key] += 1
        write_stats()
        return stats["accepted_connections"]

def pump(src, dst, stop_event):
    try:
        while not stop_event.is_set():
            readable, _, _ = select.select([src], [], [], 0.5)
            if not readable:
                continue
            data = src.recv(16384)
            if not data:
                break
            if latency > 0:
                time.sleep(latency)
            dst.sendall(data)
    except OSError:
        pass
    finally:
        stop_event.set()
        for sock in (src, dst):
            try:
                sock.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            try:
                sock.close()
            except OSError:
                pass

def handle(client):
    accepted = bump("accepted_connections")
    if accepted <= drop_first:
        bump("dropped_connections")
        time.sleep(max(latency, 0.05))
        client.close()
        return
    try:
        upstream = socket.create_connection((target_host, target_port), timeout=5)
    except OSError:
        client.close()
        raise
    bump("proxied_connections")
    stop_event = threading.Event()
    left = threading.Thread(target=pump, args=(client, upstream, stop_event), daemon=True)
    right = threading.Thread(target=pump, args=(upstream, client, stop_event), daemon=True)
    left.start()
    right.start()

server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind((listen_host, listen_port))
server.listen(16)
write_stats()
while True:
    conn, _ = server.accept()
    threading.Thread(target=handle, args=(conn,), daemon=True).start()
PY
chmod 0755 "$proxy_script"

cat >"$agent_config" <<EOF
client_id = "reconnect-smoke"
display_name = "reconnect-smoke"
telemetry_light_secs = 5
telemetry_full_secs = 60
tags = ["reconnect-smoke"]

[noise]
mode = "dev_xx"

[auth]
command_timeout_secs = 30

[network]
root_dir = "$network_root"

[[tcp_endpoints]]
label = "latency-drop-proxy"
tcp_addr = "$proxy_addr"
priority = 10
EOF

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="$gateway_control_addr" \
VPSMAN_GATEWAY_NOISE_MODE="dev_xx" \
VPSMAN_DEBUG_INTERNAL_TEST_MODE=true \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
RUST_LOG="vpsman_gateway=warn" \
  "$gateway_bin" >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_tcp 127.0.0.1 "$gateway_port"
smoke_wait_tcp 127.0.0.1 "$gateway_control_port"

python3 "$proxy_script" \
  127.0.0.1 "$proxy_port" 127.0.0.1 "$gateway_port" \
  "$latency_ms" "$drop_first_connections" "$proxy_stats" \
  >"$proxy_log" 2>&1 &
smoke_track_pid "$!"
proxy_deadline=$((SECONDS + 10))
until [[ -s "$proxy_stats" ]]; do
  if ((SECONDS >= proxy_deadline)); then
    smoke_dump_logs "latency/drop proxy did not start" "$proxy_log"
    exit 1
  fi
  sleep 0.1
done

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

deadline=$((SECONDS + deadline_secs))
until docker logs "$container_name" 2>&1 | grep -q "gateway accepted agent"; do
  if ! docker ps -q --filter "name=^/${container_name}$" | grep -q .; then
    smoke_dump_logs "reconnect-churn agent container exited before reconnecting" \
      "$gateway_log" "$proxy_log" "$proxy_stats"
    docker logs "$container_name" >&2 || true
    exit 1
  fi
  if ((SECONDS >= deadline)); then
    smoke_dump_logs "agent did not reconnect through latency/drop proxy" \
      "$gateway_log" "$proxy_log" "$proxy_stats"
    docker logs "$container_name" >&2 || true
    exit 1
  fi
  sleep 0.25
done

agent_logs="$(docker logs "$container_name" 2>&1)"
grep -q "gateway session failed" <<<"$agent_logs"

accepted_connections="$(jq -r '.accepted_connections' "$proxy_stats")"
dropped_connections="$(jq -r '.dropped_connections' "$proxy_stats")"
proxied_connections="$(jq -r '.proxied_connections' "$proxy_stats")"
if ((accepted_connections < 2 || dropped_connections < drop_first_connections || proxied_connections < 1)); then
  smoke_dump_logs "latency/drop proxy did not exercise reconnect path" \
    "$gateway_log" "$proxy_log" "$proxy_stats"
  docker logs "$container_name" >&2 || true
  exit 1
fi

jq -n \
  --argjson binary_size_bytes "$binary_size_bytes" \
  --argjson binary_size_limit_bytes "$binary_size_limit_bytes" \
  --argjson latency_ms "$latency_ms" \
  --argjson drop_first_connections "$drop_first_connections" \
  --argjson accepted_connections "$accepted_connections" \
  --argjson dropped_connections "$dropped_connections" \
  --argjson proxied_connections "$proxied_connections" \
  '{
    agent_reconnect_churn_smoke: "ok",
    checks: [
      "release_static_musl_binary",
      "constrained_128m_container",
      "latency_injected_proxy",
      "forced_connection_loss",
      "gateway_session_failure_observed",
      "successful_reconnect"
    ],
    binary_size_bytes: $binary_size_bytes,
    binary_size_limit_bytes: $binary_size_limit_bytes,
    latency_ms: $latency_ms,
    drop_first_connections: $drop_first_connections,
    accepted_connections: $accepted_connections,
    dropped_connections: $dropped_connections,
    proxied_connections: $proxied_connections
  }'
