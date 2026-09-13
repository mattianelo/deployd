"""Assemble the pinned Linux helper without game-owned or game-loaded binaries."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from build_support import directory
import build_managed
import license_inventory


LINUX_RIDS = frozenset({"linux-x64", "linux", "unix-x64", "unix", "any"})
TRACE_PROVIDER = "libcoreclrtraceptprovider.so"


def copy_tree(source, destination, ignore=None):
    if source.is_symlink() or any(path.is_symlink() for path in source.rglob("*")):
        raise ValueError("Refusing linked helper package inputs")
    shutil.copytree(source, destination, ignore=ignore)


def copy_application(source, destination):
    def ignore(directory, names):
        if Path(directory) == source / "runtimes":
            return set(names) - LINUX_RIDS
        return set()

    copy_tree(source, destination, ignore=ignore)


def copy_runtime(source, destination):
    # Deployd disables helper diagnostics; this optional provider needs an obsolete LTTng ABI.
    copy_tree(source, destination, ignore=shutil.ignore_patterns(TRACE_PROVIDER))


def validate_native_files(root):
    for path in root.rglob("*"):
        if path.is_file():
            with path.open("rb") as stream:
                header = stream.read(20)
            if header[:4] == b"\x7fELF":
                if len(header) != 20 or header[4:6] != b"\x02\x01" or header[18:20] != b"\x3e\x00":
                    raise ValueError("Helper bundle contains a native binary for another architecture")


def assemble(root, output):
    root = root.resolve()
    if os.getuid() == 0 and (root == Path("/workspace") or Path("/workspace") in root.parents):
        raise ValueError("Container root cannot build through the shared workspace")
    output = Path(output)
    directory(output.parent)
    if output.exists() or output.is_symlink():
        raise ValueError("Helper package output must not already exist")
    spec = importlib.util.spec_from_file_location("mele_build", root / "scripts/mele-helper.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.setup_sdk(root)
    module.build_native(root)
    build_managed.build(root)
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    build = Path("/build/mele")
    sdk = build / f"sdk-{configuration['sdk']['version']}"
    version = configuration["sdk"]["runtime_version"]
    with tempfile.TemporaryDirectory(dir=output.parent, prefix="mele-package-") as temporary:
        stage = Path(temporary) / "helper"
        stage.mkdir()
        app = build / "source/Deployd.Mele/bin/Release/net10.0"
        copy_application(app, stage / "app")
        runtime = stage / "runtime"
        runtime.mkdir()
        shutil.copy2(sdk / "dotnet", runtime / "dotnet")
        copy_tree(sdk / "host/fxr" / version, runtime / "host/fxr" / version)
        copy_runtime(sdk / "shared/Microsoft.NETCore.App" / version,
                  runtime / "shared/Microsoft.NETCore.App" / version)
        notices = stage / "licenses"
        notices.mkdir()
        for name in ("LICENSE.txt", "ThirdPartyNotices.txt"):
            shutil.copy2(sdk / name, notices / f"dotnet-{name}")
        for license_file in (root / "helpers/mele/native/vendor/mem").glob("license_*.txt"):
            shutil.copy2(license_file, notices / license_file.name)
        shutil.copy2(root / "LICENSE", notices / "Deployd-GPL-3.0.txt")
        texts = []
        for package in license_inventory.locked_packages(root):
            name, resolved = package["name"].lower(), package["version"]
            archive = build / "nuget" / name / resolved / f"{name}.{resolved}.nupkg"
            data = archive.read_bytes()
            license_inventory.verify_archive(data, package, sdk / "dotnet")
            record = license_inventory.inspect_package(data, package)
            texts.append(json.dumps({key: value for key, value in record.items() if key != "files"}, indent=2))
        (notices / "NuGet-notices.json").write_text("[\n" + ",\n".join(texts) + "\n]\n")
        sources = stage / "source"
        sources.mkdir()
        for project in ("LegendaryExplorerCore", "LegendaryExplorerCore.SourceGenerators"):
            shutil.copytree(build / "source" / project, sources / project,
                            ignore=shutil.ignore_patterns("bin", "obj", "Embedded", "*.pcc", "*.upk", "*.u", "*.bik", "*.dll", "*.exe", "*.asi"))
            if project == "LegendaryExplorerCore":
                embedded = directory(sources / project / "Embedded")
                shutil.copy2(build / "source" / project / "Embedded/Infos.zip", embedded / "Infos.zip")
        shutil.copytree(root / "helpers/mele", sources / "helpers/mele",
                        ignore=shutil.ignore_patterns("__pycache__"))
        shutil.copy2(root / "scripts/mele-helper.py", directory(sources / "scripts") / "mele-helper.py")
        shutil.copy2(root / "LICENSE", sources / "LICENSE")
        validate_native_files(stage / "app")
        validate_native_files(runtime)
        subprocess.run([str(runtime / "dotnet"), str(stage / "app/Deployd.Mele.dll"), "capabilities"],
                       env=dict(os.environ, DOTNET_ROOT=str(runtime), DOTNET_MULTILEVEL_LOOKUP="0", DOTNET_EnableDiagnostics="0"), check=True)
        stage.rename(output)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: package.py ABSOLUTE_OUTPUT_DIRECTORY")
    assemble(Path(__file__).resolve().parents[2], Path(sys.argv[1]))
