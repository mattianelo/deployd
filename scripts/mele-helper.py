#!/usr/bin/env python3
"""Build and validate the native MELE adapter through check.sh."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile


sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "helpers/mele"))
from build_support import directory, download


def validate(arguments):
    if len(arguments) != 1 or arguments[0] not in {"env", "build", "test", "smoke", "community-patch", "community-patch-config", "community-patch-assets", "community-patch-scripts", "community-patch-m3m", "mele-scripts", "mele-plot", "merge-dlc", "shaders", "community-patch-startup", "community-patch-tlk", "community-patch-plot", "sdk", "lock", "audit", "licenses", "bundle"}:
        raise ValueError("usage: ./check.sh mele <env|build|test|smoke|community-patch|community-patch-config|community-patch-assets|community-patch-scripts|community-patch-m3m|mele-scripts|mele-plot|merge-dlc|shaders|community-patch-startup|community-patch-tlk|community-patch-plot|sdk|lock|audit|licenses|bundle>")
    if arguments[0] == "lock" and os.environ.get("DEPLOYD_DEPENDENCY_MAINTENANCE") != "1":
        raise ValueError("Helper lock generation requires DEPLOYD_DEPENDENCY_MAINTENANCE=1")
    return arguments[0]


def run(command):
    root = Path(__file__).resolve().parent.parent
    if os.getuid() != 1000 or os.getgid() != 1000 or os.environ.get("HOME") != "/home/ubuntu":
        raise ValueError("MELE validation requires the project's non-root container identity")
    if root != Path("/workspace") or os.environ.get("DEPLOYD_BUILD_CONTAINER") != "1":
        raise ValueError("Run MELE validation through ./check.sh")
    if command == "env":
        for tool in ("cc", "c++", "cmake", "python3", "dotnet"):
            print(f"{tool}: {'available' if shutil.which(tool) else 'unavailable'}")
        return
    if command == "sdk":
        setup_sdk(root)
        return
    if command == "bundle":
        from package import assemble
        subprocess.run([sys.executable, str(root / "helpers/mele/test_package.py")], check=True)
        with tempfile.TemporaryDirectory(dir=directory(Path("/build/mele")), prefix="bundle-test-") as temporary:
            assemble(root, Path(temporary) / "helper")
        return
    if command == "licenses":
        from license_inventory import collect
        collect(root)
        return
    build_native(root)
    if command in {"build", "smoke", "community-patch", "community-patch-config", "community-patch-assets", "community-patch-scripts", "community-patch-m3m", "mele-scripts", "mele-plot", "merge-dlc", "shaders", "community-patch-startup", "community-patch-tlk", "community-patch-plot", "lock", "test", "audit"}:
        setup_sdk(root)
        spec = importlib.util.spec_from_file_location("build_managed", root / "helpers/mele/build_managed.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        if command == "test":
            subprocess.run([sys.executable, str(root / "helpers/mele/test_license_inventory.py")], check=True)
            module.codec_tests(root)
            module.build(root)
            module.transformation_tests(root)
        else:
            module.build(root, refresh_lock=command == "lock")
        if command == "smoke":
            module.smoke(root)
        if command == "community-patch":
            from community_patch_test import run as community_patch_test
            community_patch_test(root, module.transformation_tests)
        if command == "community-patch-config":
            from community_patch_test import run_config
            run_config(root, module.transformation_tests)
        if command == "community-patch-assets":
            from community_patch_assets_test import run as run_assets
            run_assets(root, module.transformation_tests)
        if command == "community-patch-scripts":
            from community_patch_scripts_test import run as run_scripts
            run_scripts(root, module.transformation_tests)
        if command == "community-patch-m3m":
            from community_patch_m3m_test import run as run_m3m
            run_m3m(root, module.transformation_tests)
        if command == "shaders":
            from shader_test import run as run_shaders
            run_shaders(root, module.transformation_tests)
        if command == "merge-dlc":
            from merge_dlc_test import run as run_merge_dlc
            run_merge_dlc(root, module.transformation_tests)
        if command == "mele-plot":
            from legendary_plot_test import run as run_plot
            run_plot(root, module.transformation_tests)
        if command == "mele-scripts":
            from legendary_scripts_test import run as run_scripts
            run_scripts(root, module.transformation_tests)
        if command == "community-patch-startup":
            from community_patch_startup_test import run as run_startup
            run_startup(root, module.transformation_tests)
        if command in {"community-patch-tlk", "community-patch-plot"}:
            from community_patch_tlk_plot_test import run as run_tlk_plot
            run_tlk_plot(root, module.transformation_tests, command.rsplit("-", 1)[1])
        if command == "audit":
            module.audit(root)
    if command in {"test", "smoke"}:
        arguments = ["--with-codec"] if command == "smoke" else []
        subprocess.run([sys.executable, str(root / "helpers/mele/native/test_adapter.py"),
                        "/build/mele/native/libdeployd_oodle.so", *arguments], check=True)



def setup_sdk(root):
    spec = json.loads((root / "helpers/mele/toolchain.json").read_text())["sdk"]
    build = directory(Path("/build/mele"))
    sdk = build / f"sdk-{spec['version']}"
    if sdk.exists():
        if sdk.is_symlink() or (sdk / ".verified-archive").read_text().strip() != spec["sha512"]:
            raise ValueError("Existing helper SDK has an unrecognized identity")
        print("Pinned helper SDK already available", flush=True)
        return
    archive = build / f"sdk-{spec['version']}.tar.gz"
    download(archive, spec, "sha512")
    with tempfile.TemporaryDirectory(dir=build, prefix="sdk-stage-") as stage:
        with tarfile.open(archive) as bundle:
            bundle.extractall(stage, filter="data")
        (Path(stage) / ".verified-archive").write_text(spec["sha512"] + "\n")
        Path(stage).rename(sdk)
    subprocess.run([str(sdk / "dotnet"), "--version"], check=True)


def build_native(root):
    source = root / "helpers/mele/native"
    build = directory(Path("/build/mele/native"))
    if any(item.is_symlink() for item in build.rglob("*")):
        raise ValueError("Refusing a linked native-helper build directory")
    vendor = source / "vendor/mem"
    identities = json.loads((vendor / "sources.json").read_text())
    for entry in identities["files"] + identities.get("additional_licenses", []):
        if Path(entry["file"]).name != entry["file"]:
            raise ValueError("Invalid pinned source path")
        data = (vendor / entry["file"]).read_bytes()
        if len(data) != entry["size"] or hashlib.sha256(data).hexdigest() != entry["sha256"]:
            raise ValueError("Pinned MEM source failed integrity verification")
    spec = importlib.util.spec_from_file_location("port_mem", source / "port_mem.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    (build / "pe_linker.c").write_text(module.port((vendor / "pe_linker.c").read_text()))
    objects = []
    for name in ("pe_linker", "winapi", "log", "search_hsearch_r"):
        input_file = build / "pe_linker.c" if name == "pe_linker" else vendor / f"{name}.c"
        output = build / f"{name}.o"
        subprocess.run(["cc", "-std=gnu11", "-O2", "-fPIC", "-fvisibility=hidden", "-fshort-wchar",
                        "-DNDEBUG", "-Wno-multichar", "-I", str(vendor), "-c", str(input_file),
                        "-o", str(output)], check=True)
        objects.append(str(output))
    output = build / "libdeployd_oodle.so"
    subprocess.run(["c++", "-std=c++17", "-O2", "-fPIC", "-fvisibility=hidden", "-Wall", "-Wextra", "-Werror",
                    "-shared", str(source / "oodle_adapter.cpp"), *objects, "-ldl", "-pthread",
                    "-Wl,-z,relro,-z,now,-z,noexecstack", "-o", str(output)], check=True)
    print("Native MELE compression adapter built", flush=True)


def main():
    try:
        if len(sys.argv) < 3 or sys.argv[1] not in {"validate", "run"}:
            raise ValueError("MELE helper command requires validate or run mode")
        command = validate(sys.argv[2:])
        if sys.argv[1] == "run":
            run(command)
        return 0
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
