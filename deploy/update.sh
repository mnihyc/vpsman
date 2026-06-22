#!/usr/bin/env bash
set -Eeuo pipefail

REPO="${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}"
SERVER_ASSET="vpsman-server-linux-x86_64.zip"
FRONTEND_ASSET="vpsman-frontend-dist.tar.gz"

usage() {
  cat <<'USAGE'
Usage:
  ./update.sh first-start [latest|v0.1.0]
  ./update.sh latest
  ./update.sh v0.1.0
  ./update.sh rollback

Environment:
  VPSMAN_RELEASE_REPO  GitHub owner/repo, default: mnihyc/vpsman
  VPSMAN_CLI_ASSET     Optional vpsctl release asset override
  VPSMAN_SUPER_PASSWORD
                       Required by first-start when compose secrets do not exist
  GITHUB_TOKEN         Optional token for GitHub release downloads
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
runtime_dir="$script_dir/runtime"
mode="update"
target="${1:-latest}"

if [[ "$target" == "first-start" ]]; then
  mode="first-start"
  target="${2:-latest}"
  if [[ $# -gt 2 ]]; then
    usage >&2
    exit 1
  fi
elif [[ $# -gt 1 ]]; then
  usage >&2
  exit 1
fi

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required tool: $1" >&2
    exit 1
  fi
}

compose() {
  if docker compose version >/dev/null 2>&1; then
    docker compose -f "$script_dir/compose.yml" "$@"
  elif command -v docker-compose >/dev/null 2>&1; then
    docker-compose -f "$script_dir/compose.yml" "$@"
  else
    echo "missing required tool: docker compose" >&2
    exit 1
  fi
}

recreate_services() {
  compose up -d --force-recreate --remove-orphans api gateway worker frontend
}

require_env() {
  if [[ ! -f "$script_dir/.env" ]]; then
    echo ".env is required in the deployment directory; create it from .env.example and edit it first" >&2
    exit 1
  fi
}

detect_cli_asset() {
  local arch
  arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64)
      printf 'vpsctl-linux-x86_64-musl\n'
      ;;
    aarch64|arm64)
      printf 'vpsctl-linux-aarch64-musl\n'
      ;;
    *)
      echo "unsupported host architecture for vpsctl release asset: $arch" >&2
      exit 1
      ;;
  esac
}

secret_file_status() {
  local secrets_dir="$script_dir/config/secrets"
  local missing=0
  local present=0
  local name
  for name in \
    vpsman_internal_token \
    vpsman_gateway_private_key_hex \
    vpsman_privilege_verifier_key_hex \
    vpsman_gateway_public_key_hex \
    operator-privilege.env
  do
    if [[ -s "$secrets_dir/$name" ]]; then
      present=$((present + 1))
    else
      missing=$((missing + 1))
    fi
  done
  printf '%s:%s\n' "$present" "$missing"
}

prepare_first_start_secrets() {
  local cli_bin="$1"
  local status present missing
  status="$(secret_file_status)"
  present="${status%%:*}"
  missing="${status##*:}"

  if [[ "$missing" == "0" ]]; then
    return 0
  fi
  if [[ "$present" != "0" ]]; then
    cat >&2 <<'EOF'
compose secrets are incomplete; refusing to overwrite a partial secret set.
Restore the missing files or deliberately replace the set with:
  vpsctl compose-secrets --secrets-dir config/secrets --force
EOF
    exit 1
  fi
  if [[ -z "${VPSMAN_SUPER_PASSWORD:-}" ]]; then
    cat >&2 <<'EOF'
first-start needs compose secrets before containers can start.
Set VPSMAN_SUPER_PASSWORD and rerun first-start, or generate secrets manually
with a release/source vpsctl before starting compose:
  vpsctl compose-secrets --secrets-dir config/secrets
EOF
    exit 1
  fi

  "$cli_bin" compose-secrets --secrets-dir "$script_dir/config/secrets"
}

download_asset() {
  local base_url="$1"
  local name="$2"
  local output="$3"
  local headers=()
  if [[ -n "${GITHUB_TOKEN:-}" ]]; then
    headers=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
  fi
  curl -fL --retry 3 --connect-timeout 10 "${headers[@]}" \
    -o "$output" \
    "$base_url/$name"
}

download_url() {
  local url="$1"
  local output="$2"
  local headers=()
  if [[ -n "${GITHUB_TOKEN:-}" ]]; then
    headers=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
  fi
  curl -fL --retry 3 --connect-timeout 10 "${headers[@]}" \
    -o "$output" \
    "$url"
}

release_base_url() {
  local requested="$1"
  if [[ "$requested" == "latest" ]]; then
    printf 'https://github.com/%s/releases/latest/download\n' "$REPO"
  else
    printf 'https://github.com/%s/releases/download/%s\n' "$REPO" "$requested"
  fi
}

release_pinned_base_url() {
  local tag="$1"
  printf 'https://github.com/%s/releases/download/%s\n' "$REPO" "$tag"
}

extract_tag() {
  local metadata="$1"
  sed -n 's/.*"tag"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "$metadata" | head -n 1
}

swap_release_dir() {
  local staged="$1"
  local current="$2"
  local previous="$3"
  local swap_tmp="${current}.swap-$$"

  rm -rf "$swap_tmp"
  if [[ -e "$current" || -L "$current" ]]; then
    mv "$current" "$swap_tmp"
  fi
  mv "$staged" "$current"
  rm -rf "$previous"
  if [[ -e "$swap_tmp" || -L "$swap_tmp" ]]; then
    mv "$swap_tmp" "$previous"
  fi
}

rollback() {
  require_env
  local server_current="$runtime_dir/server/current"
  local server_previous="$runtime_dir/server/previous"
  local frontend_current="$runtime_dir/frontend/current"
  local frontend_previous="$runtime_dir/frontend/previous"
  local cli_current="$runtime_dir/cli/current"
  local cli_previous="$runtime_dir/cli/previous"
  local server_tmp="$runtime_dir/server/rollback-$$"
  local frontend_tmp="$runtime_dir/frontend/rollback-$$"
  local cli_tmp="$runtime_dir/cli/rollback-$$"

  if [[ ! -d "$server_current" || ! -d "$server_previous" || ! -d "$frontend_current" || ! -d "$frontend_previous" || ! -d "$cli_current" || ! -d "$cli_previous" ]]; then
    echo "rollback is unavailable; current or previous server/frontend/CLI releases are missing" >&2
    exit 1
  fi

  mv "$server_current" "$server_tmp"
  mv "$server_previous" "$server_current"
  mv "$server_tmp" "$server_previous"
  mv "$frontend_current" "$frontend_tmp"
  mv "$frontend_previous" "$frontend_current"
  mv "$frontend_tmp" "$frontend_previous"
  mv "$cli_current" "$cli_tmp"
  mv "$cli_previous" "$cli_current"
  mv "$cli_tmp" "$cli_previous"
  recreate_services
  echo "rollback complete"
}

if [[ "$target" == "rollback" ]]; then
  if [[ "$mode" == "first-start" ]]; then
    echo "first-start cannot target rollback" >&2
    exit 1
  fi
  require_tool docker
  rollback
  exit 0
fi

require_env
require_tool curl
require_tool sha256sum
require_tool tar
require_tool unzip
require_tool docker

CLI_ASSET="${VPSMAN_CLI_ASSET:-$(detect_cli_asset)}"

mkdir -p "$runtime_dir/downloads" "$runtime_dir/server" "$runtime_dir/frontend" "$runtime_dir/cli"
staging_dir="$(mktemp -d "$runtime_dir/.update.XXXXXX")"
server_staged=""
frontend_staged=""
cli_staged=""
install_committed=0
cleanup() {
  rm -rf "$staging_dir"
  if [[ "$install_committed" != "1" ]]; then
    if [[ -n "$server_staged" ]]; then
      rm -rf "$server_staged"
    fi
    if [[ -n "$frontend_staged" ]]; then
      rm -rf "$frontend_staged"
    fi
    if [[ -n "$cli_staged" ]]; then
      rm -rf "$cli_staged"
    fi
  fi
}
trap cleanup EXIT

base_url="$(release_base_url "$target")"
download_asset "$base_url" "version.json" "$staging_dir/version.json"
resolved_tag="$(extract_tag "$staging_dir/version.json")"
if [[ -z "$resolved_tag" ]]; then
  echo "release manifest does not contain a tag" >&2
  exit 1
fi
pinned_base_url="$(release_pinned_base_url "$resolved_tag")"
download_url "$pinned_base_url/SHA256SUMS" "$staging_dir/SHA256SUMS"
download_url "$pinned_base_url/$SERVER_ASSET" "$staging_dir/$SERVER_ASSET"
download_url "$pinned_base_url/$FRONTEND_ASSET" "$staging_dir/$FRONTEND_ASSET"
download_url "$pinned_base_url/$CLI_ASSET" "$staging_dir/$CLI_ASSET"

grep -E "  (${SERVER_ASSET}|${FRONTEND_ASSET}|${CLI_ASSET})$" "$staging_dir/SHA256SUMS" > "$staging_dir/SHA256SUMS.selected" || true
if [[ "$(wc -l < "$staging_dir/SHA256SUMS.selected" | tr -d ' ')" != "3" ]]; then
  echo "release checksum manifest does not contain required server/frontend/CLI assets" >&2
  exit 1
fi
(cd "$staging_dir" && sha256sum -c SHA256SUMS.selected)

server_staged="$runtime_dir/server/staged-$resolved_tag"
frontend_staged="$runtime_dir/frontend/staged-$resolved_tag"
cli_staged="$runtime_dir/cli/staged-$resolved_tag"
rm -rf "$server_staged" "$frontend_staged" "$cli_staged"
mkdir -p "$server_staged" "$frontend_staged" "$cli_staged"
unzip -q "$staging_dir/$SERVER_ASSET" -d "$server_staged"
tar -xzf "$staging_dir/$FRONTEND_ASSET" -C "$frontend_staged"
cp "$staging_dir/$CLI_ASSET" "$cli_staged/vpsctl"
chmod +x "$server_staged"/bin/vpsman-api "$server_staged"/bin/vpsman-gateway "$server_staged"/bin/vpsman-worker
chmod +x "$cli_staged/vpsctl"

if [[ ! -x "$server_staged/bin/vpsman-api" || ! -d "$server_staged/migrations" ]]; then
  echo "server release layout is invalid" >&2
  exit 1
fi
if [[ ! -f "$frontend_staged/dist/index.html" ]]; then
  echo "frontend release layout is invalid" >&2
  exit 1
fi
if [[ ! -x "$cli_staged/vpsctl" ]]; then
  echo "CLI release layout is invalid" >&2
  exit 1
fi

if [[ "$mode" == "first-start" ]]; then
  prepare_first_start_secrets "$cli_staged/vpsctl"
fi

cp "$staging_dir/version.json" "$runtime_dir/downloads/version-$resolved_tag.json"
cp "$staging_dir/SHA256SUMS" "$runtime_dir/downloads/SHA256SUMS-$resolved_tag"
swap_release_dir "$server_staged" "$runtime_dir/server/current" "$runtime_dir/server/previous"
swap_release_dir "$frontend_staged" "$runtime_dir/frontend/current" "$runtime_dir/frontend/previous"
swap_release_dir "$cli_staged" "$runtime_dir/cli/current" "$runtime_dir/cli/previous"
install_committed=1

recreate_services
if [[ "$mode" == "first-start" ]]; then
  echo "started vpsman deployment at $resolved_tag"
else
  echo "updated vpsman deployment to $resolved_tag"
fi
