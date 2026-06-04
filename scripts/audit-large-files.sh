#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools find grep wc

notes_file="docs/large-file-split-notes.md"
if [[ ! -f "$notes_file" ]]; then
  echo "large-file audit failed: missing $notes_file" >&2
  exit 1
fi

is_scanned_file() {
  case "$1" in
    ./.git/*|./target/*|./frontend/node_modules/*|./frontend/dist/*|./frontend/test-results/*|./frontend/playwright-report/*|./.tmp/*)
      return 1
      ;;
  esac
  grep -Iq . "$1" || [[ ! -s "$1" ]]
}

is_line_counted_source() {
  case "$1" in
    ./crates/*.rs|./crates/**/*.rs|./frontend/src/*.ts|./frontend/src/**/*.ts|./frontend/src/*.tsx|./frontend/src/**/*.tsx|./frontend/src/*.css|./frontend/src/**/*.css|./frontend/tests/*.ts|./frontend/tests/**/*.ts|./scripts/*.sh)
      return 0
      ;;
  esac
  return 1
}

failures=0
large_files=0
while IFS= read -r -d '' file; do
  is_scanned_file "$file" || continue
  is_line_counted_source "$file" || continue
  line_count="$(wc -l <"$file")"
  if (( line_count <= 1000 )); then
    continue
  fi
  large_files=$((large_files + 1))
  if ! grep -Fq "\`$file\`" "$notes_file"; then
    echo "large-file audit violation: $file ($line_count lines) is missing from $notes_file" >&2
    failures=$((failures + 1))
  fi
done < <(find . -type f -print0)

while IFS= read -r documented; do
  [[ -n "$documented" ]] || continue
  if [[ ! -f "$documented" ]]; then
    echo "large-file audit violation: documented file does not exist: $documented" >&2
    failures=$((failures + 1))
    continue
  fi
  line_count="$(wc -l <"$documented")"
  if (( line_count <= 1000 )); then
    echo "large-file audit violation: documented file is no longer above 1000 lines and should be removed from notes: $documented" >&2
    failures=$((failures + 1))
  fi
done < <(grep -oE '`./[^`]+`' "$notes_file" | tr -d '`' | sort -u)

if (( failures > 0 )); then
  echo "large_file_audit=failed failures=$failures large_files=$large_files" >&2
  exit 1
fi

echo "large_file_audit=ok large_files=$large_files notes=$notes_file"
