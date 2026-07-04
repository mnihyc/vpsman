#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

(
  cd frontend
  npm ci
  npm run build
  npm audit --audit-level=moderate
)

created_deploy_env=0
if [[ ! -f deploy/.env ]]; then
  cp deploy/.env.example deploy/.env
  created_deploy_env=1
fi
cleanup() {
  if [[ "$created_deploy_env" == "1" ]]; then
    rm -f deploy/.env
  fi
}
trap cleanup EXIT

docker compose -f deploy/compose.yml config
