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

  if grep -Eiq '\b(DROP[[:space:]]+(TABLE|COLUMN|SCHEMA|DATABASE)|TRUNCATE[[:space:]]+TABLE|ALTER[[:space:]]+TABLE[[:space:]].+[[:space:]]DROP[[:space:]]+(TABLE|COLUMN|SCHEMA|DATABASE)[[:space:]])' "$path"; then
    fail "destructive DDL requires explicit migration policy before release: $file"
  fi
  if grep -Eiq 'ADD[[:space:]]+COLUMN[^;,\n]*NOT[[:space:]]+NULL' "$path" &&
    ! grep -Eiq 'ADD[[:space:]]+COLUMN[^;,\n]*NOT[[:space:]]+NULL[^;,\n]*DEFAULT' "$path"; then
    fail "ADD COLUMN NOT NULL must include DEFAULT for existing rows: $file"
  fi

  while IFS= read -r index_name; do
    [[ -n "$index_name" ]] || continue
    if [[ -n "${index_names[$index_name]:-}" ]]; then
      fail "duplicate index name $index_name in $file and ${index_names[$index_name]}"
    fi
    index_names[$index_name]="$file"
  done < <(
    grep -Eio 'CREATE[[:space:]]+(UNIQUE[[:space:]]+)?INDEX[[:space:]]+([A-Za-z0-9_]+)' "$path" |
      awk '{print $NF}'
  )

  expected=$((expected + 1))
done

printf '{"migration_audit":"ok","migration_count":%d,"latest":"%s","compatibility_notes":"docs/migration-compatibility.md"}\n' \
  "${#files[@]}" "${files[-1]}"
