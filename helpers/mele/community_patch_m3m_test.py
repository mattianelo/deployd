"""Validate ordered Community Patch M3M jobs without deploying game files."""

import json
from pathlib import Path
import subprocess
import tempfile

from build_support import directory, verify
from community_patch_test import GAME, MOD, copy, identity, verify_corpus
from community_patch_assets_test import ASSET_INPUTS
from community_patch_scripts_test import SCRIPT_INPUTS
from frozen_m3m import decode


ORDER = ("HUDFixes", "EyeScannerFix", "ScriptFixes", "ModSettingsMenu", "PersistentSettings",
         "FOVCamera", "ModConvoNodeFix", "KarpovPistolFix", "DebugSaves", "SideloaderFramework")
TARGETS = ("Startup_INT.pcc", "SFXGame.pcc", "BIOC_Materials.pcc")
INPUTS = dict(ASSET_INPUTS, **SCRIPT_INPUTS, **{
    "Startup_INT.pcc": (f"{GAME}/BioGame/CookedPCConsole/Startup_INT.pcc", 45198425,
                        "10e71f9eb7883789942f8f464693419838f4d8f7873de1ae0f2aa3fd84c0a860"),
    "ScriptFixes.m3m": (f"{MOD}/MergeMods/ScriptFixes.m3m", 2937,
                        "c2021ab1815a7463d5409951d2b2726e10c4e9df61c5455d08bcfb6fd7581dc1"),
    "DebugSaves.m3m": (f"{MOD}/MergeMods/DebugSaves.m3m", 31935,
                       "3e4ada7f1cf49efe5f8ce1f358d7239b8d39a6451961bcd118f1f9b6bcbce991"),
    "SideloaderFramework.m3m": (f"{MOD}/MergeMods/SideloaderFramework.m3m", 3535,
                               "5e7d8707a1265bfea9169d9c37a8c60a3f7bf5f731fac8e6035e88612b08add0"),
})


def stage_jobs(sources, root):
    assets, scripts, jobs = {}, [], []
    for name in ORDER:
        manifest, embedded = decode(sources[name + ".m3m"])
        for file in manifest["files"]:
            if file["filename"] not in TARGETS or (file["applytoalllocalizations"] and file["filename"] != "Startup_INT.pcc"):
                raise ValueError("Unexpected frozen M3M target")
            for change in file["changes"]:
                if any(change.get(key) for key in ("propertyupdates", "sequenceskipupdate", "disableconfigupdate", "newassetupdate")):
                    raise ValueError("Unexpected frozen M3M operation")
                job = {"target": "CookedPCConsole/" + file["filename"], "entry": change["entryname"],
                       "source_entry": "", "allow_new": False}
                if update := change.get("assetupdate"):
                    asset_name = update["assetname"].casefold()
                    if Path(asset_name).name != asset_name or "\\" in asset_name or not asset_name.endswith(".pcc"):
                        raise ValueError("Invalid frozen package asset name")
                    path = f"Assets/{name}/{asset_name}"
                    if path not in assets:
                        output = root / path
                        output.parent.mkdir(parents=True, exist_ok=True)
                        output.write_bytes(embedded[asset_name])
                        assets[path] = identity(root, path)
                    jobs.append(dict(job, kind="asset", input=path, source_entry=update["entryname"],
                                     allow_new=update.get("canmergeasnew", False)))
                updates = []
                if update := change.get("classupdate"):
                    updates.append(("class", update["assetname"], None))
                if update := change.get("scriptupdate"):
                    updates.append(("function", update["scriptfilename"], update["scripttext"]))
                if update := change.get("addtoclassorreplace"):
                    updates.extend(("member", file, text) for file, text in
                                   zip(update["scriptfilenames"], update["scripts"], strict=True))
                for kind, filename, text in updates:
                    if Path(filename).name != filename or "\\" in filename or not filename.endswith(".uc"):
                        raise ValueError("Invalid frozen script name")
                    if text is None:
                        text = embedded[filename.casefold()].decode("utf-8-sig")
                    path = f"Scripts/{name}/{len(scripts)}/{filename}"
                    output = root / path
                    output.parent.mkdir(parents=True, exist_ok=True)
                    output.write_text(text, encoding="utf-8")
                    scripts.append(identity(root, path))
                    jobs.append(dict(job, kind=kind, input=path))
    return list(assets.values()), scripts, jobs


