#!/usr/bin/env bash
set -Eeuo pipefail

REPO="${VPSMAN_RELEASE_REPO:-mnihyc/vpsman}"
FRONTEND_ASSET="vpsman-frontend-dist.tar.gz"
MIN_SUPPORTED_RELEASE="v0.2.0"
HEALTH_TIMEOUT_SECS="${VPSMAN_UPDATE_HEALTH_TIMEOUT_SECS:-180}"

log() {
  printf 'vpsman-update: %s\n' "$*"
}

fail() {
  printf 'vpsman-update: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'USAGE'
Usage:
  ./update.sh first-start [latest|vX.Y.Z]
  ./update.sh latest
  ./update.sh vX.Y.Z
  ./update.sh rollback
  ./update.sh recover

Environment:
  VPSMAN_RELEASE_REPO        GitHub owner/repo, default: mnihyc/vpsman
  VPSMAN_SERVER_ASSET        Optional server release asset override
  VPSMAN_CLI_ASSET           Optional vpsctl release asset override
  VPSMAN_UPDATE_HEALTH_TIMEOUT_SECS
                             Readiness timeout, default: 180
  VPSMAN_SUPER_PASSWORD      Required by first-start when compose secrets do not exist
  GITHUB_TOKEN               Optional token for GitHub release downloads

The updater accepts only release v0.2.0 or newer. Before payload activation it
verifies any existing migration history against the target release and saves a
PostgreSQL archive under runtime/update-backups/. Updates and rollbacks stop
application writers before taking that archive.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
runtime_dir="$script_dir/runtime"
transactions_dir="$runtime_dir/transactions"
transaction=""
transaction_can_cleanup=0
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

valid_release_tag() {
  local prerelease
  [[ "$1" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-([0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*))?$ ]] ||
    return 1
  prerelease="${BASH_REMATCH[5]:-}"
  [[ -z "$prerelease" || ! "$prerelease" =~ (^|\.)0[0-9]+($|\.) ]]
}

case "$target" in
  latest | rollback | recover) ;;
  *)
    valid_release_tag "$target" ||
      fail "release target must be latest, rollback, recover, or an exact vX.Y.Z tag"
    ;;
esac
if [[ "$mode" == "first-start" && ( "$target" == "rollback" || "$target" == "recover" ) ]]; then
  fail "first-start accepts only latest or an exact release tag"
fi

[[ "$REPO" =~ ^[0-9A-Za-z_.-]+/[0-9A-Za-z_.-]+$ ]] ||
  fail "VPSMAN_RELEASE_REPO must be an owner/repository pair"
[[ "$HEALTH_TIMEOUT_SECS" =~ ^(0|[1-9][0-9]*)$ &&
  "${#HEALTH_TIMEOUT_SECS}" -le 4 ]] ||
  fail "VPSMAN_UPDATE_HEALTH_TIMEOUT_SECS must be a canonical integer between 10 and 3600"
((10#$HEALTH_TIMEOUT_SECS >= 10 && 10#$HEALTH_TIMEOUT_SECS <= 3600)) ||
  fail "VPSMAN_UPDATE_HEALTH_TIMEOUT_SECS must be a canonical integer between 10 and 3600"

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    fail "missing required tool: $1"
  fi
}

compose() {
  if docker compose version >/dev/null 2>&1; then
    docker compose --env-file "$script_dir/.env" -f "$script_dir/compose.yml" "$@"
  elif command -v docker-compose >/dev/null 2>&1; then
    docker-compose --env-file "$script_dir/.env" -f "$script_dir/compose.yml" "$@"
  else
    fail "missing required tool: docker compose"
  fi
}

recreate_services() {
  compose up -d --force-recreate --remove-orphans api gateway worker frontend
}

stop_application_services() {
  compose stop frontend gateway worker api
}

require_env() {
  [[ -f "$script_dir/.env" ]] ||
    fail ".env is required in the deployment directory; create it from .env.example and edit it first"
  python3 - "$script_dir/.env" <<'PY'
import pathlib
import re
import sys

path = pathlib.Path(sys.argv[1])
password = None
for raw in path.read_text(encoding="utf-8").splitlines():
    line = raw.strip()
    if not line or line.startswith("#") or "=" not in line:
        continue
    key, value = line.split("=", 1)
    if key.strip() != "POSTGRES_PASSWORD":
        continue
    value = value.strip()
    if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
        value = value[1:-1]
    password = value

if password is None:
    raise SystemExit("vpsman-update: .env must set POSTGRES_PASSWORD explicitly")
if len(password) < 24:
    raise SystemExit("vpsman-update: POSTGRES_PASSWORD must contain at least 24 characters")
if re.fullmatch(r"[A-Za-z0-9._~-]+", password) is None:
    raise SystemExit(
        "vpsman-update: POSTGRES_PASSWORD must use URL-safe unreserved characters "
        "(letters, digits, dot, underscore, tilde, or hyphen)"
    )
if password.lower() in {
    "vpsman",
    "password",
    "changeme",
    "replacewithrandomhexpostgrespassword",
} or re.fullmatch(r"(change|replace|example|placeholder).+", password, re.I):
    raise SystemExit("vpsman-update: replace the POSTGRES_PASSWORD template placeholder")
PY
}

detect_server_asset() {
  if [[ -n "${VPSMAN_SERVER_ASSET:-}" ]]; then
    printf '%s\n' "$VPSMAN_SERVER_ASSET"
    return
  fi
  case "$(uname -m)" in
    x86_64 | amd64)
      printf 'vpsman-server-linux-x86_64.zip\n'
      ;;
    aarch64 | arm64)
      fail "official control-plane releases are currently x86_64-only; ARM agents and vpsctl are supported, but do not run the x86_64 server bundle on this host"
      ;;
    *)
      fail "unsupported control-plane host architecture: $(uname -m)"
      ;;
  esac
}

