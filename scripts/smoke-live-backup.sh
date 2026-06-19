#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools base64 curl docker grep jq python3 sha256sum shuf timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-live-backup"

api_port="$(smoke_free_port)"
pg_port="$(smoke_free_port)"
gateway_port="$(smoke_free_port)"
gateway_control_port="$(smoke_free_port)"

api_url="http://127.0.0.1:$api_port"
smoke_start_postgres "vpsman-live-backup-postgres" "$pg_port" >/dev/null
postgres_url="$SMOKE_POSTGRES_URL"
gateway_addr="127.0.0.1:$gateway_port"
gateway_control_url="http://127.0.0.1:$gateway_control_port"
internal_token="backup-smoke-internal-$(date +%s%N)"
client_id="backup-smoke-$(date +%s)"
super_password="smoke-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
privilege_verifier_key_hex="$(smoke_privilege_verifier_key_hex "$super_password" "$super_salt_hex")"

gateway_keys="$(target/debug/vpsctl noise-keygen)"
gateway_private_hex="$(jq -r '.private_key_hex' <<<"$gateway_keys")"
gateway_public_hex="$(jq -r '.public_key_hex' <<<"$gateway_keys")"
backup_keys="$(target/debug/vpsctl noise-keygen)"
backup_public_hex="$(jq -r '.public_key_hex' <<<"$backup_keys")"

api_log="$SMOKE_TMPDIR/api.log"
gateway_log="$SMOKE_TMPDIR/gateway.log"
agent_log="$SMOKE_TMPDIR/agent.log"
agent_config="$SMOKE_TMPDIR/agent.toml"
object_store_dir="$SMOKE_TMPDIR/object-store"
backup_source_dir="$SMOKE_TMPDIR/source"
selected_file="$backup_source_dir/selected.txt"
mkdir -p "$backup_source_dir"

selected_payload="vpsman live backup secret payload $(date +%s%N)"
printf '%s\n' "$selected_payload" >"$selected_file"
selected_sha="$(sha256sum "$selected_file" | awk '{print $1}')"

VPSMAN_API_BIND="127.0.0.1:$api_port" \
VPSMAN_POSTGRES_URL="$postgres_url" \
VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_GATEWAY_CONTROL_URL="$gateway_control_url" \
VPSMAN_PUBLIC_GATEWAY_ENDPOINTS="primary=$gateway_addr=10" \
VPSMAN_GATEWAY_SERVER_PUBLIC_KEY_HEX="$gateway_public_hex" \
VPSMAN_BACKUP_OBJECT_STORE_DIR="$object_store_dir" \
RUST_LOG="vpsman_api=warn" \
  target/debug/vpsman-api >"$api_log" 2>&1 &
smoke_track_pid "$!"
if ! smoke_wait_http "$api_url/health"; then
  smoke_dump_logs "API did not become healthy for live backup smoke" "$api_log"
  exit 1
fi

auth_json="$(curl -fsS \
  -H "Content-Type: application/json" \
  -d '{"username":"backup-smoke","password":"backup-smoke-password"}' \
  "$api_url/api/v1/auth/bootstrap")"
access_token="$(jq -r '.access_token' <<<"$auth_json")"
export VPSMAN_API_TOKEN="$access_token"

api_auth_get() {
  curl -fsS -H "Authorization: Bearer $access_token" "$api_url$1"
}

VPSMAN_GATEWAY_BIND="$gateway_addr" \
VPSMAN_GATEWAY_CONTROL_BIND="127.0.0.1:$gateway_control_port" \
VPSMAN_GATEWAY_PRIVATE_KEY_HEX="$gateway_private_hex" \
VPSMAN_API_URL="$api_url" \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_PRIVILEGE_VERIFIER_KEY_HEX="$privilege_verifier_key_hex" \
VPSMAN_GATEWAY_ID="backup-smoke-gateway" \
VPSMAN_GATEWAY_SPOOL_DIR="$SMOKE_TMPDIR/gateway-spool" \
RUST_LOG="vpsman_gateway=warn" \
  target/debug/vpsman-gateway >"$gateway_log" 2>&1 &
smoke_track_pid "$!"
if ! smoke_wait_tcp 127.0.0.1 "$gateway_port"; then
  smoke_dump_logs "gateway agent port did not listen for live backup smoke" \
    "$api_log" "$gateway_log"
  exit 1
