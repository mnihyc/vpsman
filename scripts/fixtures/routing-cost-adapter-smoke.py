#!/usr/bin/env python3
"""Stateful direct-argv routing-cost adapter used by the live smoke test."""

import argparse
import os
from pathlib import Path
import re


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=("status", "apply"))
    parser.add_argument("--plan-id", required=True)
    parser.add_argument("--interface", required=True)
    parser.add_argument("--side", choices=("left", "right"), required=True)
    parser.add_argument("--client-id", required=True)
    parser.add_argument("--cost", type=int)
    args = parser.parse_args()
    if args.operation == "apply" and not 1 <= (args.cost or 0) <= 65535:
        parser.error("apply requires --cost from 1 to 65535")
    if args.operation == "status" and args.cost is not None:
        parser.error("status does not accept --cost")
    return args


args = parse_args()
state_root = os.environ.get("VPSMAN_SMOKE_ROUTING_STATE_DIR")
if not state_root:
    raise SystemExit("VPSMAN_SMOKE_ROUTING_STATE_DIR is missing")

safe_client_id = re.sub(r"[^A-Za-z0-9_.-]", "_", args.client_id)
state_path = Path(state_root) / f"{safe_client_id}.cost"
state_path.parent.mkdir(parents=True, exist_ok=True)
try:
    current_cost = int(state_path.read_text(encoding="ascii").strip())
except FileNotFoundError:
    current_cost = 1000
except ValueError as error:
    raise SystemExit("stored routing cost is invalid") from error

if args.operation == "status":
    print(current_cost)
else:
    temporary_path = state_path.with_suffix(".tmp")
    temporary_path.write_text(f"{args.cost}\n", encoding="ascii")
    temporary_path.replace(state_path)
    print(
        f"updated {args.plan_id} {args.side} interface {args.interface} "
        f"to cost {args.cost}"
    )
