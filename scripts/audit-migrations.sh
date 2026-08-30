#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MIGRATIONS_DIR="$ROOT_DIR/migrations"
SEED_FILE="$MIGRATIONS_DIR/0013_seed_defaults.sql"
API_REPOSITORY="$ROOT_DIR/crates/api/src/repository/core/repository.rs"
JOBS_REPOSITORY="$ROOT_DIR/crates/api/src/repository/jobs/repository_jobs.rs"
WORKER_RUNTIME="$ROOT_DIR/crates/worker/src/main.rs"
SQLX_CATALOG_TEST="$ROOT_DIR/crates/api/src/repository/core/tests_postgres_reliability.rs"

fail() {
  printf 'migration_audit=failed reason=%s\n' "$*" >&2
  exit 1
}

expected_files=(
  0001_identity_access.sql
  0002_jobs_schedules.sql
  0003_telemetry_core.sql
  0004_network_tunnels.sql
  0005_traffic_accounting.sql
  0006_telemetry_dashboard.sql
  0007_backups_restores.sql
  0008_agent_updates.sql
  0009_config_presets_transfers.sql
  0010_system_metrics.sql
  0011_fleet_settings.sql
  0012_alert_lifecycle.sql
  0013_seed_defaults.sql
)

[[ -d "$MIGRATIONS_DIR" ]] || fail "missing migrations directory"
mapfile -t actual_files < <(
  find "$MIGRATIONS_DIR" -maxdepth 1 -type f -name '*.sql' -printf '%f\n' | sort
)
[[ "${actual_files[*]}" == "${expected_files[*]}" ]] ||
  fail "migration filenames do not match the domain schema manifest"

for file in "${expected_files[@]}"; do
  path="$MIGRATIONS_DIR/$file"
  [[ -s "$path" ]] || fail "empty migration: $file"
  tail -c 1 "$path" | grep -q $'\n' || fail "migration lacks trailing newline: $file"
done

schema_files=("${expected_files[@]:0:12}")
schema_paths=()
for file in "${schema_files[@]}"; do
  schema_paths+=("$MIGRATIONS_DIR/$file")
done

# Permanent schema files declare the final state; they never mutate an older state.
if grep -Eiq '^(ALTER|DROP|TRUNCATE|INSERT|UPDATE|DELETE|CREATE[[:space:]]+INDEX[[:space:]]+CONCURRENTLY)\b' \
  "${schema_paths[@]}"; then
  fail "schema files contain top-level mutation or concurrent-index work"
fi
if grep -Fq "NOT VALID" "${schema_paths[@]}"; then
  fail "schema constraints must be valid when created"
fi
if grep -Eiq '^(CREATE|ALTER|DROP|TRUNCATE|UPDATE|DELETE|GRANT|REVOKE)\b' "$SEED_FILE"; then
  fail "seed file must contain only inserts"
fi

ledger_name="_sqlx""_migrations"
if grep -Fq "$ledger_name" "${schema_paths[@]}" "$SEED_FILE"; then
  fail "schema files must not own the migration ledger"
fi
if grep -Fq "vpsman_internal" "${schema_paths[@]}" "$SEED_FILE"; then
  fail "domain schema files must not own SQLx's private metadata schema"
fi
if grep -rFq \
  --exclude='audit-migrations.sh' \
  --exclude='tests_postgres_reliability.rs' \
  --exclude-dir='.tmp' \
  --exclude-dir='node_modules' \
  --exclude-dir='dist' \
  "$ledger_name" \
  "$ROOT_DIR/crates" \
  "$ROOT_DIR/deploy" \
  "$ROOT_DIR/docs" \
  "$ROOT_DIR/frontend" \
  "$ROOT_DIR/scripts" \
  "$ROOT_DIR/tutorials" \
  "$ROOT_DIR/README.md"; then
  fail "application code, tooling, and active documentation must not inspect or own SQLx's internal migration ledger"
fi
public_ledger="public.$ledger_name"
if grep -rFq \
  --exclude='audit-migrations.sh' \
  --exclude-dir='.tmp' \
  --exclude-dir='node_modules' \
  --exclude-dir='dist' \
  "$public_ledger" \
  "$ROOT_DIR/crates" \
  "$ROOT_DIR/deploy" \
  "$ROOT_DIR/docs" \
  "$ROOT_DIR/frontend" \
  "$ROOT_DIR/scripts" \
  "$ROOT_DIR/tutorials" \
  "$ROOT_DIR/README.md"; then
  fail "no active source may address a SQLx ledger in the public schema"
fi

for retired in \
  'v0\.[0-9]' \
  'legacy' \
  'bootstrap_(migration|schema|version|state|complete|completed)' \
  'cutover' \
  'first_post_upgrade' \
  'backfill_completed' \
  'event_source_cutoff' \
  'alert_expression_migration' \
  'active_initialized' \
  'minima_ready' \
  'traffic_counter_sample_edge_backfill_state' \
  'evidence_prune_scan_after_seq' \
  'evidence_prune_scan_through_seq'; do
  if grep -Eiq "$retired" "${schema_paths[@]}" "$SEED_FILE"; then
    fail "retired transition control survives: $retired"
  fi
done

declare -A domain_anchors=(
  [0001_identity_access.sql]='clients monitoring_share_links operators'
  [0002_jobs_schedules.sql]='jobs schedules system_dashboard_target_metrics'
  [0003_telemetry_core.sql]='telemetry_samples telemetry_rollups telemetry_ping_facts telemetry_history_due_events telemetry_history_due_spans traffic_history_retention_cursors traffic_counter_streams traffic_counter_samples'
  [0004_network_tunnels.sql]='tunnel_plans telemetry_tunnels network_observations'
  [0005_traffic_accounting.sql]='traffic_counter_rollups traffic_counter_rollup_summary_streams'
  [0006_telemetry_dashboard.sql]='telemetry_dashboard_resource_projection_heads telemetry_dashboard_network_projection_heads telemetry_dashboard_ping_projection_heads telemetry_dashboard_resource_blocks telemetry_dashboard_network_blocks'
  [0007_backups_restores.sql]='backup_requests restore_plans migration_links'
  [0008_agent_updates.sql]='agent_update_releases'
  [0009_config_presets_transfers.sql]='configuration_presets file_transfer_sessions runtime_config_patch_generators'
  [0010_system_metrics.sql]='system_metric_rollups'
  [0011_fleet_settings.sql]='fleet_tag_settings'
  [0012_alert_lifecycle.sql]='alert_policy_evidence alert_episodes webhook_events'
)
for file in "${schema_files[@]}"; do
  for table in ${domain_anchors[$file]}; do
    count="$(grep -Fxc "CREATE TABLE public.$table (" "$MIGRATIONS_DIR/$file" || true)"
    [[ "$count" -eq 1 ]] || fail "$file does not own public.$table exactly once"
  done
done

# Target counters are exact per-client write-time projections. Job counters read
# only the indexed active set; the clock-dependent target windows keep matching
# partial indexes. No mutation may synchronize on a fleet-wide counter row.
jobs_schema="$MIGRATIONS_DIR/0002_jobs_schedules.sql"
[[ "$(grep -Ec '^CREATE TRIGGER job_targets_system_dashboard_metrics_after_(insert|update|delete) .*REFERENCING .* FOR EACH STATEMENT EXECUTE FUNCTION public\.maintain_system_dashboard_target_metrics\(\);$' "$jobs_schema")" -eq 3 ]] ||
  fail "system dashboard target projections require three set-wise transition-table triggers"
if grep -Eq '^CREATE TRIGGER job_targets_system_dashboard_metrics_.* FOR EACH ROW ' \
  "$jobs_schema"; then
  fail "system dashboard target projections must not use row-level counter triggers"
fi
if grep -Eq 'maintain_system_dashboard_job_metrics|jobs_system_dashboard_metrics|singleton' \
  "$jobs_schema"; then
  fail "system dashboard mutations still contain a fleet-wide job counter owner"
fi
[[ "$(grep -Fc 'ORDER BY client_id COLLATE "C"' "$jobs_schema")" -eq 3 ]] ||
  fail "system dashboard target owners are not acquired in Rust-compatible byte order"
[[ "$(grep -Fc 'GROUP BY client_id' "$jobs_schema")" -eq 3 ]] ||
  fail "system dashboard target transitions must reduce to one delta per touched owner"
[[ "$(grep -Fc 'FOR delta IN' "$jobs_schema")" -eq 3 ]] ||
  fail "system dashboard target owner application shape differs"
[[ "$(grep -Fc 'PERFORM public.apply_system_dashboard_target_metric_delta(' "$jobs_schema")" -eq 3 ]] ||
  fail "system dashboard target transitions must apply each grouped owner once"
grep -Fq 'CONSTRAINT system_dashboard_target_metrics_nonnegative_check CHECK (' \
  "$jobs_schema" ||
  fail "system dashboard target projection lacks its nonnegative invariant"
[[ "$(grep -Fc 'FROM system_dashboard_target_metrics' "$JOBS_REPOSITORY")" -eq 1 ]] ||
  fail "deadline reconciliation must prelock its exact target metric owners"
[[ "$(grep -Fc 'ORDER BY client_id COLLATE "C"' "$JOBS_REPOSITORY")" -eq 1 ]] ||
  fail "deadline target metric owners are not prelocked in canonical byte order"
[[ "$(grep -Fc 'system_dashboard_target_metric_owner_missing' "$JOBS_REPOSITORY")" -eq 1 ]] ||
  fail "deadline reconciliation must fail closed when an active owner is missing"
grep -Fq 'CREATE INDEX job_targets_recent_effective_terminal_idx ON public.job_targets USING btree (status, public.job_target_effective_terminal_at(status, completed_at, result_received_at, started_at, cancel_acked_at, cancel_sent_at, cancel_requested_at) DESC) WHERE (status = ANY (ARRAY['\''control_timeout'\''::text, '\''agent_timeout'\''::text, '\''agent_lost'\''::text, '\''canceled'\''::text]));' \
  "$jobs_schema" ||
  fail "system dashboard terminal-window index differs"
