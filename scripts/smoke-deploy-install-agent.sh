#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools awk bash chmod cp curl find flock grep id jq ln mktemp sha256sum stat tar
smoke_init_tmpdir "vpsman-deploy-install-agent"

fake_bin_dir="$SMOKE_TMPDIR/bin"
no_systemctl_bin="$SMOKE_TMPDIR/no-systemctl-bin"
missing_curl_bin="$SMOKE_TMPDIR/missing-curl-bin"
missing_sha_bin="$SMOKE_TMPDIR/missing-sha-bin"
fail_mv_bin="$SMOKE_TMPDIR/fail-mv-bin"
interrupt_mktemp_bin="$SMOKE_TMPDIR/interrupt-mktemp-bin"
release_download_bin="$SMOKE_TMPDIR/release-download-bin"
fake_systemctl_log="$SMOKE_TMPDIR/systemctl.log"
active_systemctl_log="$SMOKE_TMPDIR/systemctl-active.log"
atomic_failure_systemctl_log="$SMOKE_TMPDIR/systemctl-atomic-failure.log"
managed_systemctl_state="$SMOKE_TMPDIR/systemctl-state-managed"
fake_agent="$SMOKE_TMPDIR/vpsman-agent"
active_agent="$SMOKE_TMPDIR/vpsman-agent-active-replacement"
replacement_agent="$SMOKE_TMPDIR/vpsman-agent-replacement"
agent_home="$SMOKE_TMPDIR/agent-home"
staged_home="$SMOKE_TMPDIR/staged-home"
path_home="$SMOKE_TMPDIR/path-home"
download_home="$SMOKE_TMPDIR/download-home"
release_download_home="$SMOKE_TMPDIR/release-download-home"
release_manifest="$SMOKE_TMPDIR/version.json"
release_curl_log="$SMOKE_TMPDIR/release-curl.log"
invalid_fresh_home="$SMOKE_TMPDIR/invalid-fresh-home"

mkdir -p \
  "$fake_bin_dir" \
  "$no_systemctl_bin" \
  "$missing_curl_bin" \
  "$missing_sha_bin" \
  "$fail_mv_bin" \
  "$interrupt_mktemp_bin" \
  "$release_download_bin"
cat >"$fake_bin_dir/systemctl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >>"${VPSMAN_FAKE_SYSTEMCTL_LOG:?}"
state_dir="${VPSMAN_FAKE_SYSTEMCTL_STATE:?}"
args=("$@")
if [[ "${args[0]:-}" == "--user" ]]; then
  args=("${args[@]:1}")
fi
action="${args[0]:-}"

read_state() {
  local name="$1" fallback="$2" value
  value="$fallback"
  if [[ -f "$state_dir/$name" ]]; then
    IFS= read -r value <"$state_dir/$name" || true
  fi
  printf '%s' "$value"
}

write_state() {
  printf '%s\n' "$2" >"$state_dir/$1"
}

maybe_inject() {
  local phase="$1" marker signal_name

  if [[ "$phase" == "${VPSMAN_FAKE_SYSTEMCTL_FAIL_ACTION:-}" ]]; then
    marker="${VPSMAN_FAKE_SYSTEMCTL_FAILURE_MARKER:?}"
    if [[ ! -e "$marker" ]]; then
      printf '%s\n' "$phase" >"$marker"
      printf 'simulated systemctl %s failure after mutation\n' "$phase" >&2
      exit 75
    fi
  fi
  if [[ "$phase" == "${VPSMAN_FAKE_SYSTEMCTL_ABORT_ACTION:-}" ]]; then
    marker="${VPSMAN_FAKE_SYSTEMCTL_ABORT_MARKER:?}"
    if [[ ! -e "$marker" ]]; then
      signal_name="${VPSMAN_FAKE_SYSTEMCTL_ABORT_SIGNAL:-TERM}"
      printf '%s\n' "$phase" >"$marker"
      kill -s "$signal_name" "$PPID"
    fi
  fi
  if [[ "$phase" == "${VPSMAN_FAKE_SYSTEMCTL_SECOND_ABORT_ACTION:-}" ]]; then
    marker="${VPSMAN_FAKE_SYSTEMCTL_SECOND_ABORT_MARKER:?}"
    if [[ ! -e "$marker" ]]; then
      signal_name="${VPSMAN_FAKE_SYSTEMCTL_SECOND_ABORT_SIGNAL:-HUP}"
      printf '%s\n' "$phase" >"$marker"
      kill -s "$signal_name" "$PPID"
    fi
  fi
}

case "$action" in
  show)
    printf 'LoadState=%s\n' "$(read_state load-state not-found)"
    printf 'ActiveState=%s\n' "$(read_state active-state inactive)"
    printf 'UnitFileState=%s\n' "$(read_state unit-file-state "")"
    printf 'FragmentPath=%s\n' "$(read_state fragment-path "")"
    ;;
  is-active)
    [[ "$(read_state active-state inactive)" == "active" ]]
    ;;
  is-enabled)
    unit_state="$(read_state unit-file-state "")"
    printf '%s\n' "$unit_state"
    [[ "$unit_state" == "enabled" || "$unit_state" == "linked" ]]
    ;;
  link)
    unit_path="${args[-1]}"
    write_state load-state loaded
    write_state unit-file-state linked
    write_state fragment-path "$unit_path"
    write_state placement linked
    write_state topology unit-link
    maybe_inject link
    ;;
  daemon-reload)
    maybe_inject daemon-reload
    ;;
  enable)
    write_state load-state loaded
    write_state unit-file-state enabled
    write_state topology "$(read_state topology "")|enable-link"
    maybe_inject enable
    ;;
  disable)
    if [[ "$(read_state placement linked)" == "load-path" ]]; then
      write_state load-state loaded
      write_state unit-file-state disabled
    else
      write_state load-state not-found
      write_state unit-file-state ""
      write_state fragment-path ""
    fi
    write_state topology ""
    maybe_inject disable
    ;;
  start | restart)
    write_state active-state active
    maybe_inject "$action"
    ;;
  stop)
    write_state active-state inactive
    maybe_inject stop
    ;;
  *)
    printf 'unsupported fake systemctl action: %s\n' "$action" >&2
    exit 64
    ;;
esac
SH
chmod 0755 "$fake_bin_dir/systemctl"
cat >"$fail_mv_bin/mv" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

count=0
if [[ -s "${VPSMAN_SMOKE_MV_COUNTER:?}" ]]; then
  IFS= read -r count <"$VPSMAN_SMOKE_MV_COUNTER"
fi
((count += 1))
printf '%s\n' "$count" >"$VPSMAN_SMOKE_MV_COUNTER"

count_selected() {
  local selected="${1:-}"
  [[ -n "$selected" && ",$selected," == *",$count,"* ]]
}

if count_selected "${VPSMAN_SMOKE_FAIL_MV_AT:-}"; then
  printf 'simulated atomic publish failure at step %s\n' "$count" >&2
  exit 74
fi
if count_selected "${VPSMAN_SMOKE_PAUSE_BEFORE_MV_AT:-}"; then
  : >"${VPSMAN_SMOKE_PAUSE_MARKER:?}"
  while [[ ! -e "${VPSMAN_SMOKE_PAUSE_RELEASE:?}" ]]; do
    sleep 0.01
  done
fi
"${VPSMAN_SMOKE_REAL_MV:?}" "$@"
if count_selected "${VPSMAN_SMOKE_ABORT_AFTER_MV_AT:-}"; then
  marker="${VPSMAN_SMOKE_ABORT_MARKER:?}"
  if [[ ! -e "$marker" ]]; then
    printf '%s\n' "$count" >"$marker"
    kill -s "${VPSMAN_SMOKE_ABORT_SIGNAL:-TERM}" "$PPID"
  fi
fi
if count_selected "${VPSMAN_SMOKE_EXIT_AFTER_MV_AT:-}"; then
  printf 'simulated unexpected exit after atomic publication step %s\n' "$count" >&2
  exit 76
fi
SH
chmod 0755 "$fail_mv_bin/mv"
cat >"$interrupt_mktemp_bin/mktemp" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

count=0
if [[ -s "${VPSMAN_SMOKE_MKTEMP_COUNTER:?}" ]]; then
  IFS= read -r count <"$VPSMAN_SMOKE_MKTEMP_COUNTER"
fi
((count += 1))
printf '%s\n' "$count" >"$VPSMAN_SMOKE_MKTEMP_COUNTER"
path="$("${VPSMAN_SMOKE_REAL_MKTEMP:?}" "$@")"
printf '%s\n' "$path" >>"${VPSMAN_SMOKE_MKTEMP_PATH_LOG:?}"
printf '%s\n' "$path"
if [[ "$count" == "${VPSMAN_SMOKE_ABORT_AFTER_MKTEMP_AT:-}" &&
  ! -e "${VPSMAN_SMOKE_MKTEMP_ABORT_MARKER:-}" ]]; then
  printf '%s\n' "$count" >"${VPSMAN_SMOKE_MKTEMP_ABORT_MARKER:?}"
  kill -s "${VPSMAN_SMOKE_MKTEMP_ABORT_SIGNAL:-TERM}" "$PPID"
fi
SH
chmod 0755 "$interrupt_mktemp_bin/mktemp"
cat >"$release_download_bin/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

output=""
url="${!#}"
while (($#)); do
  if [[ "$1" == "-o" ]]; then
    output="${2:?}"
    break
  fi
  shift
done

[[ -n "$output" ]] || exit 64
printf '%s\n' "$url" >>"${VPSMAN_SMOKE_RELEASE_CURL_LOG:?}"
case "$url" in
  "${VPSMAN_SMOKE_RELEASE_MANIFEST_URL:?}")
    cp "${VPSMAN_SMOKE_RELEASE_MANIFEST:?}" "$output"
    ;;
  "${VPSMAN_SMOKE_RELEASE_ASSET_URL:?}")
    cp "${VPSMAN_SMOKE_RELEASE_AGENT:?}" "$output"
    ;;
  *)
    printf 'unexpected release URL: %s\n' "$url" >&2
    exit 22
    ;;
esac
SH
chmod 0755 "$release_download_bin/curl"
for tool in bash cat chmod flock id install ln mkdir mktemp mv rm rmdir stat; do
  ln -s "$(command -v "$tool")" "$no_systemctl_bin/$tool"
  ln -s "$(command -v "$tool")" "$missing_curl_bin/$tool"
  ln -s "$(command -v "$tool")" "$missing_sha_bin/$tool"
done
ln -s "$(command -v sha256sum)" "$missing_curl_bin/sha256sum"
ln -s "$(command -v curl)" "$missing_sha_bin/curl"

cat >"$fake_agent" <<'SH'
#!/usr/bin/env sh
echo vpsman-agent-deploy-smoke
SH
chmod 0755 "$fake_agent"
ln -s "$fake_agent" "$fake_bin_dir/vpsman-agent"
cat >"$active_agent" <<'SH'
#!/usr/bin/env sh
echo active-rerun-installed
SH
chmod 0755 "$active_agent"
cat >"$replacement_agent" <<'SH'
#!/usr/bin/env sh
echo replacement-must-not-be-installed
SH
chmod 0755 "$replacement_agent"
fake_agent_sha="$(sha256sum "$fake_agent" | awk '{print $1}')"

