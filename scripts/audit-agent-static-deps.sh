#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk bash cargo grep sed sort tr wc

target="${VPSMAN_AGENT_DEP_AUDIT_TARGET:-x86_64-unknown-linux-musl}"
tree_file="${VPSMAN_AGENT_DEP_AUDIT_TREE_FILE:-target/agent-static-deps-$target.txt}"
mkdir -p "$(dirname "$tree_file")"

cargo tree -p vpsman-agent --target "$target" -e normal,build --prefix none >"$tree_file"

forbidden_packages=(
  openssl
  openssl-sys
  native-tls
  libz-sys
  zstd-sys
  bzip2-sys
  xz2
  curl
  curl-sys
  libgit2-sys
  sqlite3-src
  libsqlite3-sys
)

failures=()
for package in "${forbidden_packages[@]}"; do
  if grep -Eq "^${package//+/\\+} v[0-9]" "$tree_file"; then
    failures+=("$package")
  fi
done

unexpected_sys_packages="$(
  grep -E '^[A-Za-z0-9_-]+-sys v[0-9]' "$tree_file" \
    | awk '{print $1}' \
    | sort -u \
    | grep -Ev '^(linux-raw-sys$|(windows|wasi|wasm-bindgen)-)' \
    || true
)"
if [[ -n "$unexpected_sys_packages" ]]; then
  while IFS= read -r package; do
    [[ -z "$package" ]] || failures+=("$package")
  done <<<"$unexpected_sys_packages"
fi

native_build_packages="$(
  grep -E '^(cc|cmake|pkg-config|vcpkg) v[0-9]' "$tree_file" \
    | awk '{print $1}' \
    | sort -u \
    | tr '\n' ' ' \
    | sed 's/[[:space:]]*$//' \
    || true
)"
dependency_count="$(grep -E '^[A-Za-z0-9_-]+ v[0-9]' "$tree_file" | sort -u | wc -l | tr -d ' ')"

if ((${#failures[@]} > 0)); then
  printf 'agent static dependency audit failed: %s\n' "${failures[*]}" >&2
  printf 'dependency tree: %s\n' "$tree_file" >&2
  exit 1
fi

printf '{\n'
printf '  "agent_static_dependency_audit": "ok",\n'
printf '  "target": "%s",\n' "$target"
printf '  "dependency_count": %s,\n' "$dependency_count"
printf '  "native_build_packages": "%s",\n' "$native_build_packages"
printf '  "tree_file": "%s",\n' "$tree_file"
printf '  "checks": ["musl_agent_dependency_graph", "no_dynamic_tls_crates", "no_unexpected_sys_crates"]\n'
printf '}\n'
