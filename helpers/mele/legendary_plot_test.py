"""Validate LE1/LE2 plot merges using supplied originals and generated test scripts."""

import json
from pathlib import Path
import tempfile

from build_support import directory, verify
from community_patch_test import copy, identity
from community_patch_tlk_plot_test import transform
from legendary_scripts_test import BASES


def run(root, semantic_tests):
    for game, bases in {
        "LE1": ("Core", "Engine", "GFxUI", "PlotManagerMap", "SFXOnlineFoundation", "SFXGame", "SFXStrategicAI", "SFXGameContent_Powers"),
        "LE2": (*BASES["LE2"], "SFXGame", "Startup_INT"),
    }.items():
        run_game(root, semantic_tests, game, bases)


def run_game(root, semantic_tests, game, bases):
    supplied = root / "modTesting/Mass Effect Legendary Edition (Game)/Game" / game.replace("LE", "ME")
    build = directory(Path("/build/mele"))
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    command = [str(build / f"sdk-{configuration['sdk']['version']}" / "dotnet"),
               str(build / "source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll")]
    with tempfile.TemporaryDirectory(dir=build, prefix=f"plot-{game}-") as temporary:
        stage = Path(temporary)
        for name in ("input", "original", "merged", "reapplied", "verification", "combined", "combined-verification", "removed"):
            (stage / name).mkdir()
        paths = [supplied / f"BioGame/CookedPCConsole/{name}.pcc" for name in (*bases, "PlotManager")]
        paths.append(supplied / "Binaries/Win64/oo2core_8_win64.dll")
        identities = {path: identity(path.parent, path.name) for path in paths}
        for path in paths[:-1]:
            copy(path, stage / "input/CookedPCConsole" / path.name)
        copy(paths[-1], stage / "game/Binaries/Win64/oo2core_8_win64.dll")
        target = "CookedPCConsole/PlotManager.pcc"
        copy(stage / "input" / target, stage / "original" / target)
        def function(number, value):
            return f"public function bool F{number}(BioWorldInfo bioWorld, int Argument)\n{{ return {value}; }}\n"
        contributions = []
        for dlc, mount, name, text in [
            ("DLC_MOD_First", 5, "PlotManagerUpdate", function(900001, "true") + function(900002, "false")),
            ("DLC_MOD_Second", 10, "PlotManagerUpdate", function(900001, "false") + function(900003, "F900002(bioWorld, Argument)")),
            ("DLC_MOD_Second", 10, "Additional", function(900004, "F900003(bioWorld, Argument)")),
        ]:
            path = f"DLC/{dlc}/CookedPCConsole/{name}.pmu"
            destination = stage / "input" / path
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_text(text)
            contributions.append(dict(dlc=dlc, mount=mount, manifest=identity(stage / "input", path)))
        request = dict(protocol=1, operation="mele-plot", game=game, game_root=str(stage / "game"),
                       original_root=str(stage / "original"), input_root=str(stage / "input"), output_root=str(stage / "merged"),
                       target=dict(original=identity(stage / "original", target), current=identity(stage / "input", target)),
                       dependencies=[identity(stage / "input", f"CookedPCConsole/{name}.pcc") for name in bases],
                       contributions=contributions[:1])
        immutable = {path: identity(path.parent, path.name) for path in stage.rglob("*") if path.is_file()}
        transform(command, stage, request, "merged", "plot", 2)
        transform(command, stage, dict(request, output_root=str(stage / "reapplied")), "reapplied", "plot", 2)
        combined = dict(request, output_root=str(stage / "combined"), contributions=list(reversed(contributions)))
        transform(command, stage, combined, "combined", "plot", 4)
        (stage / "semantic-request.json").write_text(json.dumps(dict(request, output_root=str(stage / "verification"))))
        (stage / "combined-request.json").write_text(json.dumps(dict(combined, output_root=str(stage / "combined-verification"))))
        semantic_tests(root, corpus=stage, corpus_kind="plot")
        removed = dict(request, output_root=str(stage / "removed"), contributions=[], dependencies=[])
        transform(command, stage, removed, "removed", "plot", 0)
        verify(stage / "removed" / target, request["target"]["original"])
        for path, expected in {**identities, **immutable}.items():
            verify(path, expected)
        print(f"{game} plot: forward reference, mount precedence, baseline rebuild, removal and cancellation passed", flush=True)
