"""Collect license evidence from the exact NuGet archives in the helper locks."""

import base64
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET
import zipfile

from build_support import directory


def locked_packages(root):
    packages = {}
    for lock in sorted((root / "helpers/mele/locks").glob("*.json")):
        for dependencies in json.loads(lock.read_text())["dependencies"].values():
            for name, entry in dependencies.items():
                if entry["type"].lower() == "project":
                    continue
                version = entry["resolved"]
                if any(character not in "abcdefghijklmnopqrstuvwxyz0123456789.-" for character in (name + version).lower()):
                    raise ValueError("Invalid locked NuGet package identity")
                key = (name.lower(), version)
                record = packages.setdefault(key, {
                    "name": name, "version": version, "content_hash": entry["contentHash"], "projects": [],
                })
                if record["content_hash"] != entry["contentHash"]:
                    raise ValueError("Conflicting NuGet content hashes in helper locks")
                record["projects"].append(lock.stem)
    if not packages:
        raise ValueError("No locked helper packages were found")
    return [packages[key] for key in sorted(packages)]


def verify_archive(data, expected, sdk):
    if base64.b64encode(hashlib.sha512(data).digest()).decode() == expected["content_hash"]:
        return
    # NuGet's content hash excludes repository signatures; a raw ZIP digest does not.
    with tempfile.TemporaryDirectory(dir="/build/mele", prefix="license-verify-") as stage:
        archive = Path(stage) / "source.nupkg"
        archive.write_bytes(data)
        result = subprocess.run([str(sdk), "nuget", "verify", str(archive), "--all"],
                                capture_output=True, text=True, check=True)
        if expected["content_hash"] not in result.stdout.split():
            raise ValueError(f"NuGet verification did not report the locked content hash for {expected['name']}")


def inspect_package(data, expected):
    record = dict(expected, package_sha256=hashlib.sha256(data).hexdigest())
    with zipfile.ZipFile(io.BytesIO(data)) as bundle:
        names = bundle.namelist()
        record["files"] = names
        if len(names) != len(set(names)):
            raise ValueError("Ambiguous entries in NuGet license evidence")
        nuspecs = [name for name in names if name.lower().endswith(".nuspec") and "/" not in name]
        if len(nuspecs) != 1:
            raise ValueError("Expected one NuGet metadata file")
        document = ET.fromstring(bundle.read(nuspecs[0]))
        metadata = [node for node in document if node.tag.split("}")[-1] == "metadata"]
        if len(metadata) != 1:
            raise ValueError("Expected one NuGet metadata section")
        fields = {node.tag.split("}")[-1]: node for node in metadata[0]}
        if fields["id"].text.lower() != expected["name"].lower() or fields["version"].text != expected["version"]:
            raise ValueError("NuGet metadata differs from the locked identity")
        license_node = fields.get("license")
        record["license"] = None if license_node is None else {
            "type": license_node.get("type"), "value": license_node.text,
        }
        for key in ("licenseUrl", "projectUrl", "copyright"):
            if key in fields:
                record[key] = fields[key].text
        if "repository" in fields:
            record["repository"] = fields["repository"].attrib
        record["license_files"] = []
        for name in names:
            filename = name.rsplit("/", 1)[-1].lower()
            declared = license_node is not None and license_node.get("type") == "file" and name == license_node.text
            if not name.endswith("/") and (declared or any(word in filename for word in ("license", "notice", "copying"))):
                if bundle.getinfo(name).file_size > 2 * 1024 * 1024:
                    raise ValueError("Oversized NuGet license evidence")
                content = bundle.read(name)
                record["license_files"].append({
                    "name": name, "sha256": hashlib.sha256(content).hexdigest(),
                    "text": content.decode("utf-8-sig"),
                })
        if license_node is not None and license_node.get("type") == "file" and not any(
            item["name"] == license_node.text for item in record["license_files"]
        ):
            raise ValueError("NuGet archive omits its declared license file")
    return record


def collect(root):
    configuration = json.loads((root / "helpers/mele/toolchain.json").read_text())
    sdk = Path("/build/mele") / f"sdk-{configuration['sdk']['version']}" / "dotnet"
    records = []
    for package in locked_packages(root):
        name, version = package["name"].lower(), package["version"]
        archive = Path("/build/mele/nuget") / name / version / f"{name}.{version}.nupkg"
        if archive.is_symlink() or archive.stat().st_size > 128 * 1024 * 1024:
            raise ValueError("Invalid cached NuGet archive for license inspection")
        data = archive.read_bytes()
        verify_archive(data, package, sdk)
        record = inspect_package(data, package)
        record["package_url"] = f"https://api.nuget.org/v3-flatcontainer/{name}/{version}/{name}.{version}.nupkg"
        records.append(record)
    export = directory(root / ".ci-artifacts/mele-licenses") / "inventory.json"
    if export.is_symlink():
        raise ValueError("Refusing a linked license inventory export")
    with tempfile.NamedTemporaryFile(mode="w", dir=export.parent, delete=False) as output:
        temporary = Path(output.name)
        try:
            output.write(json.dumps({"schema": 1, "packages": records}, indent=2) + "\n")
            output.flush()
            temporary.replace(export)
        finally:
            temporary.unlink(missing_ok=True)
    for record in records:
        print(json.dumps({key: record[key] for key in ("name", "version", "license")}), flush=True)
    print(f"Collected license evidence for {len(records)} locked packages; this is not distribution clearance.")
