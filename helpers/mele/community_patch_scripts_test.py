"""Test frozen Community Patch script jobs using verified disposable game copies."""

import json
from pathlib import Path
import struct
import subprocess
import tempfile

from build_support import directory, verify
from community_patch_test import GAME, MOD, INPUTS, copy, identity, verify_corpus
from community_patch_assets_test import ASSET_INPUTS


SCRIPT_INPUTS = {
    "codec": INPUTS["codec"],
    "SFXGame.pcc": ASSET_INPUTS["SFXGame.pcc"],
    "Core.pcc": (f"{GAME}/BioGame/CookedPCConsole/Core.pcc", 26841,
                 "fd74ff917c2fce6628c872d8eeb5a8204372f4709e56e71b09ff1fb20266ffea"),
    "Engine.pcc": (f"{GAME}/BioGame/CookedPCConsole/Engine.pcc", 9843689,
                   "296308fba3fe6294f8ed4eba5796fcc54e75e51837ff9cc261e1151124f4ad13"),
    "GFxUI.pcc": (f"{GAME}/BioGame/CookedPCConsole/GFxUI.pcc", 13553,
                  "dd58ce9c2548da72e4351e682af0971af634dd61f15201458af219c32f7fd2ff"),
    "PlotManagerMap.pcc": (f"{GAME}/BioGame/CookedPCConsole/PlotManagerMap.pcc", 2326,
                           "a14c5b5cf847d61144d1b35a8700e8666c48c3c6bc763e80f618fbf89248538d"),
    "SFXOnlineFoundation.pcc": (f"{GAME}/BioGame/CookedPCConsole/SFXOnlineFoundation.pcc", 37354,
                               "cdb2147d824c73e7091600b1dc1e5693c53ce7edef5249bf119e88ab23e69ca0"),
    "HUDFixes.m3m": (f"{MOD}/MergeMods/HUDFixes.m3m", 429627,
                     "1d9ae3440ec942898d9881c69f9c911879a3cb9ca83346c2d67bf34757bf7a10"),
    "ModSettingsMenu.m3m": (f"{MOD}/MergeMods/ModSettingsMenu.m3m", 2815,
                           "b161706426b86c2536d006aa88f620d2ed9a87a9ad504c853c8f54ff9d442c99"),
    "PersistentSettings.m3m": (f"{MOD}/MergeMods/PersistentSettings.m3m", 2097,
                              "4873b1bd448d24bde75da618d54af22c6ab056adfe623dc8f0cd942ed854d0b3"),
    "FOVCamera.m3m": (f"{MOD}/MergeMods/FOVCamera.m3m", 20349,
                      "9366ebdfd992f9cec1784de4c459406abe0c454f2e1a46fa06445294890c9e09"),
    "ModConvoNodeFix.m3m": (f"{MOD}/MergeMods/ModConvoNodeFix.m3m", 11221,
                            "3ac38a8cb325fdf8a3100b1e85527da0cd688f7620f8db96d042fe4ec04feac4"),
}


def frozen_scripts(path, destination):
    data = path.read_bytes()
    if data[:5] != b"M3MM\x01" or len(data) < 9:
        raise ValueError("Expected frozen M3M v1 script corpus")
    size = struct.unpack_from("<i", data, 5)[0]
    length = -size * 2 if size < 0 else size
    if length < 1 or length > len(data) - 9:
        raise ValueError("Invalid frozen manifest length")
    text = data[9:9 + length].decode("utf-16-le" if size < 0 else "ascii")
    if not text.endswith("\0") or "\0" in text[:-1]:
        raise ValueError("Invalid frozen manifest string")
    manifest = json.loads(text[:-1])
    if manifest["game"] != "LE1":
        raise ValueError("Unexpected frozen script game")
    scripts, merges = [], []
    for file in manifest["files"]:
        if file["filename"] != "SFXGame.pcc":
            continue
        if file["applytoalllocalizations"]:
            raise ValueError("Unexpected script localization")
        for change in file["changes"]:
            if any(change.get(key) for key in ("assetupdate", "classupdate", "propertyupdates",
                                              "sequenceskipupdate", "disableconfigupdate", "newassetupdate")):
                raise ValueError("Unexpected operation in frozen script job")
            updates = []
            if update := change.get("scriptupdate"):
                updates.append(("function", update["scriptfilename"], update["scripttext"]))
            if update := change.get("addtoclassorreplace"):
                updates.extend(("member", name, text) for name, text in
                               zip(update["scriptfilenames"], update["scripts"], strict=True))
            for kind, name, text in updates:
                if Path(name).name != name or "\\" in name or not name.endswith(".uc") or not text:
                    raise ValueError("Invalid frozen script name or text")
                relative = f"Scripts/{path.stem}/{len(scripts)}/{name}"
                output = destination / relative
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_text(text, encoding="utf-8")
                scripts.append(identity(destination, relative))
                merges.append({"entry": change["entryname"], "kind": kind, "script": relative})
    return scripts, merges


