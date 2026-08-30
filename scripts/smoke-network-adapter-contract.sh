#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT_DIR/scripts/lib-smoke.sh"

smoke_enter_root
smoke_require_tools cargo

cargo test -p vpsman-common network::tests:: -- --nocapture
cargo test -p vpsman-agent network_routing_adapter -- --nocapture
cargo test -p vpsman-api ospf -- --nocapture
cargo test -p vpsctl vty_network_ospf -- --nocapture

printf '{\n'
printf '  "network_adapter_contract_smoke": "ok",\n'
printf '  "runtime_ownership": ["agent_builtin", "external_observed", "custom_adapter"],\n'
printf '  "routing_control": "server_issued_endpoint_adapter_jobs",\n'
printf '  "unmanaged_discovery": false\n'
printf '}\n'
