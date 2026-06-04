#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools docker grep
smoke_init_tmpdir "vpsman-bird2-topology"

image="${VPSMAN_BIRD2_TOPOLOGY_IMAGE:-debian:bookworm-slim}"
container_script="$SMOKE_TMPDIR/bird2-topology-container.sh"

cat >"$container_script" <<'CONTAINER_SH'
#!/bin/sh
set -eu

cleanup() {
  if [ -f /tmp/bird-left/bird.pid ]; then
    kill "$(cat /tmp/bird-left/bird.pid)" >/dev/null 2>&1 || true
  fi
  if [ -f /tmp/bird-right/bird.pid ]; then
    kill "$(cat /tmp/bird-right/bird.pid)" >/dev/null 2>&1 || true
  fi
  ip netns del left >/dev/null 2>&1 || true
  ip netns del right >/dev/null 2>&1 || true
}
trap cleanup EXIT

ip netns add left
ip netns add right
ip link add veth-left type veth peer name veth-right
ip link set veth-left netns left
ip link set veth-right netns right

ip -n left addr add 198.51.100.1/30 dev veth-left
ip -n right addr add 198.51.100.2/30 dev veth-right
ip -n left link set lo up
ip -n right link set lo up
ip -n left link set veth-left up
ip -n right link set veth-right up
ip netns exec left sh -c 'echo 1 >/proc/sys/net/ipv6/conf/all/forwarding'
ip netns exec right sh -c 'echo 1 >/proc/sys/net/ipv6/conf/all/forwarding'

ip -n left tunnel add tunlr mode gre remote 198.51.100.2 local 198.51.100.1 ttl 255
ip -n right tunnel add tunlr mode gre remote 198.51.100.1 local 198.51.100.2 ttl 255
ip -n left addr add 10.255.0.0 peer 10.255.0.1 dev tunlr
ip -n right addr add 10.255.0.1 peer 10.255.0.0 dev tunlr
ip -n left link set tunlr up
ip -n right link set tunlr up
ip netns exec left ping -c 1 -W 1 10.255.0.1 >/dev/null

mkdir -p /tmp/bird-left /tmp/bird-right
cat >/tmp/bird-left/vpsman-ospf.conf <<'EOF'
# vpsman-managed bird2 begin left-a right-b left-right tunlr
# vpsman gre tunnel left-right: left-a -> right-b
interface "tunlr" {
  type ptp;
  cost 10;
};
# vpsman-managed bird2 end left-a right-b left-right tunlr
EOF

cat >/tmp/bird-right/vpsman-ospf.conf <<'EOF'
# vpsman-managed bird2 begin right-b left-a left-right tunlr
# vpsman gre tunnel left-right: right-b -> left-a
interface "tunlr" {
  type ptp;
  cost 10;
};
# vpsman-managed bird2 end right-b left-a left-right tunlr
EOF

cat >/tmp/bird-left/bird.conf <<'EOF'
router id 192.0.2.1;
protocol device { scan time 1; }
protocol direct {}
protocol ospf v3 vpsman_ospf {
  ipv6 { import all; export all; };
  area 0 {
    include "/tmp/bird-left/vpsman-ospf.conf";
  };
}
EOF

cat >/tmp/bird-right/bird.conf <<'EOF'
router id 192.0.2.2;
protocol device { scan time 1; }
protocol direct {}
protocol ospf v3 vpsman_ospf {
  ipv6 { import all; export all; };
  area 0 {
    include "/tmp/bird-right/vpsman-ospf.conf";
  };
}
EOF

/usr/sbin/bird -p -c /tmp/bird-left/bird.conf
/usr/sbin/bird -p -c /tmp/bird-right/bird.conf
ip netns exec left /usr/sbin/bird -c /tmp/bird-left/bird.conf -s /tmp/bird-left/bird.ctl -P /tmp/bird-left/bird.pid
ip netns exec right /usr/sbin/bird -c /tmp/bird-right/bird.conf -s /tmp/bird-right/bird.ctl -P /tmp/bird-right/bird.pid

left_neighbors=""
right_neighbors=""
for _ in $(seq 1 35); do
  left_neighbors="$(ip netns exec left /usr/sbin/birdc -s /tmp/bird-left/bird.ctl show ospf neighbors || true)"
  right_neighbors="$(ip netns exec right /usr/sbin/birdc -s /tmp/bird-right/bird.ctl show ospf neighbors || true)"
  if printf '%s\n' "$left_neighbors" | grep -q '192.0.2.2.*Full/PtP' \
    && printf '%s\n' "$right_neighbors" | grep -q '192.0.2.1.*Full/PtP'; then
    break
  fi
  sleep 1
done

printf '%s\n' "$left_neighbors" | grep -q '192.0.2.2.*Full/PtP'
printf '%s\n' "$right_neighbors" | grep -q '192.0.2.1.*Full/PtP'
ip netns exec left /usr/sbin/birdc -s /tmp/bird-left/bird.ctl show ospf interface \
  | grep -q 'Cost: 10'

echo "bird2_topology_convergence_smoke=ok"
CONTAINER_SH
chmod 0755 "$container_script"

docker run --rm \
  --privileged \
  -v "$container_script:/smoke/bird2-topology-container.sh:ro" \
  "$image" \
  sh -ec 'apt-get update >/dev/null && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends bird2 iproute2 iputils-ping >/dev/null && sh /smoke/bird2-topology-container.sh'
