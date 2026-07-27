#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VITE_CLI="$ROOT_DIR/frontend/node_modules/vite/bin/vite.js"

fail() {
  printf 'frontend Vite provenance check failed: %s\n' "$*" >&2
  exit 1
}

[[ -f "$VITE_CLI" ]] ||
  fail "frontend dependencies are missing; run npm ci in frontend first"
command -v git >/dev/null 2>&1 ||
  fail "Git is required for the provenance containment regression"

test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT
project_root="$test_root/project"
frontend_root="$project_root/frontend"
mkdir -p "$frontend_root"
cp "$ROOT_DIR/frontend/vite.config.ts" "$frontend_root/vite.config.ts"
ln -s "$ROOT_DIR/frontend/node_modules" "$frontend_root/node_modules"
printf '%s\n' \
  '<!doctype html>' \
  '<script type="module" src="/provenance.js"></script>' \
  >"$frontend_root/index.html"
printf '%s\n' \
  'globalThis.__vpsmanProvenance = [' \
  '  __VPSMAN_SOURCE_COMMIT__,' \
  '  __VPSMAN_INSTALLER_ASSET_NAME__,' \
  '  __VPSMAN_INSTALLER_SHA256__,' \
  '  __VPSMAN_RELEASE_TAG__,' \
  '].join("|");' \
  >"$frontend_root/provenance.js"

clean_env=(
  env
  -u VPSMAN_SOURCE_COMMIT
  -u VPSMAN_RELEASE_TAG
  -u GITHUB_SHA
  -u GITHUB_REF_TYPE
  -u GITHUB_REF_NAME
)

unknown_dist="$test_root/dist-unknown"
(
  cd "$frontend_root"
  "${clean_env[@]}" node "$VITE_CLI" build \
    --emptyOutDir \
    --outDir "$unknown_dist"
) >/dev/null
if find "$unknown_dist" -maxdepth 1 -type f -name 'install-agent-*.sh' -print -quit |
  grep -q .
then
  fail "ordinary build without Git provenance emitted an installer asset"
fi

explicit_commit="0123456789abcdef0123456789abcdef01234567"
explicit_dist="$test_root/dist-explicit"
(
  cd "$frontend_root"
  "${clean_env[@]}" VPSMAN_SOURCE_COMMIT="$explicit_commit" \
    node "$VITE_CLI" build \
      --emptyOutDir \
      --outDir "$explicit_dist"
) >/dev/null
grep -R -a -F -q -- "$explicit_commit" "$explicit_dist/assets" ||
  fail "explicit source provenance was not embedded without a Git checkout"
if find "$explicit_dist" -maxdepth 1 -type f -name 'install-agent-*.sh' -print -quit |
  grep -q .
then
  fail "build without committed installer bytes emitted an installer asset"
fi

tagged_log="$test_root/tagged-build.log"
if (
  cd "$frontend_root"
  "${clean_env[@]}" \
    VPSMAN_RELEASE_TAG="v1.2.3" \
    VPSMAN_SOURCE_COMMIT="$explicit_commit" \
    node "$VITE_CLI" build \
      --emptyOutDir \
      --outDir "$test_root/dist-tagged"
) >"$tagged_log" 2>&1
then
  fail "tagged build succeeded without a full Git checkout"
fi
grep -F -q -- \
  "tagged frontend build requires v1.2.3 in a full Git checkout" \
  "$tagged_log" ||
  fail "tagged build did not report its strict Git provenance requirement"

mkdir -p "$test_root/deploy"
printf '%s\n' "unrelated parent repository" >"$test_root/unrelated.txt"
printf '%s\n' \
  '#!/usr/bin/env sh' \
  'echo unrelated-parent-installer' \
  >"$test_root/deploy/install-agent.sh"
git -C "$test_root" init --quiet
git -C "$test_root" add unrelated.txt deploy/install-agent.sh
git -C "$test_root" \
  -c user.name="vpsman provenance check" \
  -c user.email="provenance-check.invalid" \
  -c commit.gpgsign=false \
  commit --quiet -m "unrelated parent"
git -C "$test_root" -c tag.gpgSign=false tag v1.2.3
ancestor_commit="$(git -C "$test_root" rev-parse HEAD)"

nested_dist="$test_root/dist-nested-parent"
(
  cd "$frontend_root"
  "${clean_env[@]}" node "$VITE_CLI" build \
    --emptyOutDir \
    --outDir "$nested_dist"
) >/dev/null
if grep -R -a -F -q -- "$ancestor_commit" "$nested_dist/assets"; then
  fail "ordinary build trusted provenance from an unrelated parent repository"
fi
if find "$nested_dist" -maxdepth 1 -type f -name 'install-agent-*.sh' -print -quit |
  grep -q .
then
  fail "ordinary build emitted an installer from an unrelated parent repository"
fi

nested_tagged_log="$test_root/nested-tagged-build.log"
if (
  cd "$frontend_root"
  "${clean_env[@]}" \
    VPSMAN_RELEASE_TAG="v1.2.3" \
    VPSMAN_SOURCE_COMMIT="$ancestor_commit" \
    node "$VITE_CLI" build \
      --emptyOutDir \
      --outDir "$test_root/dist-nested-tagged"
) >"$nested_tagged_log" 2>&1
then
  fail "tagged build trusted a matching tag from an unrelated parent repository"
fi
grep -F -q -- \
  "tagged frontend build requires v1.2.3 in a full Git checkout" \
  "$nested_tagged_log" ||
  fail "nested tagged build did not report its strict Git provenance requirement"

printf '%s\n' '{"frontend_vite_provenance":"ok"}'
