#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

if ! command -v rg >/dev/null 2>&1; then
  printf 'missing required tool: rg\n' >&2
  exit 1
fi

scan_terms=(
  '/bin/'
  '/sbin/'
  '/usr/bin/'
  '/usr/sbin/'
  '/etc/'
  '/proc'
  '/sys/class'
  'vnstat'
  'ping'
)

matches="$(rg -n --no-heading \
  -e '/bin/' \
  -e '/sbin/' \
  -e '/usr/bin/' \
  -e '/usr/sbin/' \
  -e '/etc/' \
  -e '/proc(/|$|[^[:alnum:]_-])' \
  -e '/sys/class' \
  -e 'vnstat' \
  -e '\bping\b' \
  crates frontend/src scripts docs README.md DESIGN.md 2>/dev/null || true)"

classified_pattern='#!|preset|config|argv|source|adapter|definition|default|fixture|test|smoke|placeholder|managed|platform_accounts|initd_dir|generate_frontend_contracts|file_transfer|vpsman-tunnels|vpsman-ospf|docs/|README.md|DESIGN.md|customizability|render_data_source_hot_config'
classified="$(printf '%s\n' "$matches" | rg -i "$classified_pattern" || true)"
open_candidates="$(printf '%s\n' "$matches" | rg -v -i "$classified_pattern" || true)"

count_lines() {
  if [[ -z "$1" ]]; then
    printf '0'
  else
    printf '%s\n' "$1" | wc -l | tr -d ' '
  fi
}

total_count="$(count_lines "$matches")"
classified_count="$(count_lines "$classified")"
open_count="$(count_lines "$open_candidates")"

printf '{\n'
printf '  "customizability_audit": "ok",\n'
printf '  "total_matches": %s,\n' "$total_count"
printf '  "classified_matches": %s,\n' "$classified_count"
printf '  "open_candidates": %s,\n' "$open_count"
printf '  "scan_terms": ['
for index in "${!scan_terms[@]}"; do
  [[ "$index" == "0" ]] || printf ', '
  printf '"%s"' "${scan_terms[$index]}"
done
printf ']\n'
printf '}\n'

if [[ -n "$open_candidates" ]]; then
  printf '\nOpen candidates needing preset/config review:\n'
  printf '%s\n' "$open_candidates"
fi

if [[ "${VPSMAN_CUSTOMIZABILITY_AUDIT_FAIL_ON_OPEN:-0}" == "1" && "$open_count" != "0" ]]; then
  printf '\ncustomizability audit found open candidates\n' >&2
  exit 1
fi