init_systemctl_state() {
  local state_dir="$1" registration="$2" active_state="$3" fragment_path="${4:-}"
  local topology="${5:-}"

  mkdir -p "$state_dir"
  printf '%s\n' "$active_state" >"$state_dir/active-state"
  case "$registration" in
    unlinked)
      printf 'not-found\n' >"$state_dir/load-state"
      : >"$state_dir/unit-file-state"
      : >"$state_dir/fragment-path"
      printf 'linked\n' >"$state_dir/placement"
      : >"$state_dir/topology"
      ;;
    linked)
      printf 'loaded\n' >"$state_dir/load-state"
      printf 'linked\n' >"$state_dir/unit-file-state"
      printf '%s\n' "$fragment_path" >"$state_dir/fragment-path"
      printf 'linked\n' >"$state_dir/placement"
      printf '%s\n' "${topology:-unit-link}" >"$state_dir/topology"
      ;;
    disabled | enabled)
      printf 'loaded\n' >"$state_dir/load-state"
      printf '%s\n' "$registration" >"$state_dir/unit-file-state"
      printf '%s\n' "$fragment_path" >"$state_dir/fragment-path"
      printf 'load-path\n' >"$state_dir/placement"
      printf '%s\n' "${topology:-custom-enable-topology}" >"$state_dir/topology"
      ;;
    *)
      printf '%s\n' "$registration" >"$state_dir/load-state"
      printf '%s\n' "${VPSMAN_SMOKE_UNIT_FILE_STATE:-$registration}" \
        >"$state_dir/unit-file-state"
      printf '%s\n' "$fragment_path" >"$state_dir/fragment-path"
      printf 'linked\n' >"$state_dir/placement"
      printf '%s\n' "$topology" >"$state_dir/topology"
      ;;
  esac
}

init_raw_systemctl_state() {
  local state_dir="$1" load_state="$2" active_state="$3"
  local unit_file_state="$4" fragment_path="$5" placement="${6:-linked}"

  mkdir -p "$state_dir"
  printf '%s\n' "$load_state" >"$state_dir/load-state"
  printf '%s\n' "$active_state" >"$state_dir/active-state"
  printf '%s\n' "$unit_file_state" >"$state_dir/unit-file-state"
  printf '%s\n' "$fragment_path" >"$state_dir/fragment-path"
  printf '%s\n' "$placement" >"$state_dir/placement"
  : >"$state_dir/topology"
}

assert_systemctl_state() {
  local state_dir="$1" load_state="$2" active_state="$3"
  local unit_file_state="$4" fragment_path="$5"

  test "$(cat "$state_dir/load-state")" = "$load_state"
  test "$(cat "$state_dir/active-state")" = "$active_state"
  test "$(cat "$state_dir/unit-file-state")" = "$unit_file_state"
  test "$(cat "$state_dir/fragment-path")" = "$fragment_path"
}

common_env=(
  VPSMAN_INSTALL_MODE=user
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke_A:1-2
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX=1111111111111111111111111111111111111111111111111111111111111111
  VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX=2222222222222222222222222222222222222222222222222222222222222222
  'VPSMAN_GATEWAY_ENDPOINTS=primary=127.0.0.1:9443=10,dns=gw.example.com:9443=00020,ipv6=[2001:db8::1]:9443=30'
)

init_systemctl_state "$managed_systemctl_state" unlinked inactive
export VPSMAN_FAKE_SYSTEMCTL_STATE="$managed_systemctl_state"

env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/default-start.log" 2>&1

test -x "$agent_home/bin/vpsman-agent"
test -f "$agent_home/config/agent.toml"
test -f "$agent_home/systemd/vpsman-agent.service"
grep -Fq 'tcp_addr = "127.0.0.1:9443"' "$agent_home/config/agent.toml"
grep -Fq 'tcp_addr = "gw.example.com:9443"' "$agent_home/config/agent.toml"
grep -Fq 'tcp_addr = "[2001:db8::1]:9443"' "$agent_home/config/agent.toml"
grep -Fq 'priority = 20' "$agent_home/config/agent.toml"
if grep -Fq 'priority = 00020' "$agent_home/config/agent.toml"; then
  echo "endpoint priority must be written as a canonical TOML integer" >&2
  exit 1
fi
grep -Fqx -- \
  "--user show vpsman-agent.service --no-pager --property=LoadState --property=ActiveState --property=UnitFileState --property=FragmentPath" \
  "$fake_systemctl_log"
grep -Fqx -- "--user link --force $agent_home/systemd/vpsman-agent.service" \
  "$fake_systemctl_log"
grep -Fqx -- "--user enable vpsman-agent.service" "$fake_systemctl_log"
grep -Fqx -- "--user start vpsman-agent.service" "$fake_systemctl_log"
if grep -Fq -- "--user restart vpsman-agent.service" "$fake_systemctl_log"; then
  echo "fresh install must start, not restart, the service" >&2
  exit 1
fi
grep -q "installed and enabled direct gateway agent" "$SMOKE_TMPDIR/default-start.log"
if grep -Fq "2222222222222222222222222222222222222222222222222222222222222222" "$SMOKE_TMPDIR/default-start.log"; then
  echo "agent installer echoed the operator-supplied gateway public key" >&2
  exit 1
fi

active_private_key=3333333333333333333333333333333333333333333333333333333333333333
active_topology_before="$(sha256sum "$managed_systemctl_state/topology" | awk '{print $1}')"
env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$active_systemctl_log" \
  VPSMAN_FAKE_SYSTEMCTL_ACTIVE=1 \
  VPSMAN_FAKE_SYSTEMCTL_ENABLED=1 \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$active_agent" \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.active-rerun \
  VPSMAN_AGENT_NOISE_PRIVATE_KEY_HEX="$active_private_key" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/active-rerun.log" 2>&1

test "$("$agent_home/bin/vpsman-agent")" = "active-rerun-installed"
grep -Fq 'client_id = "deploy.smoke.active-rerun"' "$agent_home/config/agent.toml"
grep -Fq "client_private_key_hex = \"$active_private_key\"" \
  "$agent_home/config/agent.toml"
grep -Fqx -- \
  "--user show vpsman-agent.service --no-pager --property=LoadState --property=ActiveState --property=UnitFileState --property=FragmentPath" \
  "$active_systemctl_log"
if grep -Eq -- "--user (link|enable|disable)" "$active_systemctl_log"; then
  echo "an already enabled owned unit must not mutate unit-file topology" >&2
  exit 1
fi
grep -Fqx -- "--user restart vpsman-agent.service" "$active_systemctl_log"
if grep -Fqx -- "--user start vpsman-agent.service" "$active_systemctl_log"; then
  echo "active reinstall must restart, not merely start, the service" >&2
  exit 1
fi
test "$active_topology_before" = \
  "$(sha256sum "$managed_systemctl_state/topology" | awk '{print $1}')"
grep -Fq "installed and restarted direct gateway agent" "$SMOKE_TMPDIR/active-rerun.log"

symlink_config_target="$SMOKE_TMPDIR/symlink-config-target"
mkdir -p "$symlink_config_target"
ln -s "$symlink_config_target" "$agent_home/config-link"
symlink_systemd_target="$SMOKE_TMPDIR/symlink-systemd-target"
mkdir -p "$symlink_systemd_target"
ln -s "$symlink_systemd_target" "$agent_home/systemd-link"
chmod 0751 "$agent_home/config"

install_tree_digest() {
  tar --sort=name --numeric-owner --mtime=@0 -C "$agent_home" -cf - . |
    sha256sum |
    awk '{print $1}'
}

install_tree_digest_before="$(install_tree_digest)"
config_mode_before="$(stat -c '%a' "$agent_home/config")"
systemctl_hash_before="$(sha256sum "$fake_systemctl_log" | awk '{print $1}')"

assert_existing_install_unchanged() {
  test "$install_tree_digest_before" = "$(install_tree_digest)"
  test "$config_mode_before" = "$(stat -c '%a' "$agent_home/config")"
  test "$systemctl_hash_before" = \
    "$(sha256sum "$fake_systemctl_log" | awk '{print $1}')"
}

assert_logged_temp_paths_absent() {
  local path_log="$1" temp_path

  while IFS= read -r temp_path; do
    [[ -n "$temp_path" ]] || continue
    if [[ -e "$temp_path" || -L "$temp_path" ]]; then
      echo "registered temporary path survived interruption: $temp_path" >&2
      exit 1
    fi
  done <"$path_log"
}

assert_no_transaction_artifacts() {
  local home="$1" leftover

  [[ -e "$home" ]] || return 0
  leftover="$(
    find "$home" \
      \( -name '*.rollback.*' -o -name '*.install.*' -o -name '.vpsman-restore.*' \) \
      -print -quit
  )"
  if [[ -n "$leftover" ]]; then
    echo "transaction artifact survived interruption: $leftover" >&2
    exit 1
  fi
}

staging_active_refusal_cases=0
staging_active_log="$SMOKE_TMPDIR/staging-active-refusal.log"
staging_active_systemctl_log="$SMOKE_TMPDIR/staging-active-systemctl.log"
if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$staging_active_systemctl_log" \
  VPSMAN_FAKE_SYSTEMCTL_ACTIVE=1 \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.staging-active-refusal \
  bash deploy/install-agent.sh >"$staging_active_log" 2>&1; then
  echo "expected staging-only replacement of an active service to fail" >&2
  exit 1
fi
grep -Fq "staging-only install refuses registered vpsman-agent.service (enabled/active)" \
  "$staging_active_log"
assert_existing_install_unchanged
test "$(awk 'END { print NR }' "$staging_active_systemctl_log")" -eq 1
grep -Fqx -- \
  "--user show vpsman-agent.service --no-pager --property=LoadState --property=ActiveState --property=UnitFileState --property=FragmentPath" \
  "$staging_active_systemctl_log"
((staging_active_refusal_cases += 1))

staging_linked_state="$SMOKE_TMPDIR/staging-linked.state"
staging_linked_log="$SMOKE_TMPDIR/staging-linked.log"
staging_linked_systemctl_log="$SMOKE_TMPDIR/staging-linked.systemctl.log"
init_systemctl_state \
  "$staging_linked_state" \
  linked \
  inactive \
  "$agent_home/systemd/vpsman-agent.service"
if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$staging_linked_systemctl_log" \
  VPSMAN_FAKE_SYSTEMCTL_STATE="$staging_linked_state" \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.staging-linked-refusal \
  bash deploy/install-agent.sh >"$staging_linked_log" 2>&1; then
  echo "expected staging-only replacement of a linked service to fail" >&2
  exit 1
fi
grep -Fq "has unsupported preexisting state linked" \
  "$staging_linked_log"
assert_existing_install_unchanged
assert_systemctl_state \
  "$staging_linked_state" \
  loaded \
  inactive \
  linked \
  "$agent_home/systemd/vpsman-agent.service"
((staging_active_refusal_cases += 1))