grep -Fxq "CREATE INDEX job_targets_deadline_due_idx ON public.job_targets USING btree (deadline_at, job_id, client_id) WHERE ((completed_at IS NULL) AND (status = ANY (ARRAY['dispatching'::text, 'running'::text])));" \
  "$jobs_schema" ||
  fail "system dashboard deadline index differs"

# Retained source tables own the fixed tier horizon. Dashboard reads use the
# bounded F16 native-tier blocks below; detail reads use indexed retained facts.
# Neither owner may reintroduce a history-sized fallback or acceptance-only
# covering index.
if grep -Eq 'telemetry_(rollups|network_rates)_client_latest_idx' \
  "$MIGRATIONS_DIR/0003_telemetry_core.sql"; then
  fail "history-sized source covering index survives the F16 owner"
fi
telemetry_schema="$MIGRATIONS_DIR/0003_telemetry_core.sql"
if grep -Eq 'network_(rx|tx)_bytes(_max)?' "$telemetry_schema"; then
  fail "resource telemetry still persists an opaque aggregate network counter"
fi
retained_tier_time_keys=(
  '    CONSTRAINT telemetry_network_rates_pkey PRIMARY KEY (bucket_secs, bucket_start, client_id, interface),'
  '    CONSTRAINT telemetry_ping_rollups_pkey PRIMARY KEY (bucket_secs, bucket_start, series_id),'
  '    CONSTRAINT telemetry_rollups_pkey PRIMARY KEY (bucket_secs, bucket_start, client_id),'
)
for key_definition in "${retained_tier_time_keys[@]}"; do
  grep -Fxq "$key_definition" "$telemetry_schema" ||
    fail "retained telemetry identity is not tier/time-local: $key_definition"
done
if grep -Eq '^CREATE INDEX telemetry_(network_rates_coarse_latest|ping_rollups_due|rollups_latest)_idx ' \
    "$telemetry_schema"; then
  fail "a retained tier/time index duplicates its tier/time-local primary key"
fi
[[ "$(grep -Fxc 'CREATE TABLE public.telemetry_network_rates (' "$telemetry_schema")" -eq 1 ]] ||
  fail "network-rate logical owner is missing or duplicated"
[[ "$(grep -Fxc ') PARTITION BY LIST (bucket_secs);' "$telemetry_schema")" -eq 1 ]] ||
  fail "network-rate logical owner is not list-partitioned by physical tier"
[[ "$(grep -Fxc 'PARTITION OF public.telemetry_network_rates' "$telemetry_schema")" -eq 2 ]] ||
  fail "network-rate history must have exact minute and coarse physical tier owners"
grep -Fxq 'CREATE TABLE public.telemetry_network_rates_minute' "$telemetry_schema" ||
  fail "transferred network-rate minute owner is missing"
grep -Fxq 'FOR VALUES IN (60);' "$telemetry_schema" ||
  fail "network-rate minute owner does not contain only the 60-second tier"
grep -Fxq 'CREATE TABLE public.telemetry_network_rates_coarse' "$telemetry_schema" ||
  fail "network-rate coarse owner is missing"
grep -Fxq 'FOR VALUES IN (300, 1800, 3600, 10800, 21600, 86400);' \
  "$telemetry_schema" ||
  fail "network-rate coarse owner does not contain the six retained tiers"
if grep -Eq '^CREATE (UNIQUE )?INDEX [^ ]+ ON public\.telemetry_network_rates([[:space:](]|$)' \
    "$telemetry_schema"; then
  fail "network-rate parent still owns a redundant non-PK index"
fi
if grep -Eq \
    'CREATE VIEW public\.telemetry_network_live_rates|CREATE FUNCTION public\.close_due_traffic_counter_stream_minutes|^[[:space:]]+(live_[a-z_]+|previous_(rx|tx)[a-z_]*)[[:space:]]+(timestamp|bigint|integer|text|boolean|numeric)' \
    "$telemetry_schema"; then
  fail "retired per-arrival network state survives outside the raw journal"
fi
if grep -Fq "interval '15 minutes'" "$telemetry_schema"; then
  fail "shared network schema still owns the single-VPS detail window"
fi
network_durable_body="$(
  sed -n '/^CREATE FUNCTION public\.telemetry_network_durable_points_source($/,/^CREATE FUNCTION public\.telemetry_network_rate_points_source($/p' \
    "$telemetry_schema"
)"
grep -Fq 'CREATE FUNCTION public.telemetry_network_durable_points_source(' \
  <<<"$network_durable_body" ||
  fail "parameterized durable network-rate history boundary is missing"
grep -Fq 'FROM public.telemetry_network_rates_minute retained' \
  <<<"$network_durable_body" ||
  fail "durable network-rate history bypasses its exact retained owner"
grep -Fq 'FROM public.telemetry_network_rates_coarse retained' \
  <<<"$network_durable_body" ||
  fail "durable network-rate history bypasses its coarse retained owner"
grep -Fq 'IF p_bucket_secs IS NULL OR p_bucket_secs = 60 THEN' \
  <<<"$network_durable_body" ||
  fail "durable network-rate history does not branch before its exact owner"
[[ "$(grep -Fc 'IF p_bucket_secs IS NULL THEN' <<<"$network_durable_body")" -eq 3 ]] ||
  fail "durable network-rate all-tier history does not branch before its coarse owner"
