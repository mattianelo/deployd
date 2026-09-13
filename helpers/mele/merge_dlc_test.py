"""Validate generated merge content using disposable game-owned inputs."""

from pathlib import Path
import struct
import tempfile

from build_support import directory, verify
from community_patch_test import copy, identity


def effective(supplied, name):
    candidates = []
    base = supplied / "BioGame/CookedPCConsole" / name
    if base.exists():
        candidates.append((0, base))
    for path in (supplied / "BioGame/DLC").glob(f"*/CookedPCConsole/{name}"):
        data = (path.parent / "Mount.dlc").read_bytes()
        if supplied.name == "ME3":
            if len(data) < 108 or struct.unpack_from("<IIII", data) != (1, 685, 205, 196715):
                raise ValueError("Invalid supplied LE3 mount")
            offset = 16
        else:
            if len(data) < 44 or struct.unpack_from("<III", data) != (684, 168, 65643):
                raise ValueError("Invalid supplied LE2 mount")
            offset = 12
        candidates.append((struct.unpack_from("<i", data, offset)[0], path))
    return max(candidates)[1]


def run(root, semantic_tests):
    supplied = root / "modTesting/Mass Effect Legendary Edition (Game)/Game/ME2"
    source = effective(supplied, "BioH_SelectGUI.pcc")
    codec = supplied / "Binaries/Win64/oo2core_8_win64.dll"
    originals = {path: identity(path.parent, path.name) for path in (source, codec)}
    with tempfile.TemporaryDirectory(dir=directory(Path("/build/mele")), prefix="merge-dlc-") as temporary:
        stage = Path(temporary)
        copy(source, stage / "input/BioH_SelectGUI.pcc")
        copy(codec, stage / "game/Binaries/Win64/oo2core_8_win64.dll")
        messages = effective(supplied, "BioD_Nor_103Messages.pcc")
        originals[messages] = identity(messages.parent, messages.name)
        copy(messages, stage / "input/BioD_Nor_103Messages.pcc")
        for name in ("Core.pcc", "Engine.pcc", "GFxUI.pcc", "WwiseAudio.pcc", "SFXOnlineFoundation.pcc", "PlotManagerMap.pcc", "SFXGame.pcc", "Startup_INT.pcc"):
            source_base = effective(supplied, name)
            originals[source_base] = identity(source_base.parent, source_base.name)
            copy(source_base, stage / "bases" / name)
        semantic_tests(root, corpus=stage, corpus_kind="merge-dlc")
        data = (stage / "ui.swf").read_bytes()
        analysis = directory(root / ".ci-artifacts/mele-analysis")
        (analysis / "ui.swf").write_bytes(data)
        for path, expected in originals.items():
            verify(path, expected)