systemd_state_refusal_cases=0
run_systemd_state_refusal() {
  local case_name="$1" load_state="$2" active_state="$3"
  local unit_file_state="$4" fragment_path="$5" expected_error="$6"
  local state_dir="$SMOKE_TMPDIR/systemd-refusal-$case_name.state"
  local systemctl_log="$SMOKE_TMPDIR/systemd-refusal-$case_name.systemctl.log"
  local install_log="$SMOKE_TMPDIR/systemd-refusal-$case_name.log"
  local state_before state_after

  init_raw_systemctl_state \
    "$state_dir" \
    "$load_state" \
    "$active_state" \
    "$unit_file_state" \
    "$fragment_path"
  state_before="$(
    tar --sort=name --mtime=@0 -C "$state_dir" -cf - . |
      sha256sum |
      awk '{print $1}'
  )"
  if env \
    PATH="$fake_bin_dir:$PATH" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$systemctl_log" \
    VPSMAN_FAKE_SYSTEMCTL_STATE="$state_dir" \
    VPSMAN_AGENT_HOME="$agent_home" \
    VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
    "${common_env[@]}" \
    VPSMAN_AGENT_CLIENT_ID="deploy.smoke.systemd-refusal-$case_name" \
    bash deploy/install-agent.sh >"$install_log" 2>&1; then
    echo "expected systemd preflight refusal: $case_name" >&2
    exit 1
  fi
  grep -Fq "$expected_error" "$install_log"
  assert_existing_install_unchanged
  test "$(awk 'END { print NR }' "$systemctl_log")" -eq 1
  state_after="$(
    tar --sort=name --mtime=@0 -C "$state_dir" -cf - . |
      sha256sum |
      awk '{print $1}'
  )"
  test "$state_before" = "$state_after"
  ((systemd_state_refusal_cases += 1))
}

run_systemd_state_refusal \
  external-unit \
  loaded \
  inactive \
  enabled \
  "$SMOKE_TMPDIR/external/vpsman-agent.service" \
  "is owned by external unit"
run_systemd_state_refusal \
  masked \
  masked \
  inactive \
  masked \
  /dev/null \
  "is masked"
run_systemd_state_refusal \
  runtime-link \
  loaded \
  inactive \
  linked-runtime \
  "$agent_home/systemd/vpsman-agent.service" \
  "uses unsupported runtime registration state linked-runtime"
run_systemd_state_refusal \
  static \
  loaded \
  inactive \
  static \
  "$agent_home/systemd/vpsman-agent.service" \
  "has unsupported unit-file state static"
run_systemd_state_refusal \
  failed \
  loaded \
  failed \
  linked \
  "$agent_home/systemd/vpsman-agent.service" \
  "has unsupported active state failed"
run_systemd_state_refusal \
  activating \
  loaded \
  activating \
  linked \
  "$agent_home/systemd/vpsman-agent.service" \
  "has unsupported active state activating"
run_systemd_state_refusal \
  linked \
  loaded \
  inactive \
  linked \
  "$agent_home/systemd/vpsman-agent.service" \
  "has unsupported preexisting state linked"
run_systemd_state_refusal \
  disabled \
  loaded \
  inactive \
  disabled \
  "$agent_home/systemd/vpsman-agent.service" \
  "has unsupported preexisting state disabled"
run_systemd_state_refusal \
  unlinked-active \
  not-found \
  active \
  "" \
  "" \
  "is active without a supported unit registration"

atomic_publication_failure_cases=0
real_mv="$(command -v mv)"
for publish_step in 1 2 3; do
  case "$publish_step" in
    1) publish_label="agent binary" ;;
    2) publish_label="agent config" ;;
    3) publish_label="systemd unit" ;;
  esac
  publish_failure_log="$SMOKE_TMPDIR/atomic-publication-failure-$publish_step.log"
  publish_systemctl_log="${atomic_failure_systemctl_log}.$publish_step"
  publish_mv_counter="$SMOKE_TMPDIR/atomic-publication-mv-counter-$publish_step"

  if env \
    PATH="$fail_mv_bin:$fake_bin_dir:$PATH" \
    VPSMAN_SMOKE_REAL_MV="$real_mv" \
    VPSMAN_SMOKE_MV_COUNTER="$publish_mv_counter" \
    VPSMAN_SMOKE_FAIL_MV_AT="$publish_step" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$publish_systemctl_log" \
    VPSMAN_FAKE_SYSTEMCTL_ACTIVE=1 \
    VPSMAN_FAKE_SYSTEMCTL_ENABLED=1 \
    VPSMAN_AGENT_HOME="$agent_home" \
    VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
    "${common_env[@]}" \
    VPSMAN_AGENT_CLIENT_ID="deploy.smoke.atomic-failure-$publish_step" \
    bash deploy/install-agent.sh >"$publish_failure_log" 2>&1; then
    echo "expected simulated $publish_label publication failure" >&2
    exit 1
  fi
  grep -Fq "simulated atomic publish failure at step $publish_step" \
    "$publish_failure_log"
  grep -Fq "could not publish the $publish_label; rollback will restore the prior installation" \
    "$publish_failure_log"
  assert_existing_install_unchanged
  test "$(grep -Fc -- \
    '--user show vpsman-agent.service --no-pager --property=LoadState --property=ActiveState --property=UnitFileState --property=FragmentPath' \
    "$publish_systemctl_log")" -eq 2
  assert_systemctl_state \
    "$managed_systemctl_state" \
    loaded \
    active \
    enabled \
    "$agent_home/systemd/vpsman-agent.service"
  if grep -Eq -- "--user (link|enable|disable)" "$publish_systemctl_log"; then
    echo "enabled rollback must not mutate unit-file topology" >&2
    exit 1
  fi
  test "$(cat "$publish_mv_counter")" -eq "$((publish_step * 2 - 1))"
  ((atomic_publication_failure_cases += 1))
done

publication_interrupt_cases=0
for publish_step in 1 2 3; do
  case "$publish_step" in
    1) publish_signal=TERM ;;
    2) publish_signal=INT ;;
    3) publish_signal=HUP ;;
  esac
  publish_interrupt_log="$SMOKE_TMPDIR/publication-interrupt-$publish_step.log"
  publish_interrupt_systemctl_log="$SMOKE_TMPDIR/publication-interrupt-$publish_step.systemctl.log"
  publish_interrupt_marker="$SMOKE_TMPDIR/publication-interrupt-$publish_step.marker"
  publish_interrupt_counter="$SMOKE_TMPDIR/publication-interrupt-$publish_step.counter"

  if env \
    PATH="$fail_mv_bin:$fake_bin_dir:$PATH" \
    VPSMAN_SMOKE_REAL_MV="$real_mv" \
    VPSMAN_SMOKE_MV_COUNTER="$publish_interrupt_counter" \
    VPSMAN_SMOKE_ABORT_AFTER_MV_AT="$publish_step" \
    VPSMAN_SMOKE_ABORT_MARKER="$publish_interrupt_marker" \
    VPSMAN_SMOKE_ABORT_SIGNAL="$publish_signal" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$publish_interrupt_systemctl_log" \
    VPSMAN_FAKE_SYSTEMCTL_STATE="$managed_systemctl_state" \
    VPSMAN_AGENT_HOME="$agent_home" \
    VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
    "${common_env[@]}" \
    VPSMAN_AGENT_CLIENT_ID="deploy.smoke.publication-interrupt-$publish_step" \
    bash deploy/install-agent.sh >"$publish_interrupt_log" 2>&1; then
    echo "expected $publish_signal after publication step $publish_step to abort" >&2
    exit 1
  fi
  test "$(cat "$publish_interrupt_marker")" = "$publish_step"
  grep -Fq "received $publish_signal during installation; restoring the prior state" \
    "$publish_interrupt_log"
  assert_existing_install_unchanged
  assert_systemctl_state \
    "$managed_systemctl_state" \
    loaded \
    active \
    enabled \
    "$agent_home/systemd/vpsman-agent.service"
  test "$(grep -Fc -- \
    '--user show vpsman-agent.service --no-pager --property=LoadState --property=ActiveState --property=UnitFileState --property=FragmentPath' \
    "$publish_interrupt_systemctl_log")" -eq 2
  if grep -Eq -- "--user (link|enable|disable)" "$publish_interrupt_systemctl_log"; then
    echo "enabled rollback must not mutate unit-file topology" >&2
    exit 1
  fi
  ((publication_interrupt_cases += 1))
done

rollback_signal_resilience_cases=0
rollback_signal_state="$SMOKE_TMPDIR/rollback-signal.state"
rollback_signal_log="$SMOKE_TMPDIR/rollback-signal.log"
rollback_signal_systemctl_log="$SMOKE_TMPDIR/rollback-signal.systemctl.log"
rollback_signal_publish_marker="$SMOKE_TMPDIR/rollback-signal.publish-marker"
rollback_signal_second_marker="$SMOKE_TMPDIR/rollback-signal.second-marker"
rollback_signal_counter="$SMOKE_TMPDIR/rollback-signal.counter"
init_systemctl_state \
  "$rollback_signal_state" \
  enabled \
  active \
  "$agent_home/systemd/vpsman-agent.service"
topology_before="$(
  sha256sum "$rollback_signal_state/topology" |
    awk '{print $1}'
)"
if env \
  PATH="$fail_mv_bin:$fake_bin_dir:$PATH" \
  VPSMAN_SMOKE_REAL_MV="$real_mv" \
  VPSMAN_SMOKE_MV_COUNTER="$rollback_signal_counter" \
  VPSMAN_SMOKE_ABORT_AFTER_MV_AT=1 \
  VPSMAN_SMOKE_ABORT_MARKER="$rollback_signal_publish_marker" \
  VPSMAN_SMOKE_ABORT_SIGNAL=TERM \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$rollback_signal_systemctl_log" \
  VPSMAN_FAKE_SYSTEMCTL_STATE="$rollback_signal_state" \
  VPSMAN_FAKE_SYSTEMCTL_SECOND_ABORT_ACTION=stop \
  VPSMAN_FAKE_SYSTEMCTL_SECOND_ABORT_MARKER="$rollback_signal_second_marker" \
  VPSMAN_FAKE_SYSTEMCTL_SECOND_ABORT_SIGNAL=HUP \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.rollback-signal \
  bash deploy/install-agent.sh >"$rollback_signal_log" 2>&1; then
  echo "expected initial TERM during publication to abort after protected rollback" >&2
  exit 1
fi
test "$(cat "$rollback_signal_publish_marker")" = 1
test "$(cat "$rollback_signal_second_marker")" = stop
grep -Fq "received TERM during installation; restoring the prior state" \
  "$rollback_signal_log"
assert_existing_install_unchanged
assert_no_transaction_artifacts "$agent_home"
assert_systemctl_state \
  "$rollback_signal_state" \
  loaded \
  active \
  enabled \
  "$agent_home/systemd/vpsman-agent.service"
test "$topology_before" = \
  "$(sha256sum "$rollback_signal_state/topology" | awk '{print $1}')"
if grep -Eq -- "--user (link|enable|disable)" \
  "$rollback_signal_systemctl_log"; then
  echo "signal-safe rollback must not mutate enabled unit-file topology" >&2
  exit 1
fi
grep -Fqx -- "--user stop vpsman-agent.service" \
  "$rollback_signal_systemctl_log"
grep -Fqx -- "--user start vpsman-agent.service" \
  "$rollback_signal_systemctl_log"
((rollback_signal_resilience_cases += 1))