[[ "$(grep -Fc 'ELSIF p_bucket_secs <> 60 THEN' <<<"$network_durable_body")" -eq 3 ]] ||
  fail "durable network-rate exact-tier history does not branch before its coarse owner"
[[ "$(grep -Fc 'retained.bucket_secs = p_bucket_secs' <<<"$network_durable_body")" -eq 4 ]] ||
  fail "durable exact coarse history does not bind its physical tier"
if grep -Fq 'OR retained.bucket_secs = p_bucket_secs' \
    <<<"$network_durable_body"; then
  fail "durable exact coarse history retains a generic-plan tier disjunction"
fi
grep -Fq 'FROM public.traffic_counter_samples sample' \
  <<<"$network_durable_body" ||
  fail "durable network-rate history bypasses closed exact traffic minutes"
grep -Eq "(WHERE|AND) sample\.source_kind = 'host'" \
  <<<"$network_durable_body" ||
  fail "durable network-rate history admits non-host traffic owners"
grep -Fq 'AND NOT sample.inbound_promoted' \
  <<<"$network_durable_body" ||
  fail "promoted traffic minutes overlap retained network-rate ownership"
grep -Fq 'retained.client_id = ANY(p_client_ids)' \
  <<<"$network_durable_body" ||
  fail "durable retained network history cannot bind an exact client owner"
grep -Fq 'sample.client_id = ANY(p_client_ids)' \
  <<<"$network_durable_body" ||
  fail "durable exact network minutes cannot bind an exact client owner"
grep -Fq 'p_interfaces TEXT[] DEFAULT NULL' \
  <<<"$network_durable_body" ||
  fail "durable network history lacks an optional exact interface owner"
grep -Fq 'p_per_stream_limit BIGINT DEFAULT NULL' \
  <<<"$network_durable_body" ||
  fail "durable network history lacks its optional exact-stream physical bound"
grep -Fq 'IF p_per_stream_limit IS NOT NULL THEN' \
  <<<"$network_durable_body" ||
  fail "durable network history never enters its bounded exact-stream owner"
grep -Fq 'p_client_ids IS NULL OR p_interfaces IS NULL' \
  <<<"$network_durable_body" ||
  fail "bounded durable network history accepts a non-exact stream scope"
grep -Fq 'cardinality(p_client_ids) <> cardinality(p_interfaces)' \
  <<<"$network_durable_body" ||
  fail "bounded durable network history accepts mismatched paired owners"
grep -Fq 'array_position(p_client_ids, NULL) IS NOT NULL' \
  <<<"$network_durable_body" ||
  fail "bounded durable network history accepts NULL client owners"
grep -Fq 'array_position(p_interfaces, NULL) IS NOT NULL' \
  <<<"$network_durable_body" ||
  fail "bounded durable network history accepts NULL interface owners"
grep -Fq 'requested_streams AS MATERIALIZED' \
  <<<"$network_durable_body" ||
  fail "bounded durable network history does not enumerate exact stream owners"
grep -Fq 'FROM unnest(p_client_ids, p_interfaces)' \
  <<<"$network_durable_body" ||
  fail "bounded durable network history does not zip exact stream owners"
grep -Fq 'ORDER BY candidate.bucket_start DESC' \
  <<<"$network_durable_body" ||
  fail "bounded durable network history lacks its canonical post-union order"
grep -Fq 'LIMIT p_per_stream_limit' \
  <<<"$network_durable_body" ||
  fail "bounded durable network history does not apply its physical stream cap"
grep -Fq 'IF p_client_ids IS NULL AND p_interfaces IS NOT NULL THEN' \
  <<<"$network_durable_body" ||
  fail "all-client durable network history ignores an explicit interface owner"
[[ "$(grep -Fc 'retained.interface = ANY(p_interfaces)' <<<"$network_durable_body")" -eq 7 ]] ||
  fail "durable retained network tiers do not bind the exact interface owner"
[[ "$(grep -Fc 'sample.interface = ANY(p_interfaces)' <<<"$network_durable_body")" -eq 3 ]] ||
  fail "durable exact network minutes do not bind the exact interface owner"

network_rate_points_body="$(
  sed -n '/^CREATE FUNCTION public\.telemetry_network_rate_points_source($/,/^CREATE FUNCTION public\.telemetry_network_current_identities_source($/p' \
    "$telemetry_schema"
)"
grep -Fq 'FROM public.telemetry_network_durable_points_source(' \
  <<<"$network_rate_points_body" ||
  fail "effective network-rate history bypasses its bounded durable owner"
grep -Fq 'FROM public.telemetry_projected_raw_network_minutes_source(p_client_ids)' \
  <<<"$network_rate_points_body" ||
  fail "effective network-rate history omits its bounded projected raw suffix"
grep -Fq 'AND (p_interfaces IS NULL OR suffix.interface = ANY(p_interfaces))' \
  <<<"$network_rate_points_body" ||
  fail "effective projected network suffix cannot bind an exact interface owner"
coarse_network_indexes=(
  'CREATE INDEX telemetry_network_rates_coarse_client_effective_idx ON public.telemetry_network_rates_coarse USING btree (client_id, interface, latest_observed_at DESC, bucket_start DESC, bucket_secs DESC);'
  'CREATE INDEX telemetry_network_rates_coarse_effective_global_idx ON public.telemetry_network_rates_coarse USING btree (latest_observed_at DESC, client_id, interface, bucket_start DESC, bucket_secs DESC);'
  'CREATE INDEX telemetry_network_rates_coarse_retention_idx ON public.telemetry_network_rates_coarse USING btree (bucket_start);'
)
for index_definition in "${coarse_network_indexes[@]}"; do
  grep -Fxq "$index_definition" "$telemetry_schema" ||
    fail "network-rate coarse index ownership differs: $index_definition"
done
minute_network_indexes=(
  'CREATE INDEX telemetry_network_rates_minute_client_effective_idx ON public.telemetry_network_rates_minute USING btree (client_id, interface, latest_observed_at DESC, bucket_start DESC);'
  'CREATE INDEX telemetry_network_rates_minute_effective_global_idx ON public.telemetry_network_rates_minute USING btree (latest_observed_at DESC, client_id, interface, bucket_start DESC);'
)
for index_definition in "${minute_network_indexes[@]}"; do
  grep -Fxq "$index_definition" "$telemetry_schema" ||
    fail "network-rate minute index ownership differs: $index_definition"
done
[[ "$(grep -Ec '^CREATE INDEX telemetry_network_rates_coarse_' "$telemetry_schema")" -eq 3 ]] ||
  fail "network-rate coarse owner must have exactly three independently consumed non-PK indexes"
# Parent statement triggers own every write, so production DML must target the
# logical parent. The bounded raw/export reader is the one intentional physical
# read owner: its minute/coarse Merge Append exposes the global index stop to a
# generic PostgreSQL plan before admission. Keep that exception exact.
network_reader="$ROOT_DIR/crates/api/src/repository/fleet/repository_telemetry_rollups.rs"
raw_candidate_body="$(
  sed -n \
    '/^pub(crate) fn raw_telemetry_network_rate_candidate_keys_sql(/,/^\/\/ Candidate keys are already page bounded\./p' \
    "$network_reader"
)"
[[ "$(grep -c 'FROM telemetry_network_rates_minute rate' <<<"$raw_candidate_body")" -eq 1 ]] ||
  fail "bounded raw network reader lost its minute physical owner"
[[ "$(grep -c 'FROM telemetry_network_rates_coarse rate' <<<"$raw_candidate_body")" -eq 1 ]] ||
  fail "bounded raw network reader lost its coarse physical owner"
