#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AUDIT_SCRIPT="$ROOT_DIR/scripts/audit-postgres-traffic-ledger.sh"
AUDIT_SQL="$ROOT_DIR/scripts/sql/audit-postgres-traffic-ledger.sql"
MIGRATION_0016="$ROOT_DIR/migrations/0016_streaming_traffic_hourly_refresh.sql"
MIGRATION_0017="$ROOT_DIR/migrations/0017_agent_suspension.sql"
MIGRATION_0018="$ROOT_DIR/migrations/0018_traffic_counter_import_class_stream_index.sql"
MIGRATION_0019="$ROOT_DIR/migrations/0019_traffic_import_same_shape_update.sql"
MIGRATION_0020="$ROOT_DIR/migrations/0020_retire_unused_traffic_cycle_usage.sql"
SMOKE_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/vpsman-traffic-ledger-audit-smoke.XXXXXX")"
CONTAINER_NAME="vpsman-traffic-ledger-audit-$$_${RANDOM}"
POSTGRES_USER="vpsman_audit_smoke"
POSTGRES_PASSWORD="vpsman_audit_smoke_password"
POSTGRES_DB="vpsman_audit_smoke"
AUDIT_USER="traffic_auditor"
AUDIT_PASSWORD="traffic_auditor_password"
ACTIVITY_USER="traffic_activity_probe"
MIGRATION_0016_SHA384="6b5644e07b7ac9bb56a0df90755d9f2b8a25598ec6846d02d798060941703c073435a593051739b0d87361af45147b1d"
MIGRATION_0016_FUNCTION_SHA256="d88de80aa8c8788af1d44007201b9a618927a204fb1edd688231cabcc95fbbc9"
MIGRATION_0017_SHA384="b1f367301f968e59b01ae4d16161753820a867e07bde1eb992bf3a9d2fb495ebef3bd4ccc55489e51430368eb7516145"
MIGRATION_0018_SHA384="f450567e725e9bc60456a4b5c2dab87de13ca4021f98de7fe214cb8907298f46564cebaa8c60b3d85e86f8830dd8bfe8"
MIGRATION_0019_SHA384="aa39b2f44989f2e6337d4eea2b98065a41dc150d676655d46d1d767992d3df0c3e3ff2d7e50b589481a993fe6e691ac8"
MIGRATION_0019_FUNCTION_SHA256="f739a35af6a49d85770b9c06de11e12c2fbe2d68320c86ca9e6d953ffe6fcab5"
MIGRATION_0020_SHA384="89d9b86df9fb4c8f5004a7688f22e50a20a09843b456c35742f44b403e377e73bd7cdb561b1388c25276ec97cb201f75"
activity_probe_pid=""
index_snapshot_pid=""
index_build_pid=""
exec 3>&2

cleanup() {
  local status="$?"
  if [[ "$CONTAINER_NAME" == vpsman-traffic-ledger-audit-* ]]; then
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
  else
    printf 'refusing to remove unexpected smoke container: %s\n' "$CONTAINER_NAME" >&3
  fi
  local managed_pid
  for managed_pid in "$activity_probe_pid" "$index_snapshot_pid" "$index_build_pid"; do
    if [[ "$managed_pid" =~ ^[0-9]+$ ]]; then
      kill "$managed_pid" >/dev/null 2>&1 || true
      wait "$managed_pid" >/dev/null 2>&1 || true
    fi
  done
  if [[ "$status" != "0" && "${VPSMAN_SMOKE_KEEP_ON_FAILURE:-0}" == "1" ]]; then
    printf 'preserved failed traffic-ledger smoke workspace: %s\n' "$SMOKE_ROOT" >&3
    return
  fi
  case "$SMOKE_ROOT" in
    "${TMPDIR:-/tmp}"/vpsman-traffic-ledger-audit-smoke.*)
      rm -rf -- "$SMOKE_ROOT"
      ;;
    *)
      printf 'refusing to clean unexpected smoke path: %s\n' "$SMOKE_ROOT" >&3
      ;;
  esac
}
trap cleanup EXIT

report_error() {
  local status="$?"
  printf 'traffic-ledger audit smoke stopped unexpectedly (status %s)\n' "$status" >&3
  if [[ -d "$SMOKE_ROOT" ]]; then
    while IFS= read -r log; do
      printf '%s\n' "--- ${log#"$SMOKE_ROOT"/} ---" >&3
      tail -n 100 "$log" >&3
    done < <(find "$SMOKE_ROOT" -type f -name '*.log' -print | sort)
  fi
  exit "$status"
}
trap report_error ERR

fail() {
  printf 'traffic-ledger audit smoke failed: %s\n' "$*" >&2
  exit 1
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || fail "missing required tool: $1"
}

for tool in awk bash cat docker find grep head mktemp psql rm sed seq sha256sum sha384sum sleep sort tail; do
  require_tool "$tool"
done
[[ -x "$AUDIT_SCRIPT" ]] || fail "audit script is not executable"
[[ -r "$AUDIT_SQL" ]] || fail "audit SQL is missing"
[[ -r "$MIGRATION_0016" ]] || fail "migration 0016 is missing"
[[ -r "$MIGRATION_0017" ]] || fail "migration 0017 is missing"
[[ -r "$MIGRATION_0018" ]] || fail "migration 0018 is missing"
[[ -r "$MIGRATION_0019" ]] || fail "migration 0019 is missing"
[[ -r "$MIGRATION_0020" ]] || fail "migration 0020 is missing"

[[ "$(sha384sum "$MIGRATION_0016" | awk '{print $1}')" == \
    "$MIGRATION_0016_SHA384" ]] ||
  fail "migration 0016 no longer matches its frozen SHA-384"
migration_0016_function_sha256="$(awk '
  /^AS \$\$$/ && ! body { body = 1; printf "\n"; next }
  /^\$\$;$/ && body { exit }
  body { printf "%s\n", $0 }
' "$MIGRATION_0016" | sha256sum | awk '{print $1}')"
[[ "$migration_0016_function_sha256" == \
    "$MIGRATION_0016_FUNCTION_SHA256" ]] ||
  fail "migration 0016 function no longer matches its frozen source SHA-256"
[[ "$(sha384sum "$MIGRATION_0017" | awk '{print $1}')" == \
    "$MIGRATION_0017_SHA384" ]] ||
  fail "migration 0017 no longer matches its frozen SHA-384"
[[ "$(sha384sum "$MIGRATION_0018" | awk '{print $1}')" == \
    "$MIGRATION_0018_SHA384" ]] ||
  fail "migration 0018 no longer matches its frozen SHA-384"
[[ "$(sha384sum "$MIGRATION_0019" | awk '{print $1}')" == \
    "$MIGRATION_0019_SHA384" ]] ||
  fail "migration 0019 no longer matches its frozen SHA-384"
[[ "$(sha384sum "$MIGRATION_0020" | awk '{print $1}')" == \
    "$MIGRATION_0020_SHA384" ]] ||
  fail "migration 0020 no longer matches its frozen SHA-384"
migration_0019_function_sha256="$(awk '
  /^AS \$\$$/ && ! body { body = 1; printf "\n"; next }
  /^\$\$;$/ && body { exit }
  body { printf "%s\n", $0 }
' "$MIGRATION_0019" | sha256sum | awk '{print $1}')"
[[ "$migration_0019_function_sha256" == \
    "$MIGRATION_0019_FUNCTION_SHA256" ]] ||
  fail "migration 0019 function no longer matches its frozen source SHA-256"

if awk '
  BEGIN { IGNORECASE = 1 }
  /^[[:space:]]*(insert|update|delete|merge|truncate|create|alter|drop|grant|revoke|copy)[[:space:]]/ {
    found = 1
  }
  END { exit(found ? 0 : 1) }
' "$AUDIT_SQL"; then
  fail "audit SQL contains a data-changing statement"
fi
if grep -Eq '^[[:space:]]*\\(!|copy)[[:space:]]' "$AUDIT_SQL"; then
  fail "audit SQL contains a shell or client-side copy command"
