#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools docker grep
smoke_init_tmpdir "vpsman-network-preset"

image="${VPSMAN_NETWORK_PRESET_IMAGE:-debian:bookworm-slim}"
container_script="$SMOKE_TMPDIR/network-preset-container.sh"

cat >"$container_script" <<'CONTAINER_SH'
#!/bin/sh
set -eu

bird_pid=""
cleanup() {
  if [ -n "$bird_pid" ] && kill -0 "$bird_pid" >/dev/null 2>&1; then
    kill "$bird_pid" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

mkdir -p /run/network /run/bird /etc/network/interfaces.d /etc/bird
ip addr add 198.51.100.10/32 dev lo 2>/dev/null || true

cat >/etc/network/interfaces <<'EOF'
source /etc/network/interfaces.d/*
EOF

cat >/etc/network/interfaces.d/vpsman-tunnels <<'EOF'
# vpsman-managed ifupdown begin left-a right-b left-right tunlr
# vpsman tunnel left-right: generated plan only
auto tunlr
iface tunlr inet static
    address 10.255.0.0
    netmask 255.255.255.254
    pointopoint 10.255.0.1
    pre-up ip tunnel add $IFACE mode gre remote 203.0.113.20 local 198.51.100.10 ttl 255
    up ip link set $IFACE up
    post-down ip tunnel del $IFACE || true
# vpsman-managed ifupdown end left-a right-b left-right tunlr
EOF

cat >/etc/bird/vpsman-ospf.conf <<'EOF'
# vpsman-managed bird2 begin left-a right-b left-right tunlr
# vpsman gre tunnel left-right: left-a -> right-b
interface "tunlr" {
  type ptp;
  cost 10;
};
# vpsman-managed bird2 end left-a right-b left-right tunlr
EOF

cat >/etc/bird/bird.conf <<'EOF'
router id 192.0.2.1;
protocol device {}
protocol direct {}
protocol ospf v3 vpsman_ospf {
  ipv6 { import all; export all; };
  area 0 {
    include "/etc/bird/vpsman-ospf.conf";
  };
}
EOF

/usr/sbin/ifreload -a -s
/usr/sbin/bird -p -c /etc/bird/bird.conf
/usr/sbin/bird -c /etc/bird/bird.conf -P /run/bird/bird.pid
sleep 0.2
bird_pid="$(cat /run/bird/bird.pid)"

/usr/sbin/ifreload -a
/usr/sbin/birdc configure
ip -d link show tunlr | grep -q 'gre remote 203.0.113.20 local 198.51.100.10'

/usr/sbin/ifdown -f tunlr
if ip link show tunlr >/dev/null 2>&1; then
  echo "pre-rollback ifdown did not remove tunlr" >&2
  exit 1
fi

cat >/etc/network/interfaces.d/vpsman-tunnels <<'EOF'
# vpsman managed tunnels removed by rollback smoke
EOF
cat >/etc/bird/vpsman-ospf.conf <<'EOF'
# vpsman managed Bird2 interfaces removed by rollback smoke
EOF

/usr/sbin/ifreload -a -s
/usr/sbin/bird -p -c /etc/bird/bird.conf
/usr/sbin/ifreload -a
/usr/sbin/birdc configure
if ip link show tunlr >/dev/null 2>&1; then
  echo "rollback reload did not remove tunlr" >&2
  exit 1
fi

echo "network_preset_container_smoke=ok"
CONTAINER_SH
chmod 0755 "$container_script"

docker run --rm \
  --cap-add NET_ADMIN \
  -v "$container_script:/smoke/network-preset-container.sh:ro" \
  "$image" \
  sh -ec 'apt-get update >/dev/null && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends bird2 ifupdown2 iproute2 >/dev/null && sh /smoke/network-preset-container.sh'
