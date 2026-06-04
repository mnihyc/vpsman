#!/usr/bin/env bash

_smoke_seed_policy_prune_artifact() {
  local label="$1"
  local sha_char="$2"
  local request_json request_id artifact_json artifact_id
  request_json="$(vpsctl_json backup-request \
    --client-id pg-agent-a \
    --paths /etc/hostname \
    --include-config \
    --note "postgres policy prune $label" \
    --confirmed)"
  request_id="$(jq -r '.id' <<<"$request_json")"
  artifact_json="$(vpsctl_json backup-artifact-record \
    --backup-request-id "$request_id" \
    --object-key "backups/pg-agent-a/policy-prune-$label.cbor.zst.age" \
    --sha256-hex "$(printf "%064s" "" | tr ' ' "$sha_char")" \
    --size-bytes 4096 \
    --confirmed)"
  artifact_id="$(jq -r '.id' <<<"$artifact_json")"
  printf '%s %s\n' "$request_id" "$artifact_id"
}

smoke_postgres_backup_policy_prune_evidence() {
  local backup_prune_policy_json
  local policy_source_count policy_prune_dry_run_json policy_prune_json
  local pruned_artifact_count retained_artifact_count cleared_link_count

  backup_prune_policy_json="$(vpsctl_json backup-policy-upsert \
    --name pg-prune-policy \
    --paths /etc/hostname \
    --include-config \
    --clients pg-agent-a \
    --interval-secs 3600 \
    --start-at-unix 4102444800 \
    --retention-days 1 \
    --keep-last 1 \
    --rotation-generation prune/v1 \
    --confirmed)"
  backup_prune_policy_schedule_id="$(jq -r '.schedule_id' <<<"$backup_prune_policy_json")"
  jq -e '
    .name == "pg-prune-policy" and
    .clients == ["pg-agent-a"] and
    .retention_days == 1 and
    .keep_last == 1 and
    .rotation_generation == "prune/v1"
  ' <<<"$backup_prune_policy_json" >/dev/null

  read -r prune_old_a_request_id prune_old_a_artifact_id < <(_smoke_seed_policy_prune_artifact old-a b)
  read -r prune_old_b_request_id prune_old_b_artifact_id < <(_smoke_seed_policy_prune_artifact old-b c)
  read -r prune_retained_request_id prune_retained_artifact_id < <(_smoke_seed_policy_prune_artifact retained d)
  backup_prune_object_root="$SMOKE_TMPDIR/backup-prune-objects"
  mkdir -p "$backup_prune_object_root/backups/pg-agent-a"
  printf 'old-a\n' >"$backup_prune_object_root/backups/pg-agent-a/policy-prune-old-a.cbor.zst.age"
  printf 'old-b\n' >"$backup_prune_object_root/backups/pg-agent-a/policy-prune-old-b.cbor.zst.age"
  printf 'retained\n' >"$backup_prune_object_root/backups/pg-agent-a/policy-prune-retained.cbor.zst.age"
  docker exec -i "$container_name" psql -U vpsman -d vpsman -v ON_ERROR_STOP=1 >/dev/null <<SQL
UPDATE backup_requests
SET source_schedule_id = '$backup_prune_policy_schedule_id'
WHERE id IN (
  '$prune_old_a_request_id',
  '$prune_old_b_request_id',
  '$prune_retained_request_id'
);

UPDATE backup_artifacts
SET created_at = now() - interval '3 days'
WHERE id = '$prune_old_a_artifact_id';

UPDATE backup_artifacts
SET created_at = now() - interval '2 days'
WHERE id = '$prune_old_b_artifact_id';

UPDATE backup_artifacts
SET created_at = now()
WHERE id = '$prune_retained_artifact_id';
SQL
  policy_source_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM backup_requests WHERE source_schedule_id = '$backup_prune_policy_schedule_id'")"
  if [[ "$policy_source_count" != "3" ]]; then
    echo "expected three policy-linked backup requests before prune" >&2
    exit 1
  fi
  policy_prune_dry_run_json="$(vpsctl_json backup-policy-prune \
    --schedule-id "$backup_prune_policy_schedule_id" \
    --dry-run \
    --metadata-only true)"
  jq -e --arg schedule_id "$backup_prune_policy_schedule_id" '
    .dry_run == true and
    .metadata_only_requested == true and
    (.policies | length == 1) and
    .policies[0].schedule_id == $schedule_id and
    .policies[0].matched_rows == 2 and
    .policies[0].pruned_rows == 0 and
    .policies[0].metadata_only == true and
    .policies[0].object_delete_attempted == false
  ' <<<"$policy_prune_dry_run_json" >/dev/null
  VPSMAN_POSTGRES_URL="$postgres_url" \
  VPSMAN_MIGRATIONS_DIR="$ROOT_DIR/migrations" \
    target/debug/vpsman-worker --once \
      --worker-id pg-backup-prune-worker \
      --worker-lease-secs 60 \
      --telemetry-rollup-secs 300 \
      --telemetry-rollup-lookback-hours 1 \
      --telemetry-prune-after-hours 1 \
      --backup-policy-prune-enabled \
      --backup-policy-prune-limit 10 \
      --backup-policy-prune-delete-objects \
      --backup-policy-prune-object-store-dir "$backup_prune_object_root" \
      >"$SMOKE_TMPDIR/backup-policy-prune-worker.log" 2>&1
  policy_prune_json="$(cat "$SMOKE_TMPDIR/backup-policy-prune-worker.log")"
  rg -q 'backup_policy_prune_pruned=2' "$SMOKE_TMPDIR/backup-policy-prune-worker.log" || {
    echo "expected backup policy retention worker to prune two artifacts" >&2
    cat "$SMOKE_TMPDIR/backup-policy-prune-worker.log" >&2 || true
    exit 1
  }
  policy_prune_worker_lease_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM worker_leases WHERE task_name = 'backup_policy_retention_prune' AND owner = 'pg-backup-prune-worker' AND lease_expires_at > now()")"
  if [[ "$policy_prune_worker_lease_count" != "1" ]]; then
    echo "expected backup policy retention worker lease evidence" >&2
    docker exec "$container_name" psql -U vpsman -d vpsman -c "SELECT task_name, owner, lease_expires_at FROM worker_leases ORDER BY task_name" >&2 || true
    exit 1
  fi
  policy_prune_api_json="$(api_post "/api/v1/backup-policies/prune" "$(jq -n --arg schedule_id "$backup_prune_policy_schedule_id" '{
    schedule_id: $schedule_id,
    dry_run: true,
    metadata_only: true
  }')")"
jq -e --arg schedule_id "$backup_prune_policy_schedule_id" '
  .dry_run == true and
  .metadata_only_requested == true and
  (.policies | length == 1) and
  .policies[0].schedule_id == $schedule_id and
  .policies[0].matched_rows == 0 and
  .policies[0].pruned_rows == 0 and
  .policies[0].metadata_only == true and
  .policies[0].object_delete_attempted == false
' <<<"$policy_prune_api_json" >/dev/null
  pruned_artifact_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM backup_artifacts WHERE id IN ('$prune_old_a_artifact_id', '$prune_old_b_artifact_id')")"
  if [[ "$pruned_artifact_count" != "0" ]]; then
    echo "expected policy prune to remove old artifact metadata rows" >&2
    exit 1
  fi
  retained_artifact_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM backup_artifacts WHERE id = '$prune_retained_artifact_id'")"
  if [[ "$retained_artifact_count" != "1" ]]; then
    echo "expected policy prune to keep newest artifact metadata row" >&2
    exit 1
  fi
  if [[ -e "$backup_prune_object_root/backups/pg-agent-a/policy-prune-old-a.cbor.zst.age" || -e "$backup_prune_object_root/backups/pg-agent-a/policy-prune-old-b.cbor.zst.age" ]]; then
    echo "expected worker object deletion to remove old local object files" >&2
    find "$backup_prune_object_root" -type f -print >&2 || true
    exit 1
  fi
  if [[ ! -e "$backup_prune_object_root/backups/pg-agent-a/policy-prune-retained.cbor.zst.age" ]]; then
    echo "expected worker object deletion to keep retained local object file" >&2
    exit 1
  fi
  cleared_link_count="$(docker exec "$container_name" psql -U vpsman -d vpsman -tAc "SELECT count(*) FROM backup_requests WHERE id IN ('$prune_old_a_request_id', '$prune_old_b_request_id') AND artifact_id IS NULL AND status = 'requested_metadata_only'")"
  if [[ "$cleared_link_count" != "2" ]]; then
    echo "expected policy prune to clear old backup request artifact links" >&2
    exit 1
  fi
api_get "/api/v1/audit?limit=120" | jq -e '
  any(.[]; .action == "backup_policy.retention_pruned" and .metadata.worker == "backup_policy_retention_worker" and .metadata.pruned_rows == 2 and .metadata.object_delete_requested == true and .metadata.object_delete_configured == true)
' >/dev/null
}