def transform(command, stage, request, name, success=True):
    path = stage / f"{name}.json"
    path.write_text(json.dumps(request))
    result = subprocess.run([*command, "transform-scripts", str(path)], capture_output=True, text=True)
    output = Path(request["output_root"])
    if not success:
        if result.returncode == 0 or any(output.iterdir()):
            raise ValueError("Failed script merge published outputs")
        return
    if result.returncode != 0:
        raise ValueError(f"Community Patch script merge failed: {result.stderr.strip()}")
    messages = [json.loads(line) for line in result.stdout.splitlines()]
    total = len(request["merges"])
    progress = [{"protocol": 1, "type": "progress", "completed": index, "total": total}
                for index in range(1, total + 1)]
    outputs = [identity(output, request["target"]["path"])]
    if messages != [*progress, {"protocol": 1, "type": "complete", "outputs": outputs}]:
        raise ValueError("Script merge progress or output identity disagrees")
    if [p.relative_to(output).as_posix() for p in output.rglob("*") if p.is_file()] != [outputs[0]["path"]]:
        raise ValueError("Script merge produced unexpected outputs")
    print(json.dumps({"test": name, "outputs": outputs}), flush=True)


def run(root, semantic_tests):
    corpus = root / "docs/modTesting"
    sources = verify_corpus(corpus, SCRIPT_INPUTS)
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    build = directory(Path("/build/mele"))
    command = [str(build / f"sdk-{configuration['sdk']['version']}" / "dotnet"),
               str(build / "source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll")]
    try:
        with tempfile.TemporaryDirectory(dir=build, prefix="community-patch-scripts-") as temporary:
            stage = Path(temporary)
            for name in ("game", "input", "merged", "reapplied", "verification", "failed"):
                (stage / name).mkdir()
            copy(sources["codec"], stage / "game/Binaries/Win64/oo2core_8_win64.dll")
            dependencies = []
            for name in ("Core.pcc", "Engine.pcc", "GFxUI.pcc", "PlotManagerMap.pcc", "SFXOnlineFoundation.pcc", "SFXGame.pcc"):
                relative = f"CookedPCConsole/{name}"
                copy(sources[name], stage / "input" / relative)
                dependencies.append(identity(stage / "input", relative))
            target = dependencies.pop()
            scripts, merges = [], []
            for name in ("HUDFixes.m3m", "ModSettingsMenu.m3m", "PersistentSettings.m3m", "FOVCamera.m3m", "ModConvoNodeFix.m3m"):
                files, changes = frozen_scripts(sources[name], stage / "input")
                scripts.extend(files)
                merges.extend(changes)
            if len(merges) != 13:
                raise ValueError("Unexpected frozen script job count")
            request = {"protocol": 1, "operation": "le1-m3m-scripts", "game_root": str(stage / "game"),
                       "input_root": str(stage / "input"), "output_root": str(stage / "merged"),
                       "target": target, "dependencies": dependencies, "scripts": scripts, "merges": merges}
            immutable = {path: identity(path.parent, path.name) for path in stage.rglob("*") if path.is_file()}
            transform(command, stage, request, "merged")
            (stage / "semantic-request.json").write_text(json.dumps(dict(request, output_root=str(stage / "verification"))))
            for item in [target, *dependencies, *scripts]:
                origin = "merged" if item == target else "input"
                copy(stage / origin / item["path"], stage / "previous" / item["path"])
            immutable.update({path: identity(path.parent, path.name) for path in (stage / "previous").rglob("*") if path.is_file()})
            repeated = dict(request, input_root=str(stage / "previous"), output_root=str(stage / "reapplied"),
                            target=identity(stage / "previous", target["path"]))
            transform(command, stage, repeated, "reapplied")
            semantic_tests(root, corpus=stage, corpus_kind="m3m-scripts")
            failed = dict(request, output_root=str(stage / "failed"),
                          merges=[*merges[:-1], dict(merges[-1], entry="MissingExport")])
            transform(command, stage, failed, "failed", success=False)
            invalid = stage / "input/Scripts/Invalid.uc"
            invalid.write_text("var NonexistentType Value;", encoding="utf-8")
            broken = dict(request, output_root=str(stage / "failed"),
                          scripts=[*scripts[:-1], identity(stage / "input", "Scripts/Invalid.uc")],
                          merges=[*merges[:-1], dict(merges[-1], script="Scripts/Invalid.uc")])
            transform(command, stage, broken, "invalid-script", success=False)
            for path, spec in immutable.items():
                verify(path, spec)
            print("Community Patch script compilation, reference comparison, reapplication, and failure checks passed.", flush=True)
    finally:
        verify_corpus(corpus, SCRIPT_INPUTS)
        print("Supplied game and script inputs retain their pinned hashes.", flush=True)