detect_cli_asset() {
  case "$(uname -m)" in
    x86_64 | amd64)
      printf 'vpsctl-linux-x86_64-musl\n'
      ;;
    aarch64 | arm64)
      printf 'vpsctl-linux-aarch64-musl\n'
      ;;
    *)
      fail "unsupported host architecture for vpsctl release asset: $(uname -m)"
      ;;
  esac
}

validate_asset_name() {
  [[ "$1" =~ ^[0-9A-Za-z._-]+$ ]] ||
    fail "release asset names must not contain paths or shell metacharacters: $1"
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

download_url() {
  local url="$1"
  local output="$2"
  local headers=()
  if [[ -n "${GITHUB_TOKEN:-}" ]]; then
    headers=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
  fi
  curl -fL --retry 3 --connect-timeout 10 "${headers[@]}" -o "$output" "$url"
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
  printf 'https://github.com/%s/releases/download/%s\n' "$REPO" "$1"
}

read_validated_manifest() {
  local metadata="$1"
  python3 - "$metadata" <<'PY'
import json
import pathlib
import re
import sys

path = pathlib.Path(sys.argv[1])
try:
    data = json.loads(path.read_text(encoding="utf-8"))
except (OSError, UnicodeError, json.JSONDecodeError) as error:
    raise SystemExit(f"invalid release manifest JSON: {error}")

if data.get("schema_version") != 2:
    raise SystemExit("release manifest schema_version must be 2")
if data.get("project") != "vpsman":
    raise SystemExit("release manifest project must be vpsman")
tag = data.get("tag")
version = data.get("version")
commit = data.get("commit")
tag_pattern = (
    r"^v(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)
if not isinstance(tag, str) or re.fullmatch(tag_pattern, tag) is None:
    raise SystemExit("release manifest tag is not a supported semantic version")
prerelease = tag.partition("-")[2]
if any(len(part) > 1 and part.startswith("0") and part.isdigit() for part in prerelease.split(".")):
    raise SystemExit("release manifest tag has a numeric prerelease identifier with a leading zero")
if version != tag[1:]:
    raise SystemExit("release manifest version does not match its tag")
if not isinstance(commit, str) or re.fullmatch(r"[0-9a-fA-F]{40}", commit) is None:
    raise SystemExit("release manifest commit must be a 40-character Git commit")
assets = data.get("assets")
if not isinstance(assets, list) or not assets:
    raise SystemExit("release manifest assets must be a non-empty list")
names = []
for asset in assets:
    if not isinstance(asset, dict) or not isinstance(asset.get("name"), str):
        raise SystemExit("release manifest contains an invalid asset entry")
    name = asset["name"]
    if re.fullmatch(r"[0-9A-Za-z._-]+", name) is None:
        raise SystemExit("release manifest contains an unsafe asset name")
    names.append(name)
if len(names) != len(set(names)):
    raise SystemExit("release manifest contains duplicate asset names")
print(f"{tag}\t{commit.lower()}")
PY
}

require_supported_release() {
  python3 - "$1" "$MIN_SUPPORTED_RELEASE" <<'PY'
import re
import sys

def parse(value):
    match = re.fullmatch(
        r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
        r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?",
        value,
    )
    if not match:
        raise SystemExit(f"unsupported release tag: {value}")
    prerelease = match.group(4)
    if prerelease and any(
        len(part) > 1 and part.startswith("0") and part.isdigit()
        for part in prerelease.split(".")
    ):
        raise SystemExit(f"unsupported release tag: {value}")
    return tuple(map(int, match.group(1, 2, 3))), prerelease

candidate, prerelease = parse(sys.argv[1])
floor, _ = parse(sys.argv[2])
if candidate < floor or (candidate == floor and prerelease is not None):
    raise SystemExit(
        f"release {sys.argv[1]} predates the supported updater boundary {sys.argv[2]}"
    )
PY
}

validate_archives() {
  python3 - "$1" "$2" <<'PY'
import pathlib
import stat
import sys
import tarfile
import zipfile

server_path = pathlib.Path(sys.argv[1])
frontend_path = pathlib.Path(sys.argv[2])

def safe_name(name):
    if not name or "\\" in name:
        return False
    path = pathlib.PurePosixPath(name)
    return not path.is_absolute() and ".." not in path.parts

with zipfile.ZipFile(server_path) as archive:
    for entry in archive.infolist():
        if not safe_name(entry.filename):
            raise SystemExit(f"unsafe server archive path: {entry.filename!r}")
        mode = entry.external_attr >> 16
        if mode and stat.S_ISLNK(mode):
            raise SystemExit(f"server archive symlink is not allowed: {entry.filename!r}")

with tarfile.open(frontend_path, "r:gz") as archive:
    for entry in archive.getmembers():
        if not safe_name(entry.name):
            raise SystemExit(f"unsafe frontend archive path: {entry.name!r}")
        if not (entry.isfile() or entry.isdir()):
            raise SystemExit(f"frontend archive special entry is not allowed: {entry.name!r}")
PY
}

wait_for_postgres() {
  local deadline=$((SECONDS + HEALTH_TIMEOUT_SECS))
  compose up -d postgres
  while ((SECONDS < deadline)); do
    if compose exec -T postgres sh -ceu \
      'exec pg_isready -U "$POSTGRES_USER" -d "$POSTGRES_DB"' >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

database_migration_rows() {
  local output="$1"
  local failed
  [[ "$(database_migration_ledger_status)" == "t" ]] ||
    fail "database has no SQLx migration ledger; use first-start for a new deployment"
  failed="$(
    compose exec -T postgres sh -ceu \
      'exec psql -v ON_ERROR_STOP=1 -At -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT count(*) FROM _sqlx_migrations WHERE NOT success"' |
      tr -d '\r'
  )"
  [[ "$failed" == "0" ]] ||
    fail "database contains a failed SQLx migration; restore or repair it before updating"
  compose exec -T postgres sh -ceu \
    'exec psql -v ON_ERROR_STOP=1 -At -F " " -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT version, encode(checksum, '\''hex'\'') FROM _sqlx_migrations WHERE success ORDER BY version"' |
    tr -d '\r' >"$output"
}

database_migration_ledger_status() {
  local exists
  exists="$(
    compose exec -T postgres sh -ceu \
      'exec psql -v ON_ERROR_STOP=1 -At -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "SELECT to_regclass('\''public._sqlx_migrations'\'') IS NOT NULL"' |
      tr -d '\r'
  )"
  case "$exists" in
    t | f)
      printf '%s\n' "$exists"
      ;;
    *)
      fail "database returned an invalid SQLx migration-ledger status"
      ;;
  esac
}

