"""Decode only the hash-verified embedded-TLK acceptance corpus."""

import lzma
import struct
import xml.etree.ElementTree as ET


def decode(path):
    data = path.read_bytes()
    if data[:5] != b"CTMD\x02":
        raise ValueError("Unexpected frozen TLK archive version")
    size, stored = struct.unpack_from("<ii", data, 5)

    def decompress(raw, size, padding=0):
        if not 0 < size <= 4 * 1024 * 1024 or len(raw) < 10:
            raise ValueError("Invalid frozen TLK block size")
        decoder = lzma.LZMADecompressor(lzma.FORMAT_ALONE, memlimit=32 * 1024 * 1024)
        value = decoder.decompress(raw[:5] + b"\xff" * 8 + raw[5:], max_length=size + padding + 1)
        if not size <= len(value) <= size + padding or any(value[size:]) or not decoder.eof or decoder.unused_data:
            raise ValueError("Invalid frozen TLK compressed data")
        return value[:size]

    header = decompress(data[13:13 + stored], size, max(size, 256))
    count = struct.unpack_from("<i", header)[0]
    if not 0 < count <= 4096:
        raise ValueError("Invalid frozen TLK entry count")
    entries, offset = [], 4
    for _ in range(count):
        start = offset
        while header[offset:offset + 2] != b"\0\0":
            offset += 2
            if offset >= len(header):
                raise ValueError("Unterminated frozen TLK name")
        name = header[start:offset].decode("utf-16-le")
        offset += 2
        start, size, length, key = struct.unpack_from("<iiiB", header, offset)
        offset += 13
        if key != 255:
            raise ValueError("Unexpected frozen TLK option key")
        entries.append((name, start, size, length))
    if header[offset:] != b"\0":
        raise ValueError("Unexpected frozen TLK option table")
    block_size = struct.unpack_from("<i", data, 13 + stored)[0]
    block = data[17 + stored:]
    if block_size != len(block):
        raise ValueError("Invalid frozen TLK data block")
    changes, end = [], 0
    for name, start, size, length in entries:
        if start != end or length <= 0:
            raise ValueError("Unexpected frozen TLK block range")
        end += length
        xml = ET.fromstring(decompress(block[start:end], size))
        package, export = name[:-4].split('.', 1)
        if not name.endswith('.xml') or any(not part or not all(ch.isascii() and (ch.isalnum() or ch == '_') for ch in part)
                                             for part in (package, *export.split('.'))):
            raise ValueError("Unexpected frozen TLK target")
        changes.append({"target": f"CookedPCConsole/{package}.pcc", "export": export,
                        "strings": [{"id": int(node.findtext('id')), "data": node.findtext('data', '')} for node in xml.findall('string')]})
    if end != len(block):
        raise ValueError("Unaccounted frozen TLK data")
    return changes
