"""Compile one bounded raw LE2 M3TO fixture through the packaged helper protocol."""

import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def identity(root, relative):
    data = (root / relative).read_bytes()
    return {"path": relative, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}


def run(root):
    option = root / "modTesting/No Headgear for Squadmates - LE2/Options/Garrus No Mic"
    manifest_name = "TextureOverride-DLC_MOD_NoHeadgearSquad_GarrusNoMic.m3to"
    package_name = "TO_DLC_MOD_NoHeadgearSquad_GarrusNoMic.pcc"
    if not option.is_dir():
        raise ValueError("The ignored No Headgear reference fixture is required for the M3TO compiler test")
    build = Path("/build/mele")
    sdk = next(build.glob("sdk-*/dotnet"))
    helper = build / "source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll"
    codec = root / ".ci-artifacts/mele-fixtures/oo2core_8_win64.dll"
    if not codec.is_file():
        raise ValueError("The verified MELE codec fixture is required for the M3TO compiler test")
    with tempfile.TemporaryDirectory(dir=build, prefix="m3to-") as temporary:
        stage = Path(temporary)
        game = stage / "game"
        inputs = stage / "inputs"
        outputs = stage / "outputs"
        cooked = inputs / "DLC/DLC_MOD_NoHeadgearSquad/CookedPCConsole"
        (game / "Binaries/Win64").mkdir(parents=True)
        cooked.mkdir(parents=True)
        outputs.mkdir()
        shutil.copyfile(codec, game / "Binaries/Win64/oo2core_8_win64.dll")
        shutil.copyfile(option / manifest_name, cooked / manifest_name)
        shutil.copyfile(option / package_name, cooked / package_name)
        manifest = f"DLC/DLC_MOD_NoHeadgearSquad/CookedPCConsole/{manifest_name}"
        package = f"DLC/DLC_MOD_NoHeadgearSquad/CookedPCConsole/{package_name}"
        request = {
            "protocol": 1, "operation": "mele-m3to", "game_root": str(game),
            "input_root": str(inputs), "output_root": str(outputs), "game": "LE2",
            "dlc": "DLC_MOD_NoHeadgearSquad", "manifests": [identity(inputs, manifest)],
            "packages": [identity(inputs, package)],
            "outputs": ["DLC/DLC_MOD_NoHeadgearSquad/CombinedTextureOverrides.btp",
                        "DLC/DLC_MOD_NoHeadgearSquad/BTPMetadata.btm"], "textures": 2,
        }
        request_path = stage / "request.json"
        request_path.write_text(json.dumps(request))
        result = subprocess.run([str(sdk), str(helper), "transform-textures", str(request_path)],
                                capture_output=True, text=True)
        if result.returncode != 0:
            raise ValueError(f"M3TO helper failed: {result.stderr.strip()}")
        events = [json.loads(line) for line in result.stdout.splitlines()]
        complete = next((event for event in events if event.get("type") == "complete"), None)
        if complete is None or len(complete.get("outputs", [])) != 2:
            raise ValueError("M3TO helper did not report its exact output inventory")
        btp = outputs / request["outputs"][0]
        if btp.read_bytes()[:6] != b"LETEXM":
            raise ValueError("M3TO helper produced an invalid BTP header")
        original_manifest = (inputs / manifest).read_bytes()

        def reject(label, manifest_bytes, change=None):
            (inputs / manifest).write_bytes(manifest_bytes)
            rejected = dict(request)
            rejected["manifests"] = [identity(inputs, manifest)]
            rejected["output_root"] = str(stage / f"rejected-{label}")
            Path(rejected["output_root"]).mkdir()
            if change is not None:
                change(rejected)
            rejected_path = stage / f"request-{label}.json"
            rejected_path.write_text(json.dumps(rejected))
            failed = subprocess.run([str(sdk), str(helper), "transform-textures", str(rejected_path)],
                                    capture_output=True, text=True)
            if failed.returncode == 0:
                raise ValueError(f"M3TO helper accepted {label} input")

        reject("malformed", b"{")
        reject("wrong-game", original_manifest.replace(b'"LE2"', b'"LE1"'))
        reject("missing-export", original_manifest.replace(b"TeamSelect_I111", b"TeamSelect_Missing"))
        reject("wrong-hash", original_manifest,
               lambda value: value["manifests"][0].update(sha256="0" * 64))
        (inputs / manifest).write_bytes(original_manifest)
        print("Raw LE2 M3TO fixture compiled successfully", flush=True)