unexpected_network_leaf_refs="$(
  grep -RsnE --include='*.rs' --exclude='tests_*.rs' \
    'telemetry_network_rates_(minute|coarse)' "$ROOT_DIR/crates" |
    grep -vF "$network_reader:" || true
)"
[[ -z "$unexpected_network_leaf_refs" ]] ||
  fail "production Rust names a network-rate physical leaf outside its bounded reader"
network_reader_without_candidate="$(
  sed \
    '/^pub(crate) fn raw_telemetry_network_rate_candidate_keys_sql(/,/^\/\/ Candidate keys are already page bounded\./d' \
    "$network_reader"
)"
if grep -qE 'telemetry_network_rates_(minute|coarse)' \
    <<<"$network_reader_without_candidate"; then
  fail "network-rate physical leaf escaped the bounded raw candidate owner"
fi
if grep -Fq 'history_retention_policies_network_observations_min_days_check' \
  "$MIGRATIONS_DIR/0003_telemetry_core.sql"; then
  fail "obsolete seven-day network-observation floor survived the one-day exact model"
fi
grep -Fxq "    CONSTRAINT history_retention_policies_traffic_rollup_min_days_check CHECK (((domain <> 'traffic_counter_rollups'::text) OR (retention_days >= 32)))," \
  "$MIGRATIONS_DIR/0003_telemetry_core.sql" ||
  fail "traffic final horizon cannot preserve a maximum-length monthly cycle"

dashboard_schema="$MIGRATIONS_DIR/0006_telemetry_dashboard.sql"
traffic_schema="$MIGRATIONS_DIR/0005_traffic_accounting.sql"
grep -Fq "'network.interfaces'::text" "$traffic_schema" ||
  fail "clean VPS-rule catalog omits network.interfaces"
if grep -Eq 'admitted_(host|tunnel)_interfaces' "$telemetry_schema"; then
  fail "interface admission is duplicated into every raw telemetry sample"
fi
dashboard_objects=(
  'CREATE TABLE public.telemetry_dashboard_clients ('
  'CREATE TABLE public.telemetry_dashboard_resource_projection_heads ('
  'CREATE TABLE public.telemetry_dashboard_network_generations ('
  'CREATE TABLE public.telemetry_dashboard_network_projection_heads ('
  'CREATE TABLE public.telemetry_dashboard_ping_projection_heads ('
  'CREATE TABLE public.telemetry_dashboard_projection_fences ('
  'CREATE TABLE public.telemetry_dashboard_block_events ('
  'CREATE TABLE public.telemetry_dashboard_generation_events ('
  'CREATE TABLE public.telemetry_dashboard_resource_generation_bounds ('
  'CREATE TABLE public.telemetry_dashboard_network_generation_bounds ('
  'CREATE TABLE public.telemetry_dashboard_resource_blocks ('
  'CREATE TABLE public.telemetry_dashboard_network_blocks ('
  'CREATE FUNCTION public.build_telemetry_dashboard_resource_generation('
  'CREATE FUNCTION public.build_telemetry_dashboard_network_generation('
  'CREATE FUNCTION public.acquire_next_telemetry_dashboard_projection_owner()'
  'CREATE FUNCTION public.claim_telemetry_dashboard_projection('
  'CREATE FUNCTION public.publish_telemetry_dashboard_projection('
)
for dashboard_object in "${dashboard_objects[@]}"; do
  [[ "$(grep -Fc "$dashboard_object" "$dashboard_schema" || true)" -eq 1 ]] ||
    fail "clean dashboard owner is missing or duplicated: $dashboard_object"
done
[[ "$(grep -Fc 'CREATE VIEW public.telemetry_dashboard_projection_heads AS' "$dashboard_schema")" -eq 1 ]] ||
  fail "unified dashboard envelope read view is missing or duplicated"
grep -Fq 'owner_id BIGINT GENERATED ALWAYS AS IDENTITY' "$dashboard_schema" ||
  fail "dashboard owners lack collision-free persistent lock identities"
if grep -Fq 'retry_at' "$dashboard_schema"; then
  fail "dashboard owner registry still carries timeout-based retry state"
fi

block_factor_body="$(
  sed -n '/CREATE FUNCTION public.telemetry_dashboard_block_factor(/,/^\$\$;/p' \
    "$dashboard_schema"
)"
grep -Fq 'SELECT 16' <<<"$block_factor_body" ||
  fail "dashboard block factor is not the fixed schema invariant F16"
dashboard_block_tables="$({
  sed -n '/CREATE TABLE public.telemetry_dashboard_resource_blocks (/,/^);/p' \
    "$dashboard_schema"
  sed -n '/CREATE TABLE public.telemetry_dashboard_network_blocks (/,/^);/p' \
    "$dashboard_schema"
})"
[[ "$(grep -Fc 'array_ndims(' <<<"$dashboard_block_tables")" -eq 17 ]] ||
  fail "dashboard block vectors do not all enforce one-dimensional arrays"
[[ "$(grep -Fc 'array_lower(' <<<"$dashboard_block_tables")" -eq 17 ]] ||
  fail "dashboard block vectors do not all enforce one-based arrays"
grep -Fq 'public.telemetry_dashboard_block_factor() * interface_width' \
  "$dashboard_schema" ||
  fail "closed network vectors are not F16 times immutable interface width"
if grep -Eq 'telemetry_dashboard_(resource|network)_active_rows|replace_telemetry_dashboard_(resource|network)_active_block' \
    "$dashboard_schema"; then
  fail "dashboard schema retains a duplicate active telemetry mirror"
fi
[[ "$(grep -Fc 'client_id, generation, interface_width' "$dashboard_schema")" -ge 4 ]] ||
  fail "network payload width is not tied to generation metadata"
grep -Fq 'telemetry_dashboard_resource_vectors_are_valid(' "$dashboard_schema" ||
  fail "resource F16 holes are not distinguished from present zero-disk rows"
grep -Fq 'telemetry_dashboard_network_vectors_are_valid(' "$dashboard_schema" ||
  fail "network F16 holes are not distinguished from present observations"
grep -Fq 'telemetry_dashboard_change_is_valid(' "$dashboard_schema" ||
  fail "head change descriptors lack canonical coordinate validation"
network_selection_body="$(
  sed -n '/CREATE FUNCTION public.telemetry_dashboard_effective_network_selection(/,/^\$\$;/p' \
    "$dashboard_schema"
)"
grep -Fq 'FROM public.vps_rule_values' <<<"$network_selection_body" ||
  fail "dashboard network selection is not bound to the clean rule schema"
grep -Fq "key = 'network.interfaces'" <<<"$network_selection_body" ||
  fail "dashboard network selection bypasses the interface admission rule"
grep -Fq "IF rate_rule IS NULL" <<<"$network_selection_body" ||
  fail "an absent network-rate rule does not select the empty set"
if grep -Eq 'to_regclass|EXECUTE' <<<"$network_selection_body"; then
  fail "dashboard network selection silently falls back around its schema owner"
fi
network_admission_body="$(
  sed -n '/CREATE FUNCTION public.telemetry_interface_is_admitted_resolved(/,/^\$\$;/p' \
    "$dashboard_schema"
)"
grep -Fq "p_source_kind = 'host'" <<<"$network_admission_body" ||
  fail "default interface admission is not host-only"
grep -Fq "left(p_interface, 1) IN ('e', 'w')" <<<"$network_admission_body" ||
  fail "default interface admission is not the canonical e*/w* boundary"
if grep -Eq '(^|[^[:alnum:]_])(LIKE|ILIKE)([^[:alnum:]_]|$)|~~' \
    <<<"$network_admission_body"; then
  fail "interface admission interprets operator patterns as SQL patterns"
