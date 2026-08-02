#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools find git grep

failures=0
checked_files=0
hygiene_user="${VPSMAN_REPO_HYGIENE_USER:-${USER:-}}"
case "$hygiene_user" in
  ""|runner|github|actions|ubuntu|root)
    hygiene_user=""
    ;;
esac
canonical_repo_slug="${VPSMAN_REPO_HYGIENE_CANONICAL_REPO:-}"
if [[ -z "$canonical_repo_slug" ]]; then
  remote_url="$(git config --get remote.origin.url 2>/dev/null || true)"
  case "$remote_url" in
    git@github.com:*)
      canonical_repo_slug="${remote_url#git@github.com:}"
      ;;
    ssh://git@github.com/*)
      canonical_repo_slug="${remote_url#ssh://git@github.com/}"
      ;;
    https://github.com/*)
      canonical_repo_slug="${remote_url#https://github.com/}"
      ;;
    http://github.com/*)
      canonical_repo_slug="${remote_url#http://github.com/}"
      ;;
    *github.com:*)
      canonical_repo_slug="${remote_url##*github.com:}"
      ;;
    *github.com/*)
      canonical_repo_slug="${remote_url##*github.com/}"
      ;;
  esac
fi
canonical_repo_slug="${canonical_repo_slug%.git}"
case "$canonical_repo_slug" in
  */*) ;;
  *) canonical_repo_slug="" ;;
esac
canonical_repo_slug_escaped="${canonical_repo_slug//\//\\/}"

is_scanned_file() {
  case "$1" in
    ./.git/*|./target/*|./deploy/runtime/*|./frontend/tmp/*|./frontend/node_modules/*|./frontend/dist/*|./frontend/test-results/*|./frontend/playwright-report/*|./.tmp/*)
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
  local filtered=""
  local line
  local line_content
  if [[ -z "$needle" ]]; then
    return
  fi
  output="$(grep -InF "$needle" "$file" 2>/dev/null || true)"
  if [[ -z "$output" ]]; then
    return
  fi
  while IFS= read -r line; do
    line_content="${line#*:}"
    case "$file" in
      ./Cargo.toml)
        [[ "$line_content" == "authors = [\"$needle\"]" ]] && continue
        ;;
      ./frontend/package.json)
        [[ "$line_content" == "  \"author\": \"$needle\"," ]] && continue
        ;;
      ./LICENSE-MIT)
        [[ "$line_content" == "Copyright (c) "*"$needle" ]] && continue
        ;;
    esac
    if [[ -n "$canonical_repo_slug" ]] &&
      { [[ "$line" == *"$canonical_repo_slug"* ]] || [[ "$line" == *"$canonical_repo_slug_escaped"* ]]; }; then
      continue
    fi
    filtered+="${line}"$'\n'
  done <<< "$output"
  if [[ -z "$filtered" ]]; then
    return
  fi
  echo "repo hygiene violation: $label in $file" >&2
  printf '%s' "$filtered" >&2
  failures=$((failures + 1))
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

done < <(
  if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git ls-files -z --cached --others --exclude-standard |
      while IFS= read -r -d '' file; do
        [[ -f "$file" ]] || continue
        printf './%s\0' "$file"
      done
  else
    find . \
      \( -path './.git' -o -path './target' -o -path './deploy/runtime' \
         -o -path './frontend/tmp' -o -path './frontend/node_modules' -o -path './frontend/dist' \
         -o -path './frontend/test-results' -o -path './frontend/playwright-report' -o -path './.tmp' \) \
      -prune -o -type f -print0
  fi
)

if (( checked_files == 0 )); then
  echo "repo hygiene violation: no files were scanned" >&2
  exit 1
fi

if (( failures > 0 )); then
  echo "repo_hygiene_scan=failed failures=$failures scanned_files=$checked_files" >&2
  exit 1
fi

echo "repo_hygiene_scan=ok scanned_files=$checked_files"