fi
if grep -Eq 'sequenced[[:space:]]+AS[[:space:]]+MATERIALIZED|SELECT[[:space:]]+sample\.\*' \
    "$AUDIT_SQL"; then
  fail "audit SQL materializes a full-row raw sequencing pass"
fi
grep -Eq '^[[:space:]]*SET LOCAL max_parallel_workers_per_gather TO 0;' \
  "$AUDIT_SQL" || fail "deep audit no longer disables parallel gather"
grep -Eq "^[[:space:]]*SET LOCAL temp_file_limit TO '256MB';" \
  "$AUDIT_SQL" || fail "deep audit no longer caps temporary files at 256 MiB"

# Exercise direct-URL plumbing without opening a connection. The exported
# function receives the exact argv/environment that the wrapper would give
# psql, but emits one valid synthetic audit row after checking that credentials
# were split into libpq environment fields and removed from argv/the child URL.
# shellcheck disable=SC2317
psql() {
  [[ "${PGHOST:-}" == "127.0.0.1" ]] || return 91
  [[ "${PGPORT:-}" == "5432" ]] || return 92
  [[ "${PGUSER:-}" == "probe-user" ]] || return 93
  [[ "${PGPASSWORD:-}" == "probe:password" ]] || return 94
  [[ "${PGDATABASE:-}" == "probe/db" ]] || return 95
  [[ "${PGSSLMODE:-}" == "disable" ]] || return 96
  [[ "${PGTARGETSESSIONATTRS:-}" == "read-write" ]] || return 97
  [[ "${PGOPTIONS:-}" == "-c default_transaction_read_only=on" ]] || return 98
  [[ -z "${VPSMAN_POSTGRES_URL+x}" ]] || return 99
  local argument
  for argument in "$@"; do
    case "$argument" in
      *postgresql://* | *probe%3Apassword* | *probe:password* | \
        *"$probe_identity_salt"*) return 100 ;;
    esac
  done
  local first_line
  IFS= read -r first_line || return 101
  [[ "$first_line" == "\\set audit_identity_salt $probe_identity_salt" ]] || return 102
  while IFS= read -r _sql_line; do :; done
  printf 'INFO\tconnection_plumbing_probe\t1\t{"credentials":"environment_only"}\n'
}
export -f psql
probe_identity_salt="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
export probe_identity_salt
connection_probe_url='postgresql://probe-user:probe%3Apassword@127.0.0.1:5432/probe%2Fdb?sslmode=disable&target_session_attrs=read-write'
if VPSMAN_AUDIT_IDENTITY_SALT="$probe_identity_salt" \
  VPSMAN_POSTGRES_URL="$connection_probe_url" \
  "$AUDIT_SCRIPT" --mode quick \
  >"$SMOKE_ROOT/connection-probe.tsv" \
  2>"$SMOKE_ROOT/connection-probe.log"; then
  connection_probe_status=0
else
  connection_probe_status="$?"
fi
unset -f psql
unset probe_identity_salt
[[ "$connection_probe_status" -eq 0 ]] ||
  fail "direct connection plumbing probe exited $connection_probe_status"
grep -Eq '^[A-Z]+[[:space:]]+audit_summary[[:space:]]+0[[:space:]]' \
  "$SMOKE_ROOT/connection-probe.tsv" ||
  fail "direct connection plumbing probe did not emit a clean summary"
if grep -R -E 'postgresql://|probe(%3A|:)password' \
  "$SMOKE_ROOT/connection-probe.tsv" "$SMOKE_ROOT/connection-probe.log"; then
  fail "direct connection plumbing probe retained URL or password text"
fi

docker run -d --rm \
  --name "$CONTAINER_NAME" \
  -e "POSTGRES_USER=$POSTGRES_USER" \
  -e "POSTGRES_PASSWORD=$POSTGRES_PASSWORD" \
  -e "POSTGRES_DB=$POSTGRES_DB" \
  -p 127.0.0.1::5432 \
  postgres:16-alpine >"$SMOKE_ROOT/container-id.log"

ready=0
for _attempt in $(seq 1 60); do
  if docker exec "$CONTAINER_NAME" \
    pg_isready --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 1
done
[[ "$ready" -eq 1 ]] || fail "PostgreSQL did not become ready"

psql_super() {
  docker exec -i "$CONTAINER_NAME" \
    psql -X -q -v ON_ERROR_STOP=1 \
      --username="$POSTGRES_USER" --dbname="$POSTGRES_DB"
}

psql_super >/dev/null <<'SQL'
CREATE TABLE public._sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),
    success BOOLEAN NOT NULL,
    checksum BYTEA NOT NULL,
    execution_time BIGINT NOT NULL
);
SQL

while IFS= read -r migration; do
  filename="${migration##*/}"
  version="${filename%%_*}"
  version="$((10#$version))"
  description="${filename#*_}"
  description="${description%.sql}"
  description="${description//_/ }"
  checksum="$(sha384sum "$migration" | awk '{print $1}')"
  if [[ "$version" -eq 18 ]]; then
    psql_super <"$migration" >"$SMOKE_ROOT/migration-${version}.log" 2>&1
    psql_super >>"$SMOKE_ROOT/migration-${version}.log" 2>&1 <<SQL
INSERT INTO public._sqlx_migrations (
    version, description, success, checksum, execution_time
) VALUES (
    $version, '$description', true, decode('$checksum', 'hex'), 0
);
SQL
  else
    {
      printf 'BEGIN;\n'
      cat "$migration"
      printf '\nINSERT INTO public._sqlx_migrations '
      printf '(version, description, success, checksum, execution_time) '
      printf "VALUES (%d, '%s', true, decode('%s', 'hex'), 0);\n" \
        "$version" "$description" "$checksum"
      printf 'COMMIT;\n'
    } | psql_super >"$SMOKE_ROOT/migration-${version}.log" 2>&1
  fi
done < <(find "$ROOT_DIR/migrations" -maxdepth 1 -type f -name '*.sql' -print | sort)

psql_super >/dev/null <<SQL
CREATE ROLE $AUDIT_USER LOGIN PASSWORD '$AUDIT_PASSWORD';
CREATE ROLE $ACTIVITY_USER LOGIN;
ALTER ROLE $AUDIT_USER SET default_transaction_read_only = on;
GRANT CONNECT ON DATABASE $POSTGRES_DB TO $AUDIT_USER;
GRANT USAGE ON SCHEMA public TO $AUDIT_USER;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO $AUDIT_USER;
GRANT pg_read_all_stats TO $AUDIT_USER;
GRANT SET ON PARAMETER temp_file_limit TO $AUDIT_USER;
SQL

psql_super >/dev/null <<'SQL'
INSERT INTO public.clients (
    id, display_name, public_key, status, internal_build_number
) VALUES
    ('audit-clean-client', 'Audit clean client', decode('01', 'hex'), 'offline', 1),
    ('audit-overlap-client', 'Audit overlap client', decode('02', 'hex'), 'offline', 1);

WITH clock AS (
    SELECT date_trunc('hour', current_timestamp) - interval '2 hours' AS baseline
), job_data AS (
    SELECT
        '11111111-1111-4111-8111-111111111111'::uuid AS job_id,
        baseline,
        extract(epoch FROM baseline + interval '1 minute')::bigint AS start_unix,
        extract(epoch FROM baseline + interval '2 minutes')::bigint AS live_unix
    FROM clock
)
INSERT INTO public.jobs (
    id, command_type, privileged, status, target_count, payload_hash,
    operation, request_fingerprint, max_timeout_secs, created_at, completed_at
)
SELECT
    job_id,
    'network_traffic_import_vnstat',
    false,
    'completed',
    2,
    repeat('a', 64),
    jsonb_build_object(
        'type', 'network_traffic_import_vnstat',
        'interfaces', jsonb_build_array('eth0'),
        'start_unix', start_unix
    ),
    repeat('b', 64),
    300,
    baseline,
    baseline + interval '3 minutes'
