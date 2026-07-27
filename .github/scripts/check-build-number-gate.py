#!/usr/bin/env python3
"""Validate component build counters and require release-to-release advancement."""

from __future__ import annotations

import argparse
import re
import runpy
import subprocess
import sys
from pathlib import Path


COMPONENTS = ("agent", "cli", "frontend", "server")
POSITIVE_INTEGER_RE = re.compile(r"^[1-9][0-9]*$")
MAX_SAFE_BUILD_NUMBER = 2**53 - 1
REPO_ROOT = Path(__file__).resolve().parents[2]


def parse_counter(source: str, value: str) -> int:
    normalized = value.strip()
    if not POSITIVE_INTEGER_RE.fullmatch(normalized):
        raise ValueError(f"{source} must contain one positive integer")
    parsed = int(normalized)
    if parsed > MAX_SAFE_BUILD_NUMBER:
        raise ValueError(
            f"{source} exceeds the cross-runtime safe integer range"
        )
    return parsed


def current_counters(repo_root: Path = REPO_ROOT) -> dict[str, int]:
    counters: dict[str, int] = {}
    for component in COMPONENTS:
        relative_path = Path("build/build-numbers") / f"{component}.txt"
        path = repo_root / relative_path
        try:
            value = path.read_text(encoding="utf-8")
        except OSError as exc:
            raise ValueError(f"failed to read {relative_path}: {exc}") from exc
        counters[component] = parse_counter(str(relative_path), value)
    return counters


