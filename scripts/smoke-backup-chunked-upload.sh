#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools base64 cmp curl grep jq sha256sum shuf timeout
smoke_build_binaries
smoke_init_tmpdir "vpsman-backup-chunked-upload"

api_port="$(smoke_free_port)"
api_url="http://127.0.0.1:$api_port"
client_id="chunked-backup-smoke-$(date +%s)"
super_password="smoke-super-password"
super_salt_hex="00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"
object_store_dir="$SMOKE_TMPDIR/object-store"
api_log="$SMOKE_TMPDIR/api.log"

VPSMAN_API_BIND="127.0.0.1:$api_port" \
VPSMAN_BACKUP_OBJECT_STORE_DIR="$object_store_dir" \
RUST_LOG="vpsman_api=warn" \
  target/debug/vpsman-api >"$api_log" 2>&1 &
smoke_track_pid "$!"
if ! smoke_wait_http "$api_url/health"; then
  smoke_dump_logs "API did not become healthy for chunked backup upload smoke" "$api_log"
  exit 1
fi

token_json="$(target/debug/vpsctl --api-url "$api_url" enrollment-token-create \
  --ttl-secs 600 \
  --default-tags chunked-backup-smoke)"
enrollment_token="$(jq -r '.token' <<<"$token_json")"
client_keys="$(target/debug/vpsctl noise-keygen)"
client_public_hex="$(jq -r '.public_key_hex' <<<"$client_keys")"
target/debug/vpsctl --api-url "$api_url" enroll-claim \
  --token "$enrollment_token" \
  --client-id "$client_id" \
  --client-public-key-hex "$client_public_hex" >/dev/null

backup_request_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" backup-request \
    --client-id "$client_id" \
    --paths /etc/hostname \
    --include-config \
    --super-salt-hex "$super_salt_hex" \
    --confirmed)"
backup_request_id="$(jq -r '.id' <<<"$backup_request_json")"

ciphertext_file="$SMOKE_TMPDIR/ciphertext.bin"
{
  printf 'synthetic encrypted payload for chunked upload smoke %s\n' "$(date +%s%N)"
  printf 'chunk-boundary-padding-'
  seq 1 50 | tr '\n' ':'
  printf '\n'
} >"$ciphertext_file"
ciphertext_b64="$(base64 <"$ciphertext_file" | tr -d '\n')"
ciphertext_sha="$(sha256sum "$ciphertext_file" | awk '{print $1}')"
artifact_file="$SMOKE_TMPDIR/artifact.json"
jq -nc \
  --arg client "$client_id" \
  --arg ciphertext_b64 "$ciphertext_b64" \
  --arg ciphertext_sha "$ciphertext_sha" \
  --arg created "$(date +%s)" \
  '{
    format: "vpsman.backup_artifact.v1",
    version: 1,
    client_id: $client,
    created_unix: ($created | tonumber),
    cipher: "x25519-chacha20poly1305",
    compression: "lz4-size-prepended",
    recipient_public_key_sha256_hex: ("a" * 64),
    ephemeral_public_key_hex: ("b" * 64),
    nonce_hex: ("c" * 24),
    ciphertext_sha256_hex: $ciphertext_sha,
    ciphertext_base64: $ciphertext_b64
  }' >"$artifact_file"
artifact_sha="$(sha256sum "$artifact_file" | awk '{print $1}')"
object_key="backups/$client_id/$backup_request_id-chunked.json"

upload_json="$(target/debug/vpsctl --api-url "$api_url" backup-artifact-upload-chunked \
  --backup-request-id "$backup_request_id" \
  --object-key "$object_key" \
  --artifact-file "$artifact_file" \
  --chunk-size-bytes 37 \
  --confirmed)"
jq -e \
  --arg client "$client_id" \
  --arg object_key "$object_key" \
  --arg sha "$artifact_sha" \
  '.client_id == $client and .object_key == $object_key and .sha256_hex == $sha and .encrypted == true and .size_bytes > 0' \
  <<<"$upload_json" >/dev/null

stored_object="$object_store_dir/$object_key"
cmp -s "$artifact_file" "$stored_object"
api_downloaded="$SMOKE_TMPDIR/api-downloaded-artifact.json"
curl -fsS "$api_url/api/v1/backups/$backup_request_id/artifact" -o "$api_downloaded"
cmp -s "$artifact_file" "$api_downloaded"

second_backup_json="$(VPSMAN_SUPER_PASSWORD="$super_password" \
  target/debug/vpsctl --api-url "$api_url" backup-request \
    --client-id "$client_id" \
    --paths /etc/hostname \
    --include-config \
    --super-salt-hex "$super_salt_hex" \
    --confirmed)"
second_backup_id="$(jq -r '.id' <<<"$second_backup_json")"
duplicate_log="$SMOKE_TMPDIR/duplicate.log"
if target/debug/vpsctl --api-url "$api_url" backup-artifact-upload-chunked \
  --backup-request-id "$second_backup_id" \
  --object-key "$object_key" \
  --artifact-file "$artifact_file" \
  --chunk-size-bytes 37 \
  --confirmed >"$duplicate_log" 2>&1; then
  echo "expected duplicate chunked backup object upload to fail" >&2
  exit 1
fi
grep -q "backup_artifact_object_exists" "$duplicate_log"

backups_json="$(curl -fsS "$api_url/api/v1/backups?limit=20")"
artifacts_json="$(curl -fsS "$api_url/api/v1/backup-artifacts?limit=20")"
audits_json="$(curl -fsS "$api_url/api/v1/audit?limit=50")"
artifact_id="$(jq -r '.id' <<<"$upload_json")"
jq -e --arg id "$backup_request_id" --arg artifact_id "$artifact_id" '
  .[] | select(.id == $id and .status == "artifact_metadata_recorded" and .artifact_id == $artifact_id)
' <<<"$backups_json" >/dev/null
jq -e --arg artifact_id "$artifact_id" --arg object_key "$object_key" '
  .[] | select(.id == $artifact_id and .object_key == $object_key and .encrypted == true)
' <<<"$artifacts_json" >/dev/null
jq -e '[.[].action] | index("backup.artifact_metadata_recorded")' <<<"$audits_json" >/dev/null

jq -n \
  --arg client_id "$client_id" \
  --arg backup_request_id "$backup_request_id" \
  --arg object_key "$object_key" \
  '{
    backup_chunked_upload_smoke: "ok",
    client_id: $client_id,
    backup_request_id: $backup_request_id,
    object_key: $object_key,
    duplicate_rejected: true
  }'