FROM job_data;

WITH clock AS (
    SELECT date_trunc('hour', current_timestamp) - interval '2 hours' AS baseline
)
INSERT INTO public.job_targets (
    job_id, client_id, status, message, exit_code, started_at,
    completed_at, result_received_at
)
SELECT
    '11111111-1111-4111-8111-111111111111'::uuid,
    'audit-clean-client',
    'completed',
    'vnStat history imported: 1 interface(s), 1 synthetic minute samples, 100 RX bytes, 200 TX bytes; live agent counters continue at the existing boundary',
    0,
    baseline,
    baseline + interval '3 minutes',
    baseline + interval '3 minutes'
FROM clock;

WITH clock AS (
    SELECT date_trunc('hour', current_timestamp) - interval '2 hours' AS baseline
)
INSERT INTO public.job_targets (
    job_id, client_id, status, message, exit_code, started_at,
    completed_at, result_received_at
)
SELECT
    '11111111-1111-4111-8111-111111111111'::uuid,
    'audit-overlap-client',
    'completed',
    'vnStat history imported: 1 interface(s), 0 synthetic minute samples, 0 RX bytes, 0 TX bytes; live agent counters continue at the existing boundary',
    0,
    baseline,
    baseline + interval '3 minutes',
    baseline + interval '3 minutes'
FROM clock;

WITH clock AS (
    SELECT date_trunc('hour', current_timestamp) - interval '2 hours' AS baseline
), payloads AS (
    SELECT
        jsonb_build_object(
            'type', 'network_traffic_import_vnstat_batch',
            'batch_index', 0,
            'buckets', jsonb_build_array(jsonb_build_object(
                'interface', 'eth0',
                'start_unix', extract(epoch FROM baseline + interval '1 minute')::bigint,
                'duration_secs', 60,
                'rx_bytes', 100,
                'tx_bytes', 200
            ))
        )::text AS batch_data,
        jsonb_build_object(
            'type', 'network_traffic_import_vnstat',
            'status', 'collected',
            'requested_start_unix', extract(epoch FROM baseline + interval '1 minute')::bigint,
            'collected_until_unix', extract(epoch FROM baseline + interval '2 minutes')::bigint,
            'interfaces', jsonb_build_array('eth0'),
            'sources', jsonb_build_array(jsonb_build_object(
                'interface', 'eth0',
                'database_created_unix', extract(epoch FROM baseline)::bigint,
                'retained_start_unix', extract(epoch FROM baseline + interval '1 minute')::bigint,
                'source_updated_unix', extract(epoch FROM baseline + interval '2 minutes')::bigint
            )),
            'batch_count', 1,
            'bucket_count', 1,
            'message', ''
        )::text AS final_data,
        baseline
    FROM clock
), encoded AS (
    SELECT
        convert_to(batch_data, 'UTF8') AS batch_data,
        convert_to(final_data, 'UTF8') AS final_data,
        baseline
    FROM payloads
)
INSERT INTO public.job_outputs (
    job_id, client_id, seq, stream, data, storage, object_key,
    data_sha256_hex, data_size_bytes, exit_code, done, received_at, created_at
)
SELECT
    '11111111-1111-4111-8111-111111111111'::uuid,
    'audit-clean-client',
    0,
    'status',
    batch_data,
    'inline',
    NULL,
    encode(sha256(batch_data), 'hex'),
    octet_length(batch_data),
    NULL,
    false,
    baseline + interval '2 minutes',
    baseline + interval '2 minutes'
FROM encoded
UNION ALL
SELECT
    '11111111-1111-4111-8111-111111111111'::uuid,
    'audit-clean-client',
    1,
    'status',
    final_data,
    'inline',
    NULL,
    encode(sha256(final_data), 'hex'),
    octet_length(final_data),
    0,
    true,
    baseline + interval '3 minutes',
    baseline + interval '3 minutes'
FROM encoded;

WITH clock AS (
    SELECT date_trunc('hour', current_timestamp) - interval '2 hours' AS baseline
), encoded AS (
    SELECT
        convert_to(jsonb_build_object(
            'type', 'network_traffic_import_vnstat',
            'status', 'collected',
            'requested_start_unix', extract(epoch FROM baseline + interval '1 minute')::bigint,
            'collected_until_unix', extract(epoch FROM baseline + interval '2 minutes')::bigint,
            'interfaces', jsonb_build_array('eth0'),
            'sources', jsonb_build_array(jsonb_build_object(
                'interface', 'eth0',
                'database_created_unix', extract(epoch FROM baseline)::bigint,
                'retained_start_unix', extract(epoch FROM baseline + interval '1 minute')::bigint,
                'source_updated_unix', extract(epoch FROM baseline + interval '2 minutes')::bigint
            )),
            'batch_count', 0,
            'bucket_count', 0,
            'message', ''
        )::text, 'UTF8') AS data,
        baseline
    FROM clock
)
INSERT INTO public.job_outputs (
    job_id, client_id, seq, stream, data, storage, object_key,
    data_sha256_hex, data_size_bytes, exit_code, done, received_at, created_at
)
SELECT
    '11111111-1111-4111-8111-111111111111'::uuid,
    'audit-overlap-client',
    0,
    'status',
    data,
    'inline',
    NULL,
    encode(sha256(data), 'hex'),
    octet_length(data),
    0,
    true,
    baseline + interval '3 minutes',
    baseline + interval '3 minutes'
FROM encoded;

WITH clock AS (
    SELECT date_trunc('hour', current_timestamp) - interval '2 hours' AS baseline
)
INSERT INTO public.traffic_counter_samples (
    client_id, source_kind, interface, observed_at, rx_bytes, tx_bytes,
    rx_counter_epoch, tx_counter_epoch, sample_source, inbound_promoted
)
SELECT
    'audit-clean-client', 'host', 'eth0', baseline,
    0, 0, 0, 0, 'vnstat_import:11111111-1111-4111-8111-111111111111', false
FROM clock
UNION ALL
SELECT
    'audit-clean-client', 'host', 'eth0', baseline + interval '1 minute',
    100, 200, 0, 0, 'vnstat_import:11111111-1111-4111-8111-111111111111', false
FROM clock
UNION ALL
SELECT
    'audit-clean-client', 'host', 'eth0', baseline + interval '2 minutes',
    10, 20, 1, 1, 'agent_networks', false
FROM clock;
SQL

host_port="$(docker port "$CONTAINER_NAME" 5432/tcp | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p' | head -n 1)"
[[ "$host_port" =~ ^[0-9]+$ ]] || fail "could not resolve PostgreSQL host port"
audit_url="postgres://$AUDIT_USER:$AUDIT_PASSWORD@127.0.0.1:$host_port/$POSTGRES_DB"

# PostgreSQL hides another role's state/xact_start fields unless the observer
# has pg_read_all_stats. Hold a real second-role transaction beyond the shipped
# five-minute threshold; do not shorten production semantics for the smoke.
docker exec -i "$CONTAINER_NAME" \
  psql -X -q -v ON_ERROR_STOP=1 \
    --username="$ACTIVITY_USER" --dbname="$POSTGRES_DB" \
  >"$SMOKE_ROOT/activity-probe.log" 2>&1 <<'SQL' &
BEGIN READ ONLY;
SELECT pg_sleep(3600);
ROLLBACK;
SQL
activity_probe_pid="$!"

activity_probe_age=""
for _attempt in $(seq 1 30); do
  activity_probe_age="$(docker exec "$CONTAINER_NAME" \
    psql -X -Atq -v ON_ERROR_STOP=1 \
      --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
      -c "SELECT COALESCE(max(floor(extract(epoch FROM clock_timestamp() - xact_start)))::bigint, -1) FROM pg_stat_activity WHERE usename = '$ACTIVITY_USER' AND xact_start IS NOT NULL")"
  [[ "$activity_probe_age" =~ ^[0-9]+$ ]] && break
  sleep 1
done
[[ "$activity_probe_age" =~ ^[0-9]+$ ]] ||
  fail "second-role transaction did not become visible"
