#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

notes_file="docs/large-file-split-notes.md"
if [[ ! -f "$notes_file" ]]; then
  echo "large-file audit failed: missing $notes_file" >&2
  exit 1
fi

python3 - "$notes_file" <<'PY'
from pathlib import Path
import os
import re
import sys

notes_path = Path(sys.argv[1])
notes = notes_path.read_text(encoding='utf-8')
documented = set(re.findall(r'`(\./[^`]+)`', notes))
ignored_dirs = {
    '.git', 'target', '.tmp',
    'frontend/node_modules', 'frontend/dist',
    'frontend/test-results', 'frontend/playwright-report',
}
line_suffixes = {'.rs', '.ts', '.tsx', '.css', '.sh'}
source_roots = ('crates/', 'frontend/src/', 'frontend/tests/', 'scripts/')


def is_ignored_dir(rel: str) -> bool:
    return any(rel == item or rel.startswith(item + '/') for item in ignored_dirs)


def is_source(rel: str, path: Path) -> bool:
    return rel.startswith(source_roots) and path.suffix in line_suffixes

recommendations = 0
large_files = []
for dirpath, dirnames, filenames in os.walk('.'):
    rel_dir = dirpath[2:] if dirpath.startswith('./') else dirpath
    dirnames[:] = [
        dirname for dirname in dirnames
        if not is_ignored_dir((Path(rel_dir) / dirname).as_posix().lstrip('./'))
    ]
    for filename in filenames:
        path = Path(dirpath) / filename
        rel_no_dot = path.as_posix()[2:] if path.as_posix().startswith('./') else path.as_posix()
        if not is_source(rel_no_dot, path):
            continue
        try:
            line_count = sum(1 for _ in path.open('rb'))
        except OSError as exc:
            print(f"large-file audit failed reading ./{rel_no_dot}: {exc}", file=sys.stderr)
            recommendations += 1
            continue
        if line_count <= 1000:
            continue
        rel = f"./{rel_no_dot}"
        large_files.append(rel)
        if rel not in documented:
            print(
                f"large-file audit recommendation: document split direction for {rel} ({line_count} lines) in {notes_path}",
                file=sys.stderr,
            )
            recommendations += 1

for rel in sorted(documented):
    path = Path(rel[2:])
    if not path.is_file():
        print(f"large-file audit recommendation: remove missing documented file from notes: {rel}", file=sys.stderr)
        recommendations += 1
        continue
    line_count = sum(1 for _ in path.open('rb'))
    if line_count <= 1000:
        print(
            f"large-file audit recommendation: documented file is no longer above 1000 lines and should be removed from notes: {rel}",
            file=sys.stderr,
        )
        recommendations += 1

print(f"large_file_audit=ok recommendations={recommendations} large_files={len(large_files)} notes={notes_path}")
PY
