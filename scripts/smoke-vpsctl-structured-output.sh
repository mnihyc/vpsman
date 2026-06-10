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

export VPSMAN_SUPER_PASSWORD="structured output password"
privilege_json="$("$bin" --output json privilege-verifier --super-salt-hex 01020304)"
jq -e '
  .super_salt_hex == "01020304"
  and (.privilege_verifier_key_hex | test("^[0-9a-f]{64}$"))
  and .gateway_env.VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX == .privilege_verifier_key_hex
  and .operator_env.VPSMAN_SUPER_SALT_HEX == .super_salt_hex
' <<<"$privilege_json" >/dev/null

plan_json="$("$bin" --output pretty-json tunnel-plan \
  --name edge-structured \
  --interface-name tunstructured \
  --kind gre \
  --left-client-id edge-a \
  --right-client-id edge-b \
  --left-underlay 203.0.113.10 \
  --right-underlay 203.0.113.20 \
  --address-pool-cidr 10.253.0.0/30 \
  --bandwidth 100m \
  --latency-ms 25)"
jq -e '
  .name == "edge-structured"
  and .kind == "gre"
  and .mutates_host == false
  and (.recommended_ospf_cost | type == "number")
' <<<"$plan_json" >/dev/null

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
printf '  "checks": ["global_output_help", "compact_json", "pretty_json", "privilege_verifier_json", "interactive_vty_rejection"]\n'
printf '}\n'
