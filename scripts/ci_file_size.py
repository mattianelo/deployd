#!/usr/bin/env python3
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
MAX_PRODUCTION_RUST_LINES = 1_000
MAX_TOTAL_RUST_LINES = 1_500


class FileSizeError(RuntimeError):
    __slots__ = ()


def _production_line_count(path: Path, lines: list[str]) -> int:
    markers = [
        index for index, line in enumerate(lines) if line.rstrip("\n") == "#[cfg(test)]"
    ]
    if not markers:
        return len(lines)
    if len(markers) != 1:
        raise FileSizeError(
            f"{path}: expected at most one top-level #[cfg(test)] test module"
        )

    marker = markers[0]
    following = marker + 1
    while following < len(lines) and not lines[following].strip():
        following += 1
    if following >= len(lines) or lines[following].strip() != "mod tests {":
        raise FileSizeError(
            f"{path}: top-level #[cfg(test)] must introduce a trailing `mod tests {{`"
        )
    if not any(line.strip() for line in lines[following + 1 :]):
        raise FileSizeError(f"{path}: trailing test module is empty")
    if next(
        (line.strip() for line in reversed(lines) if line.strip()),
        None,
    ) != "}":
        raise FileSizeError(f"{path}: trailing test module must end the file")
    return marker


def rust_line_counts(root: Path) -> list[tuple[Path, int, int]]:
    source_root = root / "src"
    if not source_root.is_dir():
        raise FileSizeError("missing Rust source directory: src")

    counts = []
    for path in sorted(source_root.rglob("*.rs")):
        with path.open(encoding="utf-8") as stream:
            lines = stream.readlines()
        relative = path.relative_to(root)
        counts.append((relative, _production_line_count(relative, lines), len(lines)))
    return counts


def validate_file_sizes(
    root: Path,
    production_maximum: int = MAX_PRODUCTION_RUST_LINES,
    total_maximum: int = MAX_TOTAL_RUST_LINES,
) -> list[tuple[Path, int, int]]:
    counts = rust_line_counts(root)
    oversized = [
        (path, production, total)
        for path, production, total in counts
        if production > production_maximum or total > total_maximum
    ]
    if oversized:
        details = "\n".join(
            f"  {path}: {production} production lines (limit {production_maximum}), "
            f"{total} total lines (limit {total_maximum})"
            for path, production, total in oversized
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
        print(
            "file-size policy passed: no Rust files, "
            f"production limit {MAX_PRODUCTION_RUST_LINES}, "
            f"total limit {MAX_TOTAL_RUST_LINES}"
        )
    else:
        print(
            "file-size policy passed: "
            f"{len(counts)} Rust files, largest production file {largest[0]} at "
            f"{largest[1]} production/{largest[2]} total lines, production limit "
            f"{MAX_PRODUCTION_RUST_LINES}, total limit {MAX_TOTAL_RUST_LINES}"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