for _attempt in $(seq 1 6); do
  ((activity_probe_age >= 300)) && break
  sleep 60
  activity_probe_age="$(docker exec "$CONTAINER_NAME" \
    psql -X -Atq -v ON_ERROR_STOP=1 \
      --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
      -c "SELECT COALESCE(max(floor(extract(epoch FROM clock_timestamp() - xact_start)))::bigint, -1) FROM pg_stat_activity WHERE usename = '$ACTIVITY_USER' AND xact_start IS NOT NULL")"
  [[ "$activity_probe_age" =~ ^[0-9]+$ ]] ||
    fail "second-role transaction ended before the five-minute threshold"
done
((activity_probe_age >= 300)) ||
  fail "second-role transaction did not reach the five-minute threshold"

database_fingerprint() {
  docker exec "$CONTAINER_NAME" \
    pg_dump --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
      --data-only --no-owner --no-privileges |
    sed -E '/^\\(un)?restrict /d' |
    sha256sum |
    awk '{print $1}'
}

run_audit() {
  local expected_status="$1" output="$2"
  shift 2
  local before after status
  before="$(database_fingerprint)"
  if VPSMAN_POSTGRES_URL="$audit_url" \
    "$AUDIT_SCRIPT" "$@" >"$output" 2>"$output.log"; then
    status=0
  else
    status="$?"
  fi
  after="$(database_fingerprint)"
  [[ "$status" -eq "$expected_status" ]] ||
    fail "expected audit status $expected_status, got $status for $*"
  [[ "$before" == "$after" ]] ||
    fail "database fingerprint changed during read-only audit: $*"
}

clean_output="$SMOKE_ROOT/clean.tsv"
run_audit 0 "$clean_output" --mode deep --writers-stopped
grep -Eq '^WARN[[:space:]]+page_checksums_disabled[[:space:]]+1[[:space:]]' "$clean_output" ||
  fail "clean audit did not distinguish the checksum warning"
if ! grep -Eq '^WARN[[:space:]]+long_running_client_transactions[[:space:]]+1[[:space:]]' \
    "$clean_output" ||
   ! grep -Eq '"max_age_seconds": ([3-9][0-9]{2}|[1-9][0-9]{3,})' \
    "$clean_output"; then
  fail "restricted auditor did not detect the second role's five-minute transaction"
fi
grep -Eq '^[A-Z]+[[:space:]]+audit_summary[[:space:]]+0[[:space:]]' "$clean_output" ||
  fail "clean audit summary reported a hard finding"
grep -Eq '^HARD[[:space:]]+import_conservation[[:space:]]+0[[:space:]].*"checked_targets": 2' \
  "$clean_output" ||
  fail "clean audit did not conserve both targets from one multi-client job independently"
if ! grep -Eq '^HARD[[:space:]]+deep_resource_bounds[[:space:]]+0[[:space:]]' \
    "$clean_output" ||
   ! grep -F '"max_parallel_workers_per_gather": 0' "$clean_output" >/dev/null ||
   ! grep -F '"temp_file_limit": "256MB"' "$clean_output" >/dev/null; then
  fail "clean deep audit did not prove its single-backend 256 MiB spill bound"
fi
if ! grep -Eq '^HARD[[:space:]]+migration_0015_index_contract[[:space:]]+0[[:space:]]' \
    "$clean_output" ||
   ! grep -F '"matching_contract_rows": 1' "$clean_output" >/dev/null ||
   ! grep -F '"ready": true' "$clean_output" >/dev/null ||
   ! grep -F '"valid": true' "$clean_output" >/dev/null; then
  fail "clean audit did not prove the exact ready/valid migration 0015 index"
fi
if ! grep -Eq '^HARD[[:space:]]+migration_0016_checksum[[:space:]]+0[[:space:]]' \
    "$clean_output" ||
   ! grep -F '"matching_rows": 1' "$clean_output" >/dev/null; then
  fail "clean audit did not prove the exact migration 0016 ledger checksum"
fi
if ! grep -Eq '^HARD[[:space:]]+migration_0016_streaming_function_contract[[:space:]]+0[[:space:]]' \
    "$clean_output" ||
   ! grep -F '"available": true' "$clean_output" >/dev/null ||
   ! grep -F '"matching_contract_rows": 1' "$clean_output" >/dev/null; then
  fail "clean audit did not prove the exact migration 0016 streaming function"
fi
for clean_contract in \
  migration_release_range \
  migration_0017_checksum \
  migration_0017_suspension_catalog_contract \
  migration_0018_checksum \
  migration_0018_import_class_index_contract \
  migration_0019_checksum \
  migration_0019_import_update_trigger_contract; do
  grep -Eq "^HARD[[:space:]]+${clean_contract}[[:space:]]+0[[:space:]]" \
    "$clean_output" || fail "clean audit did not prove ${clean_contract}"
done
grep -F '"catalog_state": "usable"' "$clean_output" >/dev/null ||
  fail "clean audit did not report the migration 0018 index usable"
for identity in audit-clean-client audit-overlap-client; do
  if grep -Fq "$identity" "$clean_output"; then
    fail "default audit output exposed a client identity"
  fi
done

# The activity probe has now proved the shipped five-minute detection boundary.
# End only that dedicated role's single backend and reap the exact docker-exec
# child before any concurrent-index fixture; an old virtual transaction would
# otherwise hold CREATE INDEX CONCURRENTLY in its final validation phase.
activity_termination="$(docker exec "$CONTAINER_NAME" \
  psql -X -Atq -v ON_ERROR_STOP=1 \
    --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
    -c "WITH candidates AS MATERIALIZED (
          SELECT pid
          FROM pg_stat_activity
          WHERE datname = current_database()
            AND usename = '$ACTIVITY_USER'
            AND backend_type = 'client backend'
        ), decision AS (
          SELECT count(*)::bigint AS candidate_count, max(pid) AS pid
          FROM candidates
        )
        SELECT candidate_count,
               CASE WHEN candidate_count = 1
                    THEN pg_terminate_backend(pid)
                    ELSE FALSE
               END
        FROM decision")"
[[ "$activity_termination" == "1|t" ]] ||
  fail "expected exactly one activity-probe backend to terminate, got: $activity_termination"
activity_probe_reaped=0
for _attempt in $(seq 1 100); do
  if ! kill -0 "$activity_probe_pid" >/dev/null 2>&1; then
    activity_probe_reaped=1
    break
  fi
  sleep 0.1
done
[[ "$activity_probe_reaped" -eq 1 ]] ||
  fail "terminated activity-probe process did not exit within 10 seconds"
activity_probe_exit=0
wait "$activity_probe_pid" || activity_probe_exit="$?"
activity_probe_pid=""
[[ "$activity_probe_exit" -ne 0 ]] ||
  fail "terminated activity-probe process unexpectedly exited successfully"

# The 0019 ledger row and trigger body are independently pinned. A wrong
# checksum must fail even when the exact trigger remains installed.
psql_super >/dev/null <<'SQL'
UPDATE public._sqlx_migrations
SET checksum = decode(repeat('00', 48), 'hex')
WHERE version = 19;
SQL
wrong_0019_ledger_output="$SMOKE_ROOT/wrong-0019-ledger.tsv"
run_audit 2 "$wrong_0019_ledger_output" --mode quick
grep -Eq '^HARD[[:space:]]+migration_0019_checksum[[:space:]]+1[[:space:]]' \
  "$wrong_0019_ledger_output" ||
  fail "audit did not hard-fail a wrong migration 0019 checksum"
psql_super >/dev/null <<SQL
UPDATE public._sqlx_migrations
SET checksum = decode('$MIGRATION_0019_SHA384', 'hex')
WHERE version = 19;
SQL

