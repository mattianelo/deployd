"""Validate frozen asset-only Community Patch merges against disposable game copies."""

import json
from pathlib import Path
import struct
import subprocess
import tempfile

from build_support import directory, verify
from community_patch_test import GAME, MOD, INPUTS, copy, identity, regular_file, verify_corpus


ASSET_INPUTS = {
    "codec": INPUTS["codec"],
    "SFXGame.pcc": (f"{GAME}/BioGame/CookedPCConsole/SFXGame.pcc", 6018634,
                    "79a1a3cc7afe2714d3d297dcb737f26314e7a302ffb854b7534a433d262ae9a5"),
    "BIOC_Materials.pcc": (f"{GAME}/BioGame/CookedPCConsole/BIOC_Materials.pcc", 13468055,
                          "a07c30f8f3351a8e9d9c4161dd2894dd62f5db81298558ef749dc7d63cd9a2ae"),
    "EyeScannerFix.m3m": (f"{MOD}/MergeMods/EyeScannerFix.m3m", 89114,
                          "a4d02c5ec41ec3d38ac1f937d6d81ba52dd6f0828c94f0f427f0cd26e9e385b4"),
    "KarpovPistolFix.m3m": (f"{MOD}/MergeMods/KarpovPistolFix.m3m", 1693258,
                            "65500cea35a30521e2d56418b196464d95e356356ed653ee5a68fe2aa54eb9df"),
}


def frozen_assets(path, destination):
    data = path.read_bytes()
    if data[:5] != b"M3MM\x01":
        raise ValueError("Expected the frozen M3M v1 corpus")
    offset = 5

    def take(size):
        nonlocal offset
        if size < 0 or offset + size > len(data):
            raise ValueError("Truncated frozen M3M input")
        result = data[offset:offset + size]
        offset += size
        return result

    def integer():
        return struct.unpack("<i", take(4))[0]

    def string():
        size = integer()
        text = take(-size * 2 if size < 0 else size).decode("utf-16-le" if size < 0 else "ascii")
        if not text.endswith("\0") or "\0" in text[:-1]:
            raise ValueError("Invalid frozen M3M string")
        return text[:-1]

    manifest = json.loads(string())
    if manifest["game"] != "LE1":
        raise ValueError("Unexpected frozen M3M target")
    assets = []
    count = integer()
    if not 1 <= count <= 16:
        raise ValueError("Unexpected frozen asset count")
    for _ in range(count):
        if take(4) != b"MMV1":
            raise ValueError("Invalid frozen M3M asset")
        name = string()
        if Path(name).name != name or not name.endswith(".pcc") or "\\" in name:
            raise ValueError("Unsafe frozen asset name")
        content = take(integer())
        output = destination / "Assets" / path.stem / name
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_bytes(content)
        assets.append(identity(destination, output.relative_to(destination).as_posix()))
    if offset != len(data):
        raise ValueError("Trailing frozen M3M data")
    merges = []
    for file in manifest["files"]:
        if file["applytoalllocalizations"] or file["filename"] not in {"SFXGame.pcc", "BIOC_Materials.pcc"}:
            raise ValueError("Unexpected frozen asset target")
        for change in file["changes"]:
            if any(change.get(key) for key in ("scriptupdate", "classupdate", "addtoclassorreplace",
                                              "propertyupdates", "sequenceskipupdate", "disableconfigupdate", "newassetupdate")):
                raise ValueError("The corpus job is not asset-only")
            asset = change["assetupdate"]
            merges.append({"target": f"CookedPCConsole/{file['filename']}", "entry": change["entryname"],
                           "asset": f"Assets/{path.stem}/{asset['assetname']}", "source_entry": asset["entryname"],
                           "allow_new": asset.get("canmergeasnew", False)})
    return assets, merges


