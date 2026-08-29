#!/usr/bin/env python3
"""Validate Deployd release metadata and classify stable release tags."""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
import xml.etree.ElementTree as ET
from dataclasses import dataclass
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
STABLE_TAG = re.compile(r"^v([0-9]+\.[0-9]+\.[0-9]+)$")


class MetadataError(RuntimeError):
    """Raised when release metadata is missing, invalid, or inconsistent."""


@dataclass(frozen=True)
class ReleaseMetadata:
    version: str
    release: bool


def stable_tag_version(tag: str) -> str | None:
    match = STABLE_TAG.fullmatch(tag)
    return None if match is None else match.group(1)


def _required_match(source: str, pattern: str, description: str) -> str:
    match = re.search(pattern, source, re.MULTILINE)
    if match is None:
        raise MetadataError(f"missing {description}")
    return match.group(1)


def _cargo_version(root: Path) -> str:
    with (root / "Cargo.toml").open("rb") as stream:
        package = tomllib.load(stream).get("package")
    if not isinstance(package, dict) or not isinstance(package.get("version"), str):
        raise MetadataError("Cargo.toml is missing package.version")
    return package["version"]


def _lockfile_version(root: Path) -> str:
    with (root / "Cargo.lock").open("rb") as stream:
        packages = tomllib.load(stream).get("package", [])
    versions = [
        package.get("version")
        for package in packages
        if isinstance(package, dict) and package.get("name") == "deployd"
    ]
    if len(versions) != 1 or not isinstance(versions[0], str):
        raise MetadataError("Cargo.lock must contain exactly one deployd package")
    return versions[0]


def _snap_version(root: Path) -> str:
    source = (root / "snap" / "snapcraft.yaml").read_text()
    return _required_match(
        source,
        r"^version:\s*['\"]?([^'\"\s]+)['\"]?\s*$",
        "top-level Snap version",
    )


def _appstream_version(root: Path) -> str:
    path = root / "data" / "io.mattianelo.deployd.metainfo.xml"
    try:
        document = ET.parse(path)
    except ET.ParseError as error:
        raise MetadataError(f"invalid AppStream XML: {error}") from error
    release = document.find("./releases/release")
    if release is None or not release.get("version"):
        raise MetadataError("AppStream metadata has no newest release version")
    return release.get("version", "")


def _readme_versions(root: Path) -> tuple[str, str]:
    source = (root / "README.md").read_text()
    badge = _required_match(
        source,
        r"^!\[Version\]\([^\n]*version-([0-9]+\.[0-9]+\.[0-9]+)-blue\)$",
        "README version badge",
    )
    announcement = _required_match(
        source,
        r"^> \*\*v([0-9]+\.[0-9]+\.[0-9]+)\*\*",
        "README release announcement",
    )
    return badge, announcement


def _changelog_has_version(root: Path, version: str) -> bool:
    source = (root / "CHANGELOG.md").read_text()
    return re.search(rf"^## \[{re.escape(version)}\]\s*$", source, re.MULTILINE) is not None


def validate_metadata(root: Path = ROOT, tag: str | None = None) -> ReleaseMetadata:
    release_version = stable_tag_version(tag) if tag is not None else None
    if tag is not None and release_version is None:
        return ReleaseMetadata(version="", release=False)

    cargo_version = _cargo_version(root)
    expected = release_version or cargo_version
    versions = {
        "Cargo.toml": cargo_version,
        "Cargo.lock": _lockfile_version(root),
        "snap/snapcraft.yaml": _snap_version(root),
        "AppStream newest release": _appstream_version(root),
    }
    readme_badge, readme_announcement = _readme_versions(root)
    versions["README version badge"] = readme_badge
    versions["README release announcement"] = readme_announcement

    mismatches = [
        f"{source} has {version}, expected {expected}"
        for source, version in versions.items()
        if version != expected
    ]
    if not _changelog_has_version(root, expected):
        mismatches.append(f"CHANGELOG.md has no [{expected}] release section")
    if mismatches:
        raise MetadataError("release metadata mismatch:\n- " + "\n- ".join(mismatches))

    return ReleaseMetadata(version=expected, release=release_version is not None)


def _write_github_output(path: Path, metadata: ReleaseMetadata) -> None:
    with path.open("a", encoding="utf-8") as stream:
        stream.write(f"release={'true' if metadata.release else 'false'}\n")
        stream.write(f"version={metadata.version}\n")


def parse_args(arguments: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT)
    parser.add_argument("--tag")
    parser.add_argument("--github-output", type=Path)
    return parser.parse_args(arguments)


def main(arguments: list[str]) -> int:
    args = parse_args(arguments)
    try:
        metadata = validate_metadata(args.root, args.tag)
        if args.github_output is not None:
            _write_github_output(args.github_output, metadata)
    except (MetadataError, OSError, tomllib.TOMLDecodeError) as error:
        print(f"release metadata validation failed: {error}", file=sys.stderr)
        return 1

    if metadata.release:
        print(f"stable release metadata is coherent for v{metadata.version}")
    elif args.tag is not None:
        print(f"tag is not a stable release: {args.tag}")
    else:
        print(f"release metadata is coherent for {metadata.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
