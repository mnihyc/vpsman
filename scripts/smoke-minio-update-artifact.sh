#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools cmp curl docker grep jq sha256sum shuf timeout
if ! curl --help all 2>/dev/null | grep -q -- '--aws-sigv4'; then
  echo "curl lacks --aws-sigv4 support, cannot verify MinIO update artifact contents" >&2
  exit 1
fi
smoke_build_binaries
smoke_init_tmpdir "vpsman-minio-update"

minio_container=""
smoke_minio_cleanup() {
  if [[ -n "${minio_container:-}" ]]; then
    docker rm -f "$minio_container" >/dev/null 2>&1 || true
  fi
  smoke_cleanup
}
trap smoke_minio_cleanup EXIT

api_port="$(smoke_free_port)"
minio_port="$(smoke_free_port)"
api_url="http://127.0.0.1:$api_port"
minio_url="http://127.0.0.1:$minio_port"
bucket="vpsman-updates"
internal_token="minio-update-internal-token-00000000"
access_key="vpsman"
secret_key="vpsman-password"
region="us-east-1"
minio_image="${VPSMAN_MINIO_IMAGE:-minio/minio:RELEASE.2025-04-22T22-12-26Z}"
minio_container="vpsman-minio-update-smoke-$(date +%s%N)"

docker run --rm -d --name "$minio_container" \
  -e "MINIO_ROOT_USER=$access_key" \
  -e "MINIO_ROOT_PASSWORD=$secret_key" \
  -p "127.0.0.1:$minio_port:9000" \
  "$minio_image" server /data >/dev/null

deadline=$((SECONDS + 60))
until curl -fsS "$minio_url/minio/health/ready" >/dev/null 2>&1; do
  if (( SECONDS >= deadline )); then
    docker logs "$minio_container" >&2 || true
    echo "MinIO did not become ready" >&2
    exit 1
  fi
  sleep 0.5
done

api_log="$SMOKE_TMPDIR/api.log"
VPSMAN_API_BIND="127.0.0.1:$api_port" \
VPSMAN_DEBUG_INTERNAL_TEST_MODE=true \
VPSMAN_INTERNAL_TOKEN="$internal_token" \
VPSMAN_UPDATE_OBJECT_ENDPOINT="$minio_url" \
VPSMAN_UPDATE_OBJECT_BUCKET="$bucket" \
VPSMAN_UPDATE_OBJECT_ACCESS_KEY="$access_key" \
VPSMAN_UPDATE_OBJECT_SECRET_KEY="$secret_key" \
VPSMAN_UPDATE_OBJECT_REGION="$region" \
VPSMAN_UPDATE_OBJECT_CREATE_BUCKET=true \
RUST_LOG="vpsman_api=warn" \
  target/debug/vpsman-api >"$api_log" 2>&1 &
smoke_track_pid "$!"
if ! smoke_wait_http "$api_url/health"; then
  smoke_dump_logs "API did not become healthy for MinIO update artifact smoke" "$api_log"
  exit 1
fi

artifact_file="$SMOKE_TMPDIR/vpsman-agent-smoke.bin"
printf 'synthetic vpsman agent update bytes for MinIO smoke %s\n' "$(date +%s%N)" >"$artifact_file"
artifact_sha="$(sha256sum "$artifact_file" | awk '{print $1}')"
signing_seed_hex="1111111111111111111111111111111111111111111111111111111111111111"
version="minio-$(date +%s%N)"

upload_json="$(target/debug/vpsctl --api-url "$api_url" agent-update-artifact-upload \
  --name vpsman-agent \
  --version "$version" \
  --channel stable \
  --artifact-file "$artifact_file" \
  --signing-seed-hex "$signing_seed_hex" \
  --notes "MinIO hosted update smoke" \
  --confirmed)"
object_key="$(jq -r '.artifact_object_key' <<<"$upload_json")"
download_path="$(jq -r '.artifact_download_path' <<<"$upload_json")"
jq -e \
  --arg sha "$artifact_sha" \
  --arg object_key "agent-updates/$artifact_sha.bin" \
  --arg download_path "/api/v1/agent-update-artifacts/$artifact_sha" \
  '.status == "artifact_hosted"
    and .artifact_sha256_hex == $sha
    and .artifact_signature_provided == true
    and .artifact_object_key == $object_key
    and .artifact_download_path == $download_path
    and .size_bytes > 0' \
  <<<"$upload_json" >/dev/null

direct_download="$SMOKE_TMPDIR/direct-agent-update.bin"
curl -fsS \
  --aws-sigv4 "aws:amz:$region:s3" \
  --user "$access_key:$secret_key" \
  "$minio_url/$bucket/$object_key" \
  -o "$direct_download"
cmp -s "$artifact_file" "$direct_download"

api_download="$SMOKE_TMPDIR/api-agent-update.bin"
curl -fsS "$api_url$download_path" -o "$api_download"
cmp -s "$artifact_file" "$api_download"

releases_json="$(target/debug/vpsctl --api-url "$api_url" agent-update-releases --limit 20)"
jq -e --arg sha "$artifact_sha" --arg object_key "$object_key" '
  .[] | select(.artifact_sha256_hex == $sha and .artifact_object_key == $object_key and .status == "artifact_hosted")
' <<<"$releases_json" >/dev/null

audits_json="$(curl -fsS "$api_url/api/v1/audit?limit=50")"
jq -e '[.[].action] | index("agent_update.artifact_uploaded")' <<<"$audits_json" >/dev/null

duplicate_log="$SMOKE_TMPDIR/duplicate.log"
if target/debug/vpsctl --api-url "$api_url" agent-update-artifact-upload \
  --name vpsman-agent \
  --version "$version" \
  --channel stable \
  --artifact-file "$artifact_file" \
  --signing-seed-hex "$signing_seed_hex" \
  --confirmed >"$duplicate_log" 2>&1; then
  echo "expected duplicate hosted update release upload to fail" >&2
  exit 1
fi
grep -q "agent_update_release_already_exists" "$duplicate_log"

jq -n \
  --arg sha "$artifact_sha" \
  --arg object_key "$object_key" \
  '{
    minio_update_artifact_smoke: "ok",
    artifact_sha256_hex: $sha,
    object_key: $object_key,
    duplicate_rejected: true
  }'