verify_database_compatible_with() {
  local migrations_dir="$1"
  local rows_file="$2"
  local version expected extra prefix
  local -a matches

  [[ -d "$migrations_dir" ]] ||
    fail "release has no migrations directory: $migrations_dir"
  database_migration_rows "$rows_file"
  while read -r version expected extra; do
    [[ -n "${version:-}" ]] || continue
    [[ "$version" =~ ^[0-9]+$ && "$expected" =~ ^[0-9a-f]{96}$ && -z "${extra:-}" ]] ||
      fail "database migration ledger returned an invalid row"
    printf -v prefix '%04d_' "$version"
    shopt -s nullglob
    matches=("$migrations_dir"/"$prefix"*.sql)
    shopt -u nullglob
    [[ "${#matches[@]}" == "1" ]] ||
      fail "target release does not contain exactly one migration for applied version $version"
    actual="$(sha384sum "${matches[0]}" | awk '{print $1}')"
    [[ "$actual" == "$expected" ]] ||
      fail "database migration $version is incompatible with the target release; restore the matching release and follow docs/migration-compatibility.md"
  done <"$rows_file"
}

database_backup_is_valid() {
  local input="$1"
  [[ -f "$input" && ! -L "$input" && -s "$input" ]] || return 1
  compose exec -T postgres sh -ceu \
    'exec pg_restore --list' <"$input" >/dev/null
}

remove_backup_partial() {
  local partial="$1"
  rm -f -- "$partial" ||
    fail "could not remove incomplete PostgreSQL backup partial $partial; preserve it for manual inspection"
}

backup_database() {
  local output="$1"
  local backup_dir backup_name partial
  backup_dir="${output%/*}"
  backup_name="${output##*/}"

  [[ -d "$backup_dir" ]] ||
    fail "PostgreSQL backup directory does not exist: $backup_dir"
  [[ ! -e "$output" && ! -L "$output" ]] ||
    fail "refusing to overwrite existing PostgreSQL backup: $output"
  partial="$(
    mktemp "$backup_dir/.${backup_name}.partial.XXXXXX"
  )" || fail "could not create a same-directory PostgreSQL backup partial"
  chmod 0600 "$partial" || {
    remove_backup_partial "$partial"
    fail "could not set mode 0600 on PostgreSQL backup partial"
  }

  if ! compose exec -T postgres sh -ceu \
    'exec pg_dump --format=custom --no-owner --no-acl -U "$POSTGRES_USER" -d "$POSTGRES_DB"' >"$partial"; then
    remove_backup_partial "$partial"
    fail "PostgreSQL pre-activation backup failed"
  fi
  if ! database_backup_is_valid "$partial"; then
    remove_backup_partial "$partial"
    fail "PostgreSQL pre-activation backup failed pg_restore validation"
  fi

  if ! mv -T --no-clobber -- "$partial" "$output"; then
    remove_backup_partial "$partial"
    fail "could not atomically publish PostgreSQL backup: $output"
  fi
  if [[ -e "$partial" || -L "$partial" ]]; then
    remove_backup_partial "$partial"
    fail "refusing to overwrite PostgreSQL backup created concurrently: $output"
  fi
  [[ -f "$output" && ! -L "$output" ]] ||
    fail "PostgreSQL backup publication did not create a regular file: $output"
}

restore_database() {
  local input="$1"
  database_backup_is_valid "$input" ||
    fail "cannot recover from an invalid PostgreSQL backup: $input"
  compose exec -T postgres sh -ceu '
    dropdb --if-exists -U "$POSTGRES_USER" "$POSTGRES_DB"
    createdb -U "$POSTGRES_USER" -O "$POSTGRES_USER" "$POSTGRES_DB"
    exec pg_restore --exit-on-error --no-owner --no-acl \
      -U "$POSTGRES_USER" -d "$POSTGRES_DB"
  ' <"$input" ||
    fail "PostgreSQL database restore failed; preserve the release transaction for manual recovery"
}

service_is_running() {
  compose ps --status running --services | grep -Fxq "$1"
}

