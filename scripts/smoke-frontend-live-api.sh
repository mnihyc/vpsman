#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash curl google-chrome jq shuf timeout

if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsman-api
fi

smoke_init_tmpdir "vpsman-frontend-live-api"

api_port="$(smoke_free_port)"
frontend_port="$(smoke_free_port)"
api_url="http://127.0.0.1:$api_port"
api_log="$SMOKE_TMPDIR/api.log"

VPSMAN_API_BIND="127.0.0.1:$api_port" \
RUST_LOG="vpsman_api=warn" \
  target/debug/vpsman-api >"$api_log" 2>&1 &
smoke_track_pid "$!"
smoke_wait_http "$api_url/health"

seed_agent() {
  local client_id="$1"
  curl -fsS \
    -H "Content-Type: application/json" \
    -d "{
      \"gateway_id\": \"frontend-live-gateway\",
      \"noise_public_key_hex\": null,
      \"hello\": {
        \"client_id\": \"$client_id\",
        \"agent_version\": \"frontend-live-smoke\",
        \"os_release\": \"Debian smoke\",
        \"arch\": \"x86_64\"
      }
    }" \
    "$api_url/internal/v1/gateway/agent-hello" >/dev/null
}

assign_agent_alias() {
  local client_id="$1"
  local display_name="$2"
  curl -fsS \
    -H "Content-Type: application/json" \
    -d "{\"display_name\":\"$display_name\"}" \
    "$api_url/api/v1/agents/$client_id/alias" >/dev/null
}

assign_agent_tag() {
  local client_id="$1"
  local tag="$2"
  curl -fsS \
    -H "Content-Type: application/json" \
    -d "{\"tag\":\"$tag\"}" \
    "$api_url/api/v1/agents/$client_id/tags" >/dev/null
}

seed_agent "live-agent-a"
seed_agent "live-agent-b"
assign_agent_alias "live-agent-a" "edge-live-a"
assign_agent_alias "live-agent-b" "edge-live-b"
assign_agent_tag "live-agent-a" edge
assign_agent_tag "live-agent-a" pool:live
assign_agent_tag "live-agent-a" bgp
assign_agent_tag "live-agent-b" edge
assign_agent_tag "live-agent-b" pool:live
assign_agent_tag "live-agent-b" bird2

jq -e '.total == 2 and .connected == 2' \
  < <(curl -fsS "$api_url/api/v1/fleet/summary") >/dev/null

if ! env \
  VPSMAN_API_PROXY="$api_url" \
  VPSMAN_FRONTEND_SMOKE_ROOT="$ROOT_DIR" \
  VPSMAN_FRONTEND_TEST_PORT="$frontend_port" \
  VPSMAN_LIVE_API_SMOKE=1 \
  bash -ic 'cd "$VPSMAN_FRONTEND_SMOKE_ROOT/frontend" && npm run test:ui -- tests/live-api-console.spec.ts --project desktop-chrome'; then
  smoke_dump_logs "frontend live API smoke failed" "$api_log"
  exit 1
fi

rm -rf frontend/test-results frontend/playwright-report

jq -n \
  --arg api_url "$api_url" \
  '{
    frontend_live_api_smoke: "ok",
    api_url: $api_url,
    checks: ["fleet", "topology_plan", "audit"]
  }'