fi
if ! smoke_wait_tcp 127.0.0.1 "$gateway_control_port"; then
  smoke_dump_logs "gateway control port did not listen for live backup smoke" \
    "$api_log" "$gateway_log"
  exit 1
fi

smoke_create_direct_agent_config \
  "$api_url" \
  "$access_token" \
  "$agent_config" \
  "$client_id" \
  "$client_id" \
  "backup-smoke" \
  "$gateway_public_hex" \
  "primary=$gateway_addr=10"

if grep -q '^\[backup\]' "$agent_config"; then
  sed -i "/^\[backup\]/a recipient_public_key_hex = \"$backup_public_hex\"" "$agent_config"
else
  cat >>"$agent_config" <<EOF

[backup]
recipient_public_key_hex = "$backup_public_hex"
max_plaintext_bytes = 1048576
EOF
fi

VPSMAN_AGENT_CONFIG="$agent_config" \
RUST_LOG="vpsman_agent=warn" \
  target/debug/vpsman-agent run >"$agent_log" 2>&1 &
smoke_track_pid "$!"

deadline=$((SECONDS + 30))
status=""
until [[ "$status" == "online" ]]; do
  if (( SECONDS >= deadline )); then
    smoke_dump_logs "agent did not become online for live backup smoke" \
      "$api_log" "$gateway_log" "$agent_log"
    exit 1
  fi
  agents_json="$(api_auth_get "/api/v1/agents" || printf '[]')"
  status="$(jq -r --arg id "$client_id" '.[] | select(.id == $id) | .status // empty' <<<"$agents_json")"
  sleep 0.25
done

reject_body="$(jq -nc \
  --arg client "$client_id" \
  --arg path "$selected_file" \
  '{
    command: "backup",
    operation: {
      type: "backup",
      paths: [$path],
      include_config: true
    },
    selector_expression: ("id:" + $client),
    target_client_ids: [$client],
    privileged: true,
    confirmed: true,
    timeout_secs: 30
  }')"
reject_json="$SMOKE_TMPDIR/reject.json"
reject_status="$(curl -sS -o "$reject_json" -w "%{http_code}" \
  -H "Authorization: Bearer $access_token" \
  -H 'content-type: application/json' \
  -d "$reject_body" \
  "$api_url/api/v1/jobs")"
if [[ "$reject_status" != "403" ]]; then
  echo "expected no-privilege-unlock backup to return 403, got $reject_status" >&2
  cat "$reject_json" >&2 || true
  exit 1
fi
jq -e '.error == "privilege_assertion_required" and .status == 403' "$reject_json" >/dev/null

backup_request_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" backup-request \
    --client-id "$client_id" \
    --paths "$selected_file" \
    --include-config \
    --super-salt-hex "$super_salt_hex" \
    --confirmed)"
backup_request_id="$(jq -r '.id' <<<"$backup_request_json")"
jq -e --arg client "$client_id" --arg path "$selected_file" '
  .client_id == $client and .status == "requested_metadata_only" and .include_config == true and .paths == [$path] and .artifact_id == null
' <<<"$backup_request_json" >/dev/null

backup_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" backup-run \
    --paths "$selected_file" \
    --include-config \
    --clients "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 30 \
    --confirmed)"
job_id="$(jq -r '.job_id' <<<"$backup_json")"
smoke_assert_job_create_queued "$backup_json" 1
smoke_wait_api_job_status "$api_url" "$job_id" completed 45 >/dev/null

job_json="$(api_auth_get "/api/v1/jobs/$job_id")"
targets_json="$(api_auth_get "/api/v1/jobs/$job_id/targets")"
outputs_json="$(api_auth_get "/api/v1/jobs/$job_id/outputs")"
audits_json="$(api_auth_get "/api/v1/audit?limit=20")"

jq -e '.status == "completed" and .command_type == "backup"' <<<"$job_json" >/dev/null
jq -e --arg client "$client_id" '.[] | select(.client_id == $client and .status == "completed" and .exit_code == 0)' <<<"$targets_json" >/dev/null
jq -e --arg path "$selected_file" '
  .[] | select(.stream == "status" and .done == true and .exit_code == 0)
  | (.data_base64 | @base64d | fromjson)
  | .type == "backup" and .encrypted == true and .include_config == true and (.paths == [$path]) and .file_count == 2
' <<<"$outputs_json" >/dev/null
jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' <<<"$audits_json" >/dev/null

