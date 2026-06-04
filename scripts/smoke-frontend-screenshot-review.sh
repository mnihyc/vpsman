#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash find google-chrome wc

smoke_init_tmpdir frontend-screenshot-review

frontend_test_port="${VPSMAN_FRONTEND_TEST_PORT:-$(smoke_free_port)}"
review_dir="${VPSMAN_SCREENSHOT_REVIEW_DIR:-$ROOT_DIR/target/frontend-screenshot-review}"
rm -rf "$review_dir"
mkdir -p "$review_dir"

env CI=1 \
  VPSMAN_FRONTEND_SMOKE_ROOT="$ROOT_DIR" \
  VPSMAN_FRONTEND_TEST_PORT="$frontend_test_port" \
  VPSMAN_SCREENSHOT_REVIEW_DIR="$review_dir" \
  bash -ic 'cd "$VPSMAN_FRONTEND_SMOKE_ROOT/frontend" && npm run test:ui -- tests/console-screenshot-review.spec.ts'

screenshot_count="$(find "$review_dir" -type f -name '*.png' | wc -l)"
manifest_count="$(find "$review_dir" -type f -name 'manifest-*.json' | wc -l)"
if (( screenshot_count < 9 || manifest_count < 2 )); then
  echo "frontend screenshot review failed: expected at least 9 screenshots and 2 manifests, got screenshots=$screenshot_count manifests=$manifest_count" >&2
  exit 1
fi

jq_payload="{\"frontend_screenshot_review\":\"ok\",\"screenshot_count\":$screenshot_count,\"manifest_count\":$manifest_count,\"artifact_dir\":\"$review_dir\"}"
printf '%s\n' "$jq_payload"
