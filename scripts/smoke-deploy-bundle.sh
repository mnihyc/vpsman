#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk bash cat cmp grep mkdir sha256sum tar
smoke_init_tmpdir "vpsman-deploy-bundle"

first_output="$SMOKE_TMPDIR/first"
second_output="$SMOKE_TMPDIR/second"
extract_dir="$SMOKE_TMPDIR/extracted"
mkdir -p "$first_output" "$second_output" "$extract_dir"

first_archive="$(
  SOURCE_DATE_EPOCH=1710000000 \
    bash scripts/package-deploy-bundle.sh v1.2.3 "$first_output"
)"
second_archive="$(
  SOURCE_DATE_EPOCH=1710000000 \
    bash scripts/package-deploy-bundle.sh v1.2.3 "$second_output"
)"

cmp "$first_archive" "$second_archive"
tar -xzf "$first_archive" -C "$extract_dir"
bundle_root="$extract_dir/vpsman-deploy-v1.2.3"

test "$(cat "$bundle_root/RELEASE_TAG")" = "v1.2.3"
test -x "$bundle_root/update.sh"
test ! -e "$bundle_root/install-agent.sh"
test -f "$bundle_root/compose.yml"
test -f "$bundle_root/nginx.conf"
test -f "$bundle_root/config/vpsman.toml"
test -f "$bundle_root/docs/production-deployment.md"
grep -qF '](docs/production-deployment.md)' "$bundle_root/README.md"
if grep -qF '](../docs/' "$bundle_root/README.md"; then
  echo "bundled README contains a broken parent-relative docs link" >&2
  exit 1
fi
test -f "$bundle_root/SECURITY.md"
test -f "$bundle_root/LICENSE-APACHE"
test -f "$bundle_root/LICENSE-MIT"

archive_hash_before="$(sha256sum "$first_archive" | awk '{print $1}')"
if SOURCE_DATE_EPOCH=1710000000 \
  bash scripts/package-deploy-bundle.sh v1.2.3 "$first_output" \
  >"$SMOKE_TMPDIR/overwrite.log" 2>&1; then
  echo "expected deployment bundle packaging to refuse overwrite" >&2
  exit 1
fi
grep -q "refusing to overwrite existing deployment bundle" \
  "$SMOKE_TMPDIR/overwrite.log"
test "$archive_hash_before" = "$(sha256sum "$first_archive" | awk '{print $1}')"

if bash scripts/package-deploy-bundle.sh v01.2.3 "$SMOKE_TMPDIR" \
  >"$SMOKE_TMPDIR/invalid-tag.log" 2>&1; then
  echo "expected deployment bundle packaging to reject a zero-padded tag" >&2
  exit 1
fi
grep -q "release tag must look like" "$SMOKE_TMPDIR/invalid-tag.log"

printf '%s\n' \
  '{"deploy_bundle_smoke":"ok","checks":["deterministic_archive","required_content","overwrite_refusal","invalid_tag_refusal"]}'
