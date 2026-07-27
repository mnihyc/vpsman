#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

python3 - <<'PY'
from pathlib import Path
import sys

if sys.version_info < (3, 11):
    raise SystemExit("Python 3.11 or newer is required for dependency audits")

import tomllib

manifest = tomllib.loads(Path("Cargo.toml").read_text("utf-8"))
toolchain = tomllib.loads(Path("rust-toolchain.toml").read_text("utf-8"))
rust_version = manifest.get("workspace", {}).get("package", {}).get("rust-version")
channel = toolchain.get("toolchain", {}).get("channel")
if not isinstance(rust_version, str) or not isinstance(channel, str):
    raise SystemExit("workspace rust-version and pinned Rust channel must be strings")
if rust_version != channel:
    raise SystemExit(
        f"workspace rust-version {rust_version!r} must match pinned Rust channel {channel!r}"
    )
PY

bash scripts/install-rust-audit-tool.sh --check

# Cargo.lock retains SQLx's optional MySQL RSA package even though this
# PostgreSQL-only workspace cannot activate it. Prove the advisory is absent
# from the maximally activated workspace graph before applying the lock-only
# waiver. Removing RSA from a future lock remains valid; activating it fails.
active_tree="$(
  cargo tree \
    --locked \
    --workspace \
    --all-features \
    --target all \
    --prefix none \
    --format '{p}'
)" || {
  printf 'failed to inspect the active workspace dependency graph\n' >&2
  exit 1
}
while IFS= read -r package; do
  case "$package" in
    "rsa v"*)
      printf 'RUSTSEC-2023-0071 is active in the workspace dependency graph: %s\n' \
        "$package" >&2
      exit 1
      ;;
  esac
done <<< "$active_tree"

cargo audit --deny unsound --ignore RUSTSEC-2023-0071