service_action_failure_cases=0
for failed_action in link daemon-reload enable restart start; do
  case "$failed_action" in
    link) failed_label="link the systemd unit" ;;
    daemon-reload) failed_label="reload the systemd manager" ;;
    enable) failed_label="enable vpsman-agent.service" ;;
    restart) failed_label="restart vpsman-agent.service" ;;
    start) failed_label="start vpsman-agent.service" ;;
  esac
  service_failure_log="$SMOKE_TMPDIR/service-failure-$failed_action.log"
  service_failure_systemctl_log="$SMOKE_TMPDIR/service-failure-$failed_action.systemctl.log"
  service_failure_marker="$SMOKE_TMPDIR/service-failure-$failed_action.marker"
  service_failure_state="$SMOKE_TMPDIR/service-failure-$failed_action.state"
  prior_registration=unlinked
  prior_active_state=inactive
  if [[ "$failed_action" == "restart" ]]; then
    prior_registration=enabled
    prior_active_state=active
  fi
  init_systemctl_state \
    "$service_failure_state" \
    "$prior_registration" \
    "$prior_active_state" \
    "$agent_home/systemd/vpsman-agent.service"
  topology_before="$(
    sha256sum "$service_failure_state/topology" |
      awk '{print $1}'
  )"

  if env \
    PATH="$fake_bin_dir:$PATH" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$service_failure_systemctl_log" \
    VPSMAN_FAKE_SYSTEMCTL_STATE="$service_failure_state" \
    VPSMAN_FAKE_SYSTEMCTL_FAIL_ACTION="$failed_action" \
    VPSMAN_FAKE_SYSTEMCTL_FAILURE_MARKER="$service_failure_marker" \
    VPSMAN_AGENT_HOME="$agent_home" \
    VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
    "${common_env[@]}" \
    VPSMAN_AGENT_CLIENT_ID="deploy.smoke.service-failure-$failed_action" \
    bash deploy/install-agent.sh >"$service_failure_log" 2>&1; then
    echo "expected simulated systemctl $failed_action failure" >&2
    exit 1
  fi
  grep -Fq "simulated systemctl $failed_action failure after mutation" "$service_failure_log"
  grep -Fq "could not $failed_label; rollback will restore the prior installation and service state" \
    "$service_failure_log"
  test "$(cat "$service_failure_marker")" = "$failed_action"
  assert_existing_install_unchanged
  test "$(grep -Fc -- \
    '--user show vpsman-agent.service --no-pager --property=LoadState --property=ActiveState --property=UnitFileState --property=FragmentPath' \
    "$service_failure_systemctl_log")" -eq 2
  test "$(grep -Fc -- '--user daemon-reload' \
    "$service_failure_systemctl_log")" -ge 1
  if [[ "$prior_registration" == unlinked ]]; then
    assert_systemctl_state \
      "$service_failure_state" \
      not-found \
      inactive \
      "" \
      ""
  else
    assert_systemctl_state \
      "$service_failure_state" \
      loaded \
      "$prior_active_state" \
      enabled \
      "$agent_home/systemd/vpsman-agent.service"
    test "$topology_before" = \
      "$(sha256sum "$service_failure_state/topology" | awk '{print $1}')"
    if grep -Eq -- "--user (link|enable|disable)" "$service_failure_systemctl_log"; then
      echo "enabled rollback must not mutate unit-file topology" >&2
      exit 1
    fi
  fi
  ((service_action_failure_cases += 1))
done

service_interrupt_cases=0
for interrupted_action in link daemon-reload enable restart start; do
  service_interrupt_state="$SMOKE_TMPDIR/service-interrupt-$interrupted_action.state"
  service_interrupt_log="$SMOKE_TMPDIR/service-interrupt-$interrupted_action.log"
  service_interrupt_systemctl_log="$SMOKE_TMPDIR/service-interrupt-$interrupted_action.systemctl.log"
  service_interrupt_marker="$SMOKE_TMPDIR/service-interrupt-$interrupted_action.marker"
  prior_registration=unlinked
  prior_active_state=inactive
  if [[ "$interrupted_action" == "restart" ]]; then
    prior_registration=enabled
    prior_active_state=active
  fi
  init_systemctl_state \
    "$service_interrupt_state" \
    "$prior_registration" \
    "$prior_active_state" \
    "$agent_home/systemd/vpsman-agent.service"
  topology_before="$(
    sha256sum "$service_interrupt_state/topology" |
      awk '{print $1}'
  )"

  if env \
    PATH="$fake_bin_dir:$PATH" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$service_interrupt_systemctl_log" \
    VPSMAN_FAKE_SYSTEMCTL_STATE="$service_interrupt_state" \
    VPSMAN_FAKE_SYSTEMCTL_ABORT_ACTION="$interrupted_action" \
    VPSMAN_FAKE_SYSTEMCTL_ABORT_MARKER="$service_interrupt_marker" \
    VPSMAN_FAKE_SYSTEMCTL_ABORT_SIGNAL=TERM \
    VPSMAN_AGENT_HOME="$agent_home" \
    VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
    "${common_env[@]}" \
    VPSMAN_AGENT_CLIENT_ID="deploy.smoke.service-interrupt-$interrupted_action" \
    bash deploy/install-agent.sh >"$service_interrupt_log" 2>&1; then
    echo "expected interruption after systemctl $interrupted_action" >&2
    exit 1
  fi
  test "$(cat "$service_interrupt_marker")" = "$interrupted_action"
  grep -Fq "received TERM during installation; restoring the prior state" \
    "$service_interrupt_log"
  assert_existing_install_unchanged
  if [[ "$prior_registration" == unlinked ]]; then
    assert_systemctl_state \
      "$service_interrupt_state" \
      not-found \
      inactive \
      "" \
      ""
  else
    assert_systemctl_state \
      "$service_interrupt_state" \
      loaded \
      "$prior_active_state" \
      enabled \
      "$agent_home/systemd/vpsman-agent.service"
    test "$topology_before" = \
      "$(sha256sum "$service_interrupt_state/topology" | awk '{print $1}')"
    if grep -Eq -- "--user (link|enable|disable)" "$service_interrupt_systemctl_log"; then
      echo "enabled rollback must not mutate unit-file topology" >&2
      exit 1
    fi
  fi
  test "$(grep -Fc -- \
    '--user show vpsman-agent.service --no-pager --property=LoadState --property=ActiveState --property=UnitFileState --property=FragmentPath' \
    "$service_interrupt_systemctl_log")" -eq 2
  ((service_interrupt_cases += 1))
done

supported_state_restore_cases=0
for restore_case in active-enabled inactive-enabled; do
  restore_state="$SMOKE_TMPDIR/service-restore-$restore_case.state"
  restore_log="$SMOKE_TMPDIR/service-restore-$restore_case.log"
  restore_systemctl_log="$SMOKE_TMPDIR/service-restore-$restore_case.systemctl.log"
  restore_marker="$SMOKE_TMPDIR/service-restore-$restore_case.marker"
  case "$restore_case" in
    active-enabled)
      prior_registration=enabled
      prior_active_state=active
      interrupted_action=restart
      ;;
    inactive-enabled)
      prior_registration=enabled
      prior_active_state=inactive
      interrupted_action=start
      ;;
  esac
  init_systemctl_state \
    "$restore_state" \
    "$prior_registration" \
    "$prior_active_state" \
    "$agent_home/systemd/vpsman-agent.service"
  topology_before="$(
    sha256sum "$restore_state/topology" |
      awk '{print $1}'
  )"

  if env \
    PATH="$fake_bin_dir:$PATH" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$restore_systemctl_log" \
    VPSMAN_FAKE_SYSTEMCTL_STATE="$restore_state" \
    VPSMAN_FAKE_SYSTEMCTL_ABORT_ACTION="$interrupted_action" \
    VPSMAN_FAKE_SYSTEMCTL_ABORT_MARKER="$restore_marker" \
    VPSMAN_FAKE_SYSTEMCTL_ABORT_SIGNAL=TERM \
    VPSMAN_AGENT_HOME="$agent_home" \
    VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
    "${common_env[@]}" \
    VPSMAN_AGENT_CLIENT_ID="deploy.smoke.service-restore-$restore_case" \
    bash deploy/install-agent.sh >"$restore_log" 2>&1; then
    echo "expected interruption for supported state restore case $restore_case" >&2
    exit 1
  fi
  grep -Fq "received TERM during installation; restoring the prior state" \
    "$restore_log"
  assert_existing_install_unchanged
  assert_systemctl_state \
    "$restore_state" \
    loaded \
    "$prior_active_state" \
    "$prior_registration" \
    "$agent_home/systemd/vpsman-agent.service"
  test "$topology_before" = \
    "$(sha256sum "$restore_state/topology" | awk '{print $1}')"
  if grep -Eq -- "--user (link|enable|disable)" "$restore_systemctl_log"; then
    echo "$restore_case must not mutate enabled unit-file topology" >&2
    exit 1
  fi
  ((supported_state_restore_cases += 1))
done

directory_rollback_cases=0
fresh_parent="$SMOKE_TMPDIR/fresh-transaction-parent"
fresh_home="$fresh_parent/agent-home"
fresh_counter="$SMOKE_TMPDIR/fresh-transaction.counter"
fresh_log="$SMOKE_TMPDIR/fresh-transaction.log"
if env \
  PATH="$fail_mv_bin:$no_systemctl_bin" \
  VPSMAN_SMOKE_REAL_MV="$real_mv" \
  VPSMAN_SMOKE_MV_COUNTER="$fresh_counter" \
  VPSMAN_SMOKE_EXIT_AFTER_MV_AT=1 \
  VPSMAN_AGENT_HOME="$fresh_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.fresh-directory-rollback \
  bash "$ROOT_DIR/deploy/install-agent.sh" >"$fresh_log" 2>&1; then
  echo "expected unexpected exit during fresh publication" >&2
  exit 1
fi
grep -Fq "simulated unexpected exit after atomic publication step 1" "$fresh_log"
test ! -e "$fresh_parent"
((directory_rollback_cases += 1))

partial_home="$SMOKE_TMPDIR/partial-directory-home"
partial_counter="$SMOKE_TMPDIR/partial-directory.counter"
partial_log="$SMOKE_TMPDIR/partial-directory.log"
mkdir -p "$partial_home/config" "$partial_home/state"
chmod 0753 "$partial_home/config"
printf 'operator-owned\n' >"$partial_home/state/operator.keep"
if env \
  PATH="$fail_mv_bin:$no_systemctl_bin" \
  VPSMAN_SMOKE_REAL_MV="$real_mv" \
  VPSMAN_SMOKE_MV_COUNTER="$partial_counter" \
  VPSMAN_SMOKE_EXIT_AFTER_MV_AT=2 \
  VPSMAN_AGENT_HOME="$partial_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.partial-directory-rollback \
  bash "$ROOT_DIR/deploy/install-agent.sh" >"$partial_log" 2>&1; then
  echo "expected unexpected exit during partial-directory publication" >&2
  exit 1
fi
test "$(stat -c '%a' "$partial_home/config")" = 753
test "$(cat "$partial_home/state/operator.keep")" = operator-owned
test ! -e "$partial_home/bin"
test ! -e "$partial_home/log"
test ! -e "$partial_home/systemd"
test ! -e "$partial_home/config/agent.toml"
((directory_rollback_cases += 1))

