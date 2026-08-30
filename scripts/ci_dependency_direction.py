#!/usr/bin/env python3
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
PROJECT_IMPORT = re.compile(r"\bcrate::(app|core|models|ui|utils)\b")
FORBIDDEN = {
    "models": {"app", "core", "models", "ui", "utils"},
    "utils": {"app", "core", "ui"},
    "core": {"app", "ui"},
    "ui": {"app"},
    "app": set(),
}


class DependencyDirectionError(RuntimeError):
    __slots__ = ()


def violations(root: Path) -> list[str]:
    source_root = root / "src"
    if not source_root.is_dir():
        raise DependencyDirectionError("missing Rust source directory: src")

    found = []
    for layer, forbidden in FORBIDDEN.items():
        layer_root = source_root / layer
        if not layer_root.exists():
            continue
        paths = [layer_root] if layer_root.is_file() else sorted(layer_root.rglob("*.rs"))
        for path in paths:
            if path.is_dir() or path.suffix != ".rs":
                continue
            for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                for dependency in PROJECT_IMPORT.findall(line):
                    if dependency in forbidden:
                        found.append(
                            f"{path.relative_to(root)}:{line_number}: "
                            f"{layer} must not depend on {dependency}"
                        )
    return found


def validate(root: Path) -> None:
    found = violations(root)
    if found:
        raise DependencyDirectionError(
            "Rust dependency direction violations:\n  " + "\n  ".join(found)
        )


def main() -> int:
    try:
        validate(ROOT)
    except (OSError, UnicodeError, DependencyDirectionError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print("dependency-direction policy passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
