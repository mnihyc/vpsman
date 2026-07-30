#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SMOKE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/vpsman-deploy-updater.XXXXXX")"
exec 3>&2

cleanup() {
  local status="$?"
  if [[ "$status" != "0" && "${VPSMAN_SMOKE_KEEP_ON_FAILURE:-0}" == "1" ]]; then
    printf 'preserved failed updater smoke workspace: %s\n' "$SMOKE_ROOT" >&3
    return
  fi
  case "$SMOKE_ROOT" in
    "${TMPDIR:-/tmp}"/vpsman-deploy-updater.*)
      rm -rf -- "$SMOKE_ROOT"
      ;;
    *)
      printf 'refusing to clean unexpected updater smoke path: %s\n' "$SMOKE_ROOT" >&3
      ;;
  esac
}
trap cleanup EXIT

report_error() {
  local status="$?"
  local log
  printf 'deploy updater smoke stopped unexpectedly (status %s)\n' "$status" >&3
  while IFS= read -r log; do
    printf '%s\n' "--- ${log#"$SMOKE_ROOT"/} ---" >&3
    tail -n 80 "$log" >&3
  done < <(find "$SMOKE_ROOT" -type f -name '*.log' -print | sort)
  exit "$status"
}
trap report_error ERR

fail() {
  printf 'deploy updater smoke failed: %s\n' "$*" >&2
  exit 1
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 ||
    fail "missing required tool: $1"
}

for tool in bash cmp cp date diff flock ln mv python3 sha256sum sha384sum stat tar unzip zip; do
  require_tool "$tool"
done

FAKE_BIN="$SMOKE_ROOT/bin"
RELEASE_DIR="$SMOKE_ROOT/release"
REAL_DATE="$(command -v date)"
REAL_MV="$(command -v mv)"
mkdir -p "$FAKE_BIN" "$RELEASE_DIR"

cat >"$FAKE_BIN/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

output=""
url=""
while (($#)); do
  case "$1" in
    -o)
      output="$2"
      shift 2
      ;;
    -H | --retry | --connect-timeout)
      shift 2
      ;;
    -*)
      shift
      ;;
    *)
      url="$1"
      shift
      ;;
  esac
done

[[ -n "$output" && -n "$url" ]]
asset="${url##*/}"
cp "$VPSMAN_SMOKE_RELEASE_DIR/$asset" "$output"
SH
chmod 0755 "$FAKE_BIN/curl"

cat >"$FAKE_BIN/date" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

if [[ -n "${VPSMAN_SMOKE_TIMESTAMP:-}" &&
  "$#" == "2" &&
  "$1" == "-u" &&
  "$2" == "+%Y%m%dT%H%M%SZ" ]]; then
  printf '%s\n' "$VPSMAN_SMOKE_TIMESTAMP"
  exit 0
fi
exec "$VPSMAN_SMOKE_REAL_DATE" "$@"
SH
chmod 0755 "$FAKE_BIN/date"

cat >"$FAKE_BIN/mv" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

if [[ "${VPSMAN_SMOKE_FAIL_FINALIZE_MV:-0}" == "1" && "$#" -ge 2 ]]; then
  source_arg="${@: -2:1}"
  destination_arg="${@: -1}"
  if [[ "$source_arg" == */old-server &&
    "$destination_arg" == */runtime/server/previous &&
    ! -e "$VPSMAN_SMOKE_MV_FAILURE_MARKER" ]]; then
    : >"$VPSMAN_SMOKE_MV_FAILURE_MARKER"
    printf 'injected old-server to previous mv failure\n' >&2
    exit 73
  fi
fi
exec "$VPSMAN_SMOKE_REAL_MV" "$@"
SH
chmod 0755 "$FAKE_BIN/mv"

cat >"$FAKE_BIN/docker" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

[[ "${1:-}" == "compose" ]] || exit 1
shift
if [[ "${1:-}" == "version" ]]; then
  exit 0
fi

while (($#)); do
  case "$1" in
    --env-file | -f)
      shift 2
      ;;
    *)
      break
      ;;
  esac
done

printf '%s\n' "$*" >>"$VPSMAN_SMOKE_DOCKER_LOG"
case "${1:-}" in
  config)
    exit 0
    ;;
  stop)
    if [[ "${VPSMAN_SMOKE_FAIL_STOP:-0}" == "1" ]]; then
      exit 76
    fi
    exit 0
    ;;
  up)
    if [[ "${VPSMAN_SMOKE_FAIL_APPLICATION_UP:-0}" == "1" &&
      " $* " == *" api "* ]]; then
      exit 1
    fi
    exit 0
    ;;
  ps)
    for service in api gateway worker frontend; do
      if [[ "$service" != "${VPSMAN_SMOKE_STOPPED_SERVICE:-}" ]]; then
        printf '%s\n' "$service"
      fi
    done
    ;;
  exec)
    endpoint="${!#}"
    case "$endpoint" in
      http://127.0.0.1/health)
        if [[ "${VPSMAN_SMOKE_HEALTH_UNREADY:-0}" == "1" ]]; then
          printf 'not-ready\n'
        else
          printf 'ok\n'
        fi
        ;;
      http://127.0.0.1/api/v1/auth/bootstrap-status)
        printf '{}\n'
        ;;
      http://127.0.0.1/api/v1/build-info)
        printf '{"release_tag":"%s"}\n' "$VPSMAN_SMOKE_BUILD_TAG"
        ;;
      *)
        case "$*" in
          *pg_isready*)
            exit 0
            ;;
          *"SELECT to_regclass("*)
            printf '%s\n' "${VPSMAN_SMOKE_MIGRATION_LEDGER_STATUS:-f}"
            ;;
          *"SELECT count(*) FROM _sqlx_migrations WHERE NOT success"*)
            printf '0\n'
            ;;
          *"SELECT version, encode(checksum"*)
            if [[ -n "${VPSMAN_SMOKE_MIGRATION_ROWS_FILE:-}" ]]; then
              cat "$VPSMAN_SMOKE_MIGRATION_ROWS_FILE"
            fi
            ;;
          *"pg_dump --format=custom"*)
            printf 'vpsman updater smoke database dump partial\n'
            if [[ "${VPSMAN_SMOKE_FAIL_PG_DUMP:-0}" == "1" ]]; then
              exit 74
            fi
            printf 'vpsman updater smoke database dump complete\n'
            ;;
          *"pg_restore --list"*)
            cat >/dev/null
            if [[ "${VPSMAN_SMOKE_FAIL_PG_RESTORE_LIST:-0}" == "1" ]]; then
              exit 75
            fi
            printf 'vpsman updater smoke archive listing\n'
            ;;
          *"dropdb --if-exists"*"pg_restore --exit-on-error"*)
            cat >/dev/null
            printf 'database restore\n' >>"$VPSMAN_SMOKE_DOCKER_LOG"
            ;;
          *)
            exit 1
            ;;
        esac
        ;;
    esac
    ;;
  *)
    exit 1
    ;;
esac
SH
chmod 0755 "$FAKE_BIN/docker"

