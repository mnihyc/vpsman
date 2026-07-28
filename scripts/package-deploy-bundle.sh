#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
  echo "usage: $0 <vX.Y.Z[-prerelease]> <output-directory>" >&2
}

if [[ "$#" -ne 2 ]]; then
  usage
  exit 2
fi

release_tag="$1"
output_dir="$2"

valid_release_tag() {
  local value="$1"
  local prerelease
  [[ "$value" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*))?$ ]] ||
    return 1
  prerelease="${BASH_REMATCH[5]:-}"
  [[ -z "$prerelease" || ! "$prerelease" =~ (^|\.)0[0-9]+($|\.) ]]
}

if ! valid_release_tag "$release_tag"; then
  echo "release tag must look like v1.2.3 or v1.2.3-rc.1: $release_tag" >&2
  exit 2
fi
if [[ ! -d "$output_dir" ]]; then
  echo "output directory does not exist: $output_dir" >&2
  exit 2
fi
output_dir="$(cd "$output_dir" && pwd)"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bundle_name="vpsman-deploy-${release_tag}"
output_path="$output_dir/${bundle_name}.tar.gz"

if [[ -e "$output_path" ]]; then
  echo "refusing to overwrite existing deployment bundle: $output_path" >&2
  exit 1
fi

stage_dir="$(mktemp -d)"
temporary_archive="$(mktemp "$output_dir/.${bundle_name}.XXXXXX.tar.gz")"
cleanup() {
  rm -rf -- "$stage_dir"
  if [[ -n "$temporary_archive" ]]; then
    rm -f -- "$temporary_archive"
  fi
}
trap cleanup EXIT

bundle_root="$stage_dir/$bundle_name"
install -d \
  "$bundle_root/config/secrets" \
  "$bundle_root/docs"

install -m 0644 "$repo_root/deploy/.env.example" "$bundle_root/.env.example"
install -m 0644 "$repo_root/deploy/compose.yml" "$bundle_root/compose.yml"
install -m 0644 "$repo_root/deploy/nginx.conf" "$bundle_root/nginx.conf"
install -m 0755 "$repo_root/deploy/update.sh" "$bundle_root/update.sh"
install -m 0644 "$repo_root/deploy/README.md" "$bundle_root/README.md"
# deploy/README.md lives one directory below repo docs but becomes the bundle
# root README, so adjust its one repository-relative runbook link.
sed -i \
  's#](../docs/production-deployment.md)#](docs/production-deployment.md)#' \
  "$bundle_root/README.md"
install -m 0644 \
  "$repo_root/deploy/AGENT_GATEWAY_INSTALL.md" \
  "$bundle_root/AGENT_GATEWAY_INSTALL.md"
install -m 0644 "$repo_root/deploy/config/vpsman.toml" "$bundle_root/config/vpsman.toml"
install -m 0644 \
  "$repo_root/deploy/config/secrets/.gitkeep" \
  "$bundle_root/config/secrets/.gitkeep"
install -m 0644 \
  "$repo_root/docs/production-deployment.md" \
  "$bundle_root/docs/production-deployment.md"
install -m 0644 \
  "$repo_root/docs/migration-compatibility.md" \
  "$bundle_root/docs/migration-compatibility.md"
install -m 0644 "$repo_root/SECURITY.md" "$bundle_root/SECURITY.md"
install -m 0644 "$repo_root/LICENSE-APACHE" "$bundle_root/LICENSE-APACHE"
install -m 0644 "$repo_root/LICENSE-MIT" "$bundle_root/LICENSE-MIT"

# The updater maintains this marker atomically after successful activation, and
# the production backup runbook records it as the authoritative payload tag.
printf '%s\n' "$release_tag" >"$bundle_root/RELEASE_TAG"
chmod 0644 "$bundle_root/RELEASE_TAG"

# Normalize archive order, ownership, and timestamps so the same tagged source
# produces the same deployment bundle bytes.
tar \
  --sort=name \
  --mtime="@${SOURCE_DATE_EPOCH:-0}" \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  -C "$stage_dir" \
  -cf - \
  "$bundle_name" |
  gzip -n > "$temporary_archive"

mv -- "$temporary_archive" "$output_path"
temporary_archive=""
echo "$output_path"
