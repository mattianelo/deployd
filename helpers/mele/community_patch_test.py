"""Exercise frozen Community Patch transformations without deploying game files."""

import hashlib
import json
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile

from build_support import directory, verify


TARGET = "CookedPCConsole/Engine.pcc"
DLC = "DLC_MOD_LE1CP"
MANIFEST = f"DLC/{DLC}/CookedPCConsole/{DLC}-2DAMerge.m3da"
PACKAGE = f"DLC/{DLC}/CookedPCConsole/{DLC}_2DA.pcc"
GAME = "Mass Effect Legendary Edition (Game)/Game/ME1"
MOD = "LE1 Community Patch"
INPUTS = {
    "engine": (f"{GAME}/BioGame/{TARGET}", 9843689,
               "296308fba3fe6294f8ed4eba5796fcc54e75e51837ff9cc261e1151124f4ad13"),
    "codec": (f"{GAME}/Binaries/Win64/oo2core_8_win64.dll", 1007616,
              "d42940381611cda3b8555f6eb9fcb1bc3b1a3b96d7e24cb98738f4b71653d415"),
    "manifest": (f"{MOD}/{MANIFEST.removeprefix('DLC/')}", 395,
                 "aafd92789ea505833fa5d407058636c09afb9b75044776bb1172ee3e20edc7f1"),
    "package": (f"{MOD}/{PACKAGE.removeprefix('DLC/')}", 4804,
                "f013a55a71da9377fbb38069e08fccd373b29dbada6c1a84cbd0decc813ec8f9"),
    "mount": (f"{MOD}/{DLC}/AutoLoad.ini", 1048,
              "7688ab053dfc24270ce08313503808420e40f1bf409c8106d80ee8c66e167f44"),
}


def regular_file(path):
    for ancestor in (*reversed(path.parents), path):
        if ancestor.is_symlink():
            raise ValueError("Linked corpus inputs and outputs are not allowed")
    if not stat.S_ISREG(path.stat().st_mode):
        raise ValueError("Corpus input or output is not a regular file")


def verify_corpus(root, inputs=None):
    sources = {}
    for name, (relative, size, digest) in (INPUTS if inputs is None else inputs).items():
        path = root / relative
        regular_file(path)
        verify(path, {"size": size, "sha256": digest})
        sources[name] = path
    return sources


def copy(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)


def identity(root, relative):
    path = root / relative
    regular_file(path)
    with path.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    return {"path": relative, "size": path.stat().st_size, "sha256": digest}


def transform(command, stage, request, name, target=TARGET):
    request_file = stage / f"{name}.json"
    request_file.write_text(json.dumps(request))
    result = subprocess.run([*command, "transform", str(request_file)], capture_output=True, text=True)
    if result.returncode != 0:
        raise ValueError(f"Community Patch {name} failed: {result.stderr.strip()}")
    messages = [json.loads(line) for line in result.stdout.splitlines()]
    count = len(request["contributions"])
    expected_progress = [{"protocol": 1, "type": "progress", "completed": index, "total": count}
                         for index in range(1, count + 1)]
    if len(messages) != count + 1 or messages[:-1] != expected_progress:
        raise ValueError("Unexpected transformation progress")
    report = messages[-1]
    output = Path(request["output_root"])
    if report != {"protocol": 1, "type": "complete", "outputs": [identity(output, target)]}:
        raise ValueError("Transformation output manifest disagrees with its file")
    if sorted(str(path.relative_to(output)) for path in output.rglob("*") if path.is_file()) != [target]:
        raise ValueError("Transformation produced unexpected files")
    print(json.dumps({"test": name, **report}), flush=True)


def run(root, semantic_tests):
    corpus = root / "docs/modTesting"
    sources = verify_corpus(corpus)
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    build = directory(Path("/build/mele"))
    command = [str(build / f"sdk-{configuration['sdk']['version']}" / "dotnet"),
               str(build / "source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll")]
    try:
        with tempfile.TemporaryDirectory(dir=build, prefix="community-patch-") as temporary:
            stage = Path(temporary)
            for name in ("game", "original", "input", "merged", "reapplied", "removed"):
                (stage / name).mkdir()
            copy(sources["codec"], stage / "game/Binaries/Win64/oo2core_8_win64.dll")
            copy(sources["engine"], stage / "original" / TARGET)
            copy(sources["engine"], stage / "input" / TARGET)
            copy(sources["manifest"], stage / "input" / MANIFEST)
            copy(sources["package"], stage / "input" / PACKAGE)
            immutable = {path: identity(path.parent, path.name)
                         for path in stage.rglob("*") if path.is_file()}
            request = {
                "protocol": 1, "operation": "le1-m3da", "game_root": str(stage / "game"),
                "original_root": str(stage / "original"), "input_root": str(stage / "input"),
                "output_root": str(stage / "merged"),
                "targets": [{"original": identity(stage / "original", TARGET),
                             "current": identity(stage / "input", TARGET)}],
                "contributions": [{"dlc": DLC, "mount": 5,
                                   "manifest": identity(stage / "input", MANIFEST),
                                   "packages": [identity(stage / "input", PACKAGE)]}],
            }
            transform(command, stage, request, "merged")
            copy(stage / "merged" / TARGET, stage / "previous" / TARGET)
            copy(sources["manifest"], stage / "previous" / MANIFEST)
            copy(sources["package"], stage / "previous" / PACKAGE)
            immutable.update({path: identity(path.parent, path.name)
                              for path in (stage / "previous").rglob("*") if path.is_file()})
            request["input_root"] = str(stage / "previous")
            request["targets"][0]["current"] = identity(stage / "previous", TARGET)
            request["output_root"] = str(stage / "reapplied")
            transform(command, stage, request, "reapplied")
            request["contributions"] = []
            request["output_root"] = str(stage / "removed")
            transform(command, stage, request, "removed")
            semantic_tests(root, corpus=stage)
            for path, spec in immutable.items():
                regular_file(path)
                verify(path, spec)
            print("Community Patch M3DA merge, reapplication, removal, and staged-source integrity passed.", flush=True)
    finally:
        verify_corpus(corpus)
        print("Supplied game and mod inputs retain their pinned hashes.", flush=True)