artifact_json="$(
  jq -r '.[] | select(.stream == "stdout") | .data_base64' <<<"$outputs_json" | while IFS= read -r item; do
    printf '%s' "$item" | base64 -d
  done
)"
artifact_file="$SMOKE_TMPDIR/artifact.json"
printf '%s' "$artifact_json" >"$artifact_file"
artifact_sha="$(sha256sum "$artifact_file" | awk '{print $1}')"
jq -e --arg client "$client_id" '
  .format == "vpsman.backup_artifact.v1" and
  .client_id == $client and
  .cipher == "x25519-chacha20poly1305" and
  .compression == "lz4-size-prepended" and
  (.recipient_public_key_sha256_hex | test("^[0-9a-f]{64}$")) and
  (.ciphertext_base64 | length > 0)
' <<<"$artifact_json" >/dev/null

if grep -Fq "$selected_payload" <<<"$artifact_json"; then
  echo "backup artifact leaked selected file plaintext" >&2
  exit 1
fi
jq -e --arg sha "$selected_sha" '.ciphertext_sha256_hex != $sha' <<<"$artifact_json" >/dev/null

backups_json="$(api_auth_get "/api/v1/backups?limit=20")"
artifacts_json="$(api_auth_get "/api/v1/backup-artifacts?limit=20")"
audits_json="$(api_auth_get "/api/v1/audit?limit=50")"
artifact_id="$(jq -r --arg id "$backup_request_id" '.[] | select(.id == $id) | .artifact_id' <<<"$backups_json")"
object_key="$(jq -r --arg artifact_id "$artifact_id" '.[] | select(.id == $artifact_id) | .object_key' <<<"$artifacts_json")"
jq -e --arg id "$backup_request_id" --arg artifact_id "$artifact_id" '
  .[] | select(.id == $id and .status == "artifact_metadata_recorded" and .artifact_id == $artifact_id)
' <<<"$backups_json" >/dev/null
jq -e --arg artifact_id "$artifact_id" --arg object_key "$object_key" --arg sha "$artifact_sha" '
  .[] | select(.id == $artifact_id and .object_key == $object_key and .sha256_hex == $sha and .encrypted == true)
' <<<"$artifacts_json" >/dev/null
stored_object="$object_store_dir/$object_key"
cmp -s "$artifact_file" "$stored_object"
if grep -Fq "$selected_payload" "$stored_object"; then
  echo "stored backup object leaked selected file plaintext" >&2
  exit 1
fi
jq -e '[.[].action] | index("backup.requested_metadata_only") and index("backup.artifact_metadata_recorded")' \
  <<<"$audits_json" >/dev/null

restore_archive="$SMOKE_TMPDIR/staged-restore.tar"
python3 - "$client_id" "$selected_file" "$agent_config" "$restore_archive" <<'PY'
import hashlib
import io
import json
import os
import stat
import sys
import tarfile
import time

client_id, selected_file, agent_config, archive_path = sys.argv[1:]
created_unix = int(time.time())

def read_entry(path, source, tar_path):
    with open(path, "rb") as handle:
        data = handle.read()
    mode = stat.S_IMODE(os.stat(path).st_mode) or 0o600
    entry = {
        "path": path if source == "selected_path" else "vpsman:agent_config",
        "source": source,
        "tar_path": tar_path,
        "mode": mode,
        "size_bytes": len(data),
        "sha256_hex": hashlib.sha256(data).hexdigest(),
        "mtime_unix": created_unix,
    }
    return entry, data

entries = [
    read_entry(selected_file, "selected_path", "vpsman-backup/files/0000.bin"),
    read_entry(agent_config, "agent_config", "vpsman-backup/files/0001.bin"),
]
manifest = {
    "format": "vpsman.backup_tar.v1",
    "client_id": client_id,
    "created_unix": created_unix,
    "files": [entry for entry, _ in entries],
}

with tarfile.open(archive_path, "w") as archive:
    manifest_bytes = json.dumps(manifest, separators=(",", ":")).encode()
    manifest_info = tarfile.TarInfo("vpsman-backup/manifest.json")
    manifest_info.size = len(manifest_bytes)
    manifest_info.mode = 0o600
    manifest_info.mtime = created_unix
    archive.addfile(manifest_info, fileobj=io.BytesIO(manifest_bytes))
    for entry, data in entries:
        info = tarfile.TarInfo(entry["tar_path"])
        info.size = len(data)
        info.mode = entry["mode"]
        info.mtime = created_unix
        archive.addfile(info, fileobj=io.BytesIO(data))
