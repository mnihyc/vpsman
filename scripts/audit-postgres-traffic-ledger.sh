#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
AUDIT_SQL="$ROOT_DIR/scripts/sql/audit-postgres-traffic-ledger.sql"

usage() {
  cat >&2 <<'EOF'
usage: scripts/audit-postgres-traffic-ledger.sh [options]

Read-only PostgreSQL traffic-ledger audit.

Connection (choose one):
  VPSMAN_POSTGRES_URL=...         connect directly with the local psql client
  --compose                      use the running Compose postgres service
  --compose-file PATH            use postgres from this Compose file

Options:
  --mode quick|deep              default: quick
  --writers-stopped              required acknowledgement for deep mode
  --show-identities              print client/interface identities instead of aliases
  --connect-timeout-secs N       default: 10; range: 1..60
  --lock-timeout-ms N            default: 2000; range: 100..60000
  --statement-timeout-ms N       default: 30000 quick, 900000 deep; range: 1000..3600000
  -h, --help

Deep mode disables parallel gather and caps PostgreSQL temporary files at
256 MiB per backend. A restricted login needs membership in pg_read_all_stats
to inspect other roles' transaction ages and SET privilege on temp_file_limit.
The current release audit requires the exact successful migration range 0001
through 0020, exact 0017/0018/0019/0020 ledger descriptions and checksums, the 0017
suspension catalog, the 0016 streaming hourly-refresh function, the 0015 and
0018 index definitions, the 0019 fail-closed import-update trigger, and the
absence of the retired traffic_cycle_usage prototype table.
Migrations 0017 and 0019 are transactional metadata/function changes and do not
rewrite retained traffic rows. Migration 0018 builds one index concurrently.

Exit status:
  0  no hard findings (warnings, if any, are reported in audit_summary)
  1  connection, timeout, SQL, or malformed-output failure
  2  one or more hard invariant findings
  64 invalid command-line usage

The database URL, password, and anonymization salt are never printed. Direct
URL parsing uses a transient mode-0600 file removed immediately and by the EXIT
trap on ordinary failures. Raw identities are printed only when
--show-identities is explicitly selected.
EOF
}

usage_error() {
  printf 'traffic-ledger audit usage error: %s\n' "$*" >&2
  usage
  exit 64
}

runtime_error() {
  printf 'traffic-ledger audit failed: %s\n' "$*" >&2
  exit 1
}

require_tool() {
  command -v "$1" >/dev/null 2>&1 || runtime_error "missing required tool: $1"
}

validate_integer_range() {
  local name="$1" value="$2" minimum="$3" maximum="$4"
  [[ "$value" =~ ^[0-9]+$ ]] || usage_error "$name must be an integer"
  ((value >= minimum && value <= maximum)) ||
    usage_error "$name must be between $minimum and $maximum"
}

mode="quick"
writers_stopped=0
show_identities=0
use_compose=0
compose_file=""
connect_timeout_secs=10
lock_timeout_ms=2000
statement_timeout_ms=""

