"""Decode already hash-verified test corpus containers, outside production installation."""

import json
import lzma
from pathlib import Path
import struct


class Reader:
    def __init__(self, data):
        self.data = data
        self.offset = 0

    def take(self, size):
        if size < 0 or size > len(self.data) - self.offset:
            raise ValueError("Truncated frozen M3M input")
        result = self.data[self.offset:self.offset + size]
        self.offset += size
        return result

    def integer(self):
        return struct.unpack("<i", self.take(4))[0]

    def string(self):
        size = self.integer()
        text = self.take(-size * 2 if size < 0 else size).decode("utf-16-le" if size < 0 else "ascii")
        if not text.endswith("\0") or "\0" in text[:-1]:
            raise ValueError("Invalid frozen M3M string")
        return text[:-1]

    def compressed(self, size, stored):
        if not 0 < size <= 128 * 1024 * 1024 or stored < 5:
            raise ValueError("Invalid frozen M3M compression lengths")
        data = self.take(stored)
        decoder = lzma.LZMADecompressor(lzma.FORMAT_ALONE, memlimit=32 * 1024 * 1024)
        output = decoder.decompress(data[:5] + b"\xff" * 8 + data[5:], max_length=size + 1)
        if len(output) != size or not decoder.eof or decoder.unused_data:
            raise ValueError("Frozen M3M compression length or end marker mismatch")
        return output

    def finish(self):
        if self.offset != len(self.data):
            raise ValueError("Trailing frozen M3M bytes")


def decode(path):
    source = Reader(path.read_bytes())
    if source.take(4) != b"M3MM":
        raise ValueError("Invalid frozen M3M magic")
    version = source.take(1)[0]
    if version == 1:
        manifest = json.loads(source.string())
    elif version == 2:
        expanded = Reader(source.compressed(source.integer(), source.integer()))
        manifest = json.loads(expanded.string())
        expanded.finish()
    else:
        raise ValueError("Unknown frozen M3M version")
    count = source.integer()
    if not 0 <= count <= 1024:
        raise ValueError("Invalid frozen M3M asset count")
    assets = {}
    for _ in range(count):
        if source.take(4) != b"MMV1":
            raise ValueError("Invalid frozen asset magic")
        name = source.string()
        if Path(name).name != name or "\\" in name or ":" in name or name in {"", ".", ".."} or name.casefold() in assets:
            raise ValueError("Invalid frozen asset name")
        size = source.integer()
        compressed = source.take(1)[0] if version == 2 else 0
        if compressed not in (0, 1):
            raise ValueError("Invalid frozen compression flag")
        assets[name.casefold()] = source.compressed(size, source.integer()) if compressed else source.take(size)
    source.finish()
    if manifest.get("game") != "LE1":
        raise ValueError("Unexpected frozen game")
    return manifest, assets