PY
restore_archive_size="$(python3 -c 'import os, sys; print(os.path.getsize(sys.argv[1]))' "$restore_archive")"
restore_archive_sha="$(sha256sum "$restore_archive" | awk '{print $1}')"

restore_root="$SMOKE_TMPDIR/restore-root"
restored_selected="$restore_root${selected_file}"
restored_config="$restore_root/vpsman/agent_config.toml"
restore_preexisting_payload="restore preexisting payload $(date +%s%N)"
mkdir -p "${restored_selected%/*}"
printf '%s\n' "$restore_preexisting_payload" >"$restored_selected"
restore_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" restore-run \
    --source-backup-request-id "$backup_request_id" \
    --target-client-id "$client_id" \
    --archive-path "$restore_archive" \
    --archive-size-bytes "$restore_archive_size" \
    --archive-sha256-hex "$restore_archive_sha" \
    --paths "$selected_file" \
    --include-config \
    --destination-root "$restore_root" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 30 \
    --force-unprivileged \
    --confirmed)"
restore_job_id="$(jq -r '.job_id' <<<"$restore_json")"
smoke_assert_job_create_queued "$restore_json" 1
smoke_wait_api_job_status "$api_url" "$restore_job_id" completed 45 >/dev/null
cmp -s "$selected_file" "$restored_selected"
test -s "$restored_config"
restore_outputs_json="$(api_auth_get "/api/v1/jobs/$restore_job_id/outputs")"
jq -e --arg path "$restored_selected" '
  .[] | select(.stream == "status" and .done == true and .exit_code == 0)
  | (.data_base64 | @base64d | fromjson)
  | .type == "restore" and .restored_count == 2
  and ([.restored_files[].destination_path] | index($path))
  and ([.restored_files[].rollback_path] | any(. != null))
' <<<"$restore_outputs_json" >/dev/null
rollback_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" restore-rollback \
    --restore-job-id "$restore_job_id" \
    --target-client-id "$client_id" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 30 \
    --force-unprivileged \
    --confirmed)"
rollback_job_id="$(jq -r '.job_id' <<<"$rollback_json")"
smoke_assert_job_create_queued "$rollback_json" 1
smoke_wait_api_job_status "$api_url" "$rollback_job_id" completed 45 >/dev/null
if [[ "$(cat "$restored_selected")" != "$restore_preexisting_payload" ]]; then
  echo "restore rollback did not restore the preexisting selected file content" >&2
  exit 1
fi
if [[ -e "$restored_config" ]]; then
  echo "restore rollback did not remove newly restored config file" >&2
  exit 1
fi
rollback_outputs_json="$(api_auth_get "/api/v1/jobs/$rollback_job_id/outputs")"
jq -e --arg restore_job_id "$restore_job_id" '
  .[] | select(.stream == "status" and .done == true and .exit_code == 0)
  | (.data_base64 | @base64d | fromjson)
  | .type == "restore_rollback" and .source_restore_job_id == $restore_job_id and .rolled_back_count == 2
  and ([.rolled_back_files[].action] | index("restored_snapshot") and index("removed_restored_file"))
' <<<"$rollback_outputs_json" >/dev/null
audits_json="$(api_auth_get "/api/v1/audit?limit=80")"
jq -e '[.[].action] | index("job.dispatch_requested") and index("job.target_result")' \
  <<<"$audits_json" >/dev/null

vty_restore_root="$SMOKE_TMPDIR/vty-restore-root"
vty_restore_log="$SMOKE_TMPDIR/vty-restore.log"
{
  printf 'enable\n'
  printf 'restore-run %s %s --archive-path %s --archive-size-bytes %s --archive-sha256-hex %s --path %s --include-config --destination-root %s --timeout 30 --force-unprivileged --confirmed\n' \
    "$backup_request_id" "$client_id" "$restore_archive" "$restore_archive_size" "$restore_archive_sha" "$selected_file" "$vty_restore_root"
  printf 'exit\n'
} | VPSMAN_SUPER_PASSWORD="$super_password" \
  VPSMAN_SUPER_SALT_HEX="$super_salt_hex" \
  target/debug/vpsctl --api-url "$api_url" vty >"$vty_restore_log" 2>&1
