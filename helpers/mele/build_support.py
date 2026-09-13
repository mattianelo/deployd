"""Integrity and filesystem boundaries shared by MELE build tooling."""

import hashlib
from pathlib import Path
import tempfile
import urllib.request


def directory(path):
    path = Path(path)
    if not path.is_absolute() or path == Path("/"):
        raise ValueError("Helper output requires a specific absolute directory")
    for ancestor in (path, *path.parents):
        if ancestor.is_symlink():
            raise ValueError("Refusing a symbolic link in a helper output directory")
        if ancestor.exists() and not ancestor.is_dir():
            raise ValueError("Helper output directory is occupied by a file")
    path.mkdir(parents=True, exist_ok=True)
    return path


def verify(path, spec, algorithm="sha256"):
    if path.is_symlink() or path.stat().st_size != spec["size"]:
        raise ValueError("Pinned helper input has an unexpected size or is a symbolic link")
    with path.open("rb") as stream:
        if hashlib.file_digest(stream, algorithm).hexdigest() != spec[algorithm]:
            raise ValueError("Pinned helper input failed checksum verification")


def download(destination, spec, algorithm="sha256"):
    directory(destination.parent)
    if destination.exists() or destination.is_symlink():
        verify(destination, spec, algorithm)
        return
    with tempfile.NamedTemporaryFile(dir=destination.parent, delete=False) as output:
        temporary = Path(output.name)
        try:
            with urllib.request.urlopen(spec["url"], timeout=60) as response:
                remaining = spec["size"] + 1
                while remaining:
                    chunk = response.read(min(1024 * 1024, remaining))
                    if not chunk:
                        break
                    output.write(chunk)
                    remaining -= len(chunk)
            output.flush()
            verify(temporary, spec, algorithm)
            temporary.replace(destination)
        finally:
            temporary.unlink(missing_ok=True)