wait_for_deployment() {
  local expected_tag="${1:-}"
  local deadline=$((SECONDS + HEALTH_TIMEOUT_SECS))
  local health bootstrap build_info
  while ((SECONDS < deadline)); do
    if service_is_running api &&
      service_is_running gateway &&
      service_is_running worker &&
      service_is_running frontend; then
      health="$(
        compose exec -T frontend wget -qO- http://127.0.0.1/health 2>/dev/null |
          tr -d '\r' || true
      )"
      bootstrap="$(
        compose exec -T frontend wget -qO- http://127.0.0.1/api/v1/auth/bootstrap-status 2>/dev/null |
          tr -d '\r' || true
      )"
      if [[ "$health" == "ok" && "$bootstrap" == \{*\} ]]; then
        if [[ -z "$expected_tag" ]]; then
          return 0
        fi
        build_info="$(
          compose exec -T frontend wget -qO- http://127.0.0.1/api/v1/build-info 2>/dev/null |
            tr -d '\r' || true
        )"
        if python3 - "$expected_tag" "$build_info" <<'PY'
import json
import sys
try:
    payload = json.loads(sys.argv[2])
except json.JSONDecodeError:
    raise SystemExit(1)
raise SystemExit(0 if payload.get("release_tag") == sys.argv[1] else 1)
PY
        then
          return 0
        fi
      fi
    fi
    sleep 1
  done
  return 1
}

safe_remove_transaction() {
  local path="$1"
  local cleanup_path
  case "$path" in
    "$transactions_dir"/update.*)
      [[ -d "$path" ]] || return 0
      cleanup_path="$transactions_dir/.cleanup.${path##*/}.$BASHPID"
      [[ ! -e "$cleanup_path" ]] ||
        fail "refusing to replace an existing transaction cleanup path: $cleanup_path"
      # Make the transaction undiscoverable atomically before recursively
      # deleting it. A crash during rm then leaves only ignored cleanup debris.
      mv -- "$path" "$cleanup_path" ||
        fail "could not quarantine completed transaction $path; preserve it for recovery"
      rm -rf -- "$cleanup_path" ||
        fail "could not remove quarantined transaction $cleanup_path; preserve it for manual cleanup"
      ;;
    "$transactions_dir"/.initializing.* | "$transactions_dir"/.cleanup.*)
      [[ -d "$path" ]] || return 0
      rm -rf -- "$path" ||
        fail "could not remove abandoned transaction directory $path; preserve it for manual cleanup"
      ;;
    *)
      fail "refusing to remove an invalid transaction path: $path"
      ;;
  esac
}

write_transaction_value() {
  local transaction="$1"
  local name="$2"
  local value="$3"
  printf '%s\n' "$value" >"$transaction/$name.tmp" ||
    fail "could not write transaction metadata $name; preserve $transaction for recovery"
  mv -- "$transaction/$name.tmp" "$transaction/$name" ||
    fail "could not commit transaction metadata $name; preserve $transaction for recovery"
}

read_transaction_value() {
  local transaction="$1"
  local name="$2"
  sed -n '1p' "$transaction/$name" 2>/dev/null || true
}

payloads_complete() {
  local kind
  for kind in server frontend cli; do
    [[ -d "$runtime_dir/$kind/current" ]] || return 1
  done
}

write_selected_asset_identity() {
  local checksums="$1"
  local output="$2"
  local role asset digest candidate name extra
  local -a roles=(server frontend cli)
  local -a assets=("$SERVER_ASSET" "$FRONTEND_ASSET" "$CLI_ASSET")
  local index

  : >"$output"
  for index in "${!roles[@]}"; do
    role="${roles[$index]}"
    asset="${assets[$index]}"
    digest=""
    while read -r candidate name extra; do
      [[ "$name" == "$asset" ]] || continue
      [[ -z "$digest" ]] ||
        fail "release checksum manifest contains duplicate selected asset $asset"
      [[ "$candidate" =~ ^[0-9A-Fa-f]{64}$ && -z "${extra:-}" ]] ||
        fail "release checksum manifest contains an invalid digest for $asset"
      digest="${candidate,,}"
    done <"$checksums"
    [[ -n "$digest" ]] ||
      fail "release checksum manifest does not contain selected asset $asset"
    printf '%s\t%s\t%s\n' "$role" "$asset" "$digest" >>"$output"
  done
}

current_release_identity_matches() {
  local verified_manifest="$1"
  local verified_assets="$2"
  local expected_tag="$3"
  local kind

  [[ -f "$script_dir/RELEASE_TAG" ]] || return 1
  [[ "$(<"$script_dir/RELEASE_TAG")" == "$expected_tag" ]] || return 1
  for kind in server frontend cli; do
    cmp -s \
      "$verified_manifest" \
      "$runtime_dir/$kind/current/.vpsman-release.json" ||
      return 1
    cmp -s \
      "$verified_assets" \
      "$runtime_dir/$kind/current/.vpsman-assets.tsv" ||
      return 1
  done
}

current_payload_layout_is_valid() {
  [[ -x "$runtime_dir/server/current/bin/vpsman-api" &&
    -x "$runtime_dir/server/current/bin/vpsman-gateway" &&
    -x "$runtime_dir/server/current/bin/vpsman-worker" &&
    -d "$runtime_dir/server/current/migrations" &&
    -f "$runtime_dir/frontend/current/dist/index.html" &&
    -x "$runtime_dir/cli/current/vpsctl" ]]
}

current_payloads_match_staged() {
  local kind
  for kind in server frontend cli; do
    diff -qr --no-dereference \
      "$transaction/staged-$kind" \
      "$runtime_dir/$kind/current" >/dev/null ||
      return 1
  done
}

write_active_release_tag() {
  local tag="$1"
  local marker="$script_dir/RELEASE_TAG"
  if [[ -z "$tag" ]]; then
    rm -f -- "$marker" "$marker.tmp" ||
      fail "could not remove the legacy active-release marker; preserve $transaction for recovery"
    log "removed the active-release marker because the rollback payload predates embedded release metadata"
    return 0
  fi
  valid_release_tag "$tag" ||
    fail "transaction release tag is invalid; preserve $transaction for manual recovery"
  printf '%s\n' "$tag" >"$marker.tmp" ||
    fail "could not write the active-release marker; preserve $transaction for recovery"
  chmod 0644 "$marker.tmp" ||
    fail "could not set active-release marker permissions; preserve $transaction for recovery"
  mv -- "$marker.tmp" "$marker" ||
    fail "could not commit the active-release marker; preserve $transaction for recovery"
}

