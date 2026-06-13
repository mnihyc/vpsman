#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash curl docker google-chrome jq shuf timeout

if [[ "${VPSMAN_SMOKE_SKIP_BUILD:-0}" != "1" ]]; then
  cargo build -p vpsman-api
fi

smoke_init_tmpdir "vpsman-frontend-live-api"

api_port="$(smoke_free_port)"
pg_port="$(smoke_free_port)"
frontend_port="$(smoke_free_port)"
api_url="http://127.0.0.1:$api_port"
smoke_start_postgres "vpsman-frontend-api-postgres" "$pg_port" >/dev/null
postgres_url="$SMOKE_POSTGRES_URL"
api_log="$SMOKE_TMPDIR/api.log"
api_pid=""
internal_token="frontend-live-api-internal-token-123456"
operator_username="frontend-live-admin"
operator_password="frontend-live-password"

start_api() {
  local attempt=0
  local deadline=$((SECONDS + 90))
  while ((SECONDS < deadline)); do
    attempt=$((attempt + 1))
    api_log="$SMOKE_TMPDIR/api-$attempt.log"
    VPSMAN_API_BIND="127.0.0.1:$api_port" \
    VPSMAN_POSTGRES_URL="$postgres_url" \
    VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
    VPSMAN_INTERNAL_TOKEN="$internal_token" \
    VPSMAN_BACKUP_OBJECT_STORE_DIR="$SMOKE_TMPDIR/object-store/backups" \
    VPSMAN_UPDATE_OBJECT_STORE_DIR="$SMOKE_TMPDIR/object-store/updates" \
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
      if ((SECONDS >= http_deadline)); then
        kill "$api_pid" >/dev/null 2>&1 || true
        wait "$api_pid" >/dev/null 2>&1 || true
        api_pid=""
        break
      fi
      sleep 0.1
    done
    if curl -fsS "$api_url/health" >/dev/null 2>&1; then
      return
    fi
    sleep 0.5
  done
  smoke_dump_logs "frontend live API did not become healthy" "$api_log"
  exit 1
}

start_api

seed_agent() {
  local client_id="$1"
  curl -fsS \
    -H "Authorization: Bearer $internal_token" \
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
    -H "Authorization: Bearer $access_token" \
    -H "Content-Type: application/json" \
    -d "{\"display_name\":\"$display_name\"}" \
    "$api_url/api/v1/agents/$client_id/alias" >/dev/null
}

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d "{\"username\":\"$operator_username\",\"password\":\"$operator_password\"}" \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"

seed_agent "live-agent-a"
seed_agent "live-agent-b"
assign_agent_alias "live-agent-a" "edge-live-a"
assign_agent_alias "live-agent-b" "edge-live-b"

jq -e '.total == 2 and .online == 2' \
  < <(curl -fsS -H "Authorization: Bearer $access_token" "$api_url/api/v1/fleet/summary") >/dev/null

if ! env \
  VPSMAN_API_PROXY="$api_url" \
  VPSMAN_FRONTEND_SMOKE_ROOT="$ROOT_DIR" \
  VPSMAN_FRONTEND_TEST_PORT="$frontend_port" \
  VPSMAN_LIVE_API_SMOKE=1 \
  VPSMAN_LIVE_API_USERNAME="$operator_username" \
  VPSMAN_LIVE_API_PASSWORD="$operator_password" \
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
