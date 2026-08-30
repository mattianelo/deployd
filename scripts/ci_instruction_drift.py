#!/usr/bin/env python3
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent

REQUIRED_FILES = (
    ".gitlab-ci.yml",
    "README.md",
    "check.sh",
    "fossil.toml",
    "rust-toolchain.toml",
    "scripts/ci_file_size.py",
    "scripts/ci_dependency_direction.py",
    "scripts/ci_policy.py",
    "scripts/rust-command.sh",
    "scripts/test_ci_file_size.py",
    "scripts/test_ci_dependency_direction.py",
)
INSTRUCTION_FILES = ("README.md",)
STALE_PATHS = (
    "src/paths.rs",
    "src/snap.rs",
    "CLAUDE.md",
    "rustfmt.toml",
    ".clippy.toml",
)
RAW_CARGO = re.compile(
    r"(?<![A-Za-z0-9_-])cargo\s+"
    r"(?:audit|build|check|clippy|doc|fmt|nextest|run|test|update)\b"
)
REPOSITORY_PATH = re.compile(
    r"(?<![A-Za-z0-9_./-])"
    r"((?:packaging|scripts|snap|src)/[A-Za-z0-9_./*-]+)"
)
CI_COMMAND_PATH = re.compile(
    r"^\s*-\s+(?:(?:python3|bash)\s+|\./)"
    r"((?:packaging|scripts)/[A-Za-z0-9_./-]+\.(?:py|sh))\b",
    re.MULTILINE,
)


def instruction_issues(root: Path) -> list[str]:
    issues = [
        f"missing required file: {relative}"
        for relative in REQUIRED_FILES
        if not (root / relative).is_file()
    ]

    for relative in INSTRUCTION_FILES:
        path = root / relative
        if not path.is_file():
            continue
        source = path.read_text()
        for stale in STALE_PATHS:
            if stale in source:
                issues.append(f"{relative}: stale repository path: {stale}")
        for match in RAW_CARGO.finditer(source):
            line = source.count("\n", 0, match.start()) + 1
            issues.append(
                f"{relative}:{line}: raw Cargo suggestion; use ./check.sh"
            )
        for match in REPOSITORY_PATH.finditer(source):
            referenced = match.group(1).rstrip(".,:;)")
            if "*" not in referenced and not (root / referenced).exists():
                issues.append(
                    f"{relative}: referenced repository path is missing: {referenced}"
                )

    pipeline = root / ".gitlab-ci.yml"
    if pipeline.is_file():
        for match in CI_COMMAND_PATH.finditer(pipeline.read_text()):
            referenced = match.group(1)
            if not (root / referenced).is_file():
                issues.append(f".gitlab-ci.yml: command path is missing: {referenced}")

    return issues


def main() -> int:
    issues = instruction_issues(ROOT)
    if issues:
        for issue in issues:
            print(f"error: {issue}", file=sys.stderr)
        return 1
    print("instruction drift check passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
