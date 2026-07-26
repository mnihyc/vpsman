#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MIGRATIONS_DIR="$ROOT_DIR/migrations"
NOTES_FILE="$ROOT_DIR/docs/migration-compatibility.md"
RELEASED_LEDGER="$MIGRATIONS_DIR/released-checksums.sha384"

fail() {
  printf 'migration_audit=failed reason=%s\n' "$*" >&2
  exit 1
}

[[ -d "$MIGRATIONS_DIR" ]] || fail "missing migrations directory"
[[ -f "$NOTES_FILE" ]] || fail "missing docs/migration-compatibility.md"
[[ -f "$RELEASED_LEDGER" ]] || fail "missing released migration checksum ledger"
command -v sha384sum >/dev/null 2>&1 || fail "sha384sum is required"

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

release_floor_tag="$(
  sed -n 's/^# release-tag: \([^[:space:]]\+\)$/\1/p' "$RELEASED_LEDGER"
)"
[[ "$release_floor_tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  fail "released checksum ledger must declare one semantic # release-tag"

declare -A released_checksums=()
released_count=0
while read -r checksum file extra; do
  [[ -n "${checksum:-}" ]] || continue
  [[ "$checksum" == \#* ]] && continue
  [[ -z "${extra:-}" ]] || fail "malformed released checksum row for $file"
  [[ "$checksum" =~ ^[0-9a-f]{96}$ ]] ||
    fail "invalid SHA-384 checksum for $file"
  [[ "$file" =~ ^[0-9]{4}_[a-z0-9_]+\.sql$ ]] ||
    fail "invalid released migration filename: $file"
  [[ -z "${released_checksums[$file]:-}" ]] ||
    fail "duplicate released migration ledger entry: $file"
  [[ -f "$MIGRATIONS_DIR/$file" ]] ||
    fail "released migration was deleted or renamed: $file"

  actual_checksum="$(sha384sum "$MIGRATIONS_DIR/$file" | awk '{print $1}')"
  [[ "$actual_checksum" == "$checksum" ]] ||
    fail "released migration bytes changed: $file"
  released_checksums[$file]="$checksum"
  released_count=$((released_count + 1))
done < "$RELEASED_LEDGER"
[[ "$released_count" -gt 0 ]] || fail "released migration checksum ledger is empty"

tag_verified="unavailable"
if command -v git >/dev/null 2>&1 &&
  git -C "$ROOT_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  latest_release_tag="$(
    git -C "$ROOT_DIR" tag --merged HEAD --list 'v[0-9]*.[0-9]*.[0-9]*' \
      --sort=-version:refname |
      head -n 1
  )"
  [[ -n "$latest_release_tag" ]] ||
    fail "no reachable release tag; fetch tags before auditing released migrations"
  git -C "$ROOT_DIR" rev-parse --verify --quiet "$release_floor_tag^{commit}" >/dev/null ||
    fail "release-tag floor $release_floor_tag is unavailable; fetch release tags"
  git -C "$ROOT_DIR" merge-base --is-ancestor "$release_floor_tag" "$latest_release_tag" ||
    fail "latest release tag $latest_release_tag predates ledger floor $release_floor_tag"

  tag_file_count=0
  while IFS= read -r tag_file; do
    [[ -n "$tag_file" ]] || continue
    [[ -n "${released_checksums[$tag_file]:-}" ]] ||
      fail "released migration missing checksum ledger entry: $tag_file ($latest_release_tag)"
    tag_checksum="$(
      git -C "$ROOT_DIR" show "$latest_release_tag:migrations/$tag_file" |
        sha384sum |
        awk '{print $1}'
    )"
    [[ "$tag_checksum" == "${released_checksums[$tag_file]}" ]] ||
      fail "checksum ledger does not match $latest_release_tag: $tag_file"
    tag_file_count=$((tag_file_count + 1))
  done < <(
    git -C "$ROOT_DIR" ls-tree --name-only "$latest_release_tag:migrations" |
      grep -E '^[0-9]{4}_[a-z0-9_]+\.sql$'
  )
  [[ "$tag_file_count" -gt 0 ]] ||
    fail "release tag $latest_release_tag contains no migrations"
  tag_verified="$latest_release_tag"
elif [[ "${VPSMAN_REQUIRE_RELEASE_TAGS:-0}" == "1" ]]; then
  fail "release-tag verification requested but git metadata is unavailable"
fi

printf '{"migration_audit":"ok","migration_count":%d,"latest":"%s","released_count":%d,"release_tag":"%s","compatibility_notes":"docs/migration-compatibility.md"}\n' \
  "${#files[@]}" "${files[-1]}" "$released_count" "$tag_verified"
