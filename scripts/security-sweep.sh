#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash cargo rg

fail() {
  echo "security sweep failed: $*" >&2
  exit 1
}

require_no_match() {
  local label="$1"
  local pattern="$2"
  shift 2
  local output
  if output="$(rg -n -- "$pattern" "$@" 2>/dev/null)"; then
    echo "security sweep violation: $label" >&2
    printf '%s\n' "$output" >&2
    exit 1
  fi
}

require_match() {
  local label="$1"
  local pattern="$2"
  shift 2
  if ! rg -q -- "$pattern" "$@"; then
    fail "missing expected security evidence: $label"
  fi
}

require_no_match \
  "server-side code must not persist or read plaintext super passwords" \
  'VPSMAN_SUPER_PASSWORD|super_password|superPassword|super-password' \
  crates/api/src crates/gateway/src migrations

require_no_match \
  "server audit/repository code must not persist privilege material fields" \
  'privilege_assertion|superPassword|super_password' \
  crates/api/src/repository*.rs migrations

require_match \
  "privilege assertion replay cache and replay error are implemented" \
  'PrivilegeReplayCache|AuthorizationError::Replay' \
  crates/common/src/auth.rs

require_match \
  "object key traversal tests are present" \
  'object_key_rejects_path_traversal' \
  crates/api/src/tests_object_store.rs

require_match \
  "operator password hash verification test is present" \
  'operator_password_hash_verifies_without_plaintext_storage' \
  crates/api/src/tests_auth.rs

cargo test -p vpsman-common auth
cargo test -p vpsman-api tests_auth
cargo test -p vpsman-api tests_object_store

printf '{\n'
printf '  "security_sweep": "ok",\n'
printf '  "checks": ["no_server_plaintext_super_password", "no_repository_privilege_material_persistence", "privilege_replay_cache", "object_key_safety", "operator_password_hashing"]\n'
printf '}\n'
