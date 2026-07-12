#!/usr/bin/env python3
"""Stateful routing-cost adapter used only by the live integration smoke."""

import json
import os
from pathlib import Path
import re
import sys


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(2)


try:
    request = json.load(sys.stdin)
except (json.JSONDecodeError, UnicodeDecodeError) as error:
    fail(f"invalid adapter request: {error}")

if request.get("contract_version") != 1:
    fail("unsupported adapter contract version")

client_id = request.get("client_id")
interface_name = request.get("interface_name")
operation = request.get("operation")
if not isinstance(client_id, str) or not isinstance(interface_name, str):
    fail("adapter request identity is missing")
if operation not in {"status", "apply"}:
    fail("adapter operation is invalid")

state_root = os.environ.get("VPSMAN_SMOKE_ROUTING_STATE_DIR")
if not state_root:
    fail("VPSMAN_SMOKE_ROUTING_STATE_DIR is missing")
safe_client_id = re.sub(r"[^A-Za-z0-9_.-]", "_", client_id)
state_path = Path(state_root) / f"{safe_client_id}.cost"
state_path.parent.mkdir(parents=True, exist_ok=True)
try:
    current_cost = int(state_path.read_text(encoding="ascii").strip())
except FileNotFoundError:
    current_cost = 1000
except ValueError:
    fail("stored routing cost is invalid")

applied_cost = None
if operation == "apply":
    desired_cost = request.get("desired_cost")
    if not isinstance(desired_cost, int) or not 1 <= desired_cost <= 65535:
        fail("desired routing cost is invalid")
    temporary_path = state_path.with_suffix(".tmp")
    temporary_path.write_text(f"{desired_cost}\n", encoding="ascii")
    temporary_path.replace(state_path)
    current_cost = desired_cost
    applied_cost = desired_cost

json.dump(
    {
        "contract_version": 1,
        "interface_name": interface_name,
        "ready": True,
        "current_cost": current_cost,
        "applied_cost": applied_cost,
        "adapter_version": "live-smoke-v1",
        "message": "operator-owned smoke adapter ready",
    },
    sys.stdout,
    separators=(",", ":"),
)
sys.stdout.write("\n")
