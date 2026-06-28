#!/usr/bin/env python3
"""Package a Deployd AppImage in the ZIP format accepted by Nexus Mods."""

from __future__ import annotations

import argparse
import os
import sys
import tempfile
import zipfile
from pathlib import Path


class PackagingError(RuntimeError):
    """Raised when the Nexus release archive cannot be created."""


def create_archive(appimage: Path, output: Path) -> None:
    if not appimage.is_file():
        raise PackagingError(f"AppImage does not exist: {appimage}")
    if output.suffix.lower() != ".zip":
        raise PackagingError(f"Nexus release archive must end in .zip: {output}")

    temporary_path: Path | None = None
    try:
        output.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(
            prefix=f".{output.name}.",
            suffix=".tmp",
            dir=output.parent,
            delete=False,
        ) as temporary:
            temporary_path = Path(temporary.name)

        with zipfile.ZipFile(
            temporary_path,
            mode="w",
            compression=zipfile.ZIP_DEFLATED,
            compresslevel=9,
        ) as archive:
            archive.write(appimage, arcname=appimage.name)

        os.replace(temporary_path, output)
        temporary_path = None
    except (OSError, RuntimeError, zipfile.BadZipFile) as error:
        raise PackagingError(f"Failed to create Nexus release archive: {error}") from error
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)


def parse_args(arguments: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--appimage", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    return parser.parse_args(arguments)


def main(arguments: list[str]) -> int:
    args = parse_args(arguments)
    try:
        create_archive(args.appimage, args.output)
    except PackagingError as error:
        print(f"Nexus packaging failed: {error}", file=sys.stderr)
        return 1

    print(f"Created Nexus release archive: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
