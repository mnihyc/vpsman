#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools bash cargo date docker tee

stamp="$(date +%Y%m%d-%H%M%S)"
log_dir="${VPSMAN_RELEASE_LOG_DIR:-target/release-check/$stamp}"
mkdir -p "$log_dir"

run_step() {
  local name="$1"
  shift
  local log="$log_dir/$name.log"
  printf '\n==> %s\n' "$name"
  "$@" 2>&1 | tee "$log"
}

run_legacy_step() {
  local name="$1"
  shift
  local log="$log_dir/$name.log"
  printf '\n==> %s\n' "$name"
  if "$@" 2>&1 | tee "$log"; then
    return 0
  fi
  printf 'legacy_check_non_blocking=%s log=%s\n' "$name" "$log" | tee -a "$log"
}

run_shell_step() {
  local name="$1"
  local command="$2"
  run_step "$name" env VPSMAN_RELEASE_ROOT="$ROOT_DIR" bash -ic \
    "cd \"\$VPSMAN_RELEASE_ROOT\" && $command"
}

skip_step() {
  local name="$1"
  local reason="$2"
  printf '\n==> %s\nskipped: %s\n' "$name" "$reason" | tee "$log_dir/$name.log"
}

run_step cargo-fmt cargo fmt --all -- --check
run_legacy_step repo-hygiene bash scripts/scan-repo-hygiene.sh
run_legacy_step large-file-audit bash scripts/audit-large-files.sh
run_legacy_step tutorial-audit bash scripts/audit-tutorials.sh
run_step customizability-audit bash scripts/audit-customizability.sh
run_step migration-compatibility-audit bash scripts/audit-migrations.sh
run_step agent-static-deps-audit bash scripts/audit-agent-static-deps.sh
run_step security-sweep bash scripts/security-sweep.sh

if [[ "${VPSMAN_RELEASE_SKIP_TESTS:-0}" == "1" ]]; then
  skip_step cargo-test-common-file-transfer "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-api-file-transfer "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-agent-file-transfer "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-vpsctl-file-transfer "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-agent-terminal "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-api-terminal "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-vpsctl-terminal "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step terminal-retention-release-gate "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-agent-network-apply "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-agent-network-runtime "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-agent-network-status "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-api-update-releases "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-api-history-retention "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-vpsctl-update-releases "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-worker-leases "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-worker-schedules "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-worker-alert-notifications "VPSMAN_RELEASE_SKIP_TESTS=1"
  skip_step cargo-test-workspace "VPSMAN_RELEASE_SKIP_TESTS=1"
else
  run_step cargo-test-common-file-transfer cargo test -p vpsman-common file_transfer
  run_step cargo-test-api-file-transfer cargo test -p vpsman-api tests_files
  run_step cargo-test-agent-file-transfer cargo test -p vpsman-agent resumable_file
  run_step cargo-test-vpsctl-file-transfer cargo test -p vpsctl file_transfer
  run_step cargo-test-agent-terminal cargo test -p vpsman-agent terminal
  run_step cargo-test-api-terminal cargo test -p vpsman-api terminal
  run_step cargo-test-vpsctl-terminal cargo test -p vpsctl terminal
  run_step terminal-retention-release-gate bash scripts/smoke-terminal-retention.sh
  run_step cargo-test-agent-network-apply cargo test -p vpsman-agent network_apply
  run_step cargo-test-agent-network-runtime cargo test -p vpsman-agent network_runtime
  run_step cargo-test-agent-network-status cargo test -p vpsman-agent network_status
  run_step cargo-test-api-update-releases cargo test -p vpsman-api tests_update_releases
  run_step cargo-test-api-history-retention cargo test -p vpsman-api history_retention
  run_step cargo-test-vpsctl-update-releases cargo test -p vpsctl update_release
  run_step cargo-test-worker-leases cargo test -p vpsman-worker worker_leases
  run_step cargo-test-worker-schedules cargo test -p vpsman-worker schedule
  run_step cargo-test-worker-alert-notifications cargo test -p vpsman-worker alert_notifications
  run_step cargo-test-workspace cargo test --workspace
fi

if [[ "${VPSMAN_RELEASE_SKIP_CLIPPY:-0}" == "1" ]]; then
  skip_step cargo-clippy "VPSMAN_RELEASE_SKIP_CLIPPY=1"
else
  run_step cargo-clippy cargo clippy --workspace --all-targets -- -D warnings
fi

if [[ "${VPSMAN_RELEASE_SKIP_MUSL:-0}" == "1" ]]; then
  skip_step cargo-build-agent-x86_64-musl "VPSMAN_RELEASE_SKIP_MUSL=1"
  skip_step cargo-build-agent-aarch64-musl "VPSMAN_RELEASE_SKIP_MUSL=1"
  skip_step cargo-build-vpsctl-x86_64-musl "VPSMAN_RELEASE_SKIP_MUSL=1"
  skip_step cargo-build-vpsctl-aarch64-musl "VPSMAN_RELEASE_SKIP_MUSL=1"
else
  run_step cargo-build-agent-x86_64-musl \
    cargo build -p vpsman-agent --target x86_64-unknown-linux-musl
  run_step cargo-build-agent-aarch64-musl \
    cargo build -p vpsman-agent --target aarch64-unknown-linux-musl
  run_step cargo-build-vpsctl-x86_64-musl \
    cargo build -p vpsctl --target x86_64-unknown-linux-musl
  run_step cargo-build-vpsctl-aarch64-musl \
    cargo build -p vpsctl --target aarch64-unknown-linux-musl
fi