def tagged_counters(reference_tag: str, repo_root: Path = REPO_ROOT) -> dict[str, int]:
    counters: dict[str, int] = {}
    for component in COMPONENTS:
        relative_path = f"build/build-numbers/{component}.txt"
        result = subprocess.run(
            ["git", "show", f"{reference_tag}:{relative_path}"],
            cwd=repo_root,
            check=False,
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            detail = result.stderr.strip() or "Git returned no failure detail"
            raise ValueError(
                f"cannot read {relative_path} from {reference_tag}: {detail}. "
                "Fetch the full release-tag history before running the gate."
            )
        counters[component] = parse_counter(
            f"{reference_tag}:{relative_path}",
            result.stdout,
        )
    return counters


def advancement_failures(
    current: dict[str, int],
    historical_maximum: dict[str, int],
    maximum_tags: dict[str, str],
) -> list[str]:
    return [
        f"{component} build number {current[component]} must be greater than "
        f"{historical_maximum[component]} from {maximum_tags[component]}"
        for component in COMPONENTS
        if current[component] <= historical_maximum[component]
    ]


def version_gate_functions() -> dict[str, object]:
    return runpy.run_path(
        str(Path(__file__).with_name("check-release-version-gate.py"))
    )


def valid_semver_tags(
    tags: list[str],
    source: str,
    *,
    warn: bool = True,
) -> list[str]:
    parse_tag = version_gate_functions()["parse_tag"]
    valid: list[str] = []
    seen: set[str] = set()
    for raw_tag in tags:
        tag = raw_tag.strip()
        if not tag or tag in seen:
            continue
        try:
            parse_tag(tag)
        except ValueError:
            if warn:
                print(
                    f"Ignoring non-semver {source} release tag: {tag}",
                    file=sys.stderr,
                )
            continue
        seen.add(tag)
        valid.append(tag)
    return valid


def published_reference_tags_from_values(
    candidate_tag: str,
    tags: list[str],
) -> list[str]:
    parse_tag = version_gate_functions()["parse_tag"]
    parse_tag(candidate_tag)
    return valid_semver_tags(tags, "published")


def published_reference_tags(candidate_tag: str, tags_file: Path) -> list[str]:
    try:
        tags = tags_file.read_text(encoding="utf-8").splitlines()
    except OSError as exc:
        raise ValueError(f"failed to read published release tags: {exc}") from exc
    return published_reference_tags_from_values(candidate_tag, tags)


def run_git(args: list[str], repo_root: Path) -> str:
    try:
        result = subprocess.run(
            ["git", *args],
            cwd=repo_root,
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError as exc:
        raise ValueError(f"failed to query local release-tag history: {exc}") from exc
    if result.returncode != 0:
        detail = result.stderr.strip() or "Git returned no failure detail"
        raise ValueError(f"failed to query local release-tag history: {detail}")
    return result.stdout


def local_reference_tags(repo_root: Path = REPO_ROOT) -> list[str]:
    shallow = run_git(["rev-parse", "--is-shallow-repository"], repo_root).strip()
    if shallow not in {"true", "false"}:
        raise ValueError(
            "failed to query local release-tag history: "
            f"unexpected shallow-repository result {shallow!r}"
        )
    if shallow == "true":
        raise ValueError(
            "local Git history is shallow; fetch complete release-tag history "
            "before running the build-number gate"
        )
    tags = run_git(["tag", "--list"], repo_root).splitlines()
    return valid_semver_tags(tags, "local")


def maximum_tagged_counters(
    reference_tags: list[str],
    read_tag=tagged_counters,
) -> tuple[dict[str, int], dict[str, str]]:
    maxima = {component: 0 for component in COMPONENTS}
    maximum_tags = {component: "" for component in COMPONENTS}
    for reference_tag in reference_tags:
        counters = read_tag(reference_tag)
        for component in COMPONENTS:
            if counters[component] > maxima[component]:
                maxima[component] = counters[component]
                maximum_tags[component] = reference_tag
    return maxima, maximum_tags


def run_self_test() -> int:
    for value, expected in (
        ("1\n", 1),
        ("42", 42),
        ("9007199254740991", 9007199254740991),
    ):
        if parse_counter("test", value) != expected:
            print(f"self-test failed to parse {value!r}", file=sys.stderr)
            return 1
    for invalid in (
        "",
        "0",
        "-1",
        "1.5",
        "1\n2",
        "9007199254740992",
        "not-a-number",
    ):
        try:
            parse_counter("test", invalid)
        except ValueError:
            continue
        print(f"self-test accepted invalid counter {invalid!r}", file=sys.stderr)
        return 1

    tagged = {
        "v1.2.3": {
            "agent": 10,
            "cli": 30,
            "frontend": 50,
            "server": 70,
        },
        "v1.2.4-rc.1": {
            "agent": 20,
            "cli": 25,
            "frontend": 60,
            "server": 65,
        },
        "v2.0.0-rc.1": {
            "agent": 15,
            "cli": 40,
            "frontend": 55,
            "server": 80,
        },
    }

    def read_test_tag(tag: str) -> dict[str, int]:
        return tagged[tag]

    rc_to_stable_tags = published_reference_tags_from_values(
        "v1.2.4",
        ["v1.2.3", "v1.2.4-rc.1"],
    )
    if "v1.2.4-rc.1" not in rc_to_stable_tags:
        print("self-test omitted a prerelease before stable publication", file=sys.stderr)
        return 1
    rc_maxima, rc_maximum_tags = maximum_tagged_counters(
        rc_to_stable_tags,
        read_test_tag,
    )
    if not advancement_failures(rc_maxima, rc_maxima, rc_maximum_tags):
        print("self-test allowed stable publication to reuse RC counters", file=sys.stderr)
        return 1

    future_prerelease_tags = published_reference_tags_from_values(
        "v1.3.0",
        ["v1.2.3", "v2.0.0-rc.1"],
    )
    if "v2.0.0-rc.1" not in future_prerelease_tags:
        print(
            "self-test omitted a future prerelease before a lower stable publication",
            file=sys.stderr,
        )
        return 1
    future_maxima, future_maximum_tags = maximum_tagged_counters(
        future_prerelease_tags,
        read_test_tag,
    )
    if not advancement_failures(
        future_maxima,
        future_maxima,
        future_maximum_tags,
    ):
        print(
            "self-test allowed a lower stable release to reuse future-prerelease counters",
            file=sys.stderr,
        )
        return 1

    maxima, maximum_tags = maximum_tagged_counters(
        list(tagged),
        read_test_tag,
    )
    expected_maxima = {
        "agent": 20,
        "cli": 40,
        "frontend": 60,
        "server": 80,
    }
    if maxima != expected_maxima:
        print(
            f"self-test did not compute per-component historical maxima: {maxima}",
            file=sys.stderr,
        )
        return 1

    advancing = {component: value + 1 for component, value in maxima.items()}
    if advancement_failures(advancing, maxima, maximum_tags):
        print("self-test rejected globally advancing counters", file=sys.stderr)
        return 1

    reused = dict(advancing)
    reused["agent"] = maxima["agent"]
    failures = advancement_failures(reused, maxima, maximum_tags)
    if len(failures) != 1 or "v1.2.4-rc.1" not in failures[0]:
        print("self-test did not reject a reused prerelease counter", file=sys.stderr)
        return 1

    if valid_semver_tags(["legacy", "v1.2.3"], "test", warn=False) != ["v1.2.3"]:
        print("self-test did not ignore a non-semver release tag", file=sys.stderr)
        return 1

    def unreadable_test_tag(tag: str) -> dict[str, int]:
        raise ValueError(f"cannot read counters from {tag}")

    try:
        maximum_tagged_counters(["v1.2.3"], unreadable_test_tag)
    except ValueError:
        pass
    else:
        print("self-test ignored an unreadable valid release tag", file=sys.stderr)
        return 1

    print("build-number gate self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-tag", help="release tag being published")
    parser.add_argument(
        "--published-release-tags-file",
        type=Path,
        help="newline-delimited published GitHub release tags",
    )
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()

    if args.self_test:
        return run_self_test()
    if bool(args.candidate_tag) != bool(args.published_release_tags_file):
        parser.error(
            "--candidate-tag and --published-release-tags-file must be provided together"
        )

    try:
        current = current_counters()
        reference_tags: list[str]
        if args.published_release_tags_file:
            reference_tags = published_reference_tags(
                args.candidate_tag,
                args.published_release_tags_file,
            )
        else:
            reference_tags = local_reference_tags()
        if not reference_tags:
            print(
                "Component build counters are valid; repository has no SemVer release tags."
            )
            return 0

        historical_maximum, maximum_tags = maximum_tagged_counters(reference_tags)
        failures = advancement_failures(
            current,
            historical_maximum,
            maximum_tags,
        )
        if failures:
            for failure in failures:
                print(f"build-number gate: {failure}", file=sys.stderr)
            print(
                "Every release republishes all four components; advance each "
                "component-scoped counter before tagging.",
                file=sys.stderr,
            )
            return 1
        print(
            "Component build counters advance beyond all "
            f"{len(reference_tags)} release tag(s): "
            + ", ".join(
                f"{component}={current[component]}" for component in COMPONENTS
            )
        )
        return 0
    except (TypeError, ValueError) as exc:
        print(f"build-number gate: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