concurrent_install_lock_cases=0
concurrent_home="$SMOKE_TMPDIR/concurrent-install-home"
concurrent_winner_log="$SMOKE_TMPDIR/concurrent-install-winner.log"
concurrent_winner_status="$SMOKE_TMPDIR/concurrent-install-winner.status"
concurrent_loser_log="$SMOKE_TMPDIR/concurrent-install-loser.log"
concurrent_systemctl_log="$SMOKE_TMPDIR/concurrent-install.systemctl.log"
concurrent_mv_counter="$SMOKE_TMPDIR/concurrent-install.mv-counter"
concurrent_pause_marker="$SMOKE_TMPDIR/concurrent-install.paused"
concurrent_pause_release="$SMOKE_TMPDIR/concurrent-install.release"
(
  set +e
  env \
    PATH="$fail_mv_bin:$fake_bin_dir:$PATH" \
    VPSMAN_SMOKE_REAL_MV="$real_mv" \
    VPSMAN_SMOKE_MV_COUNTER="$concurrent_mv_counter" \
    VPSMAN_SMOKE_PAUSE_BEFORE_MV_AT=2 \
    VPSMAN_SMOKE_PAUSE_MARKER="$concurrent_pause_marker" \
    VPSMAN_SMOKE_PAUSE_RELEASE="$concurrent_pause_release" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$concurrent_systemctl_log" \
    VPSMAN_AGENT_HOME="$concurrent_home" \
    VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
    VPSMAN_AGENT_ENABLE_SERVICE=0 \
    "${common_env[@]}" \
    VPSMAN_AGENT_CLIENT_ID=deploy.smoke.concurrent-winner \
    bash deploy/install-agent.sh >"$concurrent_winner_log" 2>&1
  printf '%s\n' "$?" >"$concurrent_winner_status"
) &
concurrent_winner_pid=$!
smoke_track_pid "$concurrent_winner_pid"
for ((concurrent_wait = 0; concurrent_wait < 1000; concurrent_wait++)); do
  [[ -e "$concurrent_pause_marker" ]] && break
  sleep 0.01
done
test -e "$concurrent_pause_marker"
concurrent_tree_before="$(
  tar --sort=name --numeric-owner --mtime=@0 -C "$concurrent_home" -cf - . |
    sha256sum |
    awk '{print $1}'
)"
set +e
env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$concurrent_systemctl_log" \
  VPSMAN_AGENT_HOME="$concurrent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.concurrent-loser \
  bash deploy/install-agent.sh >"$concurrent_loser_log" 2>&1
concurrent_loser_status=$?
set -e
concurrent_tree_after="$(
  tar --sort=name --numeric-owner --mtime=@0 -C "$concurrent_home" -cf - . |
    sha256sum |
    awk '{print $1}'
)"
: >"$concurrent_pause_release"
wait "$concurrent_winner_pid"
test "$concurrent_loser_status" -ne 0
test "$(cat "$concurrent_winner_status")" -eq 0
test "$concurrent_tree_before" = "$concurrent_tree_after"
grep -Fq \
  "another vpsman agent install is already in progress for $concurrent_home" \
  "$concurrent_loser_log"
test ! -s "$concurrent_systemctl_log"
test "$("$concurrent_home/bin/vpsman-agent")" = vpsman-agent-deploy-smoke
grep -Fq 'client_id = "deploy.smoke.concurrent-winner"' \
  "$concurrent_home/config/agent.toml"
if grep -Fq 'deploy.smoke.concurrent-loser' "$concurrent_home/config/agent.toml"; then
  echo "the refused concurrent installer must not publish its config" >&2
  exit 1
fi
assert_no_transaction_artifacts "$concurrent_home"
((concurrent_install_lock_cases += 1))

managed_concurrent_winner_home="$SMOKE_TMPDIR/managed-concurrent-winner-home"
managed_concurrent_loser_home="$SMOKE_TMPDIR/managed-concurrent-loser-home"
managed_concurrent_state="$SMOKE_TMPDIR/managed-concurrent.state"
managed_concurrent_systemctl_log="$SMOKE_TMPDIR/managed-concurrent.systemctl.log"
managed_concurrent_winner_log="$SMOKE_TMPDIR/managed-concurrent-winner.log"
managed_concurrent_winner_status="$SMOKE_TMPDIR/managed-concurrent-winner.status"
managed_concurrent_loser_log="$SMOKE_TMPDIR/managed-concurrent-loser.log"
managed_concurrent_mv_counter="$SMOKE_TMPDIR/managed-concurrent.mv-counter"
managed_concurrent_pause_marker="$SMOKE_TMPDIR/managed-concurrent.paused"
managed_concurrent_pause_release="$SMOKE_TMPDIR/managed-concurrent.release"
init_systemctl_state "$managed_concurrent_state" unlinked inactive
(
  set +e
  env \
    PATH="$fail_mv_bin:$fake_bin_dir:$PATH" \
    VPSMAN_SMOKE_REAL_MV="$real_mv" \
    VPSMAN_SMOKE_MV_COUNTER="$managed_concurrent_mv_counter" \
    VPSMAN_SMOKE_PAUSE_BEFORE_MV_AT=2 \
    VPSMAN_SMOKE_PAUSE_MARKER="$managed_concurrent_pause_marker" \
    VPSMAN_SMOKE_PAUSE_RELEASE="$managed_concurrent_pause_release" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$managed_concurrent_systemctl_log" \
    VPSMAN_FAKE_SYSTEMCTL_STATE="$managed_concurrent_state" \
    VPSMAN_AGENT_HOME="$managed_concurrent_winner_home" \
    VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
    "${common_env[@]}" \
    VPSMAN_AGENT_CLIENT_ID=deploy.smoke.managed-concurrent-winner \
    bash deploy/install-agent.sh >"$managed_concurrent_winner_log" 2>&1
  printf '%s\n' "$?" >"$managed_concurrent_winner_status"
) &
managed_concurrent_winner_pid=$!
smoke_track_pid "$managed_concurrent_winner_pid"
for ((concurrent_wait = 0; concurrent_wait < 1000; concurrent_wait++)); do
  [[ -e "$managed_concurrent_pause_marker" ]] && break
  sleep 0.01
done
test -e "$managed_concurrent_pause_marker"
managed_concurrent_tree_before="$(
  tar --sort=name --numeric-owner --mtime=@0 \
    -C "$managed_concurrent_winner_home" -cf - . |
    sha256sum |
    awk '{print $1}'
)"
managed_concurrent_systemctl_before="$(
  sha256sum "$managed_concurrent_systemctl_log" |
    awk '{print $1}'
)"
set +e
env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$managed_concurrent_systemctl_log" \
  VPSMAN_FAKE_SYSTEMCTL_STATE="$managed_concurrent_state" \
  VPSMAN_AGENT_HOME="$managed_concurrent_loser_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.managed-concurrent-loser \
  bash deploy/install-agent.sh >"$managed_concurrent_loser_log" 2>&1
managed_concurrent_loser_status=$?
set -e
test "$managed_concurrent_loser_status" -ne 0
test ! -e "$managed_concurrent_loser_home"
test "$managed_concurrent_tree_before" = "$(
  tar --sort=name --numeric-owner --mtime=@0 \
    -C "$managed_concurrent_winner_home" -cf - . |
    sha256sum |
    awk '{print $1}'
)"
test "$managed_concurrent_systemctl_before" = "$(
  sha256sum "$managed_concurrent_systemctl_log" |
    awk '{print $1}'
)"
grep -Fq \
  "another vpsman agent service install is already in progress for vpsman-agent.service" \
  "$managed_concurrent_loser_log"
: >"$managed_concurrent_pause_release"
wait "$managed_concurrent_winner_pid"
test "$(cat "$managed_concurrent_winner_status")" -eq 0
test "$("$managed_concurrent_winner_home/bin/vpsman-agent")" = \
  vpsman-agent-deploy-smoke
grep -Fq 'client_id = "deploy.smoke.managed-concurrent-winner"' \
  "$managed_concurrent_winner_home/config/agent.toml"
assert_systemctl_state \
  "$managed_concurrent_state" \
  loaded \
  active \
  enabled \
  "$managed_concurrent_winner_home/systemd/vpsman-agent.service"
assert_no_transaction_artifacts "$managed_concurrent_winner_home"
manager_lock_path="/run/user/$(id -u)/.vpsman-agent-install.lock"
test -f "$manager_lock_path"
test ! -L "$manager_lock_path"
test "$(stat -c '%u' "$manager_lock_path")" = "$(id -u)"
test "$(stat -c '%a' "$manager_lock_path")" = 600
((concurrent_install_lock_cases += 1))

mktemp_registration_race_cases=0
real_mktemp="$(command -v mktemp)"
mktemp_fresh_parent="$SMOKE_TMPDIR/mktemp-race-fresh-parent"
mktemp_fresh_home="$mktemp_fresh_parent/agent-home"
mktemp_fresh_counter="$SMOKE_TMPDIR/mktemp-race-fresh.counter"
mktemp_fresh_paths="$SMOKE_TMPDIR/mktemp-race-fresh.paths"
mktemp_fresh_marker="$SMOKE_TMPDIR/mktemp-race-fresh.marker"
mktemp_fresh_log="$SMOKE_TMPDIR/mktemp-race-fresh.log"
if env \
  PATH="$interrupt_mktemp_bin:$no_systemctl_bin" \
  VPSMAN_SMOKE_REAL_MKTEMP="$real_mktemp" \
  VPSMAN_SMOKE_MKTEMP_COUNTER="$mktemp_fresh_counter" \
  VPSMAN_SMOKE_MKTEMP_PATH_LOG="$mktemp_fresh_paths" \
  VPSMAN_SMOKE_ABORT_AFTER_MKTEMP_AT=1 \
  VPSMAN_SMOKE_MKTEMP_ABORT_MARKER="$mktemp_fresh_marker" \
  VPSMAN_SMOKE_MKTEMP_ABORT_SIGNAL=TERM \
  VPSMAN_AGENT_HOME="$mktemp_fresh_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.mktemp-race-fresh \
  bash "$ROOT_DIR/deploy/install-agent.sh" >"$mktemp_fresh_log" 2>&1; then
  echo "expected TERM in fresh staging-temp registration window to abort" >&2
  exit 1
fi
test "$(cat "$mktemp_fresh_marker")" = 1
test "$(awk 'END { print NR }' "$mktemp_fresh_paths")" -eq 1
assert_logged_temp_paths_absent "$mktemp_fresh_paths"
test ! -e "$mktemp_fresh_parent"
((mktemp_registration_race_cases += 1))

