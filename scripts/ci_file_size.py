#!/usr/bin/env python3
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
MAX_RUST_LINES = 1_500


class FileSizeError(RuntimeError):
    __slots__ = ()


def rust_line_counts(root: Path) -> list[tuple[Path, int]]:
    source_root = root / "src"
    if not source_root.is_dir():
        raise FileSizeError("missing Rust source directory: src")

    counts = []
    for path in sorted(source_root.rglob("*.rs")):
        with path.open(encoding="utf-8") as stream:
            counts.append((path.relative_to(root), sum(1 for _ in stream)))
    return counts


def validate_file_sizes(root: Path, maximum: int = MAX_RUST_LINES) -> list[tuple[Path, int]]:
    counts = rust_line_counts(root)
    oversized = [(path, lines) for path, lines in counts if lines > maximum]
    if oversized:
        details = "\n".join(
            f"  {path}: {lines} lines (limit {maximum})" for path, lines in oversized
        )
        raise FileSizeError(f"Rust source files exceed the line limit:\n{details}")
    return counts


def main() -> int:
    try:
        counts = validate_file_sizes(ROOT)
    except (OSError, UnicodeError, FileSizeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    largest = max(counts, key=lambda item: item[1], default=None)
    if largest is None:
        print(f"file-size policy passed: no Rust files, limit {MAX_RUST_LINES}")
    else:
        print(
            "file-size policy passed: "
            f"{len(counts)} Rust files, largest {largest[0]} at {largest[1]} lines, "
            f"limit {MAX_RUST_LINES}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
