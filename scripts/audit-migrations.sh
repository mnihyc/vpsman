#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MIGRATIONS_DIR="$ROOT_DIR/migrations"
NOTES_FILE="$ROOT_DIR/docs/migration-compatibility.md"

fail() {
  printf 'migration_audit=failed reason=%s\n' "$*" >&2
  exit 1
}

[[ -d "$MIGRATIONS_DIR" ]] || fail "missing migrations directory"
[[ -f "$NOTES_FILE" ]] || fail "missing docs/migration-compatibility.md"

mapfile -t files < <(find "$MIGRATIONS_DIR" -maxdepth 1 -type f -name '*.sql' -printf '%f\n' | sort)
[[ "${#files[@]}" -gt 0 ]] || fail "no migrations found"

expected=1
declare -A index_names=()
for file in "${files[@]}"; do
  if [[ ! "$file" =~ ^([0-9]{4})_[a-z0-9_]+\.sql$ ]]; then
    fail "invalid migration filename: $file"
  fi
  number="${BASH_REMATCH[1]}"
  expected_number="$(printf '%04d' "$expected")"
  [[ "$number" == "$expected_number" ]] ||
    fail "migration sequence gap: expected $expected_number but found $number in $file"

  path="$MIGRATIONS_DIR/$file"
  [[ -s "$path" ]] || fail "empty migration: $file"
  tail -c 1 "$path" | grep -q $'\n' || fail "migration lacks trailing newline: $file"
  grep -Fq "$file" "$NOTES_FILE" || fail "migration lacks compatibility note: $file"

  destructive_pattern='\b(DROP[[:space:]]+(TABLE|COLUMN|SCHEMA|DATABASE)|TRUNCATE[[:space:]]+TABLE|ALTER[[:space:]]+TABLE[[:space:]].+[[:space:]]DROP[[:space:]]+(TABLE|COLUMN|SCHEMA|DATABASE)[[:space:]])'
  mapfile -t destructive_lines < <(grep -Ei "$destructive_pattern" "$path" || true)
  if [[ "${#destructive_lines[@]}" -gt 0 ]]; then
    # 0012 is the reviewed one-owner cutover: the replacement policy fields and
    # unified episode rows are populated and constrained before these exact
    # retired stores are removed. Keep this allowlist statement-specific so a
    # later destructive edit to the same migration still fails this audit.
    reviewed_destructive=0
    if [[ "$file" == "0012_policy_owned_alerts_event_schedules.sql" &&
      "${#destructive_lines[@]}" -eq 3 ]]; then
      reviewed_destructive=1
      expected_destructive=(
        "    DROP COLUMN window_secs,"
        "DROP TABLE policy_alerts;"
        "DROP TABLE policy_rule_states;"
      )
      for index in "${!expected_destructive[@]}"; do
        if [[ "${destructive_lines[$index]}" != "${expected_destructive[$index]}" ]]; then
          reviewed_destructive=0
          break
        fi
      done
    elif [[ "$file" == "0020_retire_unused_traffic_cycle_usage.sql" &&
      "${#destructive_lines[@]}" -eq 1 ]]; then
      reviewed_destructive=1
      [[ "${destructive_lines[0]}" == "DROP TABLE IF EXISTS public.traffic_cycle_usage;" ]] ||
        reviewed_destructive=0
    fi
    [[ "$reviewed_destructive" -eq 1 ]] ||
      fail "destructive DDL requires an explicit clean-baseline decision: $file"
  fi
  if grep -Eiq 'ADD[[:space:]]+COLUMN[^;,\n]*NOT[[:space:]]+NULL' "$path" &&
    ! grep -Eiq 'ADD[[:space:]]+COLUMN[^;,\n]*NOT[[:space:]]+NULL[^;,\n]*DEFAULT' "$path"; then
    fail "ADD COLUMN NOT NULL must include DEFAULT for existing rows: $file"
  fi

  # PostgreSQL permits CONCURRENTLY and IF NOT EXISTS between INDEX and the
  # name. Keep those clauses in the match so the captured final field is the
  # actual index identifier (rather than the word CONCURRENTLY).
  index_create_pattern='CREATE[[:space:]]+(UNIQUE[[:space:]]+)?INDEX([[:space:]]+CONCURRENTLY)?[[:space:]]+(IF[[:space:]]+NOT[[:space:]]+EXISTS[[:space:]]+)?([A-Za-z0-9_]+)'
  while IFS= read -r index_name; do
    [[ -n "$index_name" ]] || continue
    if [[ -n "${index_names[$index_name]:-}" ]]; then
      if ! grep -Eiq "DROP[[:space:]]+INDEX[[:space:]]+(IF[[:space:]]+EXISTS[[:space:]]+)?${index_name}[[:space:]]*;" "$path"; then
        fail "duplicate index name $index_name in $file and ${index_names[$index_name]}"
      fi
    fi
    index_names[$index_name]="$file"
  done < <(
    grep -Eio "$index_create_pattern" "$path" |
      awk '{print $NF}'
  )

  expected=$((expected + 1))
done

printf '{"migration_audit":"ok","model":"fresh_database_canonical","component_scope":"current_repository_components_only","migration_count":%d,"latest":"%s","compatibility_notes":"docs/migration-compatibility.md"}\n' \
  "${#files[@]}" "${files[-1]}"
