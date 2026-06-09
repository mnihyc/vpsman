#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash google-chrome
smoke_init_tmpdir frontend-console-layout

frontend_test_port="${VPSMAN_FRONTEND_TEST_PORT:-$(smoke_free_port)}"

env CI=1 VPSMAN_FRONTEND_SMOKE_ROOT="$ROOT_DIR" VPSMAN_FRONTEND_TEST_PORT="$frontend_test_port" \
  bash -ic 'cd "$VPSMAN_FRONTEND_SMOKE_ROOT/frontend" && npm run test:ui'
