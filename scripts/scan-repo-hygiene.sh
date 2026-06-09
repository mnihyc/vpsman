#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools find grep wc

failures=0
checked_files=0
max_source_lines=0
max_source_file=""
split_recommendations=0
hygiene_user="${VPSMAN_REPO_HYGIENE_USER:-${USER:-}}"
case "$hygiene_user" in
  ""|runner|github|actions|ubuntu|root)
    hygiene_user=""
    ;;
esac

is_scanned_file() {
  case "$1" in
    ./.git/*|./target/*|./frontend/node_modules/*|./frontend/dist/*|./frontend/test-results/*|./frontend/playwright-report/*|./.tmp/*)
      return 1
      ;;
  esac
  grep -Iq . "$1" || [[ ! -s "$1" ]]
}

report_match() {
  local label="$1"
  local file="$2"
  local pattern="$3"
  local output
  if output="$(grep -InE "$pattern" "$file" 2>/dev/null)"; then
    echo "repo hygiene violation: $label in $file" >&2
    printf '%s\n' "$output" >&2
    failures=$((failures + 1))
  fi
}

report_fixed_match() {
  local label="$1"
  local file="$2"
  local needle="$3"
  local output
  if [[ -z "$needle" ]]; then
    return
  fi
  if output="$(grep -InF "$needle" "$file" 2>/dev/null)"; then
    echo "repo hygiene violation: $label in $file" >&2
    printf '%s\n' "$output" >&2
    failures=$((failures + 1))
  fi
}

is_line_counted_source() {
  case "$1" in
    ./crates/*.rs|./crates/**/*.rs|./frontend/src/*.ts|./frontend/src/**/*.ts|./frontend/src/*.tsx|./frontend/src/**/*.tsx|./frontend/src/*.css|./frontend/src/**/*.css|./frontend/tests/*.ts|./frontend/tests/**/*.ts|./scripts/*.sh)
      return 0
      ;;
  esac
  return 1
}

source_line_hard_limit() {
  case "$1" in
    ./frontend/tests/*|./frontend/tests/**/*|./crates/*/tests/*|./crates/*/tests/**/*|./crates/*/src/tests.rs|./crates/*/src/tests_*.rs|./crates/*/src/*_tests.rs)
      echo 5000
      ;;
    ./crates/vpsctl/src/vty*.rs|./crates/vpsctl/src/commands_*.rs)
      echo 5000
      ;;
    *)
      echo 2000
      ;;
  esac
}

while IFS= read -r -d '' file; do
  is_scanned_file "$file" || continue
  checked_files=$((checked_files + 1))

  case "$file" in
    */.env.example|./deploy/.env.example)
      ;;
    */.env|*/.env.*|.env|.env.*)
      echo "repo hygiene violation: non-example env file is present: $file" >&2
      failures=$((failures + 1))
      ;;
  esac

  home_path_pattern='/'"home"'/[A-Za-z0-9._-]+/'
  report_match "user-specific home path" "$file" "$home_path_pattern"
  report_fixed_match "known local username" "$file" "$hygiene_user"
  report_match "private-key block" "$file" '-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----'
  report_match "OpenAI API key" "$file" '(^|[^[:alnum:]_])sk-[A-Za-z0-9_-]{20,}'
  report_match "GitHub token" "$file" '(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}'
  report_match "Slack token" "$file" 'xox[abprs]-[A-Za-z0-9-]{20,}'
  report_match "AWS access key id" "$file" 'A(KIA|SIA)[A-Z0-9]{16}'

  if is_line_counted_source "$file"; then
    line_count="$(wc -l <"$file")"
    if (( line_count > max_source_lines )); then
      max_source_lines="$line_count"
      max_source_file="$file"
    fi
    if (( line_count > 1000 )); then
      echo "repo hygiene recommendation: split or justify large source file over 1000 lines: $file ($line_count)" >&2
      split_recommendations=$((split_recommendations + 1))
    fi
    hard_limit="$(source_line_hard_limit "$file")"
    if (( line_count > hard_limit )); then
      echo "repo hygiene violation: source file exceeds hard role-based line limit $hard_limit: $file ($line_count)" >&2
      failures=$((failures + 1))
    fi
  fi
done < <(find . -type f -print0)

if (( checked_files == 0 )); then
  echo "repo hygiene violation: no files were scanned" >&2
  exit 1
fi

if (( failures > 0 )); then
  echo "repo_hygiene_scan=failed failures=$failures scanned_files=$checked_files" >&2
  exit 1
fi

echo "repo_hygiene_scan=ok scanned_files=$checked_files max_source_lines=$max_source_lines max_source_file=$max_source_file split_recommendations=$split_recommendations"