def transform(command, stage, request, name, success=True):
    path = stage / f"{name}.json"
    path.write_text(json.dumps(request))
    result = subprocess.run([*command, "transform-m3m", str(path)], capture_output=True, text=True)
    output = Path(request["output_root"])
    if not success:
        if result.returncode == 0 or any(output.iterdir()):
            raise ValueError("Failed ordered M3M job published output")
        count = len(request["jobs"])
        expected = [{"protocol": 1, "type": "progress", "completed": index, "total": count} for index in range(1, count)]
        if [json.loads(line) for line in result.stdout.splitlines()] != expected:
            raise ValueError("The late failure did not occur after the preceding M3M jobs")
        return
    if result.returncode != 0:
        raise ValueError(f"Ordered M3M transformation failed: {result.stderr.strip()}")
    count = len(request["jobs"])
    expected = [{"protocol": 1, "type": "progress", "completed": index, "total": count} for index in range(1, count + 1)]
    outputs = [identity(output, item["current"]["path"]) for item in request["targets"]]
    expected.append({"protocol": 1, "type": "complete", "outputs": outputs})
    if [json.loads(line) for line in result.stdout.splitlines()] != expected:
        raise ValueError("Ordered M3M output identity or progress disagrees")
    if sorted(p.relative_to(output).as_posix() for p in output.rglob("*") if p.is_file()) != sorted(item["path"] for item in outputs):
        raise ValueError("Ordered M3M produced unexpected files")
    print(json.dumps({"test": name, "outputs": outputs}), flush=True)


def run(root, semantic_tests):
    corpus = root / "docs/modTesting"
    sources = verify_corpus(corpus, INPUTS)
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    build = directory(Path("/build/mele"))
    command = [str(build / f"sdk-{configuration['sdk']['version']}" / "dotnet"),
               str(build / "source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll")]
    try:
        with tempfile.TemporaryDirectory(dir=build, prefix="community-patch-m3m-") as temporary:
            stage = Path(temporary)
            for name in ("game", "original", "input", "merged", "reapplied", "verification", "failed"):
                (stage / name).mkdir()
            copy(sources["codec"], stage / "game/Binaries/Win64/oo2core_8_win64.dll")
            targets = []
            for name in TARGETS:
                path = f"CookedPCConsole/{name}"
                for location in ("original", "input"):
                    copy(sources[name], stage / location / path)
                targets.append({"original": identity(stage / "original", path), "current": identity(stage / "input", path)})
            dependencies = []
            for name in ("Core.pcc", "Engine.pcc", "GFxUI.pcc", "PlotManagerMap.pcc", "SFXOnlineFoundation.pcc"):
                path = f"CookedPCConsole/{name}"
                copy(sources[name], stage / "input" / path)
                dependencies.append(identity(stage / "input", path))
            assets, scripts, jobs = stage_jobs(sources, stage / "input")
            counts = {kind: sum(job["kind"] == kind for job in jobs) for kind in ("asset", "class", "function", "member")}
            if counts != {"asset": 11, "class": 1, "function": 14, "member": 11}:
                raise ValueError(f"Unexpected frozen M3M jobs: {counts}")
            request = {"protocol": 1, "operation": "mele-m3m-ordered", "game": "LE1", "game_root": str(stage / "game"),
                       "original_root": str(stage / "original"), "input_root": str(stage / "input"),
                       "output_root": str(stage / "merged"), "targets": targets,
                       "dependencies": dependencies, "assets": assets, "scripts": scripts, "jobs": jobs}
            immutable = {path: identity(path.parent, path.name) for path in stage.rglob("*") if path.is_file()}
            transform(command, stage, request, "merged")
            (stage / "semantic-request.json").write_text(json.dumps(dict(request, output_root=str(stage / "verification"))))
            for item in [*[item["current"] for item in targets], *dependencies, *assets, *scripts]:
                origin = "merged" if item["path"] in {target["current"]["path"] for target in targets} else "input"
                copy(stage / origin / item["path"], stage / "previous" / item["path"])
            immutable.update({path: identity(path.parent, path.name) for path in (stage / "previous").rglob("*") if path.is_file()})
            repeated = dict(request, input_root=str(stage / "previous"), output_root=str(stage / "reapplied"),
                            targets=[dict(item, current=identity(stage / "previous", item["current"]["path"])) for item in targets])
            transform(command, stage, repeated, "reapplied")
            semantic_tests(root, corpus=stage, corpus_kind="m3m-ordered")
            failed = dict(request, output_root=str(stage / "failed"), jobs=[*jobs[:-1], dict(jobs[-1], entry="MissingExport")])
            transform(command, stage, failed, "failed", success=False)
            for path, spec in immutable.items():
                verify(path, spec)
            print("All ten Community Patch M3Ms passed ordered compilation, reference, reapplication, and failure checks.", flush=True)
    finally:
        verify_corpus(corpus, INPUTS)
        print("Supplied game and M3M containers retain their pinned hashes.", flush=True)