SERVER_STAGE="$SMOKE_ROOT/server-stage"
FRONTEND_STAGE="$SMOKE_ROOT/frontend-stage"
mkdir -p "$SERVER_STAGE/bin" "$SERVER_STAGE/migrations" "$FRONTEND_STAGE/dist"
for binary in vpsman-api vpsman-gateway vpsman-worker; do
  cat >"$SERVER_STAGE/bin/$binary" <<'SH'
#!/bin/sh
exit 0
SH
  chmod 0755 "$SERVER_STAGE/bin/$binary"
done
printf '%s\n' '-- updater smoke migration fixture' >"$SERVER_STAGE/migrations/0001_smoke.sql"
printf '%s\n' '<!doctype html><title>vpsman updater smoke</title>' >"$FRONTEND_STAGE/dist/index.html"

(
  cd "$SERVER_STAGE"
  zip -qr "$RELEASE_DIR/vpsman-server-linux-x86_64.zip" .
)
cp \
  "$RELEASE_DIR/vpsman-server-linux-x86_64.zip" \
  "$RELEASE_DIR/vpsman-server-linux-x86_64-alt.zip"
tar -C "$FRONTEND_STAGE" -czf "$RELEASE_DIR/vpsman-frontend-dist.tar.gz" dist

cat >"$RELEASE_DIR/vpsctl-linux-x86_64-musl" <<'SH'
#!/bin/sh
if [ "${1:-}" = "--version" ]; then
  printf 'vpsctl 9.8.7 (cli build 1)\n'
  exit 0
fi
if [ "${1:-}" = "compose-secrets" ]; then
  shift
  secrets_dir=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --secrets-dir)
        secrets_dir="$2"
        shift 2
        ;;
      *)
        exit 1
        ;;
    esac
  done
  [ -n "${VPSMAN_SUPER_PASSWORD:-}" ] && [ -n "$secrets_dir" ] || exit 1
  umask 077
  mkdir -p "$secrets_dir"
  printf '%064d\n' 0 >"$secrets_dir/vpsman_internal_token"
  printf '%064d\n' 1 >"$secrets_dir/vpsman_gateway_private_key_hex"
  printf '%064d\n' 2 >"$secrets_dir/vpsman_privilege_verifier_key_hex"
  printf '%064d\n' 3 >"$secrets_dir/vpsman_gateway_public_key_hex"
  printf '%s\n' \
    'export VPSMAN_SUPER_SALT_HEX=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' \
    >"$secrets_dir/operator-privilege.env"
  printf '%s\n' '{"compose_secrets":"ok"}'
  exit 0
fi
exit 1
SH
chmod 0755 "$RELEASE_DIR/vpsctl-linux-x86_64-musl"

cat >"$RELEASE_DIR/version.json" <<'JSON'
{
  "schema_version": 3,
  "project": "vpsman",
  "tag": "v9.8.7",
  "version": "9.8.7",
  "commit": "1111111111111111111111111111111111111111",
  "assets": [
    {
      "name": "vpsman-server-linux-x86_64.zip",
      "download_url": "https://github.com/example/vpsman/releases/download/v9.8.7/vpsman-server-linux-x86_64.zip"
    },
    {
      "name": "vpsman-server-linux-x86_64-alt.zip",
      "download_url": "https://github.com/example/vpsman/releases/download/v9.8.7/vpsman-server-linux-x86_64-alt.zip"
    },
    {
      "name": "vpsman-frontend-dist.tar.gz",
      "download_url": "https://github.com/example/vpsman/releases/download/v9.8.7/vpsman-frontend-dist.tar.gz"
    },
    {
      "name": "vpsctl-linux-x86_64-musl",
      "download_url": "https://github.com/example/vpsman/releases/download/v9.8.7/vpsctl-linux-x86_64-musl"
    }
  ]
}
JSON

RELEASE_V988_DIR="$SMOKE_ROOT/release-v9.8.8"
cp -a "$RELEASE_DIR" "$RELEASE_V988_DIR"
cat >"$RELEASE_V988_DIR/vpsctl-linux-x86_64-musl" <<'SH'
#!/bin/sh
if [ "${1:-}" = "--version" ]; then
  printf 'vpsctl 9.8.8 (cli build 2)\n'
  exit 0
fi
if [ "${1:-}" = "compose-secrets" ]; then
  shift
  secrets_dir=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --secrets-dir)
        secrets_dir="$2"
        shift 2
        ;;
      *)
        exit 1
        ;;
    esac
  done
  [ -n "${VPSMAN_SUPER_PASSWORD:-}" ] && [ -n "$secrets_dir" ] || exit 1
  umask 077
  mkdir -p "$secrets_dir"
  printf '%064d\n' 0 >"$secrets_dir/vpsman_internal_token"
  printf '%064d\n' 1 >"$secrets_dir/vpsman_gateway_private_key_hex"
  printf '%064d\n' 2 >"$secrets_dir/vpsman_privilege_verifier_key_hex"
  printf '%064d\n' 3 >"$secrets_dir/vpsman_gateway_public_key_hex"
  printf '%s\n' \
    'export VPSMAN_SUPER_SALT_HEX=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' \
    >"$secrets_dir/operator-privilege.env"
  printf '%s\n' '{"compose_secrets":"ok"}'
  exit 0
fi
exit 1
SH
chmod 0755 "$RELEASE_V988_DIR/vpsctl-linux-x86_64-musl"
python3 - "$RELEASE_V988_DIR/version.json" <<'PY'
import json
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
manifest = json.loads(path.read_text(encoding="utf-8"))
manifest["tag"] = "v9.8.8"
manifest["version"] = "9.8.8"
manifest["commit"] = "2222222222222222222222222222222222222222"
for asset in manifest["assets"]:
    asset["download_url"] = (
        "https://github.com/example/vpsman/releases/download/v9.8.8/"
        + asset["name"]
    )
path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
PY

prepare_deployment() {
  local destination="$1"
  local password="${2:-0123456789abcdef0123456789abcdef}"
  local secret

  mkdir -p "$destination/config/secrets"
  cp "$ROOT_DIR/deploy/update.sh" "$destination/update.sh"
  cp "$ROOT_DIR/deploy/compose.yml" "$destination/compose.yml"
  chmod 0755 "$destination/update.sh"
  cat >"$destination/.env" <<EOF
POSTGRES_DB=vpsman
POSTGRES_USER=vpsman
POSTGRES_PASSWORD=$password
EOF
  for secret in \
    vpsman_internal_token \
    vpsman_gateway_private_key_hex \
    vpsman_privilege_verifier_key_hex \
    vpsman_gateway_public_key_hex
  do
    printf 'updater-smoke-secret\n' >"$destination/config/secrets/$secret"
  done
  printf '%s\n' \
    'export VPSMAN_SUPER_SALT_HEX=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' \
    >"$destination/config/secrets/operator-privilege.env"
}

