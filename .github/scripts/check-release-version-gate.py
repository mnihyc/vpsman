#!/usr/bin/env python3
"""Reject release tags older than the newest published GitHub release."""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass


TAG_RE = re.compile(r"^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z][0-9A-Za-z.-]*))?$")


@dataclass(frozen=True)
class Version:
    major: int
    minor: int
    patch: int
    prerelease: tuple[tuple[int, int | str], ...] | None


def parse_tag(tag: str) -> Version:
    match = TAG_RE.match(tag.strip())
    if not match:
        raise ValueError(f"{tag!r} is not a supported release tag")

    prerelease_text = match.group(4)
    prerelease = None
    if prerelease_text is not None:
        identifiers: list[tuple[int, int | str]] = []
        for part in prerelease_text.split("."):
            if not part:
                raise ValueError(f"{tag!r} has an empty prerelease identifier")
            if part.isdigit():
                identifiers.append((0, int(part)))
            else:
                identifiers.append((1, part))
        prerelease = tuple(identifiers)

    return Version(
        major=int(match.group(1)),
        minor=int(match.group(2)),
        patch=int(match.group(3)),
        prerelease=prerelease,
    )


def compare_versions(left: Version, right: Version) -> int:
    left_core = (left.major, left.minor, left.patch)
    right_core = (right.major, right.minor, right.patch)
    if left_core != right_core:
        return 1 if left_core > right_core else -1

    if left.prerelease == right.prerelease:
        return 0
    if left.prerelease is None:
        return 1
    if right.prerelease is None:
        return -1

    for left_part, right_part in zip(left.prerelease, right.prerelease):
        if left_part == right_part:
            continue
        left_kind, left_value = left_part
        right_kind, right_value = right_part
        if left_kind != right_kind:
            return -1 if left_kind < right_kind else 1
        return 1 if left_value > right_value else -1

    if len(left.prerelease) == len(right.prerelease):
        return 0
    return 1 if len(left.prerelease) > len(right.prerelease) else -1


def check_candidate(candidate_tag: str, reference_tag: str | None) -> bool:
    comparison = compare_candidate(candidate_tag, reference_tag)
    if comparison is None:
        print(f"No existing semver release tag; allowing {candidate_tag}.")
        return True

    if comparison < 0:
        print(
            f"Release tag {candidate_tag} is older than newest published release "
            f"{reference_tag}; refusing to publish an old version as latest.",
            file=sys.stderr,
        )
        return False
    if comparison == 0:
        print(
            f"Release tag {candidate_tag} matches newest published release "
            f"{reference_tag}; allowing rebuild or retag."
        )
        return True

    print(f"Release tag {candidate_tag} is newer than newest published release {reference_tag}.")
    return True


def compare_candidate(candidate_tag: str, reference_tag: str | None) -> int | None:
    candidate = parse_tag(candidate_tag)
    if not reference_tag:
        return None
    reference = parse_tag(reference_tag)
    return compare_versions(candidate, reference)


def newest_semver_tag(tags: list[str], *, warn: bool = True) -> str:
    newest_tag = ""
    newest_version: Version | None = None
    for raw_tag in tags:
        tag = raw_tag.strip()
        if not tag:
            continue
        try:
            version = parse_tag(tag)
        except ValueError:
            if warn:
                print(f"Ignoring non-semver published release tag: {tag}", file=sys.stderr)
            continue
        if newest_version is None or compare_versions(version, newest_version) > 0:
            newest_tag = tag
            newest_version = version
    return newest_tag


def run_self_test() -> int:
    cases = [
        ("v1.2.3", "", True),
        ("v1.2.3", "v1.2.3", True),
        ("v1.2.4", "v1.2.3", True),
        ("v1.2.3", "v1.2.4", False),
        ("v1.2.3-rc.1", "v1.2.3", False),
        ("v1.2.4-rc.1", "v1.2.3", True),
        ("v1.2.3-rc.2", "v1.2.3-rc.1", True),
        ("v1.2.3-rc.1", "v1.2.3-rc.2", False),
        ("v1.2.3-alpha.2", "v1.2.3-alpha.10", False),
        ("v1.2.3-alpha.beta", "v1.2.3-alpha.10", True),
    ]

    failures = 0
    for candidate_tag, latest_tag, expected in cases:
        comparison = compare_candidate(candidate_tag, latest_tag)
        actual = comparison is None or comparison >= 0
        if actual != expected:
            failures += 1
            print(
                f"self-test failed: candidate={candidate_tag} latest={latest_tag} "
                f"expected={expected} actual={actual}",
                file=sys.stderr,
            )

    if failures:
        return 1

    newest = newest_semver_tag(["legacy", "v1.2.3", "v1.2.4-rc.1", "v1.2.3"], warn=False)
    if newest != "v1.2.4-rc.1":
        print(f"self-test failed: expected newest v1.2.4-rc.1, got {newest}", file=sys.stderr)
        return 1

    print("release version gate self-test passed")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-tag", help="release tag being published")
    parser.add_argument("--latest-tag", default="", help="current latest GitHub release tag")
    parser.add_argument(
        "--published-release-tag",
        action="append",
        default=[],
        help="published release tag to include when finding the newest known release",
    )
    parser.add_argument(
        "--published-release-tags-file",
        help="newline-delimited published release tags to include when finding the newest known release",
    )
    parser.add_argument("--self-test", action="store_true", help="run built-in comparator tests")
    args = parser.parse_args()

    if args.self_test:
        return run_self_test()
    if not args.candidate_tag:
        parser.error("--candidate-tag is required unless --self-test is used")

    try:
        published_tags = list(args.published_release_tag)
        if args.published_release_tags_file:
            with open(args.published_release_tags_file, encoding="utf-8") as release_tags:
                published_tags.extend(release_tags)
        reference_tag = newest_semver_tag(published_tags) if published_tags else args.latest_tag.strip()
        return 0 if check_candidate(args.candidate_tag, reference_tag) else 1
    except ValueError as exc:
        print(exc, file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
