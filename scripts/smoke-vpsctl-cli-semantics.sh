#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash cargo mktemp

fail() {
  echo "$1" >&2
  exit 1
}

if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsctl >/dev/null
fi

bin="${VPSMAN_VPSCTL_BIN:-target/debug/vpsctl}"
if [[ ! -x "$bin" ]]; then
  fail "vpsctl binary is not executable: $bin"
fi

require_contains() {
  local text="$1"
  local expected="$2"
  local label="$3"
  if [[ "$text" != *"$expected"* ]]; then
    fail "$label missing expected text: $expected"
  fi
}

require_regex() {
  local text="$1"
  local regex="$2"
  local label="$3"
  if [[ ! "$text" =~ $regex ]]; then
    fail "$label did not match expected pattern"
  fi
}

for kind in gre ipip sit fou; do
  plan="$("$bin" tunnel-plan \
    --name "edge-$kind" \
    --interface-name "tun$kind" \
    --kind "$kind" \
    --left-client-id edge-a \
    --right-client-id edge-b \
    --left-underlay 203.0.113.10 \
    --right-underlay 203.0.113.20 \
    --address-pool-cidr 10.255.0.0/30 \
    --left-tunnel-ipv4 10.255.0.0 \
    --right-tunnel-ipv4 10.255.0.1 \
    --bandwidth 100m \
    --latency-ms 20)"
  require_contains "$plan" "\"kind\": \"$kind\"" "tunnel-plan $kind"
  require_contains "$plan" "\"mutates_host\": false" "tunnel-plan $kind"
  require_contains "$plan" "\"recommended_ospf_cost\"" "tunnel-plan $kind"
  require_contains "$plan" "\"ifupdown_snippet\"" "tunnel-plan $kind"
  require_contains "$plan" "\"bird2_interface_snippet\"" "tunnel-plan $kind"
  if [[ "$kind" == "fou" ]]; then
    require_contains "$plan" "encap fou" "tunnel-plan fou"
  else
    require_contains "$plan" "mode $kind" "tunnel-plan $kind"
  fi
done

noise_out="$("$bin" noise-keygen)"
require_regex "$noise_out" '"private_key_hex":"[0-9a-f]{64}"' "noise-keygen"
require_regex "$noise_out" '"public_key_hex":"[0-9a-f]{64}"' "noise-keygen"

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/vpsctl-cli-semantics.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT
compose_secret_dir="$tmp_dir/compose-secrets"
compose_secret_out="$(VPSMAN_SUPER_PASSWORD="correct horse battery staple" "$bin" compose-secrets \
  --secrets-dir "$compose_secret_dir" \
  --super-salt-hex 01020304)"
require_contains "$compose_secret_out" '"compose_secrets":"ok"' "compose-secrets"
for secret_name in vpsman_internal_token vpsman_gateway_private_key_hex vpsman_privilege_verifier_key_hex vpsman_gateway_public_key_hex operator-privilege.env; do
  [[ -f "$compose_secret_dir/$secret_name" ]] || fail "compose-secrets did not create $secret_name"
done
require_regex "$(cat "$compose_secret_dir/vpsman_internal_token")" '^[0-9a-f]{64}$' "compose internal token"
require_regex "$(cat "$compose_secret_dir/vpsman_gateway_private_key_hex")" '^[0-9a-f]{64}$' "compose gateway private key"
require_regex "$(cat "$compose_secret_dir/vpsman_gateway_public_key_hex")" '^[0-9a-f]{64}$' "compose gateway public key"
expected_verifier="$(smoke_privilege_verifier_key_hex "correct horse battery staple" "01020304")"
[[ "$(cat "$compose_secret_dir/vpsman_privilege_verifier_key_hex")" == "$expected_verifier" ]] \
  || fail "compose-secrets wrote unexpected privilege verifier key"
require_contains "$(cat "$compose_secret_dir/operator-privilege.env")" "VPSMAN_SUPER_SALT_HEX=01020304" "compose operator salt"
if VPSMAN_SUPER_PASSWORD="correct horse battery staple" "$bin" compose-secrets \
  --secrets-dir "$compose_secret_dir" \
  --super-salt-hex 01020304 \
  >"$tmp_dir/compose-overwrite.out" 2>"$tmp_dir/compose-overwrite.err"; then
  fail "compose-secrets overwrote existing files without --force"
fi
update_help="$("$bin" agent-update --help)"
require_contains "$update_help" "--artifact-url" "agent-update help"
require_contains "$update_help" "--sha256-hex" "agent-update help"
config_patch_help="$("$bin" config-patch --help)"
require_contains "$config_patch_help" "--config-file" "config-patch help"

printf '[auth]\njob_timeout_secs = 10\n' >"$tmp_dir/bad-config-patch.toml"
if "$bin" config-patch \
  --config-file "$tmp_dir/bad-config-patch.toml" \
  --clients edge-a \
  --confirmed \
  >"$tmp_dir/bad-config-patch.out" 2>"$tmp_dir/bad-config-patch.err"; then
  fail "config-patch accepted a disallowed section"
fi
require_contains "$(cat "$tmp_dir/bad-config-patch.err")" "config_patch_section_not_allowed:auth" "bad config patch"

if "$bin" tunnel-probe \
  --plan-file "$tmp_dir/missing-plan.json" \
  --side left \
  --count 0 \
  >"$tmp_dir/bad-probe.out" 2>"$tmp_dir/bad-probe.err"; then
  fail "tunnel-probe accepted count below bound"
fi
require_contains "$(cat "$tmp_dir/bad-probe.err")" "--count must be between" "bad probe"

if "$bin" tunnel-speed-test \
  --plan-file "$tmp_dir/missing-plan.json" \
  --server-side left \
  --duration-secs 0 \
  --confirmed \
  >"$tmp_dir/bad-speed.out" 2>"$tmp_dir/bad-speed.err"; then
  fail "tunnel-speed-test accepted duration below bound"
fi
require_contains "$(cat "$tmp_dir/bad-speed.err")" "--duration-secs must be between" "bad speed"

printf '{\n'
printf '  "vpsctl_cli_semantics_smoke": "ok",\n'
printf '  "checks": [\n'
printf '    "local_tunnel_plan_all_kinds",\n'
printf '    "noise_keygen_shape",\n'
printf '    "compose_secret_generation",\n'
printf '    "agent_update_external_url_shape",\n'
printf '    "config_patch_bounds_rejected",\n'
printf '    "network_probe_bounds_rejected",\n'
printf '    "network_speed_bounds_rejected"\n'
printf '  ]\n'
printf '}\n'
