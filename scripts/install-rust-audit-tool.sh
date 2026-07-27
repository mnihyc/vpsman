#!/usr/bin/env bash
set -euo pipefail

expected_version="0.22.2"
mode="${1:---install}"
if [[ "$#" -gt 1 || "$mode" != "--install" && "$mode" != "--check" ]]; then
  echo "usage: $0 [--install|--check]" >&2
  exit 2
fi

installed_version=""
if command -v cargo-audit >/dev/null 2>&1; then
  installed_version="$(cargo-audit --version 2>/dev/null || true)"
fi
if [[ "$installed_version" == "cargo-audit $expected_version" ]]; then
  exit 0
fi

if [[ "$mode" == "--check" ]]; then
  echo "cargo-audit $expected_version is required; run bash scripts/install-rust-audit-tool.sh" >&2
  exit 1
fi

command -v cargo >/dev/null 2>&1 || {
  echo "cargo is required to install cargo-audit $expected_version" >&2
  exit 1
}
cargo install cargo-audit --version "$expected_version" --locked --force

installed_version="$(cargo-audit --version 2>/dev/null || true)"
if [[ "$installed_version" != "cargo-audit $expected_version" ]]; then
  echo "cargo-audit installation did not provide expected version $expected_version" >&2
  exit 1
fi