finalize_transaction() {
  local transaction="$1"
  local kind release_tag retired transaction_mode
  for kind in server frontend cli; do
    if [[ -d "$transaction/old-$kind" ]]; then
      retired="$transaction/retired-$kind"
      if [[ -e "$runtime_dir/$kind/previous" || -L "$runtime_dir/$kind/previous" ]]; then
        [[ ! -e "$retired" && ! -L "$retired" ]] ||
          fail "both previous and retired $kind payloads exist; preserve $transaction for manual recovery"
        mv -- "$runtime_dir/$kind/previous" "$retired" ||
          fail "could not preserve the retired $kind payload; preserve $transaction for recovery"
      fi
      [[ ! -e "$runtime_dir/$kind/previous" && ! -L "$runtime_dir/$kind/previous" ]] ||
        fail "cannot publish the previous $kind payload because its destination exists"
      mv -- "$transaction/old-$kind" "$runtime_dir/$kind/previous" ||
        fail "could not publish the previous $kind rollback payload; preserve $transaction for recovery"
    fi
  done
  transaction_mode="$(read_transaction_value "$transaction" mode)"
  release_tag="$(read_transaction_value "$transaction" tag)"
  if [[ -z "$release_tag" && "$transaction_mode" != "rollback" ]]; then
    fail "transaction release metadata is incomplete; preserve $transaction for manual recovery"
  fi
  write_active_release_tag "$release_tag"
  write_transaction_value "$transaction" state finalized
}

recover_transaction() {
  local transaction="$1"
  local state transaction_mode backup_name backup_path kind had_current=0
  state="$(read_transaction_value "$transaction" state)"
  transaction_mode="$(read_transaction_value "$transaction" mode)"
  backup_name="$(read_transaction_value "$transaction" backup)"
  [[ "$transaction_mode" == "update" || "$transaction_mode" == "first-start" || "$transaction_mode" == "rollback" ]] ||
    fail "transaction metadata is invalid; preserve $transaction for manual recovery"
  # Update and rollback always begin with a complete current payload set. A
  # crash can occur after state=activating but before the first per-payload
  # marker is written, so recovery must still restart that existing set.
  if [[ "$transaction_mode" != "first-start" ]]; then
    had_current=1
  fi
  case "$state" in
    healthy)
      finalize_transaction "$transaction"
      install_cli_link
      safe_remove_transaction "$transaction"
      log "completed the verified release transaction"
      return 0
      ;;
    finalized)
      install_cli_link
      safe_remove_transaction "$transaction"
      log "removed the completed release transaction"
      return 0
      ;;
    preparing)
      safe_remove_transaction "$transaction"
      log "removed an uncommitted release transaction"
      return 0
      ;;
    stopping | backup_ready)
      if payloads_complete; then
        recreate_services ||
          fail "could not restart the old deployment after the interrupted preflight"
        wait_for_deployment "" ||
          fail "old deployment did not recover after the interrupted preflight"
      fi
      safe_remove_transaction "$transaction"
      log "recovered the deployment before payload activation"
      return 0
      ;;
    activating | running_new) ;;
    *)
      fail "unknown transaction state '$state'; preserve $transaction for manual recovery"
      ;;
  esac

  [[ -n "$backup_name" ]] ||
    fail "activated transaction has no database-backup metadata; preserve $transaction for manual recovery"
  [[ "$backup_name" =~ ^pre-update-[0-9]{8}T[0-9]{6}Z-v[0-9A-Za-z.-]+\.dump$ ]] ||
    fail "transaction database-backup metadata is invalid"
  backup_path="$runtime_dir/update-backups/$backup_name"
  database_backup_is_valid "$backup_path" ||
    fail "activated transaction has no valid database backup; preserve $transaction for manual recovery"
  stop_application_services ||
    fail "could not stop application services; database and payload recovery were not attempted"
  restore_database "$backup_path"

  for kind in server frontend cli; do
    if [[ -f "$transaction/had-current-$kind" ]]; then
      had_current=1
      if [[ -d "$transaction/old-$kind" ]]; then
        if [[ "$transaction_mode" == "rollback" && -d "$runtime_dir/$kind/current" ]]; then
          [[ ! -e "$runtime_dir/$kind/previous" && ! -L "$runtime_dir/$kind/previous" ]] ||
            fail "cannot recover rollback because $kind previous unexpectedly exists"
          mv -- "$runtime_dir/$kind/current" "$runtime_dir/$kind/previous" ||
            fail "could not preserve the failed rollback $kind payload; preserve $transaction for recovery"
        elif [[ -d "$runtime_dir/$kind/current" ]]; then
          mv -- "$runtime_dir/$kind/current" "$transaction/failed-$kind" ||
            fail "could not quarantine the failed $kind payload; preserve $transaction for recovery"
        fi
        [[ ! -e "$runtime_dir/$kind/current" ]] ||
          fail "cannot restore previous $kind payload because current still exists"
        mv -- "$transaction/old-$kind" "$runtime_dir/$kind/current" ||
          fail "could not restore the previous $kind payload; preserve $transaction for recovery"
      fi
    elif [[ "$transaction_mode" == "first-start" && -d "$runtime_dir/$kind/current" ]]; then
      # First-start has no old payload. An unmarked current directory can only
      # be a newly activated payload and must be removed on failed activation.
      mv -- "$runtime_dir/$kind/current" "$transaction/failed-$kind" ||
        fail "could not quarantine the failed first-start $kind payload; preserve $transaction for recovery"
    fi
  done

  if [[ "$had_current" == "1" ]]; then
    payloads_complete ||
      fail "payload recovery is incomplete; preserve $transaction for manual recovery"
    recreate_services ||
      fail "could not restart the previous deployment; preserve $transaction for recovery"
    wait_for_deployment "" ||
      fail "previous deployment payloads were restored but did not become ready"
  fi
  safe_remove_transaction "$transaction"
  log "recovered the previous deployment state"
}