for mktemp_step in 2 3 4 5 6 7; do
  case "$((mktemp_step % 3))" in
    0) mktemp_signal=HUP ;;
    1) mktemp_signal=INT ;;
    2) mktemp_signal=TERM ;;
  esac
  mktemp_race_state="$SMOKE_TMPDIR/mktemp-race-$mktemp_step.state"
  mktemp_race_systemctl_log="$SMOKE_TMPDIR/mktemp-race-$mktemp_step.systemctl.log"
  mktemp_race_counter="$SMOKE_TMPDIR/mktemp-race-$mktemp_step.counter"
  mktemp_race_paths="$SMOKE_TMPDIR/mktemp-race-$mktemp_step.paths"
  mktemp_race_marker="$SMOKE_TMPDIR/mktemp-race-$mktemp_step.marker"
  mktemp_race_log="$SMOKE_TMPDIR/mktemp-race-$mktemp_step.log"
  init_systemctl_state \
    "$mktemp_race_state" \
    enabled \
    active \
    "$agent_home/systemd/vpsman-agent.service"
  topology_before="$(
    sha256sum "$mktemp_race_state/topology" |
      awk '{print $1}'
  )"

  if env \
    PATH="$interrupt_mktemp_bin:$fake_bin_dir:$PATH" \
    VPSMAN_SMOKE_REAL_MKTEMP="$real_mktemp" \
    VPSMAN_SMOKE_MKTEMP_COUNTER="$mktemp_race_counter" \
    VPSMAN_SMOKE_MKTEMP_PATH_LOG="$mktemp_race_paths" \
    VPSMAN_SMOKE_ABORT_AFTER_MKTEMP_AT="$mktemp_step" \
    VPSMAN_SMOKE_MKTEMP_ABORT_MARKER="$mktemp_race_marker" \
    VPSMAN_SMOKE_MKTEMP_ABORT_SIGNAL="$mktemp_signal" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$mktemp_race_systemctl_log" \
    VPSMAN_FAKE_SYSTEMCTL_STATE="$mktemp_race_state" \
    VPSMAN_AGENT_HOME="$agent_home" \
    VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
    "${common_env[@]}" \
    VPSMAN_AGENT_CLIENT_ID="deploy.smoke.mktemp-race-$mktemp_step" \
    bash deploy/install-agent.sh >"$mktemp_race_log" 2>&1; then
    echo "expected $mktemp_signal after mktemp step $mktemp_step to abort" >&2
    exit 1
  fi
  test "$(cat "$mktemp_race_marker")" = "$mktemp_step"
  test "$(awk 'END { print NR }' "$mktemp_race_paths")" -eq "$mktemp_step"
  assert_logged_temp_paths_absent "$mktemp_race_paths"
  assert_no_transaction_artifacts "$agent_home"
  assert_existing_install_unchanged
  assert_systemctl_state \
    "$mktemp_race_state" \
    loaded \
    active \
    enabled \
    "$agent_home/systemd/vpsman-agent.service"
  test "$topology_before" = \
    "$(sha256sum "$mktemp_race_state/topology" | awk '{print $1}')"
  if grep -Eq -- "--user (link|enable|disable)" "$mktemp_race_systemctl_log"; then
    echo "mktemp interruption must not mutate enabled unit-file topology" >&2
    exit 1
  fi
  if ((mktemp_step >= 5)); then
    grep -Fq "received $mktemp_signal during installation; restoring the prior state" \
      "$mktemp_race_log"
    grep -Fqx -- "--user stop vpsman-agent.service" \
      "$mktemp_race_systemctl_log"
    grep -Fqx -- "--user start vpsman-agent.service" \
      "$mktemp_race_systemctl_log"
  elif grep -Eq -- "--user (stop|start|restart|daemon-reload)" \
    "$mktemp_race_systemctl_log"; then
    echo "pre-transaction mktemp interruption must not mutate service state" >&2
    exit 1
  fi
  ((mktemp_registration_race_cases += 1))
done

rollback_preservation_cases=0
preserve_home="$SMOKE_TMPDIR/rollback-preserve-home"
preserve_state="$SMOKE_TMPDIR/rollback-preserve.state"
preserve_log="$SMOKE_TMPDIR/rollback-preserve.log"
preserve_systemctl_log="$SMOKE_TMPDIR/rollback-preserve.systemctl.log"
preserve_counter="$SMOKE_TMPDIR/rollback-preserve.counter"
cp -a "$agent_home" "$preserve_home"
init_systemctl_state \
  "$preserve_state" \
  enabled \
  active \
  "$preserve_home/systemd/vpsman-agent.service"
if env \
  PATH="$fail_mv_bin:$fake_bin_dir:$PATH" \
  VPSMAN_SMOKE_REAL_MV="$real_mv" \
  VPSMAN_SMOKE_MV_COUNTER="$preserve_counter" \
  VPSMAN_SMOKE_EXIT_AFTER_MV_AT=2 \
  VPSMAN_SMOKE_FAIL_MV_AT=3 \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$preserve_systemctl_log" \
  VPSMAN_FAKE_SYSTEMCTL_STATE="$preserve_state" \
  VPSMAN_AGENT_HOME="$preserve_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.rollback-preserve \
  bash deploy/install-agent.sh >"$preserve_log" 2>&1; then
  echo "expected rollback restoration failure to remain nonzero" >&2
  exit 1
fi
grep -Fq "automatic rollback was incomplete; preserved rollback originals" \
  "$preserve_log"
grep -Fq "filesystem rollback was incomplete; leaving vpsman-agent.service inactive" \
  "$preserve_log"
test "$(find "$preserve_home" -type d -name '*.rollback.*' | wc -l)" -eq 3
test "$(find "$preserve_home" -type f -path '*.rollback.*/original' | wc -l)" -eq 3
preserved_config_original="$(
  find "$preserve_home/config" -type f -path '*.rollback.*/original' -print -quit
)"
grep -Fq 'client_id = "deploy.smoke.active-rerun"' "$preserved_config_original"
assert_systemctl_state \
  "$preserve_state" \
  loaded \
  inactive \
  enabled \
  "$preserve_home/systemd/vpsman-agent.service"
if grep -Eq -- "--user (start|restart|link|enable|disable)" \
  "$preserve_systemctl_log"; then
  echo "incomplete filesystem rollback must leave the owned service inactive without topology mutation" >&2
  exit 1
fi
((rollback_preservation_cases += 1))

invalid_source_preflight_count=0
run_invalid_source_preflight() {
  local case_name="$1" expected_error="$2" command_path="$3"
  local existing_log="$SMOKE_TMPDIR/invalid-source-$case_name-existing.log"
  local fresh_home="$SMOKE_TMPDIR/invalid-source-$case_name-fresh-home"
  local fresh_log="$SMOKE_TMPDIR/invalid-source-$case_name-fresh.log"
  shift 3

  if env \
    PATH="$command_path" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
    VPSMAN_AGENT_HOME="$agent_home" \
    VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
    "${common_env[@]}" \
    "$@" \
    bash "$ROOT_DIR/deploy/install-agent.sh" >"$existing_log" 2>&1; then
    echo "expected installer preflight refusal for existing install: $case_name" >&2
    exit 1
  fi
  grep -Fq "$expected_error" "$existing_log"
  assert_existing_install_unchanged

  if env \
    PATH="$command_path" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
    VPSMAN_AGENT_HOME="$fresh_home" \
    VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
    "${common_env[@]}" \
    "$@" \
    bash "$ROOT_DIR/deploy/install-agent.sh" >"$fresh_log" 2>&1; then
    echo "expected installer preflight refusal for fresh install: $case_name" >&2
    exit 1
  fi
  grep -Fq "$expected_error" "$fresh_log"
  test ! -e "$fresh_home"
  assert_existing_install_unchanged
  ((invalid_source_preflight_count += 1))
}

invalid_override_count=0
run_invalid_override() {
  local case_name="$1" expected_error="$2" override="$3"
  local log_file="$SMOKE_TMPDIR/invalid-override-$case_name.log"

  if (
    cd "$SMOKE_TMPDIR"
    env \
      PATH="$fake_bin_dir:$PATH" \
      VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
      VPSMAN_AGENT_HOME="$agent_home" \
      VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
      "${common_env[@]}" \
      "$override" \
      bash "$ROOT_DIR/deploy/install-agent.sh"
  ) >"$log_file" 2>&1; then
    echo "expected deploy installer to reject override case: $case_name" >&2
    exit 1
  fi
  grep -Fq "$expected_error" "$log_file"
  assert_existing_install_unchanged
  ((invalid_override_count += 1))
}

unsafe_anchor_refusal_cases=0
unsafe_anchor="$SMOKE_TMPDIR/unsafe-install-anchor"
unsafe_anchor_home="$unsafe_anchor/agent-home"
unsafe_anchor_log="$SMOKE_TMPDIR/unsafe-install-anchor.log"
mkdir -p "$unsafe_anchor"
chmod 0777 "$unsafe_anchor"
if env \
  PATH="$no_systemctl_bin" \
  VPSMAN_AGENT_HOME="$unsafe_anchor_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.unsafe-anchor \
  bash "$ROOT_DIR/deploy/install-agent.sh" >"$unsafe_anchor_log" 2>&1; then
  echo "expected a writable cross-principal install anchor to fail preflight" >&2
  exit 1
fi
grep -Fq \
  "VPSMAN_AGENT_HOME creation path has unsafe writable ancestor: $unsafe_anchor" \
  "$unsafe_anchor_log"
test ! -e "$unsafe_anchor_home"
((unsafe_anchor_refusal_cases += 1))

nested_unsafe_parent="$SMOKE_TMPDIR/nested-unsafe-parent"
nested_safe_anchor="$nested_unsafe_parent/operator-owned"
nested_unsafe_home="$nested_safe_anchor/agent-home"
nested_unsafe_log="$SMOKE_TMPDIR/nested-unsafe-anchor.log"
mkdir -p "$nested_safe_anchor"
chmod 0777 "$nested_unsafe_parent"
chmod 0700 "$nested_safe_anchor"
if env \
  PATH="$no_systemctl_bin" \
  VPSMAN_AGENT_HOME="$nested_unsafe_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.nested-unsafe-anchor \
  bash "$ROOT_DIR/deploy/install-agent.sh" >"$nested_unsafe_log" 2>&1; then
  echo "expected an unsafe ancestor above a trusted creation anchor to fail preflight" >&2
  exit 1
fi
grep -Fq \
  "VPSMAN_AGENT_HOME creation path has unsafe writable ancestor: $nested_unsafe_parent" \
  "$nested_unsafe_log"
test ! -e "$nested_unsafe_home"
((unsafe_anchor_refusal_cases += 1))

topology_override_refusal_cases=0
topology_home="$SMOKE_TMPDIR/topology-override-home"
topology_log="$SMOKE_TMPDIR/topology-override.log"
if env \
  PATH="$no_systemctl_bin" \
  VPSMAN_AGENT_HOME="$topology_home" \
  VPSMAN_AGENT_CONFIG_DIR="$topology_home/bin/vpsman-agent/config" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.topology-override \
  bash "$ROOT_DIR/deploy/install-agent.sh" >"$topology_log" 2>&1; then
  echo "expected managed directory below the binary target to fail preflight" >&2
  exit 1
fi
grep -Fq \
  "VPSMAN_AGENT_CONFIG_DIR must not equal or be nested below the managed agent binary target" \
  "$topology_log"
test ! -e "$topology_home"
assert_existing_install_unchanged
((topology_override_refusal_cases += 1))

service_name_refusal_cases=0
alternate_service_log="$SMOKE_TMPDIR/alternate-service-name.log"
alternate_service_systemctl_log="$SMOKE_TMPDIR/alternate-service-name.systemctl.log"
if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$alternate_service_systemctl_log" \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  VPSMAN_AGENT_SERVICE_NAME=alternate-agent.service \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.alternate-service-name \
  bash deploy/install-agent.sh >"$alternate_service_log" 2>&1; then
  echo "expected an existing install with an alternate service name to fail preflight" >&2
  exit 1