CONFIG_TARGET = "CookedPCConsole/Coalesced_INT.bin"
CONFIG_INPUTS = {
    "original": (f"{GAME}/BioGame/{CONFIG_TARGET}", 1464542,
                 "faea7471d285c72ccf36aa8263d1f04fe9e7a53bb0b708488ae552e34a7c4cd9"),
    "ConfigDelta-ModSettingsMenu.m3cd": (f"{MOD}/{DLC}/CookedPCConsole/ConfigDelta-ModSettingsMenu.m3cd", 959,
                                       "e07a1685ee49f56ebf2f06109f38fd25109a9b3c741221268cbe98686d173410"),
    "ConfigDelta-PCOptions_Persistent.m3cd": (f"{MOD}/{DLC}/CookedPCConsole/ConfigDelta-PCOptions_Persistent.m3cd", 465,
                                            "21d2eb484cea9107ad0a8299d694ccc8049678319c0400548669adff3523cad2"),
    "mount": INPUTS["mount"],
}


def run_config(root, semantic_tests):
    corpus = root / "docs/modTesting"
    sources = verify_corpus(corpus, CONFIG_INPUTS)
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    build = directory(Path("/build/mele"))
    command = [str(build / f"sdk-{configuration['sdk']['version']}" / "dotnet"),
               str(build / "source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll")]
    try:
        with tempfile.TemporaryDirectory(dir=build, prefix="community-patch-config-") as temporary:
            stage = Path(temporary)
            for name in ("game", "original", "input", "merged", "reapplied", "removed"):
                (stage / name).mkdir()
            copy(sources["original"], stage / "original" / CONFIG_TARGET)
            copy(sources["original"], stage / "input" / CONFIG_TARGET)
            contributions = []
            for name, source in sources.items():
                if name.endswith(".m3cd"):
                    relative = f"DLC/{DLC}/CookedPCConsole/{name}"
                    copy(source, stage / "input" / relative)
                    contributions.append({"dlc": DLC, "mount": 5, "manifest": identity(stage / "input", relative), "packages": []})
            immutable = {path: identity(path.parent, path.name) for path in stage.rglob("*") if path.is_file()}
            request = {"protocol": 1, "operation": "le1-m3cd", "game_root": str(stage / "game"),
                       "original_root": str(stage / "original"), "input_root": str(stage / "input"),
                       "output_root": str(stage / "merged"),
                       "targets": [{"original": identity(stage / "original", CONFIG_TARGET),
                                    "current": identity(stage / "input", CONFIG_TARGET)}],
                       "contributions": list(reversed(contributions))}
            transform(command, stage, request, "merged", CONFIG_TARGET)
            copy(stage / "merged" / CONFIG_TARGET, stage / "previous" / CONFIG_TARGET)
            for contribution in contributions:
                relative = contribution["manifest"]["path"]
                copy(stage / "input" / relative, stage / "previous" / relative)
            immutable.update({path: identity(path.parent, path.name)
                              for path in (stage / "previous").rglob("*") if path.is_file()})
            request["input_root"] = str(stage / "previous")
            request["targets"][0]["current"] = identity(stage / "previous", CONFIG_TARGET)
            request["output_root"] = str(stage / "reapplied")
            transform(command, stage, request, "reapplied", CONFIG_TARGET)
            request["contributions"] = []
            request["output_root"] = str(stage / "removed")
            transform(command, stage, request, "removed", CONFIG_TARGET)
            semantic_tests(root, corpus=stage, corpus_kind="m3cd")
            for path, spec in immutable.items():
                regular_file(path)
                verify(path, spec)
            print("Community Patch M3CD reference comparison, rebuilding, removal, and staged-source integrity passed.", flush=True)
    finally:
        verify_corpus(corpus, CONFIG_INPUTS)
        print("Supplied configuration and delta inputs retain their pinned hashes.", flush=True)