if [[ "${VPSMAN_RELEASE_SKIP_FRONTEND:-0}" == "1" ]]; then
  skip_step frontend-npm-install "VPSMAN_RELEASE_SKIP_FRONTEND=1"
  skip_step check-frontend-contracts "VPSMAN_RELEASE_SKIP_FRONTEND=1"
  skip_step frontend-build "VPSMAN_RELEASE_SKIP_FRONTEND=1"
  skip_step frontend-audit "VPSMAN_RELEASE_SKIP_FRONTEND=1"
else
  if [[ "${VPSMAN_RELEASE_SKIP_NPM_INSTALL:-0}" == "1" ]]; then
    skip_step frontend-npm-install "VPSMAN_RELEASE_SKIP_NPM_INSTALL=1"
  else
    run_shell_step frontend-npm-install "cd frontend && npm install"
  fi
  run_step check-frontend-contracts bash scripts/check-frontend-contracts.sh
  run_shell_step frontend-build "cd frontend && npm run build"
  run_shell_step frontend-audit "cd frontend && npm audit --audit-level=moderate"
  run_step frontend-console-layout bash scripts/smoke-frontend-console-layout.sh
  run_step frontend-screenshot-review bash scripts/smoke-frontend-screenshot-review.sh
  run_step frontend-live-api bash scripts/smoke-frontend-live-api.sh
fi

if [[ "${VPSMAN_RELEASE_SKIP_DOCKER:-0}" == "1" ]]; then
  skip_step docker-compose-config "VPSMAN_RELEASE_SKIP_DOCKER=1"
else
  run_step docker-compose-config docker compose -f deploy/compose.yml config
fi

if [[ "${VPSMAN_RELEASE_SKIP_SMOKES:-0}" == "1" ]]; then
  skip_step cargo-build-smoke-binaries "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-vpsctl-cli-help "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-ui-cli-vty-parity "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-vpsctl-cli-semantics "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-vpsctl-structured-output "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-vpsctl-live-api "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-postgres-persistence "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-postgres-live-job-output "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-agent-install-assets "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-agent-install-distro-matrix "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-agent-resource-budget "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-agent-reconnect-churn "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-agent-endpoint-failover "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-live-file-push "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-live-backup "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-backup-chunked-upload "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-minio-backup-artifact "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-minio-update-artifact "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-live-hot-config "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-live-data-source-config-patch "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-live-agent-update "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-live-network-apply "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-network-preset-container "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-bird2-topology-convergence "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-docker-50-agent-long-running-fleet "VPSMAN_RELEASE_SKIP_SMOKES=1"
  skip_step smoke-final-e2e "VPSMAN_RELEASE_SKIP_SMOKES=1"
else
  run_step cargo-build-smoke-binaries \
    cargo build -p vpsman-api -p vpsman-gateway -p vpsman-agent -p vpsctl
  run_step smoke-vpsctl-cli-help \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-vpsctl-cli-help.sh
  run_step smoke-ui-cli-vty-parity \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-ui-cli-vty-parity.sh
  run_step smoke-vpsctl-cli-semantics \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-vpsctl-cli-semantics.sh
  run_step smoke-vpsctl-structured-output \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-vpsctl-structured-output.sh
  run_step smoke-vpsctl-live-api \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-vpsctl-live-api.sh
  run_step smoke-agent-install-assets \
    bash scripts/smoke-agent-install-assets.sh
  run_step smoke-agent-install-distro-matrix \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-agent-install-distro-matrix.sh
  run_step smoke-agent-resource-budget \
    bash scripts/smoke-agent-resource-budget.sh
  run_step smoke-agent-reconnect-churn \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-agent-reconnect-churn.sh
  run_step smoke-postgres-persistence \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-postgres-persistence.sh
  run_step smoke-postgres-live-job-output \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-postgres-live-job-output.sh
  run_step smoke-agent-endpoint-failover \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-agent-endpoint-failover.sh
  run_step smoke-live-file-push \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-live-file-push.sh
  run_step smoke-live-backup \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-live-backup.sh
  run_step smoke-backup-chunked-upload \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-backup-chunked-upload.sh
  if [[ "${VPSMAN_RELEASE_RUN_S3_SMOKES:-0}" == "1" ]]; then
    run_step smoke-minio-backup-artifact \
      env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-minio-backup-artifact.sh
    run_step smoke-minio-update-artifact \
      env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-minio-update-artifact.sh
  else
    skip_step smoke-minio-backup-artifact \
      "VPSMAN_RELEASE_RUN_S3_SMOKES!=1; local filesystem object storage is the baseline"
    skip_step smoke-minio-update-artifact \
      "VPSMAN_RELEASE_RUN_S3_SMOKES!=1; local filesystem object storage is the baseline"
  fi
  run_step smoke-live-hot-config \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-live-hot-config.sh
  run_step smoke-live-data-source-config-patch \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-live-data-source-config-patch.sh
  run_step smoke-live-agent-update \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-live-agent-update.sh
  run_step smoke-live-network-apply \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-live-network-apply.sh
  run_step smoke-network-preset-container \
    bash scripts/smoke-network-preset-container.sh
  run_step smoke-bird2-topology-convergence \
    bash scripts/smoke-bird2-topology-convergence.sh
  run_step smoke-docker-50-agent-long-running-fleet \
    env VPSMAN_SMOKE_SKIP_BUILD=1 bash scripts/smoke-docker-50-agent-long-running-fleet.sh
  run_step smoke-final-e2e \
    env VPSMAN_FINAL_E2E_LOG_DIR="$log_dir" bash scripts/smoke-final-e2e.sh
fi

printf '\nrelease_check=ok log_dir=%s\n' "$log_dir" | tee "$log_dir/summary.log"
