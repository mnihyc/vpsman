#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

tutorial_dir="tutorials"
index_file="$tutorial_dir/README.md"

expected_files=(
  "00-operator-quickstart.md"
  "01-local-control-plane.md"
  "02-install-agents.md"
  "03-fleet-organization.md"
  "04-daily-operations.md"
  "05-source-templates.md"
  "06-tunnels-topology-bird2.md"
  "07-backup-restore-migration.md"
  "08-agent-updates.md"
  "09-headless-cli-vty.md"
)

fail() {
  printf 'tutorial_audit=failed reason=%q\n' "$1" >&2
  exit 1
}

[[ -d "$tutorial_dir" ]] || fail "missing tutorials directory"
[[ -f "$index_file" ]] || fail "missing tutorials README"
grep -q 'tutorials/README.md' README.md || fail "root README does not link tutorial index"
grep -q 'operator-facing' "$index_file" || fail "tutorial index must state operator-facing purpose"
grep -q 'VPSMAN_API_URL' "$index_file" || fail "tutorial index must document common environment"
grep -q 'VPSMAN_SUPER_PASSWORD' "$index_file" || fail "tutorial index must document local privilege environment"

for file in "${expected_files[@]}"; do
  path="$tutorial_dir/$file"
  [[ -f "$path" ]] || fail "missing tutorial $path"
  grep -q "$file" "$index_file" || fail "tutorial index does not list $file"

  line_count="$(wc -l < "$path" | tr -d ' ')"
  if (( line_count < 20 )); then
    fail "tutorial $path is too short to be actionable"
  fi

  case "$file" in
    00-operator-quickstart.md)
      grep -qi 'quickstart' "$path" || fail "$path must describe quickstart usage"
      ;;
    02-install-agents.md)
      grep -qi 'direct' "$path" || fail "$path must cover direct gateway identity"
      grep -qi 'noise' "$path" || fail "$path must cover Noise key provisioning"
      ;;
    05-source-templates.md)
      grep -qi 'template' "$path" || fail "$path must cover selectable source templates"
      grep -qi 'VPS-local' "$path" || fail "$path must cover VPS-local customization"
      ;;
    04-daily-operations.md)
      grep -qi 'record-table' "$path" || fail "$path must cover panel record-table controls"
      grep -qi 'filtered row' "$path" || fail "$path must cover filtered row counts"
      ;;
    06-tunnels-topology-bird2.md)
      grep -qi 'external' "$path" || fail "$path must cover external/imported tunnels"
      grep -qi 'Bird2' "$path" || fail "$path must cover Bird2/OSPF operations"
      ;;
    09-headless-cli-vty.md)
      grep -qi 'VTY' "$path" || fail "$path must cover headless VTY operations"
      grep -qi 'enable' "$path" || fail "$path must cover privileged mode"
      ;;
  esac
done

printf '{"tutorial_audit":"ok","tutorial_count":%d,"index":"%s"}\n' \
  "${#expected_files[@]}" "$index_file"