fi
interface_order_bodies="$(
  sed -n '/CREATE FUNCTION public.telemetry_dashboard_interfaces_are_canonical(/,/^\$\$;/p' \
    "$dashboard_schema"
  sed -n '/CREATE FUNCTION public.telemetry_dashboard_effective_network_selection(/,/^\$\$;/p' \
    "$dashboard_schema"
  sed -n '/CREATE FUNCTION public.telemetry_dashboard_generation_interfaces(/,/^\$\$;/p' \
    "$dashboard_schema"
)"
[[ "$(grep -Fc 'COLLATE "C"' <<<"$interface_order_bodies")" -ge 6 ]] ||
  fail "dashboard interface ordinals are not consistently byte ordered"
for change_descriptor in \
  resource_change_source_bucket_secs \
  resource_change_block_start_unix \
  network_change_source_bucket_secs \
  network_change_block_start_unix; do
  grep -Fq "$change_descriptor" "$dashboard_schema" ||
    fail "dashboard head change descriptor is missing: $change_descriptor"
done

if grep -Fq 'mutation_id' "$dashboard_schema"; then
  fail "dashboard owner-wide event model retains an unused mutation-group identity"
fi
for client_age_index in \
  telemetry_dashboard_block_events_client_age_idx \
  telemetry_dashboard_generation_events_client_age_idx; do
  grep -Fq "CREATE INDEX $client_age_index" "$dashboard_schema" ||
    fail "dashboard stale-health client-age index is missing: $client_age_index"
done
for owner_event_index in \
  telemetry_dashboard_block_events_owner_event_idx \
  telemetry_dashboard_generation_events_owner_event_idx; do
  grep -Fq "CREATE INDEX $owner_event_index" "$dashboard_schema" ||
    fail "dashboard owner-claim event index is missing: $owner_event_index"
done
grep -Fq 'CREATE TABLE public.telemetry_dashboard_ready_owners (' \
  "$dashboard_schema" ||
  fail "dashboard bounded ready-owner relation is missing"
grep -Fq 'CREATE INDEX telemetry_dashboard_ready_owners_fifo_idx' \
  "$dashboard_schema" ||
  fail "dashboard ready-owner FIFO index is missing"
for obsolete_global_fifo_index in \
  telemetry_dashboard_block_events_fifo_idx \
  telemetry_dashboard_generation_events_fifo_idx; do
  if grep -Fq "CREATE INDEX $obsolete_global_fifo_index" "$dashboard_schema"; then
    fail "dashboard event stream retains obsolete global FIFO index: $obsolete_global_fifo_index"
  fi
done
if grep -Eq '^CREATE UNIQUE INDEX telemetry_dashboard_.*events' \
  "$dashboard_schema"; then
  fail "dashboard event coordinates are incorrectly coalesced by a unique index"
fi

dashboard_acquire_body="$(
  sed -n '/CREATE FUNCTION public.acquire_next_telemetry_dashboard_projection_owner()/,/^\$\$;/p' \
    "$dashboard_schema"
)"
grep -Fq 'FROM public.telemetry_dashboard_ready_owners ready' \
  <<<"$dashboard_acquire_body" ||
  fail "dashboard owner acquisition does not use the bounded ready relation"
grep -Fq 'FROM public.telemetry_dashboard_projection_fences fence' \
  <<<"$dashboard_acquire_body" ||
  fail "dashboard acquired owner cannot resolve its immutable identity"
grep -Fq 'WHERE fence.owner_id = candidate.owner_id' \
  <<<"$dashboard_acquire_body" ||
  fail "dashboard acquired owner identity is not a primary-key lookup"
grep -Fq 'ORDER BY ready.ready_at, ready.owner_id' \
  <<<"$dashboard_acquire_body" ||
  fail "dashboard owner acquisition is not deterministic FIFO"
if grep -Fq 'LIMIT ' <<<"$dashboard_acquire_body"; then
  fail "dashboard owner acquisition has an acceptance-tuned work cap"
fi
if grep -Eq 'telemetry_dashboard_(block|generation)_events|seen_(resource|network)_client_ids' \
  <<<"$dashboard_acquire_body"; then
  fail "dashboard owner acquisition still scans or deduplicates immutable event rows"
fi
grep -Fq 'IF pg_try_advisory_lock(candidate.owner_id)' \
  <<<"$dashboard_acquire_body" ||
  fail "dashboard ownership is not acquired by persistent owner identity"
grep -Fq 'PERFORM pg_advisory_unlock(candidate.owner_id)' \
  <<<"$dashboard_acquire_body" ||
  fail "dashboard acquisition leaks a concurrently deleted owner lock"
if grep -Eq 'FOR UPDATE|SKIP LOCKED|retry_at' <<<"$dashboard_acquire_body"; then
  fail "dashboard owner acquisition still depends on mutable MVCC fences"
fi

dashboard_claim_body="$(
  sed -n '/CREATE FUNCTION public.claim_telemetry_dashboard_projection(/,/^\$\$;/p' \
    "$dashboard_schema"
)"
grep -Fq 'p_owner_id BIGINT' <<<"$dashboard_claim_body" ||
  fail "dashboard RR capture is not targeted to its pre-acquired owner"
grep -Fq 'WHERE fence.owner_id = p_owner_id' <<<"$dashboard_claim_body" ||
  fail "dashboard RR capture does not resolve the immutable owner identity"
[[ "$(grep -Ec '^    IF EXISTS \(' <<<"$dashboard_claim_body")" -eq 1 ]] ||
  fail "dashboard capture does not test owner-wide generation authority"
[[ "$(grep -Ec '^    ELSIF EXISTS \(' <<<"$dashboard_claim_body")" -eq 1 ]] ||
  fail "dashboard capture does not test owner-wide block authority"
grep -Fq 'array_agg(event.event_id ORDER BY event.event_id)' \
  <<<"$dashboard_claim_body" ||
  fail "dashboard claim does not capture immutable event identities"
[[ "$(grep -Fc 'event.client_id = locked.client_id' <<<"$dashboard_claim_body")" -eq 3 ]] ||
  fail "dashboard owner snapshot does not capture all three owner-scoped event sets"
[[ "$(grep -Fc 'event.domain = locked.domain' <<<"$dashboard_claim_body")" -eq 3 ]] ||
  fail "dashboard owner snapshot does not keep all captured event sets domain-scoped"
grep -Fq "locked.change = 'generation'" <<<"$dashboard_claim_body" ||
  fail "dashboard generation claim does not subsume visible owner block events"
if grep -Eq 'FOR UPDATE|SKIP LOCKED|retry_at' <<<"$dashboard_claim_body"; then
  fail "dashboard RR capture still touches a mutable scheduling fence"
fi
if grep -Eq 'first_bucket_start|last_bucket_start|bool_or' \
  <<<"$dashboard_claim_body"; then
  fail "dashboard claim widens its exact owner-coordinate union into a range"
fi

queue_body="$(
  sed -n '/CREATE FUNCTION public.telemetry_dashboard_event_queued_at(/,/^\$\$;/p' \
    "$dashboard_schema"
)"
grep -Fq "current_setting('vpsman.telemetry_accepted_at', TRUE)" \
  <<<"$queue_body" ||
  fail "dashboard pending age does not inherit telemetry acceptance"
grep -Fq 'statement_timestamp()' <<<"$queue_body" ||
  fail "non-telemetry dashboard mutations lack a queue timestamp"

publish_body="$(
  sed -n '/CREATE FUNCTION public.publish_telemetry_dashboard_projection(/,/^\$\$;/p' \
    "$dashboard_schema"
)"
grep -Fq 'new_revision BIGINT := p_expected_revision + 1' \
  <<<"$publish_body" ||
  fail "dashboard revisions are not gapless per owner"
[[ "$(grep -Fc '= p_expected_generation' <<<"$publish_body")" -ge 4 ]] ||
  fail "dashboard publication does not fence every generation path"
