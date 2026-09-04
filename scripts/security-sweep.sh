#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash cargo

cargo test -p vpsman-common auth
cargo test -p vpsman-api tests_auth
cargo test -p vpsman-api tests_object_store

printf '{\n'
printf '  "security_sweep": "ok",\n'
printf '  "checks": ["authentication_behavior", "object_store_behavior"]\n'
printf '}\n'
