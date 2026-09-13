"""Validate LE2/LE3 compilation against disposable copies of supplied packages."""

import json
from pathlib import Path
import tempfile

from build_support import directory, verify
from community_patch_test import copy, identity
from community_patch_m3m_test import transform


BASES = {
    "LE2": ("Core", "Engine", "GFxUI", "WwiseAudio", "SFXOnlineFoundation", "PlotManagerMap"),
    "LE3": ("Core", "Engine", "GameFramework", "GFxUI", "WwiseAudio", "SFXOnlineFoundation"),
}


def run(root, semantic_tests):
    corpus = root / "modTesting/Mass Effect Legendary Edition (Game)/Game"
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    build = directory(Path("/build/mele"))
    command = [str(build / f"sdk-{configuration['sdk']['version']}" / "dotnet"),
               str(build / "source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll")]
    for game, bases in BASES.items():
        supplied = corpus / game.replace("LE", "ME")
        with tempfile.TemporaryDirectory(dir=build, prefix=f"scripts-{game}-") as temporary:
            stage = Path(temporary)
            for name in ("game", "original", "input", "merged", "reapplied", "verification", "failed"):
                (stage / name).mkdir()
            files = [supplied / f"BioGame/CookedPCConsole/{name}.pcc" for name in (*bases, "SFXGame")]
            files.append(supplied / "Binaries/Win64/oo2core_8_win64.dll")
            identities = {path: identity(path.parent, path.name) for path in files}
            copy(files[-1], stage / "game/Binaries/Win64/oo2core_8_win64.dll")
            for path in files[:-1]:
                copy(path, stage / "input/CookedPCConsole" / path.name)
            target = "CookedPCConsole/SFXGame.pcc"
            copy(stage / "input" / target, stage / "original" / target)
            scripts, jobs = [], []
            for kind, entry, text in [
                ("class", "DeploydScriptTest", "class DeploydScriptTest extends Object;\nfunction int Value() { return 7; }"),
                ("function", "DeploydScriptTest.Value", "function int Value() { return 9; }"),
                ("member", "DeploydScriptTest", "function bool Added() { return true; }"),
            ]:
                path = f"Scripts/{kind}.uc"
                (stage / "input/Scripts").mkdir(exist_ok=True)
                (stage / "input" / path).write_text(text)
                scripts.append(identity(stage / "input", path))
                jobs.append(dict(target=target, entry=entry, kind=kind, input=path, source_entry="", allow_new=False))
            targets = [dict(original=identity(stage / "original", target), current=identity(stage / "input", target))]
            dependencies = [identity(stage / "input", f"CookedPCConsole/{name}.pcc") for name in bases]
            request = dict(protocol=1, operation="mele-m3m-ordered", game=game,
                           game_root=str(stage / "game"), original_root=str(stage / "original"),
                           input_root=str(stage / "input"), output_root=str(stage / "merged"),
                           targets=targets, dependencies=dependencies, assets=[], scripts=scripts, jobs=jobs)
            immutable = {path: identity(path.parent, path.name) for path in stage.rglob("*") if path.is_file()}
            transform(command, stage, request, "merged")
            repeated = dict(request, output_root=str(stage / "reapplied"))
            transform(command, stage, repeated, "reapplied")
            (stage / "semantic-request.json").write_text(json.dumps(dict(request, output_root=str(stage / "verification"))))
            semantic_tests(root, corpus=stage, corpus_kind="m3m-ordered")
            failed = dict(request, output_root=str(stage / "failed"), jobs=[*jobs[:-1], dict(jobs[-1], entry="MissingExport")])
            transform(command, stage, failed, "failed", success=False)
            for path, expected in {**identities, **immutable}.items():
                verify(path, expected)
            print(f"{game}: class, function, member, reference output, baseline rebuild and failure isolation passed", flush=True)