fi
grep -Fq \
  "VPSMAN_AGENT_SERVICE_NAME must be vpsman-agent.service; custom service identities are not upgrade-safe" \
  "$alternate_service_log"
test ! -s "$alternate_service_systemctl_log"
assert_existing_install_unchanged
((service_name_refusal_cases += 1))

fresh_custom_service_home="$SMOKE_TMPDIR/fresh-custom-service-home"
fresh_custom_service_log="$SMOKE_TMPDIR/fresh-custom-service.log"
if env \
  PATH="$no_systemctl_bin" \
  VPSMAN_AGENT_HOME="$fresh_custom_service_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  VPSMAN_AGENT_SERVICE_NAME=alternate-agent.service \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.fresh-custom-service-name \
  bash "$ROOT_DIR/deploy/install-agent.sh" >"$fresh_custom_service_log" 2>&1; then
  echo "expected a fresh install with a non-upgradeable custom service name to fail" >&2
  exit 1
fi
grep -Fq \
  "VPSMAN_AGENT_SERVICE_NAME must be vpsman-agent.service; custom service identities are not upgrade-safe" \
  "$fresh_custom_service_log"
test ! -e "$fresh_custom_service_home"
assert_existing_install_unchanged
((service_name_refusal_cases += 1))

invalid_client_id_count=0
run_invalid_client_id() {
  local case_name="$1" expected_error="$2" override="$3"
  local fresh_home="$SMOKE_TMPDIR/fresh-invalid-client-id-$case_name"
  local fresh_log="$SMOKE_TMPDIR/invalid-client-id-$case_name-fresh.log"

  run_invalid_override \
    "client-id-$case_name-existing" \
    "$expected_error" \
    "$override"

  if env \
    PATH="$fake_bin_dir:$PATH" \
    VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
    VPSMAN_AGENT_HOME="$fresh_home" \
    VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
    "${common_env[@]}" \
    "$override" \
    bash "$ROOT_DIR/deploy/install-agent.sh" >"$fresh_log" 2>&1; then
    echo "expected deploy installer to reject fresh client id case: $case_name" >&2
    exit 1
  fi
  grep -Fq "$expected_error" "$fresh_log"
  test ! -e "$fresh_home"
  assert_existing_install_unchanged
  ((invalid_client_id_count += 1))
}

run_invalid_client_id \
  empty \
  "VPSMAN_AGENT_CLIENT_ID is required" \
  "VPSMAN_AGENT_CLIENT_ID="
run_invalid_client_id \
  whitespace \
  "VPSMAN_AGENT_CLIENT_ID must contain only ASCII letters, digits, '.', '_', ':', and '-'" \
  "VPSMAN_AGENT_CLIENT_ID=agent id"
client_id_control=$'VPSMAN_AGENT_CLIENT_ID=agent\nid'
run_invalid_client_id \
  control-character \
  "VPSMAN_AGENT_CLIENT_ID must contain only ASCII letters, digits, '.', '_', ':', and '-'" \
  "$client_id_control"
client_id_non_ascii=$'VPSMAN_AGENT_CLIENT_ID=agent-\303\251'
run_invalid_client_id \
  non-ascii \
  "VPSMAN_AGENT_CLIENT_ID must contain only ASCII letters, digits, '.', '_', ':', and '-'" \
  "$client_id_non_ascii"
run_invalid_client_id \
  invalid-punctuation \
  "VPSMAN_AGENT_CLIENT_ID must contain only ASCII letters, digits, '.', '_', ':', and '-'" \
  "VPSMAN_AGENT_CLIENT_ID=agent+id"
printf -v client_id_too_long '%129s' ''
client_id_too_long="${client_id_too_long// /a}"
run_invalid_client_id \
  too-long \
  "VPSMAN_AGENT_CLIENT_ID must not exceed 128 ASCII bytes" \
  "VPSMAN_AGENT_CLIENT_ID=$client_id_too_long"

run_invalid_override \
  relative-agent-home \
  "VPSMAN_AGENT_HOME must be an absolute path using only systemd-safe path characters" \
  "VPSMAN_AGENT_HOME=relative-agent-home"
test ! -e "$SMOKE_TMPDIR/relative-agent-home"

run_invalid_override \
  relative-state-dir \
  "VPSMAN_AGENT_STATE_DIR must be an absolute path using only systemd-safe path characters" \
  "VPSMAN_AGENT_STATE_DIR=relative-state"
test ! -e "$SMOKE_TMPDIR/relative-state"

run_invalid_override \
  config-traversal \
  "VPSMAN_AGENT_CONFIG_DIR must not contain dot or parent-directory traversal segments" \
  "VPSMAN_AGENT_CONFIG_DIR=$agent_home/config/../escape"
test ! -e "$agent_home/escape"

outside_config_dir="$SMOKE_TMPDIR/outside-config"
run_invalid_override \
  config-outside-home \
  "VPSMAN_AGENT_CONFIG_DIR must remain inside VPSMAN_AGENT_HOME" \
  "VPSMAN_AGENT_CONFIG_DIR=$outside_config_dir"
test ! -e "$outside_config_dir"

control_log_dir="${agent_home}/"$'log\nforged'
run_invalid_override \
  log-dir-control-character \
  "VPSMAN_AGENT_LOG_DIR must be an absolute path using only systemd-safe path characters" \
  "VPSMAN_AGENT_LOG_DIR=$control_log_dir"
test ! -e "$control_log_dir"

outside_systemd_dir="$SMOKE_TMPDIR/outside-systemd"
run_invalid_override \
  systemd-dir-outside-home \
  "VPSMAN_AGENT_SYSTEMD_DIR must remain inside VPSMAN_AGENT_HOME" \
  "VPSMAN_AGENT_SYSTEMD_DIR=$outside_systemd_dir"
test ! -e "$outside_systemd_dir"

run_invalid_override \
  service-name-traversal \
  "VPSMAN_AGENT_SERVICE_NAME must be a safe systemd .service unit name" \
  "VPSMAN_AGENT_SERVICE_NAME=../escaped.service"
test ! -e "$agent_home/escaped.service"

run_invalid_override \
  config-symlink-traversal \
  "VPSMAN_AGENT_CONFIG_DIR must not traverse symbolic links" \
  "VPSMAN_AGENT_CONFIG_DIR=$agent_home/config-link"
test ! -e "$symlink_config_target/agent.toml"

run_invalid_override \
  systemd-dir-symlink-traversal \
  "VPSMAN_AGENT_SYSTEMD_DIR must not traverse symbolic links" \
  "VPSMAN_AGENT_SYSTEMD_DIR=$agent_home/systemd-link"
test ! -e "$symlink_systemd_target/vpsman-agent.service"

run_invalid_override \
  retry-noncanonical \
  "VPSMAN_GATEWAY_RETRY_SECS must be a canonical integer from 1 through 3600" \
  "VPSMAN_GATEWAY_RETRY_SECS=060"
run_invalid_override \
  retry-out-of-range \
  "VPSMAN_GATEWAY_RETRY_SECS must be a canonical integer from 1 through 3600" \
  "VPSMAN_GATEWAY_RETRY_SECS=3601"
run_invalid_override \
  connect-timeout-noncanonical \
  "VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS must be a canonical integer from 1 through 300" \
  "VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS=+10"
run_invalid_override \
  connect-timeout-out-of-range \
  "VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS must be a canonical integer from 1 through 300" \
  "VPSMAN_GATEWAY_CONNECT_TIMEOUT_SECS=301"

if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  "${common_env[@]}" \
  VPSMAN_GATEWAY_ENDPOINTS='primary=127.0.0.1:9443=10,malformed-later-entry' \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/invalid-existing.log" 2>&1; then
  echo "expected deploy installer to reject a malformed later endpoint" >&2
  exit 1
fi
grep -q "endpoint must be label=host:port=priority" \
  "$SMOKE_TMPDIR/invalid-existing.log"
assert_existing_install_unchanged

if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$invalid_fresh_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  "${common_env[@]}" \
  VPSMAN_GATEWAY_ENDPOINTS='ipv6=2001:db8::1:9443=10' \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/invalid-address.log" 2>&1; then
  echo "expected deploy installer to reject unbracketed IPv6" >&2
  exit 1
fi
grep -q "endpoint address must be IPv4:port, hostname:port, or \\[IPv6\\]:port" \
  "$SMOKE_TMPDIR/invalid-address.log"
test ! -e "$invalid_fresh_home"

if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_AGENT_HOME="$invalid_fresh_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  "${common_env[@]}" \
  VPSMAN_GATEWAY_ENDPOINTS='ipv4=001.2.3.4:9443=10' \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/ambiguous-ipv4.log" 2>&1; then
  echo "expected deploy installer to reject leading-zero IPv4 octets" >&2
  exit 1
fi
grep -q "endpoint address must be IPv4:port, hostname:port, or \\[IPv6\\]:port" \
  "$SMOKE_TMPDIR/ambiguous-ipv4.log"
test ! -e "$invalid_fresh_home"

if env \
  PATH="$no_systemctl_bin" \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/missing-systemctl.log" 2>&1; then
  echo "expected deploy installer to require systemctl before service installation" >&2
  exit 1
fi
grep -q "systemctl is required when VPSMAN_AGENT_ENABLE_SERVICE=1" \
  "$SMOKE_TMPDIR/missing-systemctl.log"
assert_existing_install_unchanged

if env \
  PATH="$no_systemctl_bin" \
  VPSMAN_AGENT_HOME="$agent_home" \
  VPSMAN_AGENT_BINARY_PATH="$replacement_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/missing-systemctl-staging-existing.log" 2>&1; then
  echo "expected existing staging-only install to require systemd state inspection" >&2
  exit 1
fi
grep -q "systemctl is required to prove an existing staging-only unit is unregistered" \
  "$SMOKE_TMPDIR/missing-systemctl-staging-existing.log"
assert_existing_install_unchanged

env \
  PATH="$no_systemctl_bin" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$staged_home" \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/staged-only.log" 2>&1

test -x "$staged_home/bin/vpsman-agent"
if grep -q "$staged_home" "$fake_systemctl_log"; then
  echo "staging-only install must not call systemctl" >&2
  exit 1
fi
grep -q "staging-only install complete; no service was started" "$SMOKE_TMPDIR/staged-only.log"
grep -Fq "start in foreground:" "$SMOKE_TMPDIR/staged-only.log"
grep -Fq "$staged_home/bin/vpsman-agent" "$SMOKE_TMPDIR/staged-only.log"

staging_unlinked_success_cases=0
staged_unlinked_state="$SMOKE_TMPDIR/systemctl-state-staged-unlinked"
staged_unlinked_systemctl_log="$SMOKE_TMPDIR/systemctl-staged-unlinked.log"
init_systemctl_state "$staged_unlinked_state" unlinked inactive
env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$staged_unlinked_systemctl_log" \
  VPSMAN_FAKE_SYSTEMCTL_STATE="$staged_unlinked_state" \
  VPSMAN_AGENT_HOME="$staged_home" \
  VPSMAN_AGENT_BINARY_PATH="$active_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  VPSMAN_AGENT_CLIENT_ID=deploy.smoke.staged-unlinked-rerun \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/staged-unlinked-rerun.log" 2>&1