# Replacing the same-signature function body must fail the source contract;
# restoring the exact migration immediately keeps later fixtures independent.
psql_super >/dev/null <<'SQL'
CREATE OR REPLACE FUNCTION public.refresh_traffic_counter_hourly_usage_after_update()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    RETURN NULL;
END;
$$;
SQL
wrong_0019_function_output="$SMOKE_ROOT/wrong-0019-function.tsv"
run_audit 2 "$wrong_0019_function_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0019_import_update_trigger_contract[[:space:]]+1[[:space:]]' \
    "$wrong_0019_function_output" ||
   ! grep -F '"matching_function_rows": 0' \
    "$wrong_0019_function_output" >/dev/null; then
  fail "audit did not hard-fail a wrong migration 0019 trigger function"
fi
psql_super <"$MIGRATION_0019" >/dev/null

# Migration 0017 needs both its exact ledger source and its validated catalog
# boundary. A same-name but weakened constraint must not pass merely because
# the ledger row is intact.
psql_super >/dev/null <<'SQL'
ALTER TABLE public.clients
    RENAME CONSTRAINT clients_suspension_state_check
    TO clients_suspension_state_check_expected;
ALTER TABLE public.clients
    ADD CONSTRAINT clients_suspension_state_check CHECK (status IS NOT NULL);
SQL
wrong_0017_catalog_output="$SMOKE_ROOT/wrong-0017-catalog.tsv"
run_audit 2 "$wrong_0017_catalog_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0017_suspension_catalog_contract[[:space:]]+[1-9][0-9]*[[:space:]]' \
    "$wrong_0017_catalog_output" ||
   ! grep -F '"required_tokens_present": false' \
    "$wrong_0017_catalog_output" >/dev/null; then
  fail "audit did not hard-fail a weakened migration 0017 suspension constraint"
fi
psql_super >/dev/null <<'SQL'
ALTER TABLE public.clients
    DROP CONSTRAINT clients_suspension_state_check;
ALTER TABLE public.clients
    RENAME CONSTRAINT clients_suspension_state_check_expected
    TO clients_suspension_state_check;
SQL

psql_super >/dev/null <<'SQL'
UPDATE public._sqlx_migrations
SET checksum = decode(repeat('00', 48), 'hex')
WHERE version = 17;
SQL
wrong_0017_ledger_output="$SMOKE_ROOT/wrong-0017-ledger.tsv"
run_audit 2 "$wrong_0017_ledger_output" --mode quick
grep -Eq '^HARD[[:space:]]+migration_0017_checksum[[:space:]]+1[[:space:]]' \
  "$wrong_0017_ledger_output" ||
  fail "audit did not hard-fail a wrong migration 0017 checksum"
psql_super >/dev/null <<SQL
UPDATE public._sqlx_migrations
SET checksum = decode('$MIGRATION_0017_SHA384', 'hex')
WHERE version = 17;
SQL

# A valid unledgered 0018 index is not enough: the current startup helper must
# first ledger the exact no-transaction migration before accepting work.
psql_super >/dev/null <<'SQL'
DELETE FROM public._sqlx_migrations WHERE version = 18;
SQL
unledgered_0018_output="$SMOKE_ROOT/unledgered-0018.tsv"
run_audit 2 "$unledgered_0018_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0018_checksum[[:space:]]+1[[:space:]]' \
    "$unledgered_0018_output" ||
   ! grep -F '"catalog_state": "usable"' "$unledgered_0018_output" >/dev/null; then
  fail "audit did not distinguish a valid but unledgered migration 0018 index"
fi
psql_super >/dev/null <<SQL
INSERT INTO public._sqlx_migrations (
    version, description, success, checksum, execution_time
) VALUES (
    18,
    'traffic counter import class stream index',
    true,
    decode('$MIGRATION_0018_SHA384', 'hex'),
    0
);
SQL

# A ledgered but missing index is recoverable by one current-binary restart,
# but remains a hard pre-start audit state.
psql_super >/dev/null <<'SQL'
DROP INDEX public.traffic_counter_samples_import_class_stream_idx;
SQL
missing_0018_index_output="$SMOKE_ROOT/missing-0018-index.tsv"
run_audit 2 "$missing_0018_index_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0018_import_class_index_contract[[:space:]]+1[[:space:]]' \
    "$missing_0018_index_output" ||
   ! grep -F '"catalog_state": "missing_recoverable_by_current_startup"' \
    "$missing_0018_index_output" >/dev/null; then
  fail "audit did not hard-fail a ledgered but missing migration 0018 index"
fi
psql_super <"$MIGRATION_0018" >/dev/null

# The startup helper deliberately refuses to drop a wrong same-name object.
# The audit must preserve that fail-closed distinction for operator recovery.
psql_super >/dev/null <<'SQL'
ALTER INDEX public.traffic_counter_samples_import_class_stream_idx
    RENAME TO traffic_counter_samples_import_class_stream_idx_expected;
CREATE INDEX traffic_counter_samples_import_class_stream_idx
    ON public.traffic_counter_samples (client_id);
SQL
wrong_0018_index_output="$SMOKE_ROOT/wrong-0018-index.tsv"
run_audit 2 "$wrong_0018_index_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0018_import_class_index_contract[[:space:]]+1[[:space:]]' \
    "$wrong_0018_index_output" ||
   ! grep -F '"catalog_state": "wrong_same_name_operator_action_required"' \
    "$wrong_0018_index_output" >/dev/null; then
  fail "audit did not fail closed for a wrong same-name migration 0018 index"
fi
psql_super >/dev/null <<'SQL'
DROP INDEX public.traffic_counter_samples_import_class_stream_idx;
ALTER INDEX public.traffic_counter_samples_import_class_stream_idx_expected
    RENAME TO traffic_counter_samples_import_class_stream_idx;
SQL

# Reproduce the real interrupted-CREATE-CONCURRENTLY crash state. An old
# repeatable-read snapshot holds the final validation phase open; canceling the
# exact build after its catalog row appears deterministically leaves the
# migration-owned definition invalid. The audit must label it as restart-
# recoverable, not confuse it with a wrong same-name object.
psql_super >/dev/null <<'SQL'
DROP INDEX public.traffic_counter_samples_import_class_stream_idx;
SQL
docker exec -e PGAPPNAME=vpsman-smoke-index-snapshot -i "$CONTAINER_NAME" \
  psql -X -q -v ON_ERROR_STOP=1 \
    --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
  >"$SMOKE_ROOT/index-snapshot.log" 2>&1 <<'SQL' &
BEGIN TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY;
SELECT count(*) FROM public.traffic_counter_samples;
SELECT pg_sleep(60);
ROLLBACK;
SQL
index_snapshot_pid="$!"
snapshot_ready=0
for _attempt in $(seq 1 50); do
  snapshot_ready="$(docker exec "$CONTAINER_NAME" \
    psql -X -Atq -v ON_ERROR_STOP=1 \
      --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
      -c "SELECT count(*) FROM pg_stat_activity WHERE application_name='vpsman-smoke-index-snapshot' AND xact_start IS NOT NULL")"
  [[ "$snapshot_ready" == "1" ]] && break
  sleep 0.1
done
[[ "$snapshot_ready" == "1" ]] ||
  fail "interrupted-index fixture could not establish its old snapshot"

docker exec -e PGAPPNAME=vpsman-smoke-index-build -i "$CONTAINER_NAME" \
  psql -X -q -v ON_ERROR_STOP=1 \
    --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
  >"$SMOKE_ROOT/interrupted-0018-build.log" 2>&1 <<'SQL' &
CREATE INDEX CONCURRENTLY traffic_counter_samples_import_class_stream_idx
    ON public.traffic_counter_samples (
        client_id,
        source_kind,
        interface,
        (sample_source LIKE 'vnstat_import:%'),
        observed_at
    );
SQL
index_build_pid="$!"
invalid_exact_ready=0
for _attempt in $(seq 1 100); do
  invalid_exact_ready="$(docker exec "$CONTAINER_NAME" \
    psql -X -Atq -v ON_ERROR_STOP=1 \
      --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
      -c "SELECT count(*) FROM pg_class relation JOIN pg_namespace namespace ON namespace.oid=relation.relnamespace JOIN pg_index index_catalog ON index_catalog.indexrelid=relation.oid WHERE namespace.nspname='public' AND relation.relname='traffic_counter_samples_import_class_stream_idx' AND NOT index_catalog.indisvalid")"
  [[ "$invalid_exact_ready" == "1" ]] && break
  sleep 0.1