[[ "$(grep -Fc '= p_expected_revision' <<<"$publish_body")" -ge 4 ]] ||
  fail "dashboard publication does not CAS every revision path"
grep -Fq 'event.event_id = ANY(COALESCE(' <<<"$publish_body" ||
  fail "dashboard publication does not consume exact event IDs"
grep -Fq "RAISE EXCEPTION 'dashboard generation capture changed'" \
  <<<"$publish_body" ||
  fail "dashboard coalescing does not validate its exact generation event IDs"
if grep -Fq 'telemetry_dashboard_projection_fences' <<<"$publish_body"; then
  fail "dashboard publication mutates its immutable owner registry"
fi
grep -Fq "'vpsman_telemetry_projection'," <<<"$publish_body" ||
  fail "dashboard publication lacks committed typed invalidation"
grep -Fq "resource_change = 'block'" <<<"$publish_body" ||
  fail "resource head does not publish exact block descriptors"
grep -Fq "network_change = 'block'" <<<"$publish_body" ||
  fail "network head does not publish exact block descriptors"
notice_body="$(
  sed -n '/    notice := jsonb_build_object(/,/    PERFORM pg_notify(/p' \
    <<<"$publish_body"
)"
grep -Fq 'WITH ORDINALITY coordinate(tier, block_start, ordinality)' \
  <<<"$publish_body" ||
  fail "dashboard block notification is not fragmented at exact coordinates"
grep -Fq 'ARRAY[block_coordinate.tier]::INTEGER[]' <<<"$notice_body" ||
  fail "dashboard block notification embeds an unbounded tier array"
grep -Fq 'ARRAY[block_coordinate.block_start]::BIGINT[]' <<<"$notice_body" ||
  fail "dashboard block notification embeds an unbounded start array"
grep -Fq "'complete', block_coordinate.ordinality = cardinality(" \
  <<<"$notice_body" ||
  fail "dashboard block notification does not fence its final fragment"
grep -Fq "'previous_revision', p_expected_revision" <<<"$notice_body" ||
  fail "dashboard notification cannot prove a contiguous resident revision span"

if grep -Eq 'UPDATE public\.telemetry_dashboard_(resource|network)_blocks|ON CONFLICT .*telemetry_dashboard_(resource|network)_blocks' \
  "$dashboard_schema"; then
  fail "closed dashboard blocks are updated in place"
fi
[[ "$(grep -Ec '^CREATE TRIGGER telemetry_rollups_dashboard_after_(insert|delete|update)$' "$dashboard_schema")" -eq 3 ]] ||
  fail "resource projection needs three statement transition triggers"
[[ "$(grep -Ec '^CREATE TRIGGER telemetry_network_rates_dashboard_after_(insert|delete|update)$' "$dashboard_schema")" -eq 3 ]] ||
  fail "network projection needs three statement transition triggers"
[[ "$(grep -Ec '^CREATE TRIGGER traffic_counter_samples_dashboard_after_(insert|delete|update)$' "$dashboard_schema")" -eq 3 ]] ||
  fail "closed native network samples need three statement transition triggers"
[[ "$(grep -Fc 'FOR EACH STATEMENT' "$dashboard_schema")" -ge 9 ]] ||
  fail "retained dashboard projections are not set-wise statement owners"
[[ "$(grep -Fc 'public.telemetry_network_durable_points_source(' "$dashboard_schema")" -eq 3 ]] ||
  fail "dashboard network owners do not each use one bounded durable source"
[[ "$(grep -Fc 'ARRAY[p_client_id],' "$dashboard_schema")" -ge 3 ]] ||
  fail "dashboard network durable sources do not bind their exact client"
[[ "$(grep -Ec '^[[:space:]]+p_interfaces$' "$dashboard_schema")" -ge 3 ]] ||
  fail "dashboard network durable sources do not bind their frozen interface vector"
grep -Fq 'CREATE FUNCTION public.queue_telemetry_dashboard_network_membership_change(' \
  "$dashboard_schema" ||
  fail "live network membership changes lack their generation-only helper"
if grep -Fq 'queue_telemetry_dashboard_network_coordinate' "$dashboard_schema"; then
  fail "obsolete live network coordinate helper survives"
fi
grep -Fq "'change', 'initialize'" "$dashboard_schema" ||
  fail "ready-empty client initialization is not announced"
grep -Fq "'change', 'remove'" "$dashboard_schema" ||
  fail "cascading client removal is not announced"

retired_dashboard_objects=(
  'telemetry_dashboard_all_grid'
  'telemetry_dashboard_tile_'
  'telemetry_dashboard_resource_tiles'
  'telemetry_dashboard_network_tiles'
  'telemetry_dashboard_repair'
  'telemetry_dashboard_facts_fallback'
  'summary_day_receipts'
  'authority_span'
  'resource_complete'
  'network_complete'
  'telemetry_resource_summary_'
  'telemetry_network_summary_'
  'telemetry_dashboard_projection_dirty_domains'
  'lock_next_telemetry_dashboard_dirty_domain'
  'reconstruct_telemetry_'
  'refresh_telemetry_resource_summary_suffix'
  'refresh_telemetry_network_summary_suffix'
  'vpsman.telemetry_history_promotion'
  'network_selection_hash'
  'SET work_mem'
)
for retired_dashboard_object in "${retired_dashboard_objects[@]}"; do
  if grep -Fq "$retired_dashboard_object" "$dashboard_schema"; then
    fail "retired dashboard mechanism survives: $retired_dashboard_object"
  fi
done

retired_dashboard_runtime_objects=(
  'telemetry_dashboard_repair'
  'telemetry_dashboard_facts_fallback'
  'telemetry_dashboard_projection_dirty_domains'
  'lock_next_telemetry_dashboard_dirty_domain'
  'reconstruct_telemetry_'
  'refresh_telemetry_resource_summary_suffix'
  'refresh_telemetry_network_summary_suffix'
  'vpsman.telemetry_history_promotion'
)
for retired_dashboard_runtime_object in \
  "${retired_dashboard_runtime_objects[@]}"; do
  if grep -rFq \
    --exclude='audit-migrations.sh' \
    --exclude='tests_repository_telemetry_rollups.rs' \
    --exclude-dir='.tmp' \
    --exclude-dir='node_modules' \
    --exclude-dir='dist' \
    "$retired_dashboard_runtime_object" \
    "$ROOT_DIR/crates" \
    "$ROOT_DIR/deploy" \
    "$ROOT_DIR/docs" \
    "$ROOT_DIR/frontend" \
    "$ROOT_DIR/migrations" \
    "$ROOT_DIR/scripts" \
    "$ROOT_DIR/tutorials" \
    "$ROOT_DIR/README.md" \
    "$ROOT_DIR/CONTRIBUTING.md"; then
    fail "retired dashboard runtime symbol survives outside its negative contract test: $retired_dashboard_runtime_object"
  fi
done

dashboard_non_ping="$(
  sed -n '1,/^-- Ping owns no range tree here\./p' "$dashboard_schema"
)"
if grep -Eq 'LOCK TABLE|FOR UPDATE OF (source|head)' \
  <<<"$dashboard_non_ping"; then
  fail "resource/network projection takes an out-of-scope lock"
fi
[[ "$(grep -Fc 'pg_try_advisory_lock(candidate.owner_id)' <<<"$dashboard_non_ping")" -eq 1 ]] ||
  fail "resource/network projection has an unexpected advisory acquisition"
[[ "$(grep -Fc 'pg_advisory_unlock(candidate.owner_id)' <<<"$dashboard_non_ping")" -eq 1 ]] ||
  fail "resource/network acquisition has an unexpected stale-owner release"