run_updater() {
  local deployment="$1"
  shift
  (
    cd "$deployment"
    env \
      PATH="$FAKE_BIN:$PATH" \
      VPSMAN_RELEASE_REPO=example/vpsman \
      VPSMAN_SMOKE_RELEASE_DIR="$RELEASE_DIR" \
      VPSMAN_SMOKE_DOCKER_LOG="$deployment/docker.log" \
      VPSMAN_SMOKE_BUILD_TAG="${VPSMAN_SMOKE_BUILD_TAG:-v9.8.7}" \
      VPSMAN_SMOKE_FAIL_FINALIZE_MV="${VPSMAN_SMOKE_FAIL_FINALIZE_MV:-0}" \
      VPSMAN_SMOKE_FAIL_PG_DUMP="${VPSMAN_SMOKE_FAIL_PG_DUMP:-0}" \
      VPSMAN_SMOKE_FAIL_PG_RESTORE_LIST="${VPSMAN_SMOKE_FAIL_PG_RESTORE_LIST:-0}" \
      VPSMAN_SMOKE_FAIL_STOP="${VPSMAN_SMOKE_FAIL_STOP:-0}" \
      VPSMAN_SMOKE_MV_FAILURE_MARKER="$deployment/mv-failure-fired" \
      VPSMAN_SMOKE_REAL_DATE="$REAL_DATE" \
      VPSMAN_SMOKE_REAL_MV="$REAL_MV" \
      VPSMAN_SMOKE_TIMESTAMP="${VPSMAN_SMOKE_TIMESTAMP:-}" \
      VPSMAN_UPDATE_HEALTH_TIMEOUT_SECS="${VPSMAN_UPDATE_HEALTH_TIMEOUT_SECS:-10}" \
      bash ./update.sh "$@"
  )
}

release_state_digest() {
  local deployment="$1"
  (
    cd "$deployment"
    tar \
      --sort=name \
      --mtime=@0 \
      --owner=0 \
      --group=0 \
      --numeric-owner \
      -cf - \
      RELEASE_TAG \
      vpsctl \
      runtime/downloads \
      runtime/update-backups \
      runtime/server/current \
      runtime/server/previous \
      runtime/frontend/current \
      runtime/frontend/previous \
      runtime/cli/current \
      runtime/cli/previous
  ) | sha256sum | awk '{print $1}'
}

assert_no_service_mutation() {
  local deployment="$1"
  local label="$2"
  if grep -Eq '^(stop|up)( |$)' "$deployment/docker.log"; then
    fail "$label stopped or started services"
  fi
}