done
[[ "$invalid_exact_ready" == "1" ]] ||
  fail "interrupted-index fixture did not expose an invalid exact catalog row"
psql_super >/dev/null <<'SQL'
SELECT pg_cancel_backend(pid)
FROM pg_stat_activity
WHERE application_name = 'vpsman-smoke-index-build';
SQL
if wait "$index_build_pid"; then
  fail "interrupted migration 0018 concurrent build unexpectedly succeeded"
fi
index_build_pid=""
psql_super >/dev/null <<'SQL'
SELECT pg_terminate_backend(pid)
FROM pg_stat_activity
WHERE application_name = 'vpsman-smoke-index-snapshot';
SQL
wait "$index_snapshot_pid" >/dev/null 2>&1 || true
index_snapshot_pid=""

invalid_exact_0018_output="$SMOKE_ROOT/invalid-exact-0018.tsv"
run_audit 2 "$invalid_exact_0018_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0018_import_class_index_contract[[:space:]]+1[[:space:]]' \
    "$invalid_exact_0018_output" ||
   ! grep -F '"catalog_state": "exact_invalid_recoverable_by_current_startup"' \
    "$invalid_exact_0018_output" >/dev/null ||
   ! grep -F '"valid": false' "$invalid_exact_0018_output" >/dev/null; then
  fail "audit did not classify the interrupted exact migration 0018 build"
fi
psql_super >/dev/null <<'SQL'
DROP INDEX public.traffic_counter_samples_import_class_stream_idx;
SQL
psql_super <"$MIGRATION_0018" >/dev/null

# An exact current-release audit must not accept an absent or checksum-altered
# migration 0016 ledger row, even while the function itself remains available.
psql_super >/dev/null <<'SQL'
UPDATE public._sqlx_migrations
SET checksum = decode(repeat('00', 48), 'hex')
WHERE version = 16;
SQL
wrong_0016_output="$SMOKE_ROOT/wrong-0016.tsv"
run_audit 2 "$wrong_0016_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0016_checksum[[:space:]]+1[[:space:]]' \
    "$wrong_0016_output" ||
   ! grep -F '"present_rows": 1' "$wrong_0016_output" >/dev/null ||
   ! grep -F '"matching_rows": 0' "$wrong_0016_output" >/dev/null; then
  fail "audit did not hard-fail a wrong migration 0016 checksum"
fi
psql_super >/dev/null <<SQL
UPDATE public._sqlx_migrations
SET checksum = decode('$MIGRATION_0016_SHA384', 'hex')
WHERE version = 16;
SQL

psql_super >/dev/null <<'SQL'
DELETE FROM public._sqlx_migrations WHERE version = 16;
SQL
missing_0016_output="$SMOKE_ROOT/missing-0016.tsv"
run_audit 2 "$missing_0016_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0016_checksum[[:space:]]+1[[:space:]]' \
    "$missing_0016_output" ||
   ! grep -F '"present_rows": 0' "$missing_0016_output" >/dev/null; then
  fail "audit did not hard-fail a missing migration 0016 ledger row"
fi
psql_super >/dev/null <<SQL
INSERT INTO public._sqlx_migrations (
    version, description, success, checksum, execution_time
) VALUES (
    16,
    'streaming traffic hourly refresh',
    true,
    decode('$MIGRATION_0016_SHA384', 'hex'),
    0
);
SQL

# The exact ledger is also insufficient when the expected streaming function
# is absent or replaced under the same signature.
psql_super >/dev/null <<'SQL'
ALTER FUNCTION public.refresh_traffic_counter_hourly_usage(
    TEXT[], TEXT[], TEXT[], TIMESTAMPTZ[], BOOLEAN
) RENAME TO refresh_traffic_counter_hourly_usage_expected;
SQL
missing_function_output="$SMOKE_ROOT/missing-0016-function.tsv"
run_audit 2 "$missing_function_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0016_streaming_function_contract[[:space:]]+1[[:space:]]' \
    "$missing_function_output" ||
   ! grep -F '"available": false' "$missing_function_output" >/dev/null; then
  fail "audit did not hard-fail a missing migration 0016 streaming function"
fi

psql_super >/dev/null <<'SQL'
CREATE FUNCTION public.refresh_traffic_counter_hourly_usage(
    changed_client_ids TEXT[],
    changed_source_kinds TEXT[],
    changed_interfaces TEXT[],
    changed_observed_at TIMESTAMPTZ[],
    rebuild_entire_streams BOOLEAN DEFAULT FALSE
) RETURNS VOID
LANGUAGE plpgsql
AS $$
BEGIN
    RETURN;
END;
$$;
SQL
wrong_function_output="$SMOKE_ROOT/wrong-0016-function.tsv"
run_audit 2 "$wrong_function_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0016_streaming_function_contract[[:space:]]+1[[:space:]]' \
    "$wrong_function_output" ||
   ! grep -F '"available": true' "$wrong_function_output" >/dev/null ||
   ! grep -F '"matching_contract_rows": 0' "$wrong_function_output" >/dev/null; then
  fail "audit did not hard-fail a wrong migration 0016 streaming function"
fi
psql_super >/dev/null <<'SQL'
DROP FUNCTION public.refresh_traffic_counter_hourly_usage(
    TEXT[], TEXT[], TEXT[], TIMESTAMPTZ[], BOOLEAN
);
ALTER FUNCTION public.refresh_traffic_counter_hourly_usage_expected(
    TEXT[], TEXT[], TEXT[], TIMESTAMPTZ[], BOOLEAN
) RENAME TO refresh_traffic_counter_hourly_usage;
SQL

# An exact migration-ledger row is insufficient when a same-name index has the
# wrong keys. The audit must reject that state as HARD, then accept the restored
# migration-created definition for the remaining fixtures.
psql_super >/dev/null <<'SQL'
ALTER INDEX public.telemetry_network_rates_client_effective_idx
    RENAME TO telemetry_network_rates_client_effective_idx_expected;
CREATE INDEX telemetry_network_rates_client_effective_idx
    ON public.telemetry_network_rates (client_id);
SQL
wrong_index_output="$SMOKE_ROOT/wrong-index.tsv"
run_audit 2 "$wrong_index_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0015_index_contract[[:space:]]+1[[:space:]]' \
    "$wrong_index_output" ||
   ! grep -F '"matching_contract_rows": 0' "$wrong_index_output" >/dev/null ||
   ! grep -F '"ready": true' "$wrong_index_output" >/dev/null ||
   ! grep -F '"valid": true' "$wrong_index_output" >/dev/null; then
  fail "audit did not hard-fail an exact 0015 ledger with a same-name wrong index"
fi
psql_super >/dev/null <<'SQL'
DROP INDEX public.telemetry_network_rates_client_effective_idx;
SQL

# A deliberately failed concurrent unique build leaves a same-name invalid
# catalog entry. This exercises indisvalid/indisready handling independently of
# the valid-but-wrong definition above.
if psql_super >"$SMOKE_ROOT/invalid-index-build.log" 2>&1 <<'SQL'
CREATE UNIQUE INDEX CONCURRENTLY telemetry_network_rates_client_effective_idx
    ON public.clients (status);
SQL
then
  fail "invalid-index fixture unexpectedly built a unique duplicate-status index"
fi
invalid_index_output="$SMOKE_ROOT/invalid-index.tsv"
run_audit 2 "$invalid_index_output" --mode quick
if ! grep -Eq '^HARD[[:space:]]+migration_0015_index_contract[[:space:]]+1[[:space:]]' \
    "$invalid_index_output" ||
   ! grep -F '"matching_contract_rows": 0' "$invalid_index_output" >/dev/null ||
   ! grep -F '"valid": false' "$invalid_index_output" >/dev/null; then
  fail "audit did not hard-fail an exact 0015 ledger with an invalid same-name index"
