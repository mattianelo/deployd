"""Project pinned Legendary Explorer sources into a Linux console build."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import xml.etree.ElementTree as ET

from build_support import directory, download, verify


def prepare_source(root, build, spec):
    archive = build / "legendary-explorer.tar.gz"
    if not archive.exists():
        cached = root / ".ci-artifacts/mele-sources/legendary-explorer.tar.gz"
        if cached.exists():
            verify(cached, spec)
            shutil.copyfile(cached, archive)
        else:
            download(archive, spec)
    verify(archive, spec)
    source = build / "source"
    if source.exists():
        if source.is_symlink() or (source / ".verified-archive").read_text().strip() != spec["sha256"]:
            raise ValueError("Unrecognized Legendary Explorer source directory")
        return source
    prefix = f"LegendaryExplorer-{spec['commit']}/LegendaryExplorer/"
    projects = {"LegendaryExplorerCore", "LegendaryExplorerCore.SourceGenerators"}
    with tempfile.TemporaryDirectory(dir=build, prefix="source-stage-") as stage:
        with tarfile.open(archive) as bundle:
            for member in bundle:
                if not member.name.startswith(prefix):
                    continue
                relative = Path(member.name[len(prefix):])
                if not relative.parts or relative.parts[0] not in projects:
                    continue
                if ".." in relative.parts or relative.is_absolute() or not (member.isfile() or member.isdir()):
                    raise ValueError("Unsafe pinned source archive member")
                member.name = str(relative)
                bundle.extract(member, stage, filter="data")
        (Path(stage) / ".verified-archive").write_text(spec["sha256"] + "\n")
        Path(stage).rename(source)
    return source


def write_changed(path, data):
    if not path.exists() or path.read_bytes() != data:
        path.write_bytes(data)


def linux_project(data):
    project = ET.fromstring(data)
    for group in project:
        for item in list(group):
            if item.tag == "EmbeddedResource" and item.get("Include") is not None:
                group.remove(item)
                continue
            if item.tag == "ContentWithTargetPath" or (
                item.tag == "PackageReference" and item.get("Include") in {"Microsoft.Win32.Registry", "System.Buffers"}
            ):
                group.remove(item)
    resources = ET.SubElement(project, "ItemGroup")
    ET.SubElement(resources, "EmbeddedResource", Include="Embedded/Infos.zip")
    return ET.tostring(project, encoding="utf-8")


def verify_source(root, source, archive, spec):
    prefix = f"LegendaryExplorer-{spec['commit']}/LegendaryExplorer/"
    projects = {"LegendaryExplorerCore", "LegendaryExplorerCore.SourceGenerators"}
    expected = set()
    with tarfile.open(archive) as bundle:
        for member in bundle:
            if not member.isfile() or not member.name.startswith(prefix):
                continue
            relative = Path(member.name[len(prefix):])
            if not relative.parts or relative.parts[0] not in projects:
                continue
            if relative.is_absolute() or ".." in relative.parts:
                raise ValueError("Unsafe pinned source archive member")
            expected.add(relative)
            with bundle.extractfile(member) as stream:
                data = stream.read()
            path = source / relative
            if path.is_symlink():
                raise ValueError("A cached helper source became a symbolic link")
            if relative == Path("LegendaryExplorerCore/LegendaryExplorerCore.csproj"):
                write_changed(path, linux_project(data))
            elif relative == Path("LegendaryExplorerCore/Compression/OodleHelper.cs"):
                write_changed(path, (root / "helpers/mele/managed/OodleHelper.cs").read_bytes())
            else:
                verify(path, {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    for project in projects:
        for path in (source / project).rglob("*"):
            relative = path.relative_to(source)
            if path.is_file() and relative not in expected and not (
                relative.parts[1] in {"bin", "obj"} or relative == Path(project) / "packages.lock.json"
            ):
                raise ValueError("Unexpected file in the pinned helper source cache")


def build(root, refresh_lock=False):
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    build = directory(Path("/build/mele"))
    source = prepare_source(root, build, configuration["legendary_explorer"])
    if any(path.is_symlink() for path in source.rglob("*")):
        raise ValueError("Refusing linked helper source or build output")
    verify_source(root, source, build / "legendary-explorer.tar.gz", configuration["legendary_explorer"])
    helper = source / "Deployd.Mele"
    helper.mkdir(exist_ok=True)
    for name in ("Deployd.Mele.csproj", "Program.cs", "TransformProtocol.cs", "M3daMerge.cs", "M3cdMerge.cs", "Le1Config.cs", "M3mAssets.cs", "M3mScripts.cs", "M3mClasses.cs", "M3mOrdered.cs", "PackageOutput.cs", "TlkMerge.cs", "PlotMerge.cs", "ShaderMerge.cs", "GlobalShaderFile.cs", "MergeDlcConfig.cs", "EmailGraph.cs", "SquadStreaming.cs", "MergeStartup.cs", "MergeDlcInputs.cs", "MergeDlc.cs", "SquadUi.cs"):
        write_changed(helper / name, (root / "helpers/mele/managed" / name).read_bytes())
    projects = ("LegendaryExplorerCore", "LegendaryExplorerCore.SourceGenerators", "Deployd.Mele")
    if not refresh_lock:
        for name in projects:
            write_changed(source / name / "packages.lock.json",
                          (root / "helpers/mele/locks" / f"{name}.json").read_bytes())
    sdk = build / f"sdk-{configuration['sdk']['version']}" / "dotnet"
    environment = dict(os.environ, DOTNET_CLI_TELEMETRY_OPTOUT="1", DOTNET_SKIP_FIRST_TIME_EXPERIENCE="1",
                       DOTNET_NOLOGO="1", NUGET_PACKAGES="/build/mele/nuget")
    subprocess.run([str(sdk), "build", str(helper / "Deployd.Mele.csproj"), "-c", "Release",
                    "--nologo", "-v", "minimal", "-p:RestorePackagesWithLockFile=true",
                    f"-p:RestoreLockedMode={'false' if refresh_lock else 'true'}"],
                   env=environment, check=True)
    if refresh_lock:
        export = directory(root / ".ci-artifacts/mele-locks")
        if export.resolve() != export or any(path.is_symlink() for path in export.rglob("*")):
            raise ValueError("Refusing a linked helper lock export directory")
        for name in projects:
            shutil.copyfile(source / name / "packages.lock.json", export / f"{name}.json")
    output = helper / "bin/Release/net10.0"
    shutil.copyfile(build / "native/libdeployd_oodle.so", output / "libdeployd_oodle.so")
    subprocess.run([str(sdk), str(output / "Deployd.Mele.dll"), "capabilities"], env=environment, check=True)


def smoke(root):
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    sdk = Path("/build/mele") / f"sdk-{configuration['sdk']['version']}" / "dotnet"
    helper = Path("/build/mele/source/Deployd.Mele/bin/Release/net10.0/Deployd.Mele.dll")
    fixtures = root / ".ci-artifacts/mele-fixtures"
    codec = fixtures / "oo2core_8_win64.dll"
    packages = json.loads((fixtures / "packages.json").read_text())
    with tempfile.TemporaryDirectory(dir="/build/mele", prefix="smoke-") as temporary:
        stage = Path(temporary)
        game = stage / "game"
        binary = game / "Binaries/Win64/oo2core_8_win64.dll"
        binary.parent.mkdir(parents=True)
        shutil.copyfile(codec, binary)
        codec_tests(root, game)
        transformation_tests(root, game)
        for index, entry in enumerate(packages):
            if entry["game"] not in {"ME1", "ME2", "ME3"}:
                raise ValueError("Unrecognized fixture game")
            name = entry["fixture"]
            if Path(name).name != name or not name.endswith(".pcc"):
                raise ValueError("Invalid package fixture filename")
            input_file = fixtures / entry["game"] / name
            verify(input_file, entry)
            output = stage / f"{index}.pcc"
            result = subprocess.run([str(sdk), str(helper), "verify-roundtrip", str(game), str(input_file),
                                     str(output)], capture_output=True, text=True, check=True)
            report = json.loads(result.stdout)
            expected = {"ME1": "LE1", "ME2": "LE2", "ME3": "LE3"}[entry["game"]]
            if report.get("game") != expected or report.get("type") != "validated" or report.get("protocol") != 1:
                raise ValueError("Package round trip reported an unexpected game or result")
            verify(output, report)
            print(result.stdout.strip(), flush=True)
            verify(input_file, entry)
            collision = subprocess.run([str(sdk), str(helper), "verify-roundtrip", str(game), str(input_file),
                                        str(output)], capture_output=True, text=True)
            if collision.returncode == 0:
                raise ValueError("Helper accepted an existing output file")
            verify(output, report)
        if binary.read_bytes() != codec.read_bytes():
            raise ValueError("Codec source changed during the package round trip")
        binary.write_bytes(bytes(1007616))
        invalid_output = stage / "invalid.pcc"
        rejected = subprocess.run([str(sdk), str(helper), "verify-roundtrip", str(game), str(input_file),
                                   str(invalid_output)], capture_output=True, text=True)
        if rejected.returncode == 0 or invalid_output.exists():
            raise ValueError("An unverified codec produced a helper output")


def codec_tests(root, game=None):
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    sdk = Path("/build/mele") / f"sdk-{configuration['sdk']['version']}" / "dotnet"
    project = directory(Path("/build/mele/codec-tests"))
    if project.resolve() != project or any(path.is_symlink() for path in project.rglob("*")):
        raise ValueError("Refusing a linked managed-test directory")
    for name in ("CodecTests.csproj", "CodecTests.cs", "OodleHelper.cs"):
        write_changed(project / name, (root / "helpers/mele/managed" / name).read_bytes())
    environment = dict(os.environ, DOTNET_CLI_TELEMETRY_OPTOUT="1", DOTNET_SKIP_FIRST_TIME_EXPERIENCE="1",
                       DOTNET_NOLOGO="1", NUGET_PACKAGES="/build/mele/nuget")
    subprocess.run([str(sdk), "build", str(project / "CodecTests.csproj"), "-c", "Release", "--nologo"],
                   env=environment, check=True)
    output = project / "bin/Release/net10.0"
    shutil.copyfile("/build/mele/native/libdeployd_oodle.so", output / "libdeployd_oodle.so")
    arguments = [str(game)] if game is not None else []
    subprocess.run([str(sdk), str(output / "CodecTests.dll"), *arguments], env=environment, check=True)


def transformation_tests(root, game=None, corpus=None, corpus_kind="m3da"):
    if corpus_kind not in {"m3da", "m3cd", "m3m-assets", "m3m-scripts", "m3m-ordered", "m3m-startup", "tlk", "plot", "merge-dlc", "shaders"} or (game is not None and corpus is not None):
        raise ValueError("Invalid transformation test selection")
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    sdk = Path("/build/mele") / f"sdk-{configuration['sdk']['version']}" / "dotnet"
    project = directory(Path("/build/mele/source/TransformationTests"))
    if project.resolve() != project or any(path.is_symlink() for path in project.rglob("*")):
        raise ValueError("Refusing a linked transformation-test directory")
    for name in ("TransformationTests.csproj", "TransformationTests.cs", "CommunityPatchTests.cs", "TransformProtocol.cs",
                 "M3daMerge.cs", "M3cdMerge.cs", "Le1Config.cs", "M3mAssets.cs", "M3mScripts.cs", "M3mClasses.cs", "M3mOrdered.cs", "PackageOutput.cs", "TlkMerge.cs", "PlotMerge.cs", "ShaderMerge.cs", "GlobalShaderFile.cs", "MergeDlcConfig.cs", "EmailGraph.cs", "SquadStreaming.cs", "MergeStartup.cs", "MergeDlcInputs.cs", "MergeDlc.cs", "SquadUi.cs", "ConfigTests.cs", "AssetMergeTests.cs", "ScriptMergeTests.cs", "OrderedM3mTests.cs", "TlkPlotTests.cs", "PlotCorpusTests.cs", "MergeDlcTests.cs", "ShaderMergeTests.cs"):
        write_changed(project / name, (root / "helpers/mele/managed" / name).read_bytes())
    write_changed(project / "packages.lock.json", (root / "helpers/mele/locks/Deployd.Mele.json").read_bytes())
    environment = dict(os.environ, DOTNET_CLI_TELEMETRY_OPTOUT="1", DOTNET_SKIP_FIRST_TIME_EXPERIENCE="1",
                       DOTNET_NOLOGO="1", NUGET_PACKAGES="/build/mele/nuget")
    subprocess.run([str(sdk), "build", str(project / "TransformationTests.csproj"), "-c", "Release", "--nologo",
                    "-p:RestorePackagesWithLockFile=true", "-p:RestoreLockedMode=true"], env=environment, check=True)
    output = project / "bin/Release/net10.0"
    shutil.copyfile("/build/mele/native/libdeployd_oodle.so", output / "libdeployd_oodle.so")
    flag = {"shaders": "--shaders", "merge-dlc": "--merge-dlc", "tlk": "--community-patch-tlk", "plot": "--community-patch-plot", "m3da": "--community-patch", "m3cd": "--community-patch-config",
            "m3m-startup": "--community-patch-startup", "m3m-ordered": "--community-patch-m3m", "m3m-scripts": "--community-patch-scripts", "m3m-assets": "--community-patch-assets"}[corpus_kind]
    arguments = [flag, str(corpus)] if corpus else ([str(game)] if game else [])
    subprocess.run([str(sdk), str(output / "TransformationTests.dll"), *arguments],
                   env=environment, check=True)


def audit(root):
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    sdk = Path("/build/mele") / f"sdk-{configuration['sdk']['version']}" / "dotnet"
    projects = ("LegendaryExplorerCore", "LegendaryExplorerCore.SourceGenerators", "Deployd.Mele")
    for name in projects:
        project = Path("/build/mele/source") / name / f"{name}.csproj"
        result = subprocess.run([str(sdk), "package", "list", "--project", str(project),
                                 "--no-restore", "--vulnerable", "--include-transitive", "--format", "json",
                                 "--output-version", "1"], capture_output=True, text=True, check=True)
        report = json.loads(result.stdout)
        validate_audit_report(report)
        print(f"{name}: no known NuGet vulnerabilities reported", flush=True)


def validate_audit_report(report):
    if report.get("version") != 1 or not report.get("projects") or report.get("logs"):
        raise ValueError("NuGet audit did not produce a complete clean report")
    for project in report["projects"]:
        if not project.get("path") or project.get("logs"):
            raise ValueError("NuGet audit could not check a helper project")
        for framework in project.get("frameworks", []):
            for category in ("topLevelPackages", "transitivePackages"):
                for package in framework.get(category, []):
                    if package.get("vulnerabilities"):
                        raise ValueError(f"NuGet reported a vulnerability in {package['id']}")
