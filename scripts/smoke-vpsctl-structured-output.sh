#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash cargo jq mktemp

if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsctl >/dev/null
fi

bin="${VPSMAN_VPSCTL_BIN:-target/debug/vpsctl}"
if [[ ! -x "$bin" ]]; then
  smoke_fail "vpsctl binary is not executable: $bin"
fi

tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/vpsctl-structured-output.XXXXXX")"
trap 'rm -rf "$tmp_dir"' EXIT

noise_json="$("$bin" --output json noise-keygen)"
jq -e '
  (.private_key_hex | test("^[0-9a-f]{64}$"))
  and (.public_key_hex | test("^[0-9a-f]{64}$"))
' <<<"$noise_json" >/dev/null

plain_plan_json="$("$bin" --output pretty-json tunnel-plan \
  --name edge-structured \
  --interface-name tunstructured \
  --kind gre \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-remote-underlay 203.0.113.10 \
  --right-remote-underlay 203.0.113.20 \
  --address-pool-cidr 10.253.0.0/30 \
  --left-tunnel-ipv4-cidr 10.253.0.0/31 \
  --right-tunnel-ipv4-cidr 10.253.0.1/31 \
  --bandwidth-mbps 137)"
jq -e '
  .name == "edge-structured"
  and .kind == "gre"
  and .bandwidth_mbps == 137
  and .latency_primary_family == "ipv4"
  and (.conflicts | length == 0)
  and (has("ospf") | not)
  and (has("recommended_ospf_cost") | not)
' <<<"$plain_plan_json" >/dev/null

ospf_plan_json="$("$bin" --output pretty-json tunnel-plan \
  --name edge-structured-ospf \
  --interface-name tunospf \
  --kind gre \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-remote-underlay 203.0.113.10 \
  --right-remote-underlay 203.0.113.20 \
  --address-pool-cidr 10.253.0.4/30 \
  --left-tunnel-ipv4-cidr 10.253.0.4/31 \
  --right-tunnel-ipv4-cidr 10.253.0.5/31 \
  --bandwidth-mbps 137 \
  --ospf \
  --ospf-latency-ms 25 \
  --ospf-packet-loss-ratio 0.01 \
  --ospf-preference 1.2 \
  --left-routing-adapter-template-id 00000000-0000-0000-0000-000000000101 \
  --right-routing-adapter-template-id 00000000-0000-0000-0000-000000000102)"
jq -e '
  .name == "edge-structured-ospf"
  and .ospf.mode == "reviewed"
  and .ospf.planned_latency_ms == 25
  and .ospf.left_adapter_template_id == "00000000-0000-0000-0000-000000000101"
  and .ospf.right_adapter_template_id == "00000000-0000-0000-0000-000000000102"
  and (.recommended_ospf_cost | type == "number")
' <<<"$ospf_plan_json" >/dev/null

jsonl_normalized="$("$bin" --output json job-follow \
  --api-url "http://127.0.0.1:9" \
  --job-id 00000000-0000-0000-0000-000000000001 \
  --max-polls 0 \
  --json 2>"$tmp_dir/job-follow.err" || true)"
if [[ -n "$jsonl_normalized" ]]; then
  jq -e 'type == "object"' <<<"$jsonl_normalized" >/dev/null
fi

if "$bin" --output json vty >"$tmp_dir/vty.out" 2>"$tmp_dir/vty.err"; then
  smoke_fail "vpsctl --output json vty should reject interactive output normalization"
fi
grep -q -- "--output is not supported for the interactive vty shell" "$tmp_dir/vty.err"

help_text="$("$bin" --help)"
[[ "$help_text" == *"--output <OUTPUT>"* ]] || smoke_fail "root help missing --output"
[[ "$help_text" == *"[env: VPSMAN_OUTPUT="* ]] || smoke_fail "root help missing VPSMAN_OUTPUT"

printf '{\n'
printf '  "vpsctl_structured_output_smoke": "ok",\n'
printf '  "checks": ["global_output_help", "compact_json", "plain_tunnel_json", "explicit_ospf_json", "interactive_vty_rejection"]\n'
printf '}\n'
