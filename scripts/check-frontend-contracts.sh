#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

generated="target/check-frontend-contracts/protocolContracts.ts"
mkdir -p "$(dirname "$generated")"
cargo run -q -p vpsman-common --bin generate_frontend_contracts -- "$generated"
cmp -s "$generated" frontend/src/generated/protocolContracts.ts || {
  echo "frontend protocol contracts are stale; run npm run --prefix frontend generate:contracts" >&2
  diff -u frontend/src/generated/protocolContracts.ts "$generated" >&2 || true
  exit 1
}
printf '{"frontend_protocol_contracts":"ok"}\n'
