#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST_DIR="${VPSMAN_FRONTEND_DIST_DIR:-$ROOT_DIR/frontend/dist}"
ASSET_DIR="$DIST_DIR/assets"

fail() {
  printf 'frontend installer asset audit failed: %s\n' "$*" >&2
  exit 1
}

[[ -d "$ASSET_DIR" ]] || fail "$DIST_DIR/assets is missing; build the frontend first"

source_commit="${VPSMAN_SOURCE_COMMIT:-${GITHUB_SHA:-}}"
if [[ -z "$source_commit" ]]; then
  source_commit="$(git -C "$ROOT_DIR" rev-parse HEAD)"
fi
[[ "$source_commit" =~ ^[0-9a-fA-F]{40}$ ]] ||
  fail "source commit must be exactly 40 hexadecimal characters"
source_commit="${source_commit,,}"

release_tag="${VPSMAN_RELEASE_TAG:-}"
if [[ -z "$release_tag" && "${GITHUB_REF_TYPE:-}" == "tag" ]]; then
  release_tag="${GITHUB_REF_NAME:-}"
fi

asset_contains() {
  grep -R -a -F -q -- "$1" "$ASSET_DIR"
}

mapfile -t emitted_installers < <(
  find "$DIST_DIR" -maxdepth 1 -type f -name 'install-agent-*.sh' -print | sort
)

if [[ -n "$release_tag" ]]; then
  ((${#emitted_installers[@]} == 0)) ||
    fail "tagged frontend must use the release asset, not emit a source installer"
  asset_contains "https://github.com/mnihyc/vpsman/releases/download/" ||
    fail "tagged frontend does not contain the exact-tag release base URL"
  asset_contains "$release_tag" ||
    fail "tagged frontend does not contain its release tag"
  asset_contains "SHA256SUMS.installer" ||
    fail "tagged frontend does not verify the installer checksum manifest"
  printf '{"frontend_installer_asset":"ok","mode":"tagged","tag":"%s"}\n' "$release_tag"
  exit 0
fi

((${#emitted_installers[@]} == 1)) ||
  fail "source frontend must emit exactly one content-addressed installer"
expected_sha="$(
  git -C "$ROOT_DIR" show "$source_commit:deploy/install-agent.sh" |
    sha256sum |
    awk '{print $1}'
)" || fail "source commit does not contain deploy/install-agent.sh"
expected_name="install-agent-$source_commit-$expected_sha.sh"
[[ "${emitted_installers[0]##*/}" == "$expected_name" ]] ||
  fail "emitted installer filename does not match its commit and SHA-256"
actual_sha="$(sha256sum "${emitted_installers[0]}" | awk '{print $1}')"
[[ "$actual_sha" == "$expected_sha" ]] ||
  fail "emitted installer bytes do not match the committed installer"
asset_contains "https://raw.githubusercontent.com/mnihyc/vpsman/" ||
  fail "source frontend does not contain the exact-commit download base URL"
asset_contains "$source_commit" ||
  fail "source frontend does not contain its source commit"
asset_contains "$expected_sha" ||
  fail "source frontend does not contain the installer checksum"
printf '{"frontend_installer_asset":"ok","mode":"source","commit":"%s","sha256":"%s"}\n' \
  "$source_commit" \
  "$expected_sha"