duplicate_tables="$({
  grep -Eh '^CREATE TABLE public\.[a-zA-Z0-9_]+ \(' "${schema_paths[@]}" || true
} | awk '{print $3}' | sort | uniq -d)"
[[ -z "$duplicate_tables" ]] || fail "duplicate table definitions: $duplicate_tables"
duplicate_functions="$({
  grep -Eh '^CREATE FUNCTION public\.[a-zA-Z0-9_]+' "${schema_paths[@]}" || true
} | awk '{print $3}' | sed 's/(.*//' | sort | uniq -d)"
[[ -z "$duplicate_functions" ]] || fail "duplicate function definitions: $duplicate_functions"
duplicate_indexes="$({
  grep -Eh '^CREATE (UNIQUE )?INDEX [a-zA-Z0-9_]+' "${schema_paths[@]}" || true
} | awk '{if ($2 == "UNIQUE") print $4; else print $3}' | sort | uniq -d)"
[[ -z "$duplicate_indexes" ]] || fail "duplicate index definitions: $duplicate_indexes"

declare -A expected_seed_statements=(
  [alert_policy_lifecycle_meta]=1
  [alert_telemetry_policy_activation]=1
  [fleet_tag_settings]=1
  [traffic_history_retention_cursors]=1
  [policy_groups]=5
  [policy_rules]=21
  [runtime_config_patch_generators]=1
)
while IFS= read -r table; do
  [[ -n "${expected_seed_statements[$table]:-}" ]] || fail "unexpected seeded table: $table"
done < <(
  sed -n 's/^INSERT INTO public\.\([a-zA-Z0-9_]*\) .*/\1/p' "$SEED_FILE" | sort -u
)
for table in "${!expected_seed_statements[@]}"; do
  actual="$(grep -Fc "INSERT INTO public.$table " "$SEED_FILE" || true)"
  expected="${expected_seed_statements[$table]}"
  [[ "$actual" -eq "$expected" ]] ||
    fail "seed statement count for public.$table is $actual, expected $expected"
done

# Telemetry producers append immutable eligibility events without touching the
# unique retention-owned span ledger. Ordinary telemetry advances only to its
# immediate successor. Automatic observations use the same adjacent ownership;
# a late additive fragment recreates that edge and walks forward idempotently.
telemetry_schema="$MIGRATIONS_DIR/0003_telemetry_core.sql"
network_schema="$MIGRATIONS_DIR/0004_network_tunnels.sql"
system_metric_schema="$MIGRATIONS_DIR/0010_system_metrics.sql"
network_observation_rollup_indexes=(
  'CREATE INDEX network_observation_rollups_retention_idx ON public.network_observation_rollups USING btree (bucket_start, series_id) INCLUDE (bucket_secs);'
  'CREATE INDEX network_observation_rollups_terminal_frontier_idx ON public.network_observation_rollups USING btree (bucket_secs, bucket_start, series_id, health_state);'
  'CREATE INDEX network_observation_rollups_series_time_idx ON public.network_observation_rollups USING btree (series_id, bucket_start, bucket_secs, health_state);'
)
for index_definition in "${network_observation_rollup_indexes[@]}"; do
  grep -Fxq "$index_definition" "$network_schema" ||
    fail "network-observation rollup index ownership differs: $index_definition"
done
[[ "$(grep -Ec '^CREATE INDEX network_observation_rollups_' "$network_schema")" -eq 3 ]] ||
  fail "network-observation rollups must have only their global-time, terminal-tier, and series consumer indexes"
grep -Fxq "CREATE INDEX network_observations_manual_retention_idx ON public.network_observations USING btree (observed_at, id) WHERE (source = 'manual'::text);" \
  "$network_schema" ||
  fail "manual network evidence lacks its source-exact terminal frontier"
grep -Fxq "CREATE INDEX network_observations_observed_idx ON public.network_observations USING btree (observed_at DESC, id DESC);" \
  "$network_schema" ||
  fail "mixed-source exact network history lacks its independent time frontier"
network_observation_retention_triggers=(
  network_observations_retention_publish_insert
  network_observations_retention_publish_update
  network_observations_retention_delete
  network_observation_rollups_retention_delete
  network_observation_latest_retention_delete
  network_observation_series_retention_deactivate
)
for trigger_name in "${network_observation_retention_triggers[@]}"; do
  grep -Fq "CREATE TRIGGER $trigger_name" "$network_schema" ||
    fail "network-observation retention writer trigger is missing: $trigger_name"
done
grep -Fxq '    CONSTRAINT system_metric_rollups_pkey PRIMARY KEY (bucket_secs, bucket_start, metric)' \
  "$system_metric_schema" ||
  fail "system metric identity is not tier/time-local"
grep -Fq 'CONSTRAINT telemetry_history_due_events_pkey PRIMARY KEY (event_id)' \
  "$telemetry_schema" || fail "telemetry due-event identity owner is missing"
grep -Fq 'event_id bigint GENERATED ALWAYS AS IDENTITY NOT NULL' \
  "$telemetry_schema" || fail "telemetry due events lack append-only identities"
grep -Fq 'coalesce_ready_at timestamp with time zone NOT NULL' \
  "$telemetry_schema" || fail "telemetry due events lack natural readiness boundaries"
grep -Fq 'CONSTRAINT telemetry_history_due_events_coalesce_ready_at_check CHECK (' \
  "$telemetry_schema" || fail "telemetry due-event readiness is not constrained"
if grep -Fq 'CREATE INDEX telemetry_history_due_events_coordinate_idx' \
    "$telemetry_schema"; then
  fail "unused telemetry due-event coordinate index survives alongside the readiness owner"
fi
grep -Fq 'CREATE INDEX telemetry_history_due_events_ready_idx' \
  "$telemetry_schema" || fail "telemetry due-event readiness index is missing"
[[ "$(grep -Fc 'owner_identity text[] NOT NULL' "$telemetry_schema")" -eq 2 ]] ||
  fail "telemetry due events and spans must carry exact natural owner identities"
grep -Fq 'CONSTRAINT telemetry_history_due_spans_pkey PRIMARY KEY (' \
  "$telemetry_schema" || fail "telemetry due-span primary owner is missing"
grep -Fq 'CREATE INDEX telemetry_history_due_spans_due_idx ON public.telemetry_history_due_spans USING btree (domain, due_at, source_bucket_secs, destination_bucket_secs, destination_start);' \
  "$telemetry_schema" || fail "telemetry due-span domain/deadline index is missing"
[[ "$(grep -Fc 'EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(' "$telemetry_schema")" -eq 6 ]] ||
  fail "ordinary telemetry requires insert/update due-event triggers on three tables"
[[ "$(grep -Fc 'EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(' "$network_schema")" -eq 2 ]] ||
  fail "network observations require insert/update due-event triggers"
[[ "$(grep -Fc 'EXECUTE FUNCTION public.enqueue_telemetry_history_due_events(' "$system_metric_schema")" -eq 2 ]] ||
  fail "system metrics require insert/update due-event triggers"
for due_event_trigger in \
  telemetry_network_rates_due_events_insert \
  telemetry_network_rates_due_events_update \
  telemetry_ping_rollups_due_events_insert \
  telemetry_ping_rollups_due_events_update \
  telemetry_rollups_due_events_insert \
  telemetry_rollups_due_events_update; do
  grep -Fq "CREATE TRIGGER $due_event_trigger" "$telemetry_schema" ||
    fail "ordinary telemetry due-event trigger is missing: $due_event_trigger"
done
for due_event_trigger in \
  network_observation_rollups_due_events_insert \
  network_observation_rollups_due_events_update; do
  grep -Fq "CREATE TRIGGER $due_event_trigger" "$network_schema" ||
    fail "network due-event trigger is missing: $due_event_trigger"