test "$("$staged_home/bin/vpsman-agent")" = active-rerun-installed
assert_systemctl_state \
  "$staged_unlinked_state" \
  not-found \
  inactive \
  "" \
  ""
test "$(awk 'END { print NR }' "$staged_unlinked_systemctl_log")" -eq 1
((staging_unlinked_success_cases += 1))

path_systemctl_state="$SMOKE_TMPDIR/systemctl-state-path"
path_systemctl_log="$SMOKE_TMPDIR/systemctl-path.log"
init_systemctl_state "$path_systemctl_state" unlinked inactive
env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$path_systemctl_log" \
  VPSMAN_FAKE_SYSTEMCTL_STATE="$path_systemctl_state" \
  VPSMAN_AGENT_HOME="$path_home" \
  VPSMAN_AGENT_USE_PATH=1 \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/use-path.log" 2>&1

test -x "$path_home/bin/vpsman-agent"
grep -Fq "staging-only install complete" "$SMOKE_TMPDIR/use-path.log"

download_systemctl_state="$SMOKE_TMPDIR/systemctl-state-download"
download_systemctl_log="$SMOKE_TMPDIR/systemctl-download.log"
init_systemctl_state "$download_systemctl_state" unlinked inactive
env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$download_systemctl_log" \
  VPSMAN_FAKE_SYSTEMCTL_STATE="$download_systemctl_state" \
  VPSMAN_AGENT_HOME="$download_home" \
  VPSMAN_AGENT_BINARY_URL="file://$fake_agent" \
  VPSMAN_AGENT_BINARY_SHA256="$fake_agent_sha" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/custom-url.log" 2>&1

test -x "$download_home/bin/vpsman-agent"

release_manifest_url="https://releases.example.invalid/v1.2.3/version.json"
case "$(uname -m)" in
  x86_64 | amd64)
    release_asset_name="vpsman-agent-linux-x86_64-musl"
    ;;
  aarch64 | arm64)
    release_asset_name="vpsman-agent-linux-aarch64-musl"
    ;;
  *)
    echo "unsupported smoke-test architecture: $(uname -m)" >&2
    exit 1
    ;;
esac
release_asset_url="https://artifacts.example.invalid/opaque/$release_asset_name"
jq -n \
  --arg name "$release_asset_name" \
  --arg url "$release_asset_url" \
  '{assets:[{download_url:$url,name:$name}],project:"vpsman",schema_version:3,tag:"v1.2.3"}' \
  >"$release_manifest"
env \
  PATH="$release_download_bin:$fake_bin_dir:$PATH" \
  VPSMAN_AGENT_HOME="$release_download_home" \
  VPSMAN_AGENT_BINARY_PATH= \
  VPSMAN_AGENT_BINARY_URL= \
  VPSMAN_AGENT_USE_PATH=0 \
  VPSMAN_AGENT_RELEASE=v1.2.3 \
  VPSMAN_RELEASE_BASE_URL="${release_manifest_url%/version.json}" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  VPSMAN_SMOKE_RELEASE_CURL_LOG="$release_curl_log" \
  VPSMAN_SMOKE_RELEASE_MANIFEST_URL="$release_manifest_url" \
  VPSMAN_SMOKE_RELEASE_MANIFEST="$release_manifest" \
  VPSMAN_SMOKE_RELEASE_ASSET_URL="$release_asset_url" \
  VPSMAN_SMOKE_RELEASE_AGENT="$fake_agent" \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/release-download.log" 2>&1

test "$("$release_download_home/bin/vpsman-agent")" = "vpsman-agent-deploy-smoke"
grep -Fqx "$release_manifest_url" "$release_curl_log"
grep -Fqx "$release_asset_url" "$release_curl_log"
test "$(awk 'END { print NR }' "$release_curl_log")" -eq 2

run_invalid_source_preflight \
  missing-curl \
  "missing required tool: curl" \
  "$missing_curl_bin" \
  VPSMAN_AGENT_BINARY_PATH= \
  VPSMAN_AGENT_BINARY_URL="file://$fake_agent" \
  VPSMAN_AGENT_BINARY_SHA256="$fake_agent_sha" \
  VPSMAN_AGENT_ENABLE_SERVICE=0

run_invalid_source_preflight \
  missing-sha256sum \
  "missing required tool: sha256sum" \
  "$missing_sha_bin" \
  VPSMAN_AGENT_BINARY_PATH= \
  VPSMAN_AGENT_BINARY_URL="file://$fake_agent" \
  VPSMAN_AGENT_BINARY_SHA256="$fake_agent_sha" \
  VPSMAN_AGENT_ENABLE_SERVICE=0

run_invalid_source_preflight \
  missing-sha256 \
  "VPSMAN_AGENT_BINARY_SHA256 must be exactly 64 hex characters" \
  "$fake_bin_dir:$PATH" \
  VPSMAN_AGENT_BINARY_PATH= \
  VPSMAN_AGENT_BINARY_URL="file://$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0

run_invalid_source_preflight \
  invalid-sha256 \
  "VPSMAN_AGENT_BINARY_SHA256 must be exactly 64 hex characters" \
  "$fake_bin_dir:$PATH" \
  VPSMAN_AGENT_BINARY_PATH= \
  VPSMAN_AGENT_BINARY_URL="file://$fake_agent" \
  VPSMAN_AGENT_BINARY_SHA256=not-a-sha256 \
  VPSMAN_AGENT_ENABLE_SERVICE=0

run_invalid_source_preflight \
  sha256-mismatch \
  "computed checksum did NOT match" \
  "$fake_bin_dir:$PATH" \
  VPSMAN_AGENT_BINARY_PATH= \
  VPSMAN_AGENT_BINARY_URL="file://$fake_agent" \
  VPSMAN_AGENT_BINARY_SHA256=0000000000000000000000000000000000000000000000000000000000000000 \
  VPSMAN_AGENT_ENABLE_SERVICE=0

run_invalid_source_preflight \
  invalid-enable-service-boolean \
  "VPSMAN_AGENT_ENABLE_SERVICE must be a boolean" \
  "$fake_bin_dir:$PATH" \
  VPSMAN_AGENT_ENABLE_SERVICE=maybe

run_invalid_source_preflight \
  invalid-use-path-boolean \
  "VPSMAN_AGENT_USE_PATH must be a boolean" \
  "$fake_bin_dir:$PATH" \
  VPSMAN_AGENT_USE_PATH=perhaps

run_invalid_source_preflight \
  missing-binary-path \
  "VPSMAN_AGENT_BINARY_PATH must be a readable regular file" \
  "$fake_bin_dir:$PATH" \
  "VPSMAN_AGENT_BINARY_PATH=$SMOKE_TMPDIR/does-not-exist" \
  VPSMAN_AGENT_ENABLE_SERVICE=0

run_invalid_source_preflight \
  ambiguous-binary-source \
  "set only one of VPSMAN_AGENT_BINARY_PATH" \
  "$fake_bin_dir:$PATH" \
  VPSMAN_AGENT_BINARY_URL="file://$fake_agent" \
  VPSMAN_AGENT_BINARY_SHA256="$fake_agent_sha" \
  VPSMAN_AGENT_ENABLE_SERVICE=0

run_invalid_source_preflight \
  checksum-without-url \
  "VPSMAN_AGENT_BINARY_SHA256 is only valid with VPSMAN_AGENT_BINARY_URL" \
  "$fake_bin_dir:$PATH" \
  VPSMAN_AGENT_BINARY_SHA256="$fake_agent_sha" \
  VPSMAN_AGENT_ENABLE_SERVICE=0

if env \
  PATH="$fake_bin_dir:$PATH" \
  VPSMAN_FAKE_SYSTEMCTL_LOG="$fake_systemctl_log" \
  VPSMAN_AGENT_HOME="$SMOKE_TMPDIR/obsolete-env-home" \
  VPSMAN_AGENT_DISPLAY_NAME=obsolete-local-display \
  VPSMAN_AGENT_BINARY_PATH="$fake_agent" \
  VPSMAN_AGENT_ENABLE_SERVICE=0 \
  "${common_env[@]}" \
  bash deploy/install-agent.sh >"$SMOKE_TMPDIR/obsolete-env.log" 2>&1; then
  echo "expected deploy installer to reject runtime config env in bootstrap install" >&2
  exit 1
fi
grep -q "VPSMAN_AGENT_DISPLAY_NAME is server runtime config" \
  "$SMOKE_TMPDIR/obsolete-env.log"

jq -n \
  --argjson atomic_publication_failure_cases "$atomic_publication_failure_cases" \
  --argjson concurrent_install_lock_cases "$concurrent_install_lock_cases" \
  --argjson directory_rollback_cases "$directory_rollback_cases" \
  --argjson invalid_override_cases "$invalid_override_count" \
  --argjson invalid_client_id_cases "$invalid_client_id_count" \
  --argjson invalid_source_preflight_cases "$invalid_source_preflight_count" \
  --argjson mktemp_registration_race_cases "$mktemp_registration_race_cases" \
  --argjson publication_interrupt_cases "$publication_interrupt_cases" \
  --argjson rollback_preservation_cases "$rollback_preservation_cases" \
  --argjson rollback_signal_resilience_cases "$rollback_signal_resilience_cases" \
  --argjson service_action_failure_cases "$service_action_failure_cases" \
  --argjson service_interrupt_cases "$service_interrupt_cases" \
  --argjson staging_active_refusal_cases "$staging_active_refusal_cases" \
  --argjson staging_unlinked_success_cases "$staging_unlinked_success_cases" \
  --argjson supported_state_restore_cases "$supported_state_restore_cases" \
  --argjson systemd_state_refusal_cases "$systemd_state_refusal_cases" \
  --argjson topology_override_refusal_cases "$topology_override_refusal_cases" \
  --argjson unsafe_anchor_refusal_cases "$unsafe_anchor_refusal_cases" \
  --argjson service_name_refusal_cases "$service_name_refusal_cases" \
  '{
    deploy_install_agent: "ok",
    atomic_publication_failure_cases: $atomic_publication_failure_cases,
    concurrent_install_lock_cases: $concurrent_install_lock_cases,
    directory_rollback_cases: $directory_rollback_cases,
    invalid_override_cases: $invalid_override_cases,
    invalid_client_id_cases: $invalid_client_id_cases,
    invalid_source_preflight_cases: $invalid_source_preflight_cases,
    mktemp_registration_race_cases: $mktemp_registration_race_cases,
    publication_interrupt_cases: $publication_interrupt_cases,
    rollback_preservation_cases: $rollback_preservation_cases,
    rollback_signal_resilience_cases: $rollback_signal_resilience_cases,
    service_action_failure_cases: $service_action_failure_cases,
    service_interrupt_cases: $service_interrupt_cases,
    staging_active_refusal_cases: $staging_active_refusal_cases,
    staging_unlinked_success_cases: $staging_unlinked_success_cases,
    supported_state_restore_cases: $supported_state_restore_cases,
    systemd_state_refusal_cases: $systemd_state_refusal_cases,
    topology_override_refusal_cases: $topology_override_refusal_cases,
    unsafe_anchor_refusal_cases: $unsafe_anchor_refusal_cases,
    service_name_refusal_cases: $service_name_refusal_cases
  }'