fi
psql_super >/dev/null <<'SQL'
DROP INDEX public.telemetry_network_rates_client_effective_idx;
ALTER INDEX public.telemetry_network_rates_client_effective_idx_expected
    RENAME TO telemetry_network_rates_client_effective_idx;
SQL

# A late SQL decoding failure must produce runtime status 1 without publishing
# the valid rows psql emitted before it failed. The restricted audit remains
# read-only, so its full data fingerprint must also remain unchanged.
psql_super >/dev/null <<'SQL'
INSERT INTO public.jobs (
    id, command_type, privileged, status, target_count, payload_hash,
    operation, request_fingerprint, max_timeout_secs, created_at, completed_at
) VALUES (
    '22222222-2222-4222-8222-222222222222'::uuid,
    'network_traffic_import_vnstat',
    false,
    'completed',
    1,
    repeat('e', 64),
    jsonb_build_object(
        'type', 'network_traffic_import_vnstat',
        'interfaces', jsonb_build_array('late0'),
        'start_unix', extract(epoch FROM current_timestamp - interval '1 day')::bigint
    ),
    repeat('f', 64),
    300,
    current_timestamp - interval '1 minute',
    current_timestamp
);

INSERT INTO public.job_targets (
    job_id, client_id, status, message, exit_code, started_at,
    completed_at, result_received_at
) VALUES (
    '22222222-2222-4222-8222-222222222222'::uuid,
    'audit-overlap-client',
    'completed',
    'vnStat history imported: 1 interface(s), 0 synthetic minute samples, 0 RX bytes, 0 TX bytes; live agent counters continue at the existing boundary',
    0,
    current_timestamp - interval '1 minute',
    current_timestamp,
    current_timestamp
);

WITH invalid_output AS (
    SELECT decode('80', 'hex') AS data
)
INSERT INTO public.job_outputs (
    job_id, client_id, seq, stream, data, storage, object_key,
    data_sha256_hex, data_size_bytes, exit_code, done, received_at, created_at
)
SELECT
    '22222222-2222-4222-8222-222222222222'::uuid,
    'audit-overlap-client',
    0,
    'status',
    data,
    'inline',
    NULL,
    encode(sha256(data), 'hex'),
    octet_length(data),
    0,
    true,
    current_timestamp,
    current_timestamp
FROM invalid_output;
SQL

late_failure_output="$SMOKE_ROOT/late-failure.tsv"
run_audit 1 "$late_failure_output" --mode deep --writers-stopped
[[ ! -s "$late_failure_output" ]] ||
  fail "late SQL failure published partial audit rows"
grep -Fq 'PostgreSQL audit query failed' "$late_failure_output.log" ||
  fail "late SQL failure did not report a bounded runtime failure"
psql_super >/dev/null <<'SQL'
DELETE FROM public.jobs
WHERE id = '22222222-2222-4222-8222-222222222222'::uuid;
SQL

psql_super >/dev/null <<'SQL'
WITH base AS (
    SELECT date_bin(
        interval '3 hours',
        current_timestamp - interval '40 days',
        TIMESTAMPTZ '1970-01-01 00:00:00+00'
    ) AS bucket_start
)
INSERT INTO public.traffic_counter_rollups (
    client_id, source_kind, interface, origin_kind, bucket_secs,
    bucket_start, rx_bytes, tx_bytes, rx_valid_count, tx_valid_count,
    any_valid_count, rx_reset_count, tx_reset_count, any_reset_count,
    first_observed_at, latest_observed_at
)
SELECT
    'audit-overlap-client', 'host', 'overlap0', 'live', 10800,
    bucket_start, 10, 20, 1, 1, 1, 0, 0, 0,
    bucket_start + interval '1 minute', bucket_start + interval '2 hours'
FROM base
UNION ALL
SELECT
    'audit-overlap-client', 'host', 'overlap0', 'live', 3600,
    bucket_start, 10, 20, 1, 1, 1, 0, 0, 0,
    bucket_start + interval '1 minute', bucket_start + interval '30 minutes'
FROM base;
SQL

warning_output="$SMOKE_ROOT/warning.tsv"
run_audit 0 "$warning_output" --mode quick
grep -Eq '^WARN[[:space:]]+rollup_finer_overlap[[:space:]]+[1-9][0-9]*[[:space:]]' \
  "$warning_output" || fail "warning-only audit did not report rollup overlap"
grep -Eq '^WARN[[:space:]]+audit_summary[[:space:]]+0[[:space:]].*"warning_count":[1-9]' \
  "$warning_output" || fail "warning-only summary did not retain warning count"

psql_super >/dev/null <<'SQL'
INSERT INTO public.jobs (
    id, command_type, privileged, status, target_count, payload_hash,
    operation, request_fingerprint, max_timeout_secs, created_at, completed_at
) VALUES (
    '33333333-3333-4333-8333-333333333333'::uuid,
    'network_traffic_import_vnstat',
    false,
    'completed',
    1,
    repeat('c', 64),
    jsonb_build_object(
        'type', 'network_traffic_import_vnstat',
        'interfaces', jsonb_build_array('missing-final0'),
        'start_unix', extract(epoch FROM current_timestamp - interval '1 day')::bigint
    ),
    repeat('d', 64),
    300,
    current_timestamp - interval '1 minute',
    current_timestamp
);

INSERT INTO public.job_targets (
    job_id, client_id, status, message, exit_code, started_at,
    completed_at, result_received_at
) VALUES (
    '33333333-3333-4333-8333-333333333333'::uuid,
    'audit-overlap-client',
    'completed',
    'vnStat history imported: 1 interface(s), 0 synthetic minute samples, 0 RX bytes, 0 TX bytes; live agent counters continue at the existing boundary',
    0,
    current_timestamp - interval '1 minute',
    current_timestamp,
    current_timestamp
);

WITH encoded AS (
    SELECT convert_to(jsonb_build_object(
        'type', 'network_traffic_import_vnstat_batch',
        'batch_index', 0,
        'buckets', jsonb_build_array()
    )::text, 'UTF8') AS data
)
INSERT INTO public.job_outputs (
    job_id, client_id, seq, stream, data, storage, object_key,
    data_sha256_hex, data_size_bytes, exit_code, done, received_at, created_at
)
SELECT
    '33333333-3333-4333-8333-333333333333'::uuid,
    'audit-overlap-client',
    0,
    'status',
    data,
    'inline',
    NULL,
    encode(sha256(data), 'hex'),
    octet_length(data),
    NULL,
    false,
    current_timestamp,
    current_timestamp
FROM encoded;

INSERT INTO public.jobs (
    id, command_type, privileged, status, target_count, payload_hash,
    operation, request_fingerprint, max_timeout_secs, created_at, completed_at
) VALUES (
    '55555555-5555-4555-8555-555555555555'::uuid,
    'network_traffic_import_vnstat',
    false,
    'completed',
    1,
    repeat('6', 64),
    jsonb_build_object(
        'type', 'network_traffic_import_vnstat',
        'interfaces', jsonb_build_array('badsummary0'),
        'start_unix', extract(epoch FROM current_timestamp - interval '1 day')::bigint
    ),
    repeat('7', 64),
    300,
    current_timestamp - interval '2 minutes',
    current_timestamp - interval '1 minute'
);

INSERT INTO public.job_targets (
    job_id, client_id, status, message, exit_code, started_at,
    completed_at, result_received_at
) VALUES (
    '55555555-5555-4555-8555-555555555555'::uuid,
    'audit-overlap-client',
    'completed',
    'summary unavailable',
    0,
    current_timestamp - interval '2 minutes',
    current_timestamp - interval '1 minute',
    current_timestamp - interval '1 minute'
);