done
if grep -Eq '^CREATE TRIGGER .*_due_spans_(insert|update)$' \
    "$telemetry_schema" "$network_schema" "$system_metric_schema"; then
  fail "misnamed due-span producer trigger objects survive in clean migrations"
fi
if grep -Fq "WHEN TG_ARGV[0] = 'network_observation_rollups' THEN -1" \
    "$telemetry_schema"; then
  fail "obsolete all-future observation fanout survives"
fi
grep -Fq "JOIN phases phase ON row.bucket_secs = phase.source_bucket_secs" \
  "$telemetry_schema" || fail "telemetry no longer advances one adjacent tier at a time"
due_event_enqueue_body="$(
  sed -n '/CREATE FUNCTION public.enqueue_telemetry_history_due_events()/,/^\$\$;/p' \
    "$telemetry_schema"
)"
grep -Fq 'INSERT INTO public.telemetry_history_due_events (' \
  <<<"$due_event_enqueue_body" || fail "telemetry producers do not append due events"
grep -Fq 'SELECT DISTINCT' <<<"$due_event_enqueue_body" ||
  fail "due-event producer does not reduce one statement to distinct coordinates"
grep -Fq 'AS coalesce_ready_at' <<<"$due_event_enqueue_body" ||
  fail "due-event producer does not assign natural readiness"
if grep -Fq 'vpsman.network_observation_promotion' <<<"$due_event_enqueue_body"; then
  fail "observation promotion still suppresses successor production"
fi
grep -Fq 'phase.destination_bucket_secs)' <<<"$due_event_enqueue_body" ||
  fail "due events lost their destination completeness boundary"
grep -Fq 'ORDER BY domain, owner_identity, source_bucket_secs,' <<<"$due_event_enqueue_body" ||
  fail "due-event producer coordinates are not deterministic"
grep -Fq "'system_metric_rollups'::text" <<<"$due_event_enqueue_body" ||
  fail "system metric due events are not an accepted producer domain"
if grep -Eq 'telemetry_history_due_spans|FOR KEY SHARE|ON CONFLICT|DO UPDATE' \
    <<<"$due_event_enqueue_body"; then
  fail "producer due-event trigger still locks or mutates the unique due-span owner"
fi

# Traffic retention owns five fixed phase rows (raw, terminal prune, and three
# rollup transitions). Its frontier and scan timestamp are cursor state, never
# a per-stream queue or an append-growing table.
grep -Fq "traffic_frontier_start timestamp with time zone" \
  "$telemetry_schema" ||
  fail "traffic raw global frontier column is missing"
grep -Fq "traffic_scan_after timestamp with time zone" \
  "$telemetry_schema" ||
  fail "traffic scan cursor column is missing"
grep -Fq "traffic_history_retention_cursors_frontier_check" \
  "$telemetry_schema" ||
  fail "traffic raw global frontier shape constraint is missing"
[[ "$(grep -Ec "^    \\('traffic_counter_samples'," "$SEED_FILE")" -eq 5 ]] ||
  fail "traffic retention must seed exactly five fixed phase cursors"
if grep -Fq 'history_retention_scan_cursors' "$telemetry_schema" "$SEED_FILE"; then
  fail "obsolete shared retention cursor ownership survives in clean schema"
fi
[[ "$(grep -Ec '^INSERT INTO public\.' "$SEED_FILE")" -eq 31 ]] ||
  fail "seed file must contain exactly 31 insert statements"
[[ "$(grep -Ec 'ON CONFLICT' "$SEED_FILE")" -eq 31 ]] ||
  fail "every seed insert must be idempotent"
for generator_id in \
  55555555-5555-4555-8555-555555555555 \
  66666666-6666-4666-8666-666666666666; do
  [[ "$(grep -Fc "$generator_id" "$SEED_FILE")" -eq 1 ]] ||
    fail "built-in generator seed is missing or duplicated"
done

[[ ! -e "$ROOT_DIR/crates/server-core/src/runtime/postgres_migrations.rs" ]] ||
  fail "obsolete shared migration subsystem survives"
if grep -Fq "run_postgres_migrations" \
  "$ROOT_DIR/crates/server-core/src/lib.rs" "$API_REPOSITORY" "$WORKER_RUNTIME"; then
  fail "obsolete shared migration wrapper survives"
fi
grep -Fq "sqlx::migrate::Migrator::new(migrations_dir)" "$API_REPOSITORY" ||
  fail "API startup must use the ordinary SQLx migrator"
grep -Fq "sqlx::migrate::Migrator::new(migrations_dir)" "$WORKER_RUNTIME" ||
  fail "worker startup must use the ordinary SQLx migrator"
for runtime in "$API_REPOSITORY" "$WORKER_RUNTIME"; do
  grep -Fq 'const SQLX_METADATA_SCHEMA: &str = "vpsman_internal";' "$runtime" ||
    fail "PostgreSQL runtime lacks the exact private SQLx metadata schema"
  grep -Fq 'const SQLX_METADATA_SCHEMA_LOCK_KEY: i64 = 0x5650_534d_5351_4c58;' "$runtime" ||
    fail "PostgreSQL runtime lacks the shared startup-only schema owner"
  grep -Fq 'SELECT pg_advisory_xact_lock($1)' "$runtime" ||
    fail "private SQLx schema creation is not transaction-scoped"
  grep -Fq 'CREATE SCHEMA IF NOT EXISTS vpsman_internal AUTHORIZATION CURRENT_USER' "$runtime" ||
    fail "PostgreSQL runtime does not provision the private SQLx schema"
  grep -Fq 'SET search_path TO vpsman_internal, public' "$runtime" ||
    fail "ordinary SQLx does not run with the private metadata schema first"
  grep -Fq 'current_schema()' "$runtime" ||
    fail "PostgreSQL runtime does not fail closed on SQLx schema selection"
  grep -Fq '.run(&mut migration_connection)' "$runtime" ||
    fail "ordinary SQLx is not bound to the dedicated migration connection"
  if ! grep -Fq '.options([("search_path", "public")])' "$runtime"; then
    grep -Fq '[("search_path", "public"), ("jit", "off")]' "$runtime" &&
      grep -Fq '.options(API_POSTGRES_SESSION_OPTIONS)' "$runtime" ||
      fail "normal application connections are not pinned to the public schema"
  fi
  grep -Fq 'failed to close the dedicated PostgreSQL migration connection' "$runtime" ||
    fail "dedicated SQLx connection does not have an explicit close contract"
done
[[ "$(grep -rF 'sqlx::migrate::Migrator::new' \
  "$ROOT_DIR/crates/api" "$ROOT_DIR/crates/worker" --include='*.rs' | wc -l)" -eq 2 ]] ||
  fail "API/worker contain a direct SQLx migrator outside the two startup owners"
grep -Fq 'postgres_sqlx_metadata_is_private_and_application_connections_are_public' \
  "$SQLX_CATALOG_TEST" ||
  fail "private SQLx restart/catalog contract test is missing"
[[ "$(grep -Fc 'vpsman_internal._sqlx_migrations' "$SQLX_CATALOG_TEST")" -eq 1 ]] ||
  fail "private SQLx catalog contract does not inspect exactly one ledger"
grep -Fq 'assert_eq!(application_schema, "public");' "$SQLX_CATALOG_TEST" ||
  fail "private SQLx catalog contract does not prove the application schema"
grep -Fq 'assert_eq!(private_ledger_rows, 13);' "$SQLX_CATALOG_TEST" ||
  fail "private SQLx catalog contract does not prove all clean migrations"
grep -Fq 'assert_eq!(public_internal_relations, 0);' "$SQLX_CATALOG_TEST" ||
  fail "private SQLx catalog contract does not reject public internal relations"

printf '{"migration_audit":"ok","model":"domain_declarative","migration_count":13,"schema_files":12,"seed_statements":31}\n'