assert_no_active_transaction() {
  local deployment="$1"
  local label="$2"
  [[ -z "$(find "$deployment/runtime/transactions" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
    fail "$label left a transaction journal"
}

assert_backup_mode_0600() {
  local backup="$1"
  local label="$2"
  [[ -f "$backup" && ! -L "$backup" ]] ||
    fail "$label did not publish a regular backup file"
  [[ "$(stat -c '%a' "$backup")" == "600" ]] ||
    fail "$label backup mode is not 0600"
}

FIRST_START="$SMOKE_ROOT/first-start"
prepare_deployment "$FIRST_START"
for secret in \
  vpsman_internal_token \
  vpsman_gateway_private_key_hex \
  vpsman_privilege_verifier_key_hex \
  vpsman_gateway_public_key_hex \
  operator-privilege.env
do
  rm -f -- "$FIRST_START/config/secrets/$secret"
done
VPSMAN_SUPER_PASSWORD='updater-super-password-must-not-print' \
  run_updater "$FIRST_START" first-start v9.8.7 >"$FIRST_START/update.log" 2>&1
for kind in server frontend cli; do
  [[ -d "$FIRST_START/runtime/$kind/current" ]] ||
    fail "first-start did not activate $kind"
  [[ -f "$FIRST_START/runtime/$kind/current/.vpsman-release.json" ]] ||
    fail "first-start did not record $kind release identity"
  [[ ! -e "$FIRST_START/runtime/$kind/previous" ]] ||
    fail "first-start unexpectedly created previous $kind"
done
[[ -L "$FIRST_START/vpsctl" ]] || fail "first-start did not create the CLI link"
[[ "$(<"$FIRST_START/RELEASE_TAG")" == "v9.8.7" ]] ||
  fail "first-start did not commit the active release marker"
[[ "$(stat -c '%a' "$FIRST_START/RELEASE_TAG")" == "644" ]] ||
  fail "first-start active release marker permissions are not stable"
mapfile -t first_start_backups < <(
  find "$FIRST_START/runtime/update-backups" \
    -type f \
    -name 'pre-update-*-v9.8.7.dump' \
    -print
)
[[ "${#first_start_backups[@]}" == "1" ]] ||
  fail "first-start did not retain exactly one pre-activation database backup"
assert_backup_mode_0600 "${first_start_backups[0]}" "first-start"
grep -Fq 'pg_restore --list' "$FIRST_START/docker.log" ||
  fail "first-start did not validate its database backup with pg_restore --list"
[[ -z "$(
  find "$FIRST_START/runtime/update-backups" \
    -mindepth 1 \
    -maxdepth 1 \
    -name '.*.partial.*' \
    -print -quit
)" ]] || fail "first-start left a PostgreSQL backup partial"
[[ -z "$(find "$FIRST_START/runtime/transactions" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
  fail "first-start left an active transaction"
grep -Fq 'started vpsman deployment at v9.8.7' "$FIRST_START/update.log" ||
  fail "first-start did not report the activated release"
grep -Fq \
  'VPSMAN_SUPER_SALT_HEX=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' \
  "$FIRST_START/update.log" ||
  fail "first-start did not print its generated privilege salt"
grep -Fq \
  'persistent copy: ./config/secrets/operator-privilege.env (keep private)' \
  "$FIRST_START/update.log" ||
  fail "first-start did not identify the persistent operator salt file"
if grep -Fq 'updater-super-password-must-not-print' "$FIRST_START/update.log"; then
  fail "first-start printed the super password"
fi

VERSION_FLOW="$SMOKE_ROOT/version-flow"
MUTATION_FAILURE="$SMOKE_ROOT/mutation-failure"
cp -a "$FIRST_START" "$VERSION_FLOW"
cp -a "$FIRST_START" "$MUTATION_FAILURE"
version_flow_migration_rows="$SMOKE_ROOT/version-flow-migration-rows.txt"
: >"$version_flow_migration_rows"
: >"$VERSION_FLOW/docker.log"
if ! RELEASE_DIR="$RELEASE_V988_DIR" \
  VPSMAN_SMOKE_BUILD_TAG=v9.8.8 \
  VPSMAN_SMOKE_MIGRATION_LEDGER_STATUS=t \
  VPSMAN_SMOKE_MIGRATION_ROWS_FILE="$version_flow_migration_rows" \
  VPSMAN_SMOKE_TIMESTAMP=20260726T010101Z \
  run_updater "$VERSION_FLOW" v9.8.8 \
  >"$VERSION_FLOW/update.log" 2>&1; then
  fail "successful version-to-version update fixture failed"
fi
[[ "$(<"$VERSION_FLOW/RELEASE_TAG")" == "v9.8.8" ]] ||
  fail "successful update did not commit v9.8.8"
grep -Fq '"tag": "v9.8.8"' \
  "$VERSION_FLOW/runtime/server/current/.vpsman-release.json" ||
  fail "successful update did not activate the v9.8.8 server"
grep -Fq '"tag": "v9.8.7"' \
  "$VERSION_FLOW/runtime/server/previous/.vpsman-release.json" ||
  fail "successful update did not retain v9.8.7 for rollback"
grep -Fq 'updated vpsman deployment to v9.8.8' "$VERSION_FLOW/update.log" ||
  fail "successful update did not report its activated release"
assert_backup_mode_0600 \
  "$VERSION_FLOW/runtime/update-backups/pre-update-20260726T010101Z-v9.8.8.dump" \
  "successful update"
assert_no_active_transaction "$VERSION_FLOW" "successful update"

: >"$VERSION_FLOW/docker.log"
if ! VPSMAN_SMOKE_BUILD_TAG=v9.8.7 \
  VPSMAN_SMOKE_MIGRATION_LEDGER_STATUS=t \
  VPSMAN_SMOKE_MIGRATION_ROWS_FILE="$version_flow_migration_rows" \
  VPSMAN_SMOKE_TIMESTAMP=20260726T010102Z \
  run_updater "$VERSION_FLOW" rollback \
  >"$VERSION_FLOW/rollback.log" 2>&1; then
  fail "successful rollback fixture failed"
fi
[[ "$(<"$VERSION_FLOW/RELEASE_TAG")" == "v9.8.7" ]] ||
  fail "rollback did not commit v9.8.7"
grep -Fq '"tag": "v9.8.7"' \
  "$VERSION_FLOW/runtime/server/current/.vpsman-release.json" ||
  fail "rollback did not reactivate the v9.8.7 server"
grep -Fq '"tag": "v9.8.8"' \
  "$VERSION_FLOW/runtime/server/previous/.vpsman-release.json" ||
  fail "rollback did not retain v9.8.8 as the next rollback payload"
grep -Fq 'rollback complete' "$VERSION_FLOW/rollback.log" ||
  fail "rollback did not report completion"
assert_backup_mode_0600 \
  "$VERSION_FLOW/runtime/update-backups/pre-update-20260726T010102Z-v9.8.7.dump" \
  "successful rollback"
assert_no_active_transaction "$VERSION_FLOW" "successful rollback"

: >"$MUTATION_FAILURE/docker.log"
if RELEASE_DIR="$RELEASE_V988_DIR" \
  VPSMAN_SMOKE_BUILD_TAG=v9.8.8 \
  VPSMAN_SMOKE_FAIL_FINALIZE_MV=1 \
  VPSMAN_SMOKE_MIGRATION_LEDGER_STATUS=t \
  VPSMAN_SMOKE_MIGRATION_ROWS_FILE="$version_flow_migration_rows" \
  VPSMAN_SMOKE_TIMESTAMP=20260726T010103Z \
  run_updater "$MUTATION_FAILURE" v9.8.8 \
  >"$MUTATION_FAILURE/update.log" 2>&1; then
  fail "injected finalization mutation failure was masked"
fi
grep -Fq 'could not publish the previous server rollback payload' \
  "$MUTATION_FAILURE/update.log" ||
  fail "injected finalization mutation failure was not explicit"
[[ -f "$MUTATION_FAILURE/mv-failure-fired" ]] ||
  fail "finalization mutation failure hook did not fire"
mapfile -t mutation_transactions < <(
  find "$MUTATION_FAILURE/runtime/transactions" \
    -mindepth 1 \
    -maxdepth 1 \
    -type d \
    -name 'update.*' \
    -print
)
[[ "${#mutation_transactions[@]}" == "1" ]] ||
  fail "finalization mutation failure did not preserve one transaction journal"
[[ "$(<"${mutation_transactions[0]}/state")" == "healthy" ]] ||
  fail "finalization mutation failure did not preserve the healthy journal state"
[[ -d "${mutation_transactions[0]}/old-server" ]] ||
  fail "finalization mutation failure lost the server rollback payload"
[[ "$(<"$MUTATION_FAILURE/RELEASE_TAG")" == "v9.8.7" ]] ||
  fail "finalization mutation failure committed the new release marker"
grep -Fq '"tag": "v9.8.8"' \
  "$MUTATION_FAILURE/runtime/server/current/.vpsman-release.json" ||
  fail "finalization mutation failure damaged the healthy current payload"

VPSMAN_SMOKE_BUILD_TAG=v9.8.8 \
  run_updater "$MUTATION_FAILURE" recover \
  >"$MUTATION_FAILURE/recover.log" 2>&1
[[ "$(<"$MUTATION_FAILURE/RELEASE_TAG")" == "v9.8.8" ]] ||
  fail "recovery did not finalize the release after a mutation failure"
grep -Fq '"tag": "v9.8.7"' \
  "$MUTATION_FAILURE/runtime/server/previous/.vpsman-release.json" ||
  fail "recovery did not publish the preserved server rollback payload"
assert_no_active_transaction \
  "$MUTATION_FAILURE" \
  "finalization mutation failure recovery"

mkdir -p "$FIRST_START/runtime/transactions/update.repeat-current"
printf 'update\n' >"$FIRST_START/runtime/transactions/update.repeat-current/mode"
printf 'preparing\n' >"$FIRST_START/runtime/transactions/update.repeat-current/state"
if run_updater "$FIRST_START" v9.8.7 >"$FIRST_START/repeat-with-journal.log" 2>&1; then
  fail "same-release update bypassed an unfinished transaction"
fi
grep -Fq 'an interrupted transaction exists; run ./update.sh recover' \
  "$FIRST_START/repeat-with-journal.log" ||
  fail "same-release update did not preserve the explicit recovery path"
[[ -d "$FIRST_START/runtime/transactions/update.repeat-current" ]] ||
  fail "same-release update removed the unfinished transaction"
run_updater "$FIRST_START" recover >"$FIRST_START/repeat-recover.log" 2>&1
[[ ! -e "$FIRST_START/runtime/transactions/update.repeat-current" ]] ||
  fail "explicit recovery did not clear the unfinished same-release transaction"

for kind in server frontend cli; do
  mkdir -p "$FIRST_START/runtime/$kind/previous"
  printf 'v9.8.6 rollback sentinel\n' \
    >"$FIRST_START/runtime/$kind/previous/release-id"
done

repeat_state_before="$(release_state_digest "$FIRST_START")"

DRIFT_RELEASE="$SMOKE_ROOT/release-payload-drift"
DRIFT_SERVER_STAGE="$SMOKE_ROOT/server-stage-payload-drift"
cp -a "$RELEASE_DIR" "$DRIFT_RELEASE"
cp -a "$SERVER_STAGE" "$DRIFT_SERVER_STAGE"
printf 'same-tag payload drift\n' >"$DRIFT_SERVER_STAGE/payload-drift"
rm -f "$DRIFT_RELEASE/vpsman-server-linux-x86_64.zip"
(
  cd "$DRIFT_SERVER_STAGE"
  zip -qr "$DRIFT_RELEASE/vpsman-server-linux-x86_64.zip" .
)
: >"$FIRST_START/docker.log"
if RELEASE_DIR="$DRIFT_RELEASE" \
  run_updater "$FIRST_START" v9.8.7 \
    >"$FIRST_START/repeat-payload-drift.log" 2>&1; then
  fail "same-tag update accepted changed selected asset contents"
fi
grep -Fq 'current payload layout or contents are incomplete or corrupt' \
  "$FIRST_START/repeat-payload-drift.log" ||
  fail "same-tag payload drift refusal was not explicit"
[[ "$(release_state_digest "$FIRST_START")" == "$repeat_state_before" ]] ||
  fail "same-tag payload drift mutated current, previous, or release records"
assert_no_service_mutation "$FIRST_START" "same-tag payload drift"
assert_no_active_transaction "$FIRST_START" "same-tag payload drift"

saved_frontend_index="$SMOKE_ROOT/current-frontend-index"
mv "$FIRST_START/runtime/frontend/current/dist/index.html" "$saved_frontend_index"
missing_payload_state="$(release_state_digest "$FIRST_START")"
: >"$FIRST_START/docker.log"
if run_updater "$FIRST_START" v9.8.7 >"$FIRST_START/repeat-missing-payload.log" 2>&1; then
  fail "same-tag update claimed a missing current payload was verified"
fi
grep -Fq 'current payload layout or contents are incomplete or corrupt' \
  "$FIRST_START/repeat-missing-payload.log" ||
  fail "same-tag missing payload refusal was not explicit"
[[ "$(release_state_digest "$FIRST_START")" == "$missing_payload_state" ]] ||
  fail "same-tag missing payload check mutated the damaged deployment or rollback slot"
assert_no_service_mutation "$FIRST_START" "same-tag missing payload check"
assert_no_active_transaction "$FIRST_START" "same-tag missing payload check"
mv "$saved_frontend_index" "$FIRST_START/runtime/frontend/current/dist/index.html"
[[ "$(release_state_digest "$FIRST_START")" == "$repeat_state_before" ]] ||
  fail "current payload fixture did not restore after corruption check"

: >"$FIRST_START/docker.log"
if (
  export VPSMAN_SMOKE_STOPPED_SERVICE=worker
  run_updater "$FIRST_START" v9.8.7
) >"$FIRST_START/repeat-stopped-service.log" 2>&1; then
  fail "same-tag update claimed a stopped current service was verified"
fi
grep -Fq 'live services are stopped, unhealthy, or report a different build tag' \
  "$FIRST_START/repeat-stopped-service.log" ||
  fail "same-tag stopped-service refusal was not explicit"
[[ "$(release_state_digest "$FIRST_START")" == "$repeat_state_before" ]] ||
  fail "same-tag stopped-service check mutated current or previous payloads"
assert_no_service_mutation "$FIRST_START" "same-tag stopped-service check"
assert_no_active_transaction "$FIRST_START" "same-tag stopped-service check"

: >"$FIRST_START/docker.log"
if (
  export VPSMAN_SMOKE_HEALTH_UNREADY=1
  run_updater "$FIRST_START" v9.8.7
) >"$FIRST_START/repeat-unready-service.log" 2>&1; then
  fail "same-tag update claimed an unhealthy current service was verified"
fi
grep -Fq 'live services are stopped, unhealthy, or report a different build tag' \
  "$FIRST_START/repeat-unready-service.log" ||
  fail "same-tag unhealthy-service refusal was not explicit"
[[ "$(release_state_digest "$FIRST_START")" == "$repeat_state_before" ]] ||
  fail "same-tag unhealthy-service check mutated current or previous payloads"
assert_no_service_mutation "$FIRST_START" "same-tag unhealthy-service check"
assert_no_active_transaction "$FIRST_START" "same-tag unhealthy-service check"

: >"$FIRST_START/docker.log"
run_updater "$FIRST_START" v9.8.7 >"$FIRST_START/repeat-exact.log" 2>&1
grep -Fq 'release v9.8.7 payloads and live services are already active and verified' \
  "$FIRST_START/repeat-exact.log" ||
  fail "exact same-release update did not report its verified no-op"
[[ "$(release_state_digest "$FIRST_START")" == "$repeat_state_before" ]] ||
  fail "exact same-release no-op mutated current, previous, or release records"
grep -Fq 'http://127.0.0.1/api/v1/build-info' "$FIRST_START/docker.log" ||
  fail "exact same-release no-op did not verify the live build tag"
assert_no_service_mutation "$FIRST_START" "exact same-release no-op"
assert_no_active_transaction "$FIRST_START" "exact same-release no-op"

: >"$FIRST_START/docker.log"
run_updater "$FIRST_START" latest >"$FIRST_START/repeat-latest.log" 2>&1
grep -Fq 'release v9.8.7 payloads and live services are already active and verified' \
  "$FIRST_START/repeat-latest.log" ||
  fail "latest resolving to the current release did not report its verified no-op"
[[ "$(release_state_digest "$FIRST_START")" == "$repeat_state_before" ]] ||
  fail "latest same-release no-op mutated current, previous, or release records"
grep -Fq 'http://127.0.0.1/api/v1/build-info' "$FIRST_START/docker.log" ||
  fail "latest same-release no-op did not verify the live build tag"
assert_no_service_mutation "$FIRST_START" "latest same-release no-op"
assert_no_active_transaction "$FIRST_START" "latest same-release no-op"
for kind in server frontend cli; do
  [[ "$(<"$FIRST_START/runtime/$kind/previous/release-id")" == \
    "v9.8.6 rollback sentinel" ]] ||
    fail "same-release no-op replaced the previous $kind rollback payload"
done

FAILED_FIRST_START="$SMOKE_ROOT/failed-first-start"
prepare_deployment "$FAILED_FIRST_START"
migration_rows="$FAILED_FIRST_START/migration-rows.txt"
printf '1 %s\n' \
  "$(sha384sum "$SERVER_STAGE/migrations/0001_smoke.sql" | awk '{print $1}')" \
  >"$migration_rows"
if (
  export VPSMAN_SMOKE_FAIL_APPLICATION_UP=1
  export VPSMAN_SMOKE_MIGRATION_LEDGER_STATUS=t
  export VPSMAN_SMOKE_MIGRATION_ROWS_FILE="$migration_rows"
  run_updater "$FAILED_FIRST_START" first-start v9.8.7
) >"$FAILED_FIRST_START/update.log" 2>&1; then
  fail "failed first-start unexpectedly passed readiness"
fi
grep -Fq 'database restore' "$FAILED_FIRST_START/docker.log" ||
  fail "failed first-start did not restore its pre-activation database backup"
for kind in server frontend cli; do
  [[ ! -e "$FAILED_FIRST_START/runtime/$kind/current" ]] ||
    fail "failed first-start left the $kind payload active"
done
[[ -z "$(find "$FAILED_FIRST_START/runtime/transactions" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
  fail "failed first-start left an active transaction after database recovery"

PG_DUMP_FAILURE="$SMOKE_ROOT/pg-dump-failure"
prepare_deployment "$PG_DUMP_FAILURE"
if VPSMAN_SMOKE_FAIL_PG_DUMP=1 \
  VPSMAN_SMOKE_TIMESTAMP=20260726T010104Z \
  run_updater "$PG_DUMP_FAILURE" first-start v9.8.7 \
  >"$PG_DUMP_FAILURE/update.log" 2>&1; then
  fail "failed pg_dump unexpectedly published a database backup"
fi
grep -Fq 'PostgreSQL pre-activation backup failed' "$PG_DUMP_FAILURE/update.log" ||
  fail "pg_dump failure was not explicit"
[[ -z "$(
  find "$PG_DUMP_FAILURE/runtime/update-backups" \
    -mindepth 1 \
    -maxdepth 1 \
    -print -quit
)" ]] || fail "pg_dump failure left a final backup or partial"
mapfile -t pg_dump_transactions < <(
  find "$PG_DUMP_FAILURE/runtime/transactions" \
    -mindepth 1 \
    -maxdepth 1 \
    -type d \
    -name 'update.*' \
    -print
)
[[ "${#pg_dump_transactions[@]}" == "1" &&
  "$(<"${pg_dump_transactions[0]}/state")" == "stopping" ]] ||
  fail "pg_dump failure did not preserve its pre-activation transaction"
run_updater "$PG_DUMP_FAILURE" recover >"$PG_DUMP_FAILURE/recover.log" 2>&1
assert_no_active_transaction "$PG_DUMP_FAILURE" "pg_dump failure recovery"

PG_VALIDATE_FAILURE="$SMOKE_ROOT/pg-validate-failure"
prepare_deployment "$PG_VALIDATE_FAILURE"
if VPSMAN_SMOKE_FAIL_PG_RESTORE_LIST=1 \
  VPSMAN_SMOKE_TIMESTAMP=20260726T010105Z \
  run_updater "$PG_VALIDATE_FAILURE" first-start v9.8.7 \
  >"$PG_VALIDATE_FAILURE/update.log" 2>&1; then
  fail "invalid pg_dump archive unexpectedly published a database backup"
fi
grep -Fq 'failed pg_restore validation' "$PG_VALIDATE_FAILURE/update.log" ||
  fail "database backup validation failure was not explicit"
[[ -z "$(
  find "$PG_VALIDATE_FAILURE/runtime/update-backups" \
    -mindepth 1 \
    -maxdepth 1 \
    -print -quit
)" ]] || fail "database backup validation failure left a final backup or partial"
run_updater "$PG_VALIDATE_FAILURE" recover >"$PG_VALIDATE_FAILURE/recover.log" 2>&1
assert_no_active_transaction "$PG_VALIDATE_FAILURE" "backup validation failure recovery"

BACKUP_COLLISION="$SMOKE_ROOT/backup-collision"
prepare_deployment "$BACKUP_COLLISION"
mkdir -p "$BACKUP_COLLISION/runtime/update-backups"
collision_backup="$BACKUP_COLLISION/runtime/update-backups/pre-update-20260726T010106Z-v9.8.7.dump"
printf 'existing backup must not be overwritten\n' >"$collision_backup"
chmod 0600 "$collision_backup"
collision_hash_before="$(sha256sum "$collision_backup" | awk '{print $1}')"
if VPSMAN_SMOKE_TIMESTAMP=20260726T010106Z \
  run_updater "$BACKUP_COLLISION" first-start v9.8.7 \
  >"$BACKUP_COLLISION/update.log" 2>&1; then
  fail "backup publication overwrote an existing destination"
fi
grep -Fq 'refusing to overwrite existing PostgreSQL backup' \
  "$BACKUP_COLLISION/update.log" ||
  fail "database backup collision refusal was not explicit"
[[ "$(sha256sum "$collision_backup" | awk '{print $1}')" == \
  "$collision_hash_before" ]] ||
  fail "database backup collision mutated the existing backup"
[[ -z "$(
  find "$BACKUP_COLLISION/runtime/update-backups" \
    -mindepth 1 \
    -maxdepth 1 \
    -name '.*.partial.*' \
    -print -quit
)" ]] || fail "database backup collision left a partial"
run_updater "$BACKUP_COLLISION" recover >"$BACKUP_COLLISION/recover.log" 2>&1
assert_no_active_transaction "$BACKUP_COLLISION" "backup collision recovery"

ABANDONED_BACKUP="$SMOKE_ROOT/abandoned-backup-partial"
prepare_deployment "$ABANDONED_BACKUP"
mkdir -p "$ABANDONED_BACKUP/runtime/update-backups"
abandoned_backup_partial="$ABANDONED_BACKUP/runtime/update-backups/.pre-update-20260726T010107Z-v9.8.7.dump.partial.Ab12Cd"
printf 'crash-left backup partial\n' >"$abandoned_backup_partial"
chmod 0600 "$abandoned_backup_partial"
run_updater "$ABANDONED_BACKUP" recover >"$ABANDONED_BACKUP/recover.log" 2>&1
[[ ! -e "$abandoned_backup_partial" ]] ||
  fail "recovery did not remove a recognized abandoned backup partial"
grep -Fq 'removed 1 abandoned PostgreSQL backup partial' \
  "$ABANDONED_BACKUP/recover.log" ||
  fail "abandoned backup-partial cleanup was not reported"

SUSPICIOUS_BACKUP="$SMOKE_ROOT/suspicious-backup-partial"
prepare_deployment "$SUSPICIOUS_BACKUP"
mkdir -p "$SUSPICIOUS_BACKUP/runtime/update-backups"
printf 'must remain\n' >"$SUSPICIOUS_BACKUP/partial-target"
suspicious_backup_partial="$SUSPICIOUS_BACKUP/runtime/update-backups/.pre-update-20260726T010108Z-v9.8.7.dump.partial.Ab12Cd"
ln -s "$SUSPICIOUS_BACKUP/partial-target" "$suspicious_backup_partial"
if run_updater "$SUSPICIOUS_BACKUP" recover \
  >"$SUSPICIOUS_BACKUP/recover.log" 2>&1; then
  fail "recovery removed an unrecognized backup partial symlink"
fi
grep -Fq 'unrecognized PostgreSQL backup partial' \
  "$SUSPICIOUS_BACKUP/recover.log" ||
  fail "suspicious backup partial was not identified explicitly"
[[ -L "$suspicious_backup_partial" &&
  "$(<"$SUSPICIOUS_BACKUP/partial-target")" == "must remain" ]] ||
  fail "suspicious backup partial handling mutated the symlink or its target"

MISSING_BACKUP_RECOVERY="$SMOKE_ROOT/missing-backup-recovery"
prepare_deployment "$MISSING_BACKUP_RECOVERY"
mkdir -p \
  "$MISSING_BACKUP_RECOVERY/runtime/server/current" \
  "$MISSING_BACKUP_RECOVERY/runtime/frontend/current" \
  "$MISSING_BACKUP_RECOVERY/runtime/cli/current" \
  "$MISSING_BACKUP_RECOVERY/runtime/transactions/update.missing-backup/old-server"
printf 'candidate-server\n' \
  >"$MISSING_BACKUP_RECOVERY/runtime/server/current/payload"
printf 'old-frontend\n' \
  >"$MISSING_BACKUP_RECOVERY/runtime/frontend/current/payload"
printf 'old-cli\n' >"$MISSING_BACKUP_RECOVERY/runtime/cli/current/payload"
printf 'old-server\n' \
  >"$MISSING_BACKUP_RECOVERY/runtime/transactions/update.missing-backup/old-server/payload"
printf 'update\n' \
  >"$MISSING_BACKUP_RECOVERY/runtime/transactions/update.missing-backup/mode"
printf 'activating\n' \
  >"$MISSING_BACKUP_RECOVERY/runtime/transactions/update.missing-backup/state"
: >"$MISSING_BACKUP_RECOVERY/runtime/transactions/update.missing-backup/had-current-server"
: >"$MISSING_BACKUP_RECOVERY/docker.log"
if run_updater "$MISSING_BACKUP_RECOVERY" recover \
  >"$MISSING_BACKUP_RECOVERY/recover.log" 2>&1; then
  fail "activation recovery continued without database-backup metadata"
fi
grep -Fq 'activated transaction has no database-backup metadata' \
  "$MISSING_BACKUP_RECOVERY/recover.log" ||
  fail "missing recovery backup metadata was not explicit"
[[ "$(<"$MISSING_BACKUP_RECOVERY/runtime/server/current/payload")" == \
  "candidate-server" &&
  "$(<"$MISSING_BACKUP_RECOVERY/runtime/transactions/update.missing-backup/old-server/payload")" == \
  "old-server" ]] ||
  fail "missing recovery backup metadata mutated payloads"
[[ -d "$MISSING_BACKUP_RECOVERY/runtime/transactions/update.missing-backup" ]] ||
  fail "missing recovery backup metadata removed the transaction journal"
if grep -Eq '^(stop|database restore)( |$)' \
  "$MISSING_BACKUP_RECOVERY/docker.log"; then
  fail "missing recovery backup metadata stopped services or restored the database"
fi

RECOVERY="$SMOKE_ROOT/recovery"
prepare_deployment "$RECOVERY"
mkdir -p \
  "$RECOVERY/runtime/server/current" \
  "$RECOVERY/runtime/frontend/current" \
  "$RECOVERY/runtime/cli/current" \
  "$RECOVERY/runtime/update-backups" \
  "$RECOVERY/runtime/transactions/update.interrupted/old-server"
printf 'new-server\n' >"$RECOVERY/runtime/server/current/payload"
printf 'old-frontend\n' >"$RECOVERY/runtime/frontend/current/payload"
printf 'old-cli\n' >"$RECOVERY/runtime/cli/current/payload"
printf 'old-server\n' >"$RECOVERY/runtime/transactions/update.interrupted/old-server/payload"
printf 'update\n' >"$RECOVERY/runtime/transactions/update.interrupted/mode"
printf 'activating\n' >"$RECOVERY/runtime/transactions/update.interrupted/state"
printf 'pre-update-20260726T010109Z-v9.8.7.dump\n' \
  >"$RECOVERY/runtime/transactions/update.interrupted/backup"
: >"$RECOVERY/runtime/transactions/update.interrupted/had-current-server"
printf '%s\n' \
  'vpsman updater smoke database dump partial' \
  'vpsman updater smoke database dump complete' \
  >"$RECOVERY/runtime/update-backups/pre-update-20260726T010109Z-v9.8.7.dump"
chmod 0600 \
  "$RECOVERY/runtime/update-backups/pre-update-20260726T010109Z-v9.8.7.dump"

STOP_FAILURE_RECOVERY="$SMOKE_ROOT/stop-failure-recovery"
cp -a "$RECOVERY" "$STOP_FAILURE_RECOVERY"
: >"$STOP_FAILURE_RECOVERY/docker.log"
if VPSMAN_SMOKE_FAIL_STOP=1 \
  run_updater "$STOP_FAILURE_RECOVERY" recover \
  >"$STOP_FAILURE_RECOVERY/recover.log" 2>&1; then
  fail "activation recovery continued after application stop failed"
fi
grep -Fq 'database and payload recovery were not attempted' \
  "$STOP_FAILURE_RECOVERY/recover.log" ||
  fail "application-stop recovery refusal was not explicit"
[[ "$(<"$STOP_FAILURE_RECOVERY/runtime/server/current/payload")" == \
  "new-server" &&
  "$(<"$STOP_FAILURE_RECOVERY/runtime/transactions/update.interrupted/old-server/payload")" == \
  "old-server" ]] ||
  fail "failed application stop mutated recovery payloads"
[[ -d "$STOP_FAILURE_RECOVERY/runtime/transactions/update.interrupted" ]] ||
  fail "failed application stop removed the recovery journal"
if grep -Fq 'database restore' "$STOP_FAILURE_RECOVERY/docker.log"; then
  fail "failed application stop still restored the database"
fi

run_updater "$RECOVERY" recover >"$RECOVERY/recover.log" 2>&1
[[ "$(<"$RECOVERY/runtime/server/current/payload")" == "old-server" ]] ||
  fail "recovery did not restore the already-touched server payload"
[[ "$(<"$RECOVERY/runtime/frontend/current/payload")" == "old-frontend" ]] ||
  fail "recovery removed the untouched frontend payload"
[[ "$(<"$RECOVERY/runtime/cli/current/payload")" == "old-cli" ]] ||
  fail "recovery removed the untouched CLI payload"
[[ ! -e "$RECOVERY/runtime/transactions/update.interrupted" ]] ||
  fail "recovery left the completed transaction"
grep -Fq 'up -d --force-recreate --remove-orphans api gateway worker frontend' \
  "$RECOVERY/docker.log" ||
  fail "recovery did not restart the existing deployment"

HEALTHY_RECOVERY="$SMOKE_ROOT/healthy-recovery"
prepare_deployment "$HEALTHY_RECOVERY"
mkdir -p \
  "$HEALTHY_RECOVERY/runtime/server/current" \
  "$HEALTHY_RECOVERY/runtime/frontend/current" \
  "$HEALTHY_RECOVERY/runtime/cli/current" \
  "$HEALTHY_RECOVERY/runtime/transactions/update.healthy/old-server" \
  "$HEALTHY_RECOVERY/runtime/transactions/update.healthy/old-frontend" \
  "$HEALTHY_RECOVERY/runtime/transactions/update.healthy/old-cli"
printf 'update\n' >"$HEALTHY_RECOVERY/runtime/transactions/update.healthy/mode"
printf 'healthy\n' >"$HEALTHY_RECOVERY/runtime/transactions/update.healthy/state"
printf 'v9.8.7\n' >"$HEALTHY_RECOVERY/runtime/transactions/update.healthy/tag"
run_updater "$HEALTHY_RECOVERY" recover >"$HEALTHY_RECOVERY/recover.log" 2>&1
[[ "$(<"$HEALTHY_RECOVERY/RELEASE_TAG")" == "v9.8.7" ]] ||
  fail "healthy transaction recovery did not commit the active release marker"
for kind in server frontend cli; do
  [[ -d "$HEALTHY_RECOVERY/runtime/$kind/previous" ]] ||
    fail "healthy transaction recovery did not finalize previous $kind"
done
[[ ! -e "$HEALTHY_RECOVERY/runtime/transactions/update.healthy" ]] ||
  fail "healthy transaction recovery left its journal visible"

FINALIZED_RECOVERY="$SMOKE_ROOT/finalized-recovery"
prepare_deployment "$FINALIZED_RECOVERY"
mkdir -p "$FINALIZED_RECOVERY/runtime/transactions/update.finalized"
printf 'update\n' >"$FINALIZED_RECOVERY/runtime/transactions/update.finalized/mode"
printf 'finalized\n' >"$FINALIZED_RECOVERY/runtime/transactions/update.finalized/state"
run_updater "$FINALIZED_RECOVERY" recover >"$FINALIZED_RECOVERY/recover.log" 2>&1
[[ ! -e "$FINALIZED_RECOVERY/runtime/transactions/update.finalized" ]] ||
  fail "finalized transaction recovery left its journal visible"

ABANDONED="$SMOKE_ROOT/abandoned-transaction-staging"
prepare_deployment "$ABANDONED"
mkdir -p \
  "$ABANDONED/runtime/transactions/.initializing.crashed" \
  "$ABANDONED/runtime/transactions/.cleanup.update.crashed"
run_updater "$ABANDONED" recover >"$ABANDONED/recover.log" 2>&1
[[ -z "$(find "$ABANDONED/runtime/transactions" -mindepth 1 -maxdepth 1 -print -quit)" ]] ||
  fail "recovery did not remove abandoned hidden transaction directories"
grep -Fq 'removed 2 abandoned transaction staging directories' "$ABANDONED/recover.log" ||
  fail "abandoned transaction cleanup was not reported"

PLACEHOLDER="$SMOKE_ROOT/placeholder"
prepare_deployment "$PLACEHOLDER" replacewithrandomhexpostgrespassword
if run_updater "$PLACEHOLDER" recover >"$PLACEHOLDER/update.log" 2>&1; then
  fail "updater accepted the PostgreSQL password placeholder"
fi
grep -Fq 'replace the POSTGRES_PASSWORD template placeholder' "$PLACEHOLDER/update.log" ||
  fail "placeholder refusal was not actionable"

INVALID_TAG="$SMOKE_ROOT/invalid-tag"
prepare_deployment "$INVALID_TAG"
if run_updater "$INVALID_TAG" first-start v1.2.3-01 >"$INVALID_TAG/update.log" 2>&1; then
  fail "updater accepted a non-canonical semantic version"
fi
grep -Fq 'release target must be latest' "$INVALID_TAG/update.log" ||
  fail "invalid release tag refusal was not actionable"

INVALID_CORE_TAG="$SMOKE_ROOT/invalid-core-tag"
prepare_deployment "$INVALID_CORE_TAG"
if run_updater "$INVALID_CORE_TAG" first-start v01.2.3 >"$INVALID_CORE_TAG/update.log" 2>&1; then
  fail "updater accepted a zero-padded semantic-version core identifier"
fi
grep -Fq 'release target must be latest' "$INVALID_CORE_TAG/update.log" ||
  fail "zero-padded release tag refusal was not actionable"

NONCANONICAL_HEALTH_TIMEOUT="$SMOKE_ROOT/noncanonical-health-timeout"
prepare_deployment "$NONCANONICAL_HEALTH_TIMEOUT"
if VPSMAN_UPDATE_HEALTH_TIMEOUT_SECS=0180 \
  run_updater "$NONCANONICAL_HEALTH_TIMEOUT" first-start v9.8.7 \
  >"$NONCANONICAL_HEALTH_TIMEOUT/update.log" 2>&1; then
  fail "updater accepted a leading-zero health timeout"
fi
grep -Fq 'must be a canonical integer between 10 and 3600' \
  "$NONCANONICAL_HEALTH_TIMEOUT/update.log" ||
  fail "leading-zero health timeout refusal was not actionable"
[[ ! -e "$NONCANONICAL_HEALTH_TIMEOUT/runtime" ]] ||
  fail "invalid health timeout mutated deployment runtime"

UNSAFE_ASSET="$SMOKE_ROOT/unsafe-asset"
prepare_deployment "$UNSAFE_ASSET"
if (
  export VPSMAN_SERVER_ASSET='../outside.zip'
  run_updater "$UNSAFE_ASSET" first-start v9.8.7
) >"$UNSAFE_ASSET/update.log" 2>&1; then
  fail "updater accepted an asset path"
fi
grep -Fq 'release asset names must not contain paths' "$UNSAFE_ASSET/update.log" ||
  fail "unsafe asset refusal was not actionable"

MALICIOUS_RELEASE="$SMOKE_ROOT/malicious-release"
cp -a "$RELEASE_DIR" "$MALICIOUS_RELEASE"
python3 - "$MALICIOUS_RELEASE/vpsman-server-linux-x86_64.zip" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1], "w") as archive:
    archive.writestr("../escaped", "must not escape")
PY
MALICIOUS="$SMOKE_ROOT/malicious"
prepare_deployment "$MALICIOUS"
if (
  export RELEASE_DIR="$MALICIOUS_RELEASE"
  run_updater "$MALICIOUS" first-start v9.8.7
) >"$MALICIOUS/update.log" 2>&1; then
  fail "updater extracted a path-traversal archive"
fi
grep -Fq 'unsafe server archive path' "$MALICIOUS/update.log" ||
  fail "path-traversal archive refusal was not actionable"
[[ ! -e "$SMOKE_ROOT/escaped" ]] ||
  fail "path-traversal archive escaped its transaction directory"

printf '%s\n' \
  '{"deploy_updater_smoke":"ok","checks":["first_start","first_start_privilege_salt_output","version_manifest_asset_selection","atomic_validated_backup","successful_version_update","successful_rollback","finalization_mutation_failure_guard","finalization_mutation_failure_recovery","same_release_recovery_guard","same_tag_payload_drift_guard","same_tag_missing_payload_guard","same_tag_stopped_service_guard","same_tag_unready_service_guard","exact_same_release_noop","latest_same_release_noop","failed_first_start_database_restore","pg_dump_partial_cleanup","backup_validation_failure_cleanup","backup_collision_refusal","abandoned_backup_partial_cleanup","suspicious_backup_partial_refusal","missing_backup_recovery_guard","recovery_stop_failure_guard","interrupted_activation_recovery","healthy_finalize_recovery","finalized_journal_cleanup","abandoned_staging_cleanup","placeholder_password_refusal","invalid_tag_refusal","zero_padded_core_tag_refusal","noncanonical_health_timeout_refusal","unsafe_asset_refusal","archive_path_traversal_refusal"]}'
