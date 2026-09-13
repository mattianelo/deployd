"""Validate global shader merges against disposable game-owned cache copies."""

from pathlib import Path
import tempfile

from build_support import directory, verify
from community_patch_test import copy, identity


def run(root, semantic_tests):
    supplied = root / "modTesting/Mass Effect Legendary Edition (Game)/Game"
    with tempfile.TemporaryDirectory(dir=directory(Path("/build/mele")), prefix="shaders-") as temporary:
        stage = Path(temporary)
        originals = {}
        for index in (1, 2, 3):
            source = supplied / f"ME{index}/BioGame/CookedPCConsole/GlobalShaderCache-PC-D3D-SM5.bin"
            originals[source] = identity(source.parent, source.name)
            copy(source, stage / f"LE{index}.bin")
        semantic_tests(root, corpus=stage, corpus_kind="shaders")
        for source, expected in originals.items():
            verify(source, expected)
