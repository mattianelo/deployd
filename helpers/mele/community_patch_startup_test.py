"""Validate Community Patch's localization-wide assets against every supplied LE1 Startup."""

import json
from pathlib import Path
import tempfile

from build_support import directory, verify
from community_patch_test import GAME, copy, identity, verify_corpus
from community_patch_m3m_test import INPUTS, transform
from frozen_m3m import decode


STARTUPS = {
    "DE": (45303814, "4b512923dae19a313b2dc443b263094be0024a87ef110fe7636773bee6062ef4"),
    "ES": (45240501, "d4bcc2ef6ae99e3438f6b7bd3d131eb24998ed9307f6bebbe5ac1b9411a4fdd3"),
    "FE": (45246985, "5f8927bd21ad645d37042004952d333ee71a1db43989c98f1dc8d761e7874c2f"),
    "FR": (45246934, "7da91f18354e2d761b0c2c8d119d80045152d4087825c254a658dae28f3dc9e5"),
    "GE": (45303818, "f15b9d41c7d7709bae9d20f0b12828f85ea202edf5acb05da15b0c141bac0cc2"),
    "IE": (45217972, "820e53a0d32a87220485bc406152d5f5edd4e0bd6f63dc5a01b2f9aa35197e85"),
    "INT": (45198425, "10e71f9eb7883789942f8f464693419838f4d8f7873de1ae0f2aa3fd84c0a860"),
    "IT": (45217965, "fd75a5a757b5e9e1a9086e1c0350cb4ea2766efeac2c7ecf1f7c696e24df0a62"),
    "JA": (45161787, "a759627083efda5be0ecb0d5cbc094fef0e8a4b02fbdebcb52ef3615b5b49ac0"),
    "PL": (45299952, "b0d233e368789a54d4a1fc8302373d94e3d8eb80addfb083d31740b655aea5b7"),
    "PLPC": (45299642, "5a6b91a1b8ebc699435a0e980257bcc92cb6dbf05b74647a689500e7f8c42786"),
    "RA": (45315310, "efa29ec8a49e1d74dc4e1f2e3a0db87ce5a6f915b48509f4854c14e567f8b526"),
    "RU": (45315249, "9bcdddeb3d0d6bb3b4948737a2c4f78f2f10888ff2764ebc6464bcfd73c33446"),
}
CORPUS = {name: INPUTS[name] for name in ("codec", "HUDFixes.m3m", "DebugSaves.m3m")}
CORPUS.update({f"Startup_{code}.pcc": (f"{GAME}/BioGame/CookedPCConsole/Startup_{code}.pcc", *spec)
               for code, spec in STARTUPS.items()})


def stage_jobs(sources, root, target):
    if target not in {f"CookedPCConsole/Startup_{code}.pcc" for code in STARTUPS}:
        raise ValueError("Unknown localized Startup target")
    assets, jobs = {}, []
    for name in ("HUDFixes", "DebugSaves"):
        manifest, embedded = decode(sources[name + ".m3m"])
        localized = [file for file in manifest["files"] if file["applytoalllocalizations"]]
        if len(localized) != 1 or localized[0]["filename"] != "Startup_INT.pcc":
            raise ValueError("Unexpected frozen localization-wide file declaration")
        for change in localized[0]["changes"]:
            if any(change.get(key) for key in ("classupdate", "scriptupdate", "addtoclassorreplace",
                                              "propertyupdates", "sequenceskipupdate", "disableconfigupdate", "newassetupdate")):
                raise ValueError("Unexpected localized operation")
            update = change["assetupdate"]
            filename = update["assetname"].casefold()
            if Path(filename).name != filename or "\\" in filename or not filename.endswith(".pcc"):
                raise ValueError("Invalid localized asset filename")
            path = f"Assets/{name}/{filename}"
            if path not in assets:
                data = embedded[filename]
                output = root / path
                output.parent.mkdir(parents=True, exist_ok=True)
                output.write_bytes(data)
                assets[path] = identity(root, path)
            jobs.append({"target": target, "entry": change["entryname"], "kind": "asset", "input": path,
                         "source_entry": update["entryname"], "allow_new": update.get("canmergeasnew", False)})
    if len(assets) != 2 or len(jobs) != 6:
        raise ValueError("Unexpected frozen localized asset count")
    return list(assets.values()), jobs


def run(root, semantic_tests):
    corpus = root / "docs/modTesting"
    sources = verify_corpus(corpus, CORPUS)
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    build = directory(Path("/build/mele"))
    command = [str(build / f"sdk-{configuration['sdk']['version']}" / "dotnet"),
               str(build / "source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll")]
    try:
        with tempfile.TemporaryDirectory(dir=build, prefix="community-patch-startup-") as temporary:
            stage = Path(temporary)
            copy(sources["codec"], stage / "game/Binaries/Win64/oo2core_8_win64.dll")
            immutable = {}
            for code in STARTUPS:
                variant = stage / f"Startup_{code}"
                for name in ("original", "input", "merged", "reapplied", "verification", "failed"):
                    (variant / name).mkdir(parents=True)
                target = f"CookedPCConsole/Startup_{code}.pcc"
                for location in ("original", "input"):
                    copy(sources[f"Startup_{code}.pcc"], variant / location / target)
                assets, jobs = stage_jobs(sources, variant / "input", target)
                request = {"protocol": 1, "operation": "mele-m3m-ordered", "game": "LE1", "game_root": str(stage / "game"),
                           "original_root": str(variant / "original"), "input_root": str(variant / "input"),
                           "output_root": str(variant / "merged"), "targets": [{"original": identity(variant / "original", target),
                                                                                "current": identity(variant / "input", target)}],
                           "dependencies": [], "assets": assets, "scripts": [], "jobs": jobs}
                immutable.update({path: identity(path.parent, path.name) for path in variant.rglob("*") if path.is_file()})
                transform(command, variant, request, f"merged-{code}")
                (variant / "semantic-request.json").write_text(json.dumps(dict(request, output_root=str(variant / "verification"))))
                copy(variant / "merged" / target, variant / "previous" / target)
                for asset in assets:
                    copy(variant / "input" / asset["path"], variant / "previous" / asset["path"])
                immutable.update({path: identity(path.parent, path.name) for path in (variant / "previous").rglob("*") if path.is_file()})
                repeated = dict(request, input_root=str(variant / "previous"), output_root=str(variant / "reapplied"),
                                targets=[dict(request["targets"][0], current=identity(variant / "previous", target))])
                transform(command, variant, repeated, f"reapplied-{code}")
                failed = dict(request, output_root=str(variant / "failed"), jobs=[*jobs[:-1], dict(jobs[-1], entry="MissingExport")])
                transform(command, variant, failed, f"failed-{code}", success=False)
            semantic_tests(root, corpus=stage, corpus_kind="m3m-startup")
            for path, spec in immutable.items():
                verify(path, spec)
            print("All 13 LE1 Startup variants passed six localized asset updates, reference, reapplication, and recovery checks.", flush=True)
    finally:
        verify_corpus(corpus, CORPUS)
        print("Supplied localized game and mod files retain their pinned hashes.", flush=True)