WITH encoded AS (
    SELECT convert_to(jsonb_build_object(
        'type', 'network_traffic_import_vnstat',
        'status', 'collected',
        'requested_start_unix', extract(epoch FROM current_timestamp - interval '1 day')::bigint,
        'collected_until_unix', extract(epoch FROM current_timestamp - interval '1 minute')::bigint,
        'interfaces', jsonb_build_array('badsummary0'),
        'sources', jsonb_build_array(jsonb_build_object(
            'interface', 'badsummary0',
            'database_created_unix', extract(epoch FROM current_timestamp - interval '2 days')::bigint,
            'retained_start_unix', extract(epoch FROM current_timestamp - interval '1 day')::bigint,
            'source_updated_unix', extract(epoch FROM current_timestamp - interval '1 minute')::bigint
        )),
        'batch_count', 0,
        'bucket_count', 0,
        'message', ''
    )::text, 'UTF8') AS data
)
INSERT INTO public.job_outputs (
    job_id, client_id, seq, stream, data, storage, object_key,
    data_sha256_hex, data_size_bytes, exit_code, done, received_at, created_at
)
SELECT
    '55555555-5555-4555-8555-555555555555'::uuid,
    'audit-overlap-client',
    0,
    'status',
    data,
    'inline',
    NULL,
    encode(sha256(data), 'hex'),
    octet_length(data),
    0,
    true,
    current_timestamp - interval '1 minute',
    current_timestamp - interval '1 minute'
FROM encoded;

INSERT INTO public.jobs (
    id, command_type, privileged, status, target_count, payload_hash,
    operation, request_fingerprint, max_timeout_secs, created_at, completed_at
) VALUES (
    '44444444-4444-4444-8444-444444444444'::uuid,
    'network_traffic_import_vnstat',
    false,
    'completed',
    1,
    repeat('8', 64),
    jsonb_build_object(
        'type', 'network_traffic_import_vnstat',
        'interfaces', jsonb_build_array('eth0', 'partial1'),
        'start_unix', extract(epoch FROM current_timestamp - interval '4 hours')::bigint
    ),
    repeat('9', 64),
    300,
    current_timestamp - interval '4 hours',
    current_timestamp - interval '3 hours'
);

INSERT INTO public.job_targets (
    job_id, client_id, status, message, exit_code, started_at,
    completed_at, result_received_at
) VALUES (
    '44444444-4444-4444-8444-444444444444'::uuid,
    'audit-clean-client',
    'completed',
    'vnStat history imported: 2 interface(s), 0 synthetic minute samples, 0 RX bytes, 0 TX bytes; live agent counters continue at the existing boundary',
    0,
    current_timestamp - interval '4 hours',
    current_timestamp - interval '3 hours',
    current_timestamp - interval '3 hours'
);

WITH encoded AS (
    SELECT convert_to(jsonb_build_object(
        'type', 'network_traffic_import_vnstat',
        'status', 'collected',
        'requested_start_unix', extract(epoch FROM current_timestamp - interval '4 hours')::bigint,
        'collected_until_unix', extract(epoch FROM current_timestamp - interval '3 hours')::bigint,
        'interfaces', jsonb_build_array('eth0', 'partial1'),
        'sources', jsonb_build_array(
            jsonb_build_object(
                'interface', 'eth0',
                'database_created_unix', extract(epoch FROM current_timestamp - interval '5 hours')::bigint,
                'retained_start_unix', extract(epoch FROM current_timestamp - interval '4 hours')::bigint,
                'source_updated_unix', extract(epoch FROM current_timestamp - interval '3 hours')::bigint
            ),
            jsonb_build_object(
                'interface', 'partial1',
                'database_created_unix', extract(epoch FROM current_timestamp - interval '5 hours')::bigint,
                'retained_start_unix', extract(epoch FROM current_timestamp - interval '4 hours')::bigint,
                'source_updated_unix', extract(epoch FROM current_timestamp - interval '3 hours')::bigint
            )
        ),
        'batch_count', 0,
        'bucket_count', 0,
        'message', ''
    )::text, 'UTF8') AS data
)
INSERT INTO public.job_outputs (
    job_id, client_id, seq, stream, data, storage, object_key,
    data_sha256_hex, data_size_bytes, exit_code, done, received_at, created_at
)
SELECT
    '44444444-4444-4444-8444-444444444444'::uuid,
    'audit-clean-client',
    0,
    'status',
    data,
    'inline',
    NULL,
    encode(sha256(data), 'hex'),
    octet_length(data),
    0,
    true,
    current_timestamp - interval '3 hours',
    current_timestamp - interval '3 hours'
FROM encoded;

UPDATE public.traffic_counter_samples
SET inbound_promoted = true
WHERE client_id = 'audit-clean-client'
  AND source_kind = 'host'
  AND interface = 'eth0'
  AND sample_source LIKE 'vnstat_import:%';

UPDATE public.traffic_counter_hourly_usage
SET rx_bytes = rx_bytes + 1
WHERE client_id = 'audit-clean-client'
  AND source_kind = 'host'
  AND interface = 'eth0';
SQL

hard_output="$SMOKE_ROOT/hard.tsv"
run_audit 2 "$hard_output" --mode deep --writers-stopped
for check in \
  raw_promoted_boundary_count \
  hourly_usage_parity \
  rollup_finer_overlap \
  import_conservation \
  completed_import_final_output_missing \
  completed_import_summary_contract \
  conservation_skipped_by_partial_replacement; do
  grep -Eq "^(HARD|WARN)[[:space:]]+${check}[[:space:]]+[1-9][0-9]*[[:space:]]" \
    "$hard_output" || fail "hard audit did not detect $check"
done
grep -Eq '^HARD[[:space:]]+audit_summary[[:space:]]+[1-9][0-9]*[[:space:]]' \
  "$hard_output" || fail "hard audit summary did not request operator action"
if grep -Fq 'audit-clean-client' "$hard_output"; then
  fail "default hard-finding output exposed a client identity"
fi
if grep -Fq '44444444-4444-4444-8444-444444444444' "$hard_output"; then
  fail "default partial-replacement output exposed a job identity"
fi
grep -Fq 'stream-' "$hard_output" || fail "default hard findings lack pseudonymous stream aliases"

shown_output="$SMOKE_ROOT/shown.tsv"
run_audit 2 "$shown_output" --mode deep --writers-stopped --show-identities
grep -Fq 'audit-clean-client/host/eth0' "$shown_output" ||
  fail "--show-identities did not reveal the explicitly requested identity"
grep -Fq 'audit-clean-client/job-44444444-4444-4444-8444-444444444444' \
  "$shown_output" ||
  fail "--show-identities did not reveal the explicitly requested job identity"

if VPSMAN_POSTGRES_URL="$audit_url" \
  "$AUDIT_SCRIPT" --mode deep >"$SMOKE_ROOT/missing-ack.log" 2>&1; then
  missing_ack_status=0
else
  missing_ack_status="$?"
fi
if VPSMAN_POSTGRES_URL="$audit_url" \
  "$AUDIT_SCRIPT" --mode invalid >"$SMOKE_ROOT/invalid-mode.log" 2>&1; then
  invalid_mode_status=0
else
  invalid_mode_status="$?"
fi
if VPSMAN_POSTGRES_URL="postgres://invalid:invalid@127.0.0.1:1/invalid" \
  "$AUDIT_SCRIPT" --connect-timeout-secs 1 >"$SMOKE_ROOT/connection.log" 2>&1; then
  connection_status=0
else
  connection_status="$?"
fi
[[ "$missing_ack_status" -eq 64 ]] || fail "deep mode without acknowledgement did not exit 64"
[[ "$invalid_mode_status" -eq 64 ]] || fail "invalid mode did not exit 64"
[[ "$connection_status" -eq 1 ]] || fail "connection failure did not exit 1"

printf '%s\n' \
  'traffic-ledger audit smoke passed: restricted reader, bounded spill, exact migration 0015-0020 ledger/catalog contracts, interrupted 0018 recovery states, clean/warning/hard/runtime/usage/connection exits, pseudonyms, output completeness, parity, overlap, conservation, and unchanged data fingerprints'