vty_restore_job_id="$(grep -Eo '"job_id":"[^"]+"' "$vty_restore_log" | head -1 | cut -d'"' -f4)"
if [[ -z "$vty_restore_job_id" ]]; then
  echo "vty restore did not print a job id" >&2
  cat "$vty_restore_log" >&2 || true
  exit 1
fi
smoke_wait_api_job_status "$api_url" "$vty_restore_job_id" completed 45 >/dev/null
vty_restored_selected="$vty_restore_root${selected_file}"
cmp -s "$selected_file" "$vty_restored_selected"

migration_restore_root="$SMOKE_TMPDIR/migration-restore-root"
migration_plan_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" restore-plan \
    --source-backup-request-id "$backup_request_id" \
    --target-client-id "$client_id" \
    --paths "$selected_file" \
    --include-config \
    --destination-root "$migration_restore_root" \
    --super-salt-hex "$super_salt_hex" \
    --note "live executable migration" \
    --confirmed)"
migration_restore_plan_id="$(jq -r '.id' <<<"$migration_plan_json")"
jq -e --arg id "$backup_request_id" --arg target "$client_id" '
  .source_backup_request_id == $id
  and .target_client_id == $target
  and .status == "planned_metadata_only"
' <<<"$migration_plan_json" >/dev/null

migration_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" migration-run \
    --restore-plan-id "$migration_restore_plan_id" \
    --archive-path "$restore_archive" \
    --archive-size-bytes "$restore_archive_size" \
    --archive-sha256-hex "$restore_archive_sha" \
    --super-salt-hex "$super_salt_hex" \
    --timeout-secs 30 \
    --force-unprivileged \
    --note "live migration run" \
    --confirmed)"
migration_job_id="$(jq -r '.restore_job.job_id' <<<"$migration_json")"
migration_link_id="$(jq -r '.migration_link.id' <<<"$migration_json")"
jq -e --arg plan "$migration_restore_plan_id" --arg target "$client_id" '
  .restore_plan_id == $plan
  and .target_client_id == $target
  and .migration_link.status == "linked_metadata_only"
  and .restore_job.target_count == 1
' <<<"$migration_json" >/dev/null
smoke_wait_api_job_status "$api_url" "$migration_job_id" completed 45 >/dev/null
migration_restored_selected="$migration_restore_root${selected_file}"
cmp -s "$selected_file" "$migration_restored_selected"
audits_json="$(api_auth_get "/api/v1/audit?limit=120")"
jq -e --arg migration_link_id "$migration_link_id" '
  .[] | select(.action == "migration.linked_metadata_only" and .target == ("migration_link:" + $migration_link_id))
' <<<"$audits_json" >/dev/null

jq -n \
  --arg client_id "$client_id" \
  --arg job_id "$job_id" \
  --arg restore_job_id "$restore_job_id" \
  --arg rollback_job_id "$rollback_job_id" \
  --arg migration_job_id "$migration_job_id" \
  --arg migration_restore_plan_id "$migration_restore_plan_id" \
  --arg migration_link_id "$migration_link_id" \
  --arg backup_request_id "$backup_request_id" \
  --arg object_key "$object_key" \
  --arg selected_file "$selected_file" \
  --arg selected_sha "$selected_sha" \
  --arg restored_selected "$restored_selected" \
  --arg vty_restored_selected "$vty_restored_selected" \
  --arg migration_restored_selected "$migration_restored_selected" \
  --arg restore_archive "$restore_archive" \
  '{
    live_backup_smoke: "ok",
    no_privilege_unlock_rejected: true,
    client_id: $client_id,
    job_id: $job_id,
    restore_job_id: $restore_job_id,
    rollback_job_id: $rollback_job_id,
    migration_job_id: $migration_job_id,
    migration_restore_plan_id: $migration_restore_plan_id,
    migration_link_id: $migration_link_id,
    backup_request_id: $backup_request_id,
    object_key: $object_key,
    selected_file: $selected_file,
    restored_selected_file: $restored_selected,
    vty_restored_selected_file: $vty_restored_selected,
    migration_restored_selected_file: $migration_restored_selected,
    staged_restore_archive: $restore_archive,
    selected_sha256_hex: $selected_sha,
    checks: ["agent_encrypted_backup", "no_plaintext_in_artifact", "auto_object_store_link", "artifact_metadata_link", "restore_run", "restore_rollback", "vty_restore_run", "migration_run_restore", "job_output_status", "audit"]
  }'
