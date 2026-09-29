#!/usr/bin/env python3
"""Copy only Deployd build inputs into a disposable packaging source tree."""

import argparse
import os
from pathlib import Path
import shutil
import stat
import tempfile


INPUTS = (
    "Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain.toml", "LICENSE",
    "src", "data", "licenses", "helpers/mele", "scripts/mele-helper.py",
    "scripts/package_source.py", "snap/snapcraft.yaml", "snap/snapcraft-dev.yaml",
)
MARKER = ".deployd-package-source"
IDENTITY = "deployd-package-source-v1\n"


def checked_path(value):
    path = Path(value).absolute()
    if path == Path("/") or ".." in path.parts:
        raise ValueError("Packaging requires a specific source and destination")
    if any(parent.is_symlink() for parent in (path, *path.parents)):
        raise ValueError("Packaging paths must not contain symbolic links")
    return path


def inventory(root):
    def visit(path):
        mode = path.lstat().st_mode
        if stat.S_ISREG(mode):
            yield path.relative_to(root)
        elif stat.S_ISDIR(mode):
            for child in sorted(path.iterdir()):
                if child.name != "__pycache__" and child.suffix not in {".pyc", ".pyo"}:
                    yield from visit(child)
        else:
            raise ValueError("Packaging inputs must be regular files or directories")

    for relative in INPUTS:
        path = root / relative
        for parent in path.parents:
            if parent == root:
                break
            if parent.is_symlink():
                raise ValueError("Packaging input ancestors must not be links")
        yield from visit(path)


def copy_source(source, destination):
    source, destination = checked_path(source), checked_path(destination)
    if os.getuid() == 0 and (destination == Path("/workspace") or Path("/workspace") in destination.parents):
        raise ValueError("Container root cannot write through the shared workspace")
    if source == destination or destination in source.parents:
        raise ValueError("Packaging destination overlaps its source")
    for relative in INPUTS:
        path = source / relative
        if destination == path or path in destination.parents:
            raise ValueError("Packaging destination is inside a build input")
    files = list(inventory(source))
    if destination.exists():
        if not destination.is_dir():
            raise ValueError("Packaging destination is not a directory")
        if any(destination.iterdir()):
            marker = destination / MARKER
            if marker.is_symlink() or not marker.is_file() or marker.read_text() != IDENTITY:
                raise ValueError("Packaging destination is not an owned source copy; clean this part first")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=destination.parent, prefix="deployd-source-") as temporary:
        stage = Path(temporary) / "new"
        stage.mkdir()
        for relative in files:
            target = stage / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source / relative, target, follow_symlinks=False)
            if not stat.S_ISREG(target.lstat().st_mode):
                raise ValueError("A packaging input changed while copying")
        (stage / MARKER).write_text(IDENTITY)
        previous = Path(temporary) / "previous"
        if destination.exists():
            destination.rename(previous)
        try:
            stage.rename(destination)
        except OSError:
            if previous.exists():
                previous.rename(destination)
            raise
    return len(files)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    args = parser.parse_args()
    try:
        count = copy_source(args.source, args.destination)
    except (OSError, ValueError) as error:
        parser.exit(1, f"Packaging source preparation failed: {error}\n")
    print(f"Prepared {count} build source files")


if __name__ == "__main__":
    main()
