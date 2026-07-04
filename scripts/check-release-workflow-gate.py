#!/usr/bin/env python3
"""Verify tag releases cannot publish before release quality checks pass."""

from __future__ import annotations

import sys
from pathlib import Path


WORKFLOW_PATH = Path(".github/workflows/release.yml")
QUALITY_SCRIPT_PATH = Path("scripts/ci-release-quality-gate.sh")
QUALITY_JOB = "release-quality-gate"
VERSION_JOB = "release-version-gate"
ARTIFACT_JOBS = ("linux-binaries", "frontend")
PUBLISH_JOB = "github-release"
QUALITY_SCRIPT_COMMAND = f"bash {QUALITY_SCRIPT_PATH}"
REQUIRED_QUALITY_SCRIPT_COMMANDS = (
    "cargo fmt --all -- --check",
    "cargo clippy --workspace --all-targets -- -D warnings",
    "cargo test --workspace",
    "npm run build",
    "npm audit --audit-level=moderate",
    "docker compose -f deploy/compose.yml config",
)


def fail(message: str) -> None:
    print(f"release workflow gate check failed: {message}", file=sys.stderr)
    raise SystemExit(1)


def job_blocks(workflow_text: str) -> dict[str, list[str]]:
    jobs: dict[str, list[str]] = {}
    current_job: str | None = None
    in_jobs = False
    for line in workflow_text.splitlines():
        if line == "jobs:":
            in_jobs = True
            current_job = None
            continue
        if not in_jobs:
            continue
        if line and not line.startswith(" "):
            current_job = None
            continue
        if line.startswith("  ") and not line.startswith("    "):
            key = line[2:].split(":", 1)[0]
            if key and all(char.isalnum() or char in "-_" for char in key):
                current_job = key
                jobs[current_job] = [line]
                continue
        if current_job is not None:
            jobs[current_job].append(line)
    return jobs


def needs(job: list[str]) -> set[str]:
    parsed_needs: set[str] = set()
    collecting = False
    for line in job:
        if line.startswith("    needs:"):
            collecting = True
            inline = line.split(":", 1)[1].strip()
            if inline:
                parsed_needs.add(inline.strip("\"'"))
            continue
        if collecting:
            if line.startswith("      - "):
                parsed_needs.add(line.split("- ", 1)[1].strip().strip("\"'"))
                continue
            if line.startswith("    ") and line.strip():
                break
    return parsed_needs


def main() -> None:
    workflow_text = WORKFLOW_PATH.read_text()
    jobs = job_blocks(workflow_text)
    if not jobs:
        fail("release workflow has no jobs mapping")

    for job_name in (VERSION_JOB, QUALITY_JOB, *ARTIFACT_JOBS, PUBLISH_JOB):
        if job_name not in jobs:
            fail(f"missing job {job_name}")

    quality_job = jobs[QUALITY_JOB]
    if VERSION_JOB not in needs(quality_job):
        fail(f"{QUALITY_JOB} must need {VERSION_JOB}")

    quality_run_text = "\n".join(quality_job)
    if QUALITY_SCRIPT_COMMAND not in quality_run_text:
        fail(f"{QUALITY_JOB} must run `{QUALITY_SCRIPT_COMMAND}`")

    quality_script_text = QUALITY_SCRIPT_PATH.read_text()
    for command in REQUIRED_QUALITY_SCRIPT_COMMANDS:
        if command not in quality_script_text:
            fail(f"{QUALITY_SCRIPT_PATH} does not run `{command}`")

    for job_name in ARTIFACT_JOBS:
        if QUALITY_JOB not in needs(jobs[job_name]):
            fail(f"{job_name} must need {QUALITY_JOB}")

    publish_needs = needs(jobs[PUBLISH_JOB])
    for job_name in ARTIFACT_JOBS:
        if job_name not in publish_needs:
            fail(f"{PUBLISH_JOB} must need {job_name}")

    print("release workflow gate check passed")


if __name__ == "__main__":
    main()