def transform(command, stage, request, name):
    path = stage / f"{name}.json"
    path.write_text(json.dumps(request))
    result = subprocess.run([*command, "transform-assets", str(path)], capture_output=True, text=True)
    if result.returncode != 0:
        raise ValueError(f"Community Patch asset merge failed: {result.stderr.strip()}")
    messages = [json.loads(line) for line in result.stdout.splitlines()]
    total = len(request["merges"])
    progress = [{"protocol": 1, "type": "progress", "completed": index, "total": total}
                for index in range(1, total + 1)]
    root = Path(request["output_root"])
    outputs = [identity(root, entry["path"]) for entry in request["targets"]]
    if messages != [*progress, {"protocol": 1, "type": "complete", "outputs": outputs}]:
        raise ValueError("Asset merge progress or output identities disagree")
    if sorted(path.relative_to(root).as_posix() for path in root.rglob("*") if path.is_file()) != sorted(entry["path"] for entry in outputs):
        raise ValueError("Asset merge produced unexpected outputs")
    print(json.dumps({"test": name, "outputs": outputs}), flush=True)


def run(root, semantic_tests):
    corpus = root / "docs/modTesting"
    sources = verify_corpus(corpus, ASSET_INPUTS)
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    build = directory(Path("/build/mele"))
    command = [str(build / f"sdk-{configuration['sdk']['version']}" / "dotnet"),
               str(build / "source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll")]
    try:
        with tempfile.TemporaryDirectory(dir=build, prefix="community-patch-assets-") as temporary:
            stage = Path(temporary)
            for name in ("game", "input", "merged", "reapplied", "verification", "failed"):
                (stage / name).mkdir()
            copy(sources["codec"], stage / "game/Binaries/Win64/oo2core_8_win64.dll")
            targets = []
            for name in ("SFXGame.pcc", "BIOC_Materials.pcc"):
                relative = f"CookedPCConsole/{name}"
                copy(sources[name], stage / "input" / relative)
                targets.append(identity(stage / "input", relative))
            assets, merges = [], []
            for name in ("EyeScannerFix.m3m", "KarpovPistolFix.m3m"):
                files, changes = frozen_assets(sources[name], stage / "input")
                assets.extend(files)
                merges.extend(changes)
            request = {"protocol": 1, "operation": "le1-m3m-assets", "game_root": str(stage / "game"),
                       "input_root": str(stage / "input"), "output_root": str(stage / "merged"),
                       "targets": targets, "assets": assets, "merges": merges}
            immutable = {path: identity(path.parent, path.name) for path in stage.rglob("*") if path.is_file()}
            transform(command, stage, request, "merged")
            semantic = dict(request, output_root=str(stage / "verification"))
            (stage / "semantic-request.json").write_text(json.dumps(semantic))
            for item in targets:
                copy(stage / "merged" / item["path"], stage / "previous" / item["path"])
            for item in assets:
                copy(stage / "input" / item["path"], stage / "previous" / item["path"])
            immutable.update({path: identity(path.parent, path.name)
                              for path in (stage / "previous").rglob("*") if path.is_file()})
            repeated = dict(request, input_root=str(stage / "previous"), output_root=str(stage / "reapplied"),
                            targets=[identity(stage / "previous", item["path"]) for item in targets])
            transform(command, stage, repeated, "reapplied")
            semantic_tests(root, corpus=stage, corpus_kind="m3m-assets")
            failed = dict(request, output_root=str(stage / "failed"),
                          merges=[*merges[:-1], dict(merges[-1], source_entry="MissingExport")])
            file = stage / "failed.json"
            file.write_text(json.dumps(failed))
            result = subprocess.run([*command, "transform-assets", str(file)], capture_output=True, text=True)
            if result.returncode == 0 or any((stage / "failed").iterdir()):
                raise ValueError("Failed asset merge published outputs")
            for path, spec in immutable.items():
                regular_file(path)
                verify(path, spec)
            print("Community Patch eye-scanner and Karpov asset merges, reapplication, failure, and input integrity passed.", flush=True)
    finally:
        verify_corpus(corpus, ASSET_INPUTS)
        print("Supplied game and mod inputs retain their pinned hashes.", flush=True)
