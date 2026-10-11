"""Adapt the verified upstream texture implementation without changing compression."""

import json
from pathlib import Path


def project_image_source(data):
    source = data.decode("utf-8-sig").replace("\r\n", "\n")
    replacements = json.loads(Path(__file__).with_name("image_source_replacements.json").read_text())
    for before, after in replacements:
        if source.count(before) != 1:
            raise ValueError("Pinned texture source no longer matches the SkiaSharp migration")
        source = source.replace(before, after, 1)
    if "SixLabors" in source or "BCnEncoder.ImageSharp" in source:
        raise ValueError("Texture source still references ImageSharp")
    return source.encode("utf-8")