find_active_transaction() {
  local -a transactions=()
  if [[ -d "$transactions_dir" ]]; then
    mapfile -t transactions < <(
      find "$transactions_dir" -mindepth 1 -maxdepth 1 -type d -name 'update.*' -print |
        sort
    )
  fi
  if ((${#transactions[@]} > 1)); then
    fail "multiple interrupted update transactions exist under runtime/transactions; preserve them for manual inspection"
  fi
  if ((${#transactions[@]} == 1)); then
    printf '%s\n' "${transactions[0]}"
  fi
}

cleanup_abandoned_transaction_dirs() {
  local -a abandoned=()
  local path noun
  mapfile -t abandoned < <(
    find "$transactions_dir" \
      -mindepth 1 \
      -maxdepth 1 \
      -type d \
      \( -name '.initializing.*' -o -name '.cleanup.*' \) \
      -print |
      sort
  )
  for path in "${abandoned[@]}"; do
    safe_remove_transaction "$path"
  done
  if ((${#abandoned[@]} > 0)); then
    noun="directories"
    if ((${#abandoned[@]} == 1)); then
      noun="directory"
    fi
    log "removed ${#abandoned[@]} abandoned transaction staging $noun"
  fi
}

cleanup_abandoned_backup_partials() {
  local backup_dir="$runtime_dir/update-backups"
  local path name noun
  local -a partials=()

  shopt -s nullglob
  partials=("$backup_dir"/.pre-update-*.dump.partial.*)
  shopt -u nullglob
  for path in "${partials[@]}"; do
    name="${path##*/}"
    [[ -f "$path" && ! -L "$path" &&
      "$name" =~ ^\.pre-update-[0-9]{8}T[0-9]{6}Z-v[0-9A-Za-z.-]+\.dump\.partial\.[0-9A-Za-z]{6}$ ]] ||
      fail "unrecognized PostgreSQL backup partial $path; preserve it for manual inspection"
    remove_backup_partial "$path"
  done
  if ((${#partials[@]} > 0)); then
    noun="partials"
    if ((${#partials[@]} == 1)); then
      noun="partial"
    fi
    log "removed ${#partials[@]} abandoned PostgreSQL backup $noun"
  fi
}

create_transaction() {
  local transaction_mode="$1"
  local suffix visible_transaction
  transaction="$(mktemp -d "$transactions_dir/.initializing.XXXXXX")" ||
    fail "could not create a release transaction"
  transaction_can_cleanup=1
  write_transaction_value "$transaction" mode "$transaction_mode"
  write_transaction_value "$transaction" state preparing
  suffix="${transaction##*.initializing.}"
  visible_transaction="$transactions_dir/update.$suffix"
  [[ ! -e "$visible_transaction" ]] ||
    fail "refusing to replace an existing update transaction: $visible_transaction"
  mv -- "$transaction" "$visible_transaction" ||
    fail "could not publish release transaction $visible_transaction"
  transaction="$visible_transaction"
}

install_cli_link() {
  local link="$script_dir/vpsctl"
  if [[ ! -e "$link" || -L "$link" ]]; then
    ln -sfn runtime/cli/current/vpsctl "$link" ||
      fail "could not install the vpsctl link; preserve $transaction for recovery"
  else
    log "left existing non-symlink $link unchanged; use runtime/cli/current/vpsctl"
  fi
}

activate_transaction() {
  local transaction="$1"
  local transaction_mode="$2"
  local expected_tag="$3"
  local kind source

  transaction_can_cleanup=0
  write_transaction_value "$transaction" state activating
  for kind in server frontend cli; do
    if [[ -d "$runtime_dir/$kind/current" ]]; then
      : >"$transaction/had-current-$kind" ||
        fail "could not journal the current $kind payload; preserve $transaction for recovery"
      mv -- "$runtime_dir/$kind/current" "$transaction/old-$kind" ||
        fail "could not preserve the current $kind payload; preserve $transaction for recovery"
    fi
    if [[ "$transaction_mode" == "rollback" ]]; then
      source="$runtime_dir/$kind/previous"
    else
      source="$transaction/staged-$kind"
    fi
    [[ -d "$source" ]] || fail "release transaction is missing $kind payload"
    mv -- "$source" "$runtime_dir/$kind/current" ||
      fail "could not activate the candidate $kind payload; preserve $transaction for recovery"
  done
  write_transaction_value "$transaction" state running_new

  if ! recreate_services; then
    log "new deployment failed readiness or release-identity verification; restoring the previous state"
    recover_transaction "$transaction"
    if [[ "$transaction_mode" == "rollback" ]]; then
      fail "rollback candidate failed readiness; the original deployment was restored"
    fi
    fail "release $expected_tag failed readiness; the previous deployment was restored"
  fi
  if ! wait_for_deployment "$expected_tag"; then
    log "new deployment failed readiness or release-identity verification; restoring the previous state"
    recover_transaction "$transaction"
    if [[ "$transaction_mode" == "rollback" ]]; then
      fail "rollback candidate failed readiness; the original deployment was restored"
    fi
    fail "release $expected_tag failed readiness; the previous deployment was restored"
  fi

  write_transaction_value "$transaction" state healthy
  finalize_transaction "$transaction"
  install_cli_link
  safe_remove_transaction "$transaction"
}

cleanup_uncommitted_transaction() {
  if [[ "$transaction_can_cleanup" == "1" && -n "$transaction" ]]; then
    safe_remove_transaction "$transaction"
  fi
}
trap cleanup_uncommitted_transaction EXIT

require_env
require_tool docker
require_tool flock
require_tool python3
require_tool sha384sum

mkdir -p "$runtime_dir" "$transactions_dir" "$runtime_dir/update-backups"
exec 9>"$runtime_dir/update.lock"
flock -n 9 || fail "another update, rollback, or recovery is already running"

cleanup_abandoned_backup_partials
cleanup_abandoned_transaction_dirs
active_transaction="$(find_active_transaction)"
if [[ -n "$active_transaction" ]]; then
  if [[ "$target" != "recover" ]]; then
    fail "an interrupted transaction exists; run ./update.sh recover before another update"
  fi
  recover_transaction "$active_transaction"
  exit 0
fi
if [[ "$target" == "recover" ]]; then
  log "no interrupted release transaction exists"
  exit 0
fi

compose config --quiet

if [[ "$target" == "rollback" ]]; then
  [[ "$mode" != "first-start" ]] || fail "first-start cannot target rollback"
  for kind in server frontend cli; do
    [[ -d "$runtime_dir/$kind/current" && -d "$runtime_dir/$kind/previous" ]] ||
      fail "rollback is unavailable; current or previous $kind release is missing"
  done
  wait_for_postgres || fail "PostgreSQL did not become ready for rollback preflight"
  create_transaction rollback
  rollback_tag=""
  if [[ -f "$runtime_dir/server/previous/.vpsman-release.json" ]]; then
    IFS=$'\t' read -r rollback_tag _ < <(
      read_validated_manifest "$runtime_dir/server/previous/.vpsman-release.json"
    )
    require_supported_release "$rollback_tag"
  fi
  write_transaction_value "$transaction" tag "$rollback_tag"
  verify_database_compatible_with \
    "$runtime_dir/server/previous/migrations" \
    "$transaction/migration-rows.txt"
  write_transaction_value "$transaction" state stopping
  transaction_can_cleanup=0
  stop_application_services
  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  backup_name="pre-update-$timestamp-${rollback_tag:-v0.0.0-rollback}.dump"
  write_transaction_value "$transaction" backup "$backup_name"
  backup_database "$runtime_dir/update-backups/$backup_name"
  write_transaction_value "$transaction" state backup_ready
  activate_transaction "$transaction" rollback "$rollback_tag"
  log "rollback complete; pre-rollback database backup: runtime/update-backups/$backup_name"
  exit 0
fi

require_tool curl
require_tool cmp
require_tool diff
require_tool sha256sum
require_tool tar
require_tool unzip

SERVER_ASSET="$(detect_server_asset)"
CLI_ASSET="${VPSMAN_CLI_ASSET:-$(detect_cli_asset)}"
validate_asset_name "$SERVER_ASSET"
validate_asset_name "$CLI_ASSET"

for kind in server frontend cli; do
  mkdir -p "$runtime_dir/$kind"
done
if [[ "$mode" == "first-start" ]]; then
  for kind in server frontend cli; do
    [[ ! -e "$runtime_dir/$kind/current" ]] ||
      fail "first-start refuses to replace an existing $kind release; use latest instead"
  done
else
  payloads_complete ||
    fail "existing deployment payloads are incomplete; use first-start only for a new deployment"
fi

create_transaction "$mode"
mkdir -p "$transaction/downloads"

base_url="$(release_base_url "$target")"
download_url "$base_url/version.json" "$transaction/downloads/version.json"
IFS=$'\t' read -r resolved_tag resolved_commit < <(
  read_validated_manifest "$transaction/downloads/version.json"
)
require_supported_release "$resolved_tag"
if [[ "$target" != "latest" && "$resolved_tag" != "$target" ]]; then
  fail "release manifest resolved $resolved_tag but exact target $target was requested"
fi
if [[ "$target" == "latest" && "$resolved_tag" == *-* ]]; then
  fail "the stable latest endpoint resolved prerelease $resolved_tag; pin it explicitly only after review"
fi
write_transaction_value "$transaction" tag "$resolved_tag"
write_transaction_value "$transaction" commit "$resolved_commit"

pinned_base_url="$(release_pinned_base_url "$resolved_tag")"
download_url "$pinned_base_url/SHA256SUMS" "$transaction/downloads/SHA256SUMS"
download_url "$pinned_base_url/$SERVER_ASSET" "$transaction/downloads/$SERVER_ASSET"
download_url "$pinned_base_url/$FRONTEND_ASSET" "$transaction/downloads/$FRONTEND_ASSET"
download_url "$pinned_base_url/$CLI_ASSET" "$transaction/downloads/$CLI_ASSET"

awk \
  -v server="$SERVER_ASSET" \
  -v frontend="$FRONTEND_ASSET" \
  -v cli="$CLI_ASSET" \
  '$2 == server || $2 == frontend || $2 == cli || $2 == "version.json" {
     print
     seen[$2]++
   }
   END {
     valid = seen[server] == 1 &&
       seen[frontend] == 1 &&
       seen[cli] == 1 &&
       seen["version.json"] == 1
     exit valid ? 0 : 1
   }' \
  "$transaction/downloads/SHA256SUMS" \
  >"$transaction/downloads/SHA256SUMS.selected" ||
  fail "release checksum manifest does not contain each required asset exactly once"
(cd "$transaction/downloads" && sha256sum -c SHA256SUMS.selected)
write_selected_asset_identity \
  "$transaction/downloads/SHA256SUMS.selected" \
  "$transaction/selected-assets.tsv"

validate_archives \
  "$transaction/downloads/$SERVER_ASSET" \
  "$transaction/downloads/$FRONTEND_ASSET"

mkdir -p \
  "$transaction/staged-server" \
  "$transaction/staged-frontend" \
  "$transaction/staged-cli"
unzip -q "$transaction/downloads/$SERVER_ASSET" -d "$transaction/staged-server"
tar -xzf "$transaction/downloads/$FRONTEND_ASSET" -C "$transaction/staged-frontend"
cp "$transaction/downloads/$CLI_ASSET" "$transaction/staged-cli/vpsctl"
chmod +x \
  "$transaction/staged-server/bin/vpsman-api" \
  "$transaction/staged-server/bin/vpsman-gateway" \
  "$transaction/staged-server/bin/vpsman-worker" \
  "$transaction/staged-cli/vpsctl"

[[ -x "$transaction/staged-server/bin/vpsman-api" &&
  -x "$transaction/staged-server/bin/vpsman-gateway" &&
  -x "$transaction/staged-server/bin/vpsman-worker" &&
  -d "$transaction/staged-server/migrations" ]] ||
  fail "server release layout is invalid"
[[ -f "$transaction/staged-frontend/dist/index.html" ]] ||
  fail "frontend release layout is invalid"
[[ -x "$transaction/staged-cli/vpsctl" ]] ||
  fail "CLI release layout is invalid"

"$transaction/staged-server/bin/vpsman-api" --help >/dev/null
"$transaction/staged-server/bin/vpsman-gateway" --help >/dev/null
"$transaction/staged-server/bin/vpsman-worker" --help >/dev/null
"$transaction/staged-cli/vpsctl" --version |
  grep -Fq "vpsctl ${resolved_tag#v} " ||
  fail "vpsctl release identity does not match ${resolved_tag#v}"

for kind in server frontend cli; do
  cp "$transaction/downloads/version.json" \
    "$transaction/staged-$kind/.vpsman-release.json"
  cp "$transaction/selected-assets.tsv" \
    "$transaction/staged-$kind/.vpsman-assets.tsv"
done

active_release_tag=""
if [[ -f "$script_dir/RELEASE_TAG" ]]; then
  active_release_tag="$(<"$script_dir/RELEASE_TAG")"
fi
if [[ "$mode" == "update" && "$active_release_tag" == "$resolved_tag" ]]; then
  current_release_identity_matches \
    "$transaction/downloads/version.json" \
    "$transaction/selected-assets.tsv" \
    "$resolved_tag" ||
    fail "release $resolved_tag is already marked active, but its persisted manifest or selected asset identities differ from the verified target; refusing a same-tag replacement to preserve the rollback payload"
  if ! current_payload_layout_is_valid ||
    ! current_payloads_match_staged; then
    fail "release $resolved_tag is already marked active, but the current payload layout or contents are incomplete or corrupt; refusing a same-tag replacement to preserve the rollback payload"
  fi
  wait_for_deployment "$resolved_tag" ||
    fail "release $resolved_tag is already marked active, but live services are stopped, unhealthy, or report a different build tag; refusing a same-tag replacement to preserve the rollback payload"
  safe_remove_transaction "$transaction"
  transaction=""
  transaction_can_cleanup=0
  log "release $resolved_tag payloads and live services are already active and verified; no update was applied, services were not stopped, and rollback payloads were left unchanged"
  exit 0
fi

mkdir -p "$runtime_dir/downloads"
cp "$transaction/downloads/version.json" \
  "$runtime_dir/downloads/version-$resolved_tag.json"
cp "$transaction/downloads/SHA256SUMS" \
  "$runtime_dir/downloads/SHA256SUMS-$resolved_tag"

if [[ "$mode" == "first-start" ]]; then
  prepare_first_start_secrets "$transaction/staged-cli/vpsctl"
  wait_for_postgres || fail "PostgreSQL did not become ready for first-start preflight"
  if [[ "$(database_migration_ledger_status)" == "t" ]]; then
    verify_database_compatible_with \
      "$transaction/staged-server/migrations" \
      "$transaction/migration-rows.txt"
  fi
  # Even a fresh PostgreSQL instance is snapshotted before API migrations.
  # This also makes the documented restore-then-first-start path reversible.
  write_transaction_value "$transaction" state stopping
  transaction_can_cleanup=0
  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  backup_name="pre-update-$timestamp-$resolved_tag.dump"
  write_transaction_value "$transaction" backup "$backup_name"
  backup_database "$runtime_dir/update-backups/$backup_name"
  write_transaction_value "$transaction" state backup_ready
else
  wait_for_postgres || fail "PostgreSQL did not become ready for update preflight"
  verify_database_compatible_with \
    "$transaction/staged-server/migrations" \
    "$transaction/migration-rows.txt"
  write_transaction_value "$transaction" state stopping
  transaction_can_cleanup=0
  stop_application_services
  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  backup_name="pre-update-$timestamp-$resolved_tag.dump"
  write_transaction_value "$transaction" backup "$backup_name"
  backup_database "$runtime_dir/update-backups/$backup_name"
  write_transaction_value "$transaction" state backup_ready
fi

activate_transaction "$transaction" "$mode" "$resolved_tag"

if [[ "$mode" == "first-start" ]]; then
  log "started vpsman deployment at $resolved_tag"
  log "pre-start database backup: runtime/update-backups/$backup_name"
else
  log "updated vpsman deployment to $resolved_tag"
  log "pre-update database backup: runtime/update-backups/$backup_name"
fi