while (($#)); do
  case "$1" in
    --mode)
      (($# >= 2)) || usage_error "--mode requires a value"
      mode="$2"
      shift 2
      ;;
    --writers-stopped)
      writers_stopped=1
      shift
      ;;
    --show-identities)
      show_identities=1
      shift
      ;;
    --compose)
      use_compose=1
      shift
      ;;
    --compose-file)
      (($# >= 2)) || usage_error "--compose-file requires a path"
      use_compose=1
      compose_file="$2"
      shift 2
      ;;
    --connect-timeout-secs)
      (($# >= 2)) || usage_error "--connect-timeout-secs requires a value"
      connect_timeout_secs="$2"
      shift 2
      ;;
    --lock-timeout-ms)
      (($# >= 2)) || usage_error "--lock-timeout-ms requires a value"
      lock_timeout_ms="$2"
      shift 2
      ;;
    --statement-timeout-ms)
      (($# >= 2)) || usage_error "--statement-timeout-ms requires a value"
      statement_timeout_ms="$2"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    --)
      shift
      (($# == 0)) || usage_error "unexpected positional arguments"
      ;;
    -* | *)
      usage_error "unknown argument: $1"
      ;;
  esac
done

[[ "$mode" == "quick" || "$mode" == "deep" ]] ||
  usage_error "--mode must be quick or deep"
if [[ "$mode" == "deep" && "$writers_stopped" -ne 1 ]]; then
  usage_error "deep mode requires --writers-stopped"
fi
if [[ "$mode" == "quick" && "$writers_stopped" -eq 1 ]]; then
  printf 'traffic-ledger audit note: --writers-stopped is unnecessary in quick mode\n' >&2
fi

validate_integer_range "--connect-timeout-secs" "$connect_timeout_secs" 1 60
validate_integer_range "--lock-timeout-ms" "$lock_timeout_ms" 100 60000
if [[ -z "$statement_timeout_ms" ]]; then
  if [[ "$mode" == "deep" ]]; then
    statement_timeout_ms=900000
  else
    statement_timeout_ms=30000
  fi
fi
validate_integer_range "--statement-timeout-ms" "$statement_timeout_ms" 1000 3600000

[[ -r "$AUDIT_SQL" ]] || runtime_error "missing SQL program: scripts/sql/audit-postgres-traffic-ledger.sql"

require_tool awk
require_tool cat
require_tool chmod
require_tool mktemp
require_tool od
require_tool rm
require_tool tr

if [[ "$use_compose" -eq 1 ]]; then
  require_tool docker
  [[ -z "${VPSMAN_POSTGRES_URL:-}" ]] ||
    usage_error "do not combine --compose with VPSMAN_POSTGRES_URL"
  if [[ -n "$compose_file" && ! -f "$compose_file" ]]; then
    usage_error "Compose file does not exist: $compose_file"
  fi
else
  require_tool psql
  require_tool python3
  [[ -n "${VPSMAN_POSTGRES_URL:-}" ]] ||
    usage_error "set VPSMAN_POSTGRES_URL or select --compose"
fi

audit_tmp_dir="${TMPDIR:-/tmp}"
output_file=""
uri_parts_file=""
cleanup() {
  local temporary
  for temporary in "$output_file" "$uri_parts_file"; do
    [[ -n "$temporary" ]] || continue
    [[ -e "$temporary" || -L "$temporary" ]] || continue
    case "$temporary" in
      "$audit_tmp_dir"/vpsman-traffic-ledger-audit.* | \
        "$audit_tmp_dir"/vpsman-postgres-uri-parts.*)
        rm -- "$temporary"
        ;;
      *)
        printf 'refusing to clean unexpected audit temporary path: %s\n' \
          "$temporary" >&2
        ;;
    esac
  done
}
trap cleanup EXIT

pg_host_present=0
pg_host=""
pg_port_present=0
pg_port=""
pg_user_present=0
pg_user=""
pg_password_present=0
pg_password=""
pg_database=""
pg_sslmode=""
pg_sslrootcert=""
pg_sslcert=""
pg_sslkey=""
pg_sslcrl=""
pg_sslpassword=""
pg_channel_binding=""
pg_target_session_attrs=""
pg_hostaddr=""
pg_gssencmode=""
pg_keepalives=""
pg_keepalives_idle=""
pg_keepalives_interval=""
pg_keepalives_count=""
pg_tcp_user_timeout=""
pg_client_encoding=""
pg_passfile=""
pg_load_balance_hosts=""

if [[ "$use_compose" -eq 0 ]]; then
  uri_parts_file="$(mktemp "$audit_tmp_dir/vpsman-postgres-uri-parts.XXXXXX")"
  chmod 0600 "$uri_parts_file"
  if ! VPSMAN_POSTGRES_URL="$VPSMAN_POSTGRES_URL" python3 - \
    >"$uri_parts_file" <<'PY'
import os
import sys
from urllib.parse import parse_qsl, unquote, urlsplit


def fail(code: str) -> None:
    print(f"traffic-ledger audit URI parse failed: {code}", file=sys.stderr)
    raise SystemExit(2)


raw = os.environ.get("VPSMAN_POSTGRES_URL", "")
if not raw or len(raw) > 131_072 or "\x00" in raw:
    fail("invalid_length")
try:
    parsed = urlsplit(raw)
except ValueError:
    fail("invalid_uri")
if parsed.scheme not in {"postgres", "postgresql"}:
    fail("unsupported_scheme")
if parsed.fragment:
    fail("fragment_not_supported")

try:
    authority_host = parsed.hostname
    authority_port = parsed.port
except ValueError:
    fail("invalid_authority")
if authority_host is not None and "," in authority_host:
    fail("authority_multi_host_not_supported")

try:
    pairs = parse_qsl(parsed.query, keep_blank_values=True, strict_parsing=True)
except ValueError:
    fail("invalid_query")
query: dict[str, str] = {}
for key, value in pairs:
    if key in query:
        fail("duplicate_query_parameter")
    query[key] = value

reserved = {
    "application_name",
    "connect_timeout",
    "fallback_application_name",
    "options",
    "service",
}
if reserved.intersection(query):
    fail("reserved_query_parameter")

environment_queries = {
    "channel_binding": "channel_binding",
    "client_encoding": "client_encoding",
    "gssencmode": "gssencmode",
    "hostaddr": "hostaddr",
    "keepalives": "keepalives",
    "keepalives_count": "keepalives_count",
    "keepalives_idle": "keepalives_idle",
    "keepalives_interval": "keepalives_interval",
    "load_balance_hosts": "load_balance_hosts",
    "passfile": "passfile",
    "sslcert": "sslcert",
    "sslcrl": "sslcrl",
    "sslkey": "sslkey",
    "sslmode": "sslmode",
    "sslpassword": "sslpassword",
    "sslrootcert": "sslrootcert",
    "target_session_attrs": "target_session_attrs",
    "tcp_user_timeout": "tcp_user_timeout",
}
core_keys = {"dbname", "host", "password", "port", "user"}
unknown = set(query).difference(environment_queries, core_keys)
if unknown:
    fail("unsupported_query_parameter")

authority_user = unquote(parsed.username) if parsed.username is not None else None
authority_password = unquote(parsed.password) if parsed.password is not None else None
authority_database = unquote(parsed.path[1:]) if parsed.path.startswith("/") else None
if parsed.path and authority_database is None:
    fail("invalid_database_path")

host = query.get("host", unquote(authority_host) if authority_host is not None else None)
port = query.get("port", str(authority_port) if authority_port is not None else None)
user = query.get("user", authority_user)
password = query.get("password", authority_password)
database = query.get("dbname", authority_database or "")

values = [
    "1" if host is not None else "0",
    host or "",
    "1" if port is not None else "0",
    port or "",
    "1" if user is not None else "0",
    user or "",
    "1" if password is not None else "0",
    password or "",
    database,
]
values.extend(query.get(key, "") for key in environment_queries)
if any("\x00" in value for value in values):
    fail("nul_in_parameter")
for value in values:
    sys.stdout.buffer.write(value.encode("utf-8") + b"\x00")
PY
  then
    runtime_error "VPSMAN_POSTGRES_URL could not be parsed securely"
  fi
  mapfile -d '' -t pg_uri_parts <"$uri_parts_file"
  [[ "${#pg_uri_parts[@]}" -eq 27 ]] ||
    runtime_error "secure PostgreSQL URI parser returned an invalid field count"
  pg_host_present="${pg_uri_parts[0]}"
  pg_host="${pg_uri_parts[1]}"
  pg_port_present="${pg_uri_parts[2]}"
  pg_port="${pg_uri_parts[3]}"
  pg_user_present="${pg_uri_parts[4]}"
  pg_user="${pg_uri_parts[5]}"
  pg_password_present="${pg_uri_parts[6]}"
  pg_password="${pg_uri_parts[7]}"
  pg_database="${pg_uri_parts[8]}"
  pg_channel_binding="${pg_uri_parts[9]}"
  pg_client_encoding="${pg_uri_parts[10]}"
  pg_gssencmode="${pg_uri_parts[11]}"
  pg_hostaddr="${pg_uri_parts[12]}"
  pg_keepalives="${pg_uri_parts[13]}"
  pg_keepalives_count="${pg_uri_parts[14]}"
  pg_keepalives_idle="${pg_uri_parts[15]}"
  pg_keepalives_interval="${pg_uri_parts[16]}"
  pg_load_balance_hosts="${pg_uri_parts[17]}"
  pg_passfile="${pg_uri_parts[18]}"
  pg_sslcert="${pg_uri_parts[19]}"
  pg_sslcrl="${pg_uri_parts[20]}"
  pg_sslkey="${pg_uri_parts[21]}"
  pg_sslmode="${pg_uri_parts[22]}"
  pg_sslpassword="${pg_uri_parts[23]}"
  pg_sslrootcert="${pg_uri_parts[24]}"
  pg_target_session_attrs="${pg_uri_parts[25]}"
  pg_tcp_user_timeout="${pg_uri_parts[26]}"
  rm -- "$uri_parts_file"
  uri_parts_file=""
fi

identity_salt="${VPSMAN_AUDIT_IDENTITY_SALT:-}"
if [[ -z "$identity_salt" ]]; then
  identity_salt="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
fi
[[ "$identity_salt" =~ ^[0-9a-fA-F]{64}$ ]] ||
  runtime_error "VPSMAN_AUDIT_IDENTITY_SALT must be exactly 32 bytes of hexadecimal"

audit_deep="false"
[[ "$mode" == "deep" ]] && audit_deep="true"
audit_show_identities="false"
[[ "$show_identities" -eq 1 ]] && audit_show_identities="true"
audit_lock_timeout="${lock_timeout_ms}ms"
audit_statement_timeout="${statement_timeout_ms}ms"
audit_idle_timeout="$((statement_timeout_ms + 60000))ms"

output_file="$(mktemp "$audit_tmp_dir/vpsman-traffic-ledger-audit.XXXXXX")"

psql_status=0
if [[ "$use_compose" -eq 1 ]]; then
  compose_command=(docker compose)
  if [[ -n "$compose_file" ]]; then
    compose_command+=(-f "$compose_file")
  fi
  if ! "${compose_command[@]}" config --services 2>/dev/null |
    awk '$0 == "postgres" { found = 1 } END { exit(found ? 0 : 1) }'; then
    runtime_error "the selected Compose project has no postgres service"
  fi
  # Expansion is intentionally deferred to the postgres container.
  # shellcheck disable=SC2016
  if LC_ALL=C "${compose_command[@]}" exec -T \
    -e "VPSMAN_AUDIT_MODE=$mode" \
    -e "VPSMAN_AUDIT_DEEP=$audit_deep" \
    -e "VPSMAN_AUDIT_SHOW_IDENTITIES=$audit_show_identities" \
    -e "VPSMAN_AUDIT_LOCK_TIMEOUT=$audit_lock_timeout" \
    -e "VPSMAN_AUDIT_STATEMENT_TIMEOUT=$audit_statement_timeout" \
    -e "VPSMAN_AUDIT_IDLE_TIMEOUT=$audit_idle_timeout" \
    -e "PGCONNECT_TIMEOUT=$connect_timeout_secs" \
    -e "PGAPPNAME=vpsman-traffic-ledger-audit" \
    -e "PGOPTIONS=-c default_transaction_read_only=on" \
    postgres sh -ec '
      exec psql -X -q -w -A -t --field-separator="$(printf "\t")" \
        --username="$POSTGRES_USER" --dbname="$POSTGRES_DB" \
        -v ON_ERROR_STOP=1 \
        -v audit_mode="$VPSMAN_AUDIT_MODE" \
        -v audit_deep="$VPSMAN_AUDIT_DEEP" \
        -v audit_show_identities="$VPSMAN_AUDIT_SHOW_IDENTITIES" \
        -v audit_lock_timeout="$VPSMAN_AUDIT_LOCK_TIMEOUT" \
        -v audit_statement_timeout="$VPSMAN_AUDIT_STATEMENT_TIMEOUT" \
        -v audit_idle_timeout="$VPSMAN_AUDIT_IDLE_TIMEOUT"
    ' < <(
      printf '\\set audit_identity_salt %s\n' "$identity_salt"
      cat "$AUDIT_SQL"
    ) >"$output_file"; then
    :
  else
    psql_status="$?"
  fi
else
  if (
    # libpq does not expand a URI placed in PGDATABASE on every supported
    # client build. Keep the original URI out of psql argv and split it into
    # ordinary libpq environment fields in the parent shell instead.
    unset VPSMAN_POSTGRES_URL
    if [[ "$pg_host_present" == "1" ]]; then
      export PGHOST="$pg_host"
    else
      unset PGHOST
    fi
    if [[ "$pg_port_present" == "1" ]]; then
      export PGPORT="$pg_port"
    else
      unset PGPORT
    fi
    if [[ "$pg_user_present" == "1" ]]; then
      export PGUSER="$pg_user"
    else
      unset PGUSER
    fi
    if [[ "$pg_password_present" == "1" ]]; then
      export PGPASSWORD="$pg_password"
    fi
    export PGDATABASE="$pg_database"
    [[ -z "$pg_channel_binding" ]] || export PGCHANNELBINDING="$pg_channel_binding"
    [[ -z "$pg_client_encoding" ]] || export PGCLIENTENCODING="$pg_client_encoding"
    [[ -z "$pg_gssencmode" ]] || export PGGSSENCMODE="$pg_gssencmode"
    [[ -z "$pg_hostaddr" ]] || export PGHOSTADDR="$pg_hostaddr"
    [[ -z "$pg_keepalives" ]] || export PGKEEPALIVES="$pg_keepalives"
    [[ -z "$pg_keepalives_count" ]] || export PGKEEPALIVESCOUNT="$pg_keepalives_count"
    [[ -z "$pg_keepalives_idle" ]] || export PGKEEPALIVESIDLE="$pg_keepalives_idle"
    [[ -z "$pg_keepalives_interval" ]] || export PGKEEPALIVESINTERVAL="$pg_keepalives_interval"
    [[ -z "$pg_load_balance_hosts" ]] || export PGLOADBALANCEHOSTS="$pg_load_balance_hosts"
    [[ -z "$pg_passfile" ]] || export PGPASSFILE="$pg_passfile"
    [[ -z "$pg_sslcert" ]] || export PGSSLCERT="$pg_sslcert"
    [[ -z "$pg_sslcrl" ]] || export PGSSLCRL="$pg_sslcrl"
    [[ -z "$pg_sslkey" ]] || export PGSSLKEY="$pg_sslkey"
    [[ -z "$pg_sslmode" ]] || export PGSSLMODE="$pg_sslmode"
    [[ -z "$pg_sslpassword" ]] || export PGSSLPASSWORD="$pg_sslpassword"
    [[ -z "$pg_sslrootcert" ]] || export PGSSLROOTCERT="$pg_sslrootcert"
    [[ -z "$pg_target_session_attrs" ]] ||
      export PGTARGETSESSIONATTRS="$pg_target_session_attrs"
    [[ -z "$pg_tcp_user_timeout" ]] || export PGTCPUSER_TIMEOUT="$pg_tcp_user_timeout"
    export LC_ALL=C
    export PGCONNECT_TIMEOUT="$connect_timeout_secs"
    export PGAPPNAME="vpsman-traffic-ledger-audit"
    export PGOPTIONS="-c default_transaction_read_only=on"
    psql -X -q -w -A -t --field-separator=$'\t' \
      -v ON_ERROR_STOP=1 \
      -v "audit_mode=$mode" \
      -v "audit_deep=$audit_deep" \
      -v "audit_show_identities=$audit_show_identities" \
      -v "audit_lock_timeout=$audit_lock_timeout" \
      -v "audit_statement_timeout=$audit_statement_timeout" \
      -v "audit_idle_timeout=$audit_idle_timeout" \
      < <(
        printf '\\set audit_identity_salt %s\n' "$identity_salt"
        cat "$AUDIT_SQL"
      )
  ) >"$output_file"; then
    :
  else
    psql_status="$?"
  fi
fi

if [[ "$psql_status" -ne 0 ]]; then
  runtime_error "PostgreSQL audit query failed (psql status $psql_status)"
fi

summary_counts="$(awk -F '\t' '
  BEGIN {
    info = 0
    warning = 0
    hard = 0
    rows = 0
    invalid = 0
  }
  {
    rows++
    if (NF != 4 || ($1 != "INFO" && $1 != "WARN" && $1 != "HARD") ||
        $2 !~ /^[a-z0-9_]+$/ || $3 !~ /^[0-9]+$/ ||
        $4 !~ /^\{.*\}$/) {
      invalid = 1
      next
    }
    if ($1 == "INFO") info += $3
    if ($1 == "WARN") warning += $3
    if ($1 == "HARD") hard += $3
  }
  END {
    if (invalid || rows == 0) exit 65
    printf "%.0f\t%.0f\t%.0f\t%.0f", info, warning, hard, rows
  }
' "$output_file")" || runtime_error "PostgreSQL audit produced malformed output"

IFS=$'\t' read -r info_count warning_count hard_count check_count <<<"$summary_counts"
cat "$output_file"

summary_severity="INFO"
if [[ "$warning_count" != "0" ]]; then
  summary_severity="WARN"
fi
if [[ "$hard_count" != "0" ]]; then
  summary_severity="HARD"
fi
printf '%s\taudit_summary\t%s\t{"mode":"%s","check_rows":%s,"info_count":%s,"warning_count":%s,"hard_count":%s,"identities":"%s"}\n' \
  "$summary_severity" \
  "$hard_count" \
  "$mode" \
  "$check_count" \
  "$info_count" \
  "$warning_count" \
  "$hard_count" \
  "$([[ "$show_identities" -eq 1 ]] && printf 'shown' || printf 'pseudonymized')"

if [[ "$hard_count" != "0" ]]; then
  exit 2
fi
