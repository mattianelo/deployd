"""Boundary tests for the helper wrapper, source pins, and build cache."""

import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import subprocess
import struct
import lzma
import tarfile
import tempfile
import unittest
from unittest import mock
import xml.etree.ElementTree as ET


ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "helpers/mele"))

import build_managed
import build_support
import community_patch_test
import community_patch_assets_test
import community_patch_scripts_test
import community_patch_m3m_test
import community_patch_startup_test
import community_patch_tlk_plot_test
import frozen_tlk
import frozen_m3m

SPEC = importlib.util.spec_from_file_location("mele_wrapper", ROOT / "scripts/mele-helper.py")
wrapper = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(wrapper)


class WrapperTests(unittest.TestCase):
    def test_accepts_only_bounded_helper_actions(self):
        for name in ("env", "sdk", "build", "test", "smoke", "community-patch", "community-patch-config", "community-patch-assets", "community-patch-scripts", "community-patch-m3m", "community-patch-startup", "community-patch-tlk", "community-patch-plot", "shaders", "audit", "licenses"):
            self.assertEqual(wrapper.validate([name]), name)
        for arguments in ([], ["run"], ["build", "--output", "/"], ["../build"], ["test", "extra"]):
            with self.assertRaises(ValueError):
                wrapper.validate(arguments)

    def test_lock_regeneration_requires_the_maintenance_marker(self):
        with mock.patch.dict(os.environ, {}, clear=True):
            with self.assertRaises(ValueError):
                wrapper.validate(["lock"])
        with mock.patch.dict(os.environ, {"DEPLOYD_DEPENDENCY_MAINTENANCE": "1"}):
            self.assertEqual(wrapper.validate(["lock"]), "lock")

    def test_rejects_container_root_before_running_a_build(self):
        with mock.patch.object(os, "getuid", return_value=0), mock.patch.object(wrapper, "build_native") as build:
            with self.assertRaises(ValueError):
                wrapper.run("build")
            build.assert_not_called()


class AssetCorpusTests(unittest.TestCase):
    def test_changed_asset_corpus_fails_before_starting_the_helper(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            folder = root / "docs/modTesting"
            folder.mkdir(parents=True)
            (folder / "input").write_bytes(b"changed")
            with mock.patch.object(community_patch_assets_test, "ASSET_INPUTS", {
                "asset": ("input", 7, hashlib.sha256(b"correct").hexdigest())
            }), mock.patch.object(community_patch_assets_test.subprocess, "run") as execute:
                with self.assertRaises(ValueError):
                    community_patch_assets_test.run(root, mock.Mock())
                execute.assert_not_called()

    def test_asset_completion_requires_every_output_and_exact_progress(self):
        with tempfile.TemporaryDirectory() as temporary:
            stage = Path(temporary)
            output = stage / "output"
            (output / "CookedPCConsole").mkdir(parents=True)
            (output / "CookedPCConsole/SFXGame.pcc").write_bytes(b"synthetic output")
            identity = community_patch_test.identity(output, "CookedPCConsole/SFXGame.pcc")
            request = {"output_root": str(output), "targets": [identity], "merges": [{}]}
            progress = {"protocol": 1, "type": "progress", "completed": 1, "total": 1}
            complete = {"protocol": 1, "type": "complete", "outputs": [identity]}
            result = subprocess.CompletedProcess([], 0, "\n".join(map(json.dumps, [progress, complete])), "")
            with mock.patch.object(community_patch_assets_test.subprocess, "run", return_value=result):
                community_patch_assets_test.transform(["helper"], stage, request, "valid")
                for messages in ([complete], [progress, dict(complete, outputs=[])],
                                 [dict(progress, completed=0), complete]):
                    result.stdout = "\n".join(map(json.dumps, messages))
                    with self.assertRaises(ValueError):
                        community_patch_assets_test.transform(["helper"], stage, request, "invalid")
                result.stdout = "\n".join(map(json.dumps, [progress, complete]))
                (output / "unexpected").write_bytes(b"extra")
                with self.assertRaises(ValueError):
                    community_patch_assets_test.transform(["helper"], stage, request, "extra")
                result.returncode = 1
                with self.assertRaises(ValueError):
                    community_patch_assets_test.transform(["helper"], stage, request, "failed")


class ScriptCorpusTests(unittest.TestCase):
    def test_changed_script_corpus_fails_before_starting_the_helper(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            folder = root / "docs/modTesting"
            folder.mkdir(parents=True)
            (folder / "input").write_bytes(b"changed")
            with mock.patch.object(community_patch_scripts_test, "SCRIPT_INPUTS", {
                "script": ("input", 7, hashlib.sha256(b"correct").hexdigest())
            }), mock.patch.object(community_patch_scripts_test.subprocess, "run") as execute:
                with self.assertRaises(ValueError):
                    community_patch_scripts_test.run(root, mock.Mock())
                execute.assert_not_called()

    def test_script_completion_requires_verified_output_and_progress(self):
        with tempfile.TemporaryDirectory() as temporary:
            stage = Path(temporary)
            output = stage / "output"
            (output / "CookedPCConsole").mkdir(parents=True)
            (output / "CookedPCConsole/SFXGame.pcc").write_bytes(b"synthetic output")
            identity = community_patch_test.identity(output, "CookedPCConsole/SFXGame.pcc")
            request = {"output_root": str(output), "target": identity, "merges": [{}]}
            progress = {"protocol": 1, "type": "progress", "completed": 1, "total": 1}
            complete = {"protocol": 1, "type": "complete", "outputs": [identity]}
            result = subprocess.CompletedProcess([], 0, "\n".join(map(json.dumps, [progress, complete])), "")
            with mock.patch.object(community_patch_scripts_test.subprocess, "run", return_value=result):
                community_patch_scripts_test.transform(["helper"], stage, request, "valid")
                for messages in ([complete], [progress, dict(complete, outputs=[])],
                                 [dict(progress, completed=0), complete]):
                    result.stdout = "\n".join(map(json.dumps, messages))
                    with self.assertRaises(ValueError):
                        community_patch_scripts_test.transform(["helper"], stage, request, "invalid")
                result.stdout = "\n".join(map(json.dumps, [progress, complete]))
                (output / "unexpected").write_bytes(b"extra")
                with self.assertRaises(ValueError):
                    community_patch_scripts_test.transform(["helper"], stage, request, "extra")
                result.returncode = 1
                with self.assertRaises(ValueError):
                    community_patch_scripts_test.transform(["helper"], stage, request, "failed", success=False)

    def test_frozen_scripts_preserve_function_and_member_order(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "example.m3m"
            change = {"entryname": "Example", "scriptupdate": {"scriptfilename": "Run.uc", "scripttext": "function Run() {}"},
                      "addtoclassorreplace": {"scriptfilenames": ["Value.uc"], "scripts": ["var int Value;"]}}
            manifest = {"game": "LE1", "files": [{"filename": "SFXGame.pcc", "applytoalllocalizations": False,
                                                   "changes": [change]}]}

            def write():
                data = (json.dumps(manifest) + "\0").encode("utf-16-le")
                source.write_bytes(b"M3MM\x01" + struct.pack("<i", -len(data) // 2) + data)

            write()
            scripts, merges = community_patch_scripts_test.frozen_scripts(source, root)
            self.assertEqual([item["kind"] for item in merges], ["function", "member"])
            self.assertEqual(len(scripts), 2)
            change["scriptupdate"]["scriptfilename"] = "../outside.uc"
            write()
            with self.assertRaises(ValueError):
                community_patch_scripts_test.frozen_scripts(source, root)


class OrderedCorpusTests(unittest.TestCase):
    def test_rejects_changed_corpus_before_running_helper(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            folder = root / "docs/modTesting"
            folder.mkdir(parents=True)
            (folder / "input").write_bytes(b"changed")
            with mock.patch.object(community_patch_m3m_test, "INPUTS", {
                "input": ("input", 7, hashlib.sha256(b"correct").hexdigest())
            }), mock.patch.object(community_patch_m3m_test.subprocess, "run") as execute:
                with self.assertRaises(ValueError):
                    community_patch_m3m_test.run(root, mock.Mock())
                execute.assert_not_called()

    def test_frozen_decoder_requires_complete_lzma_stream(self):
        content = b"synthetic script source"
        compressed = lzma.compress(content, format=lzma.FORMAT_ALONE)
        stored = compressed[:5] + compressed[13:]
        self.assertEqual(frozen_m3m.Reader(stored).compressed(len(content), len(stored)), content)
        for data, size in ((stored[:-1], len(content)), (stored + b"extra", len(content)),
                           (stored, len(content) + 1), (stored, len(content) - 1)):
            with self.assertRaises((ValueError, lzma.LZMAError)):
                frozen_m3m.Reader(data).compressed(size, len(data))

    def test_frozen_decoder_rejects_paths_and_trailing_data(self):
        def string(text):
            data = (text + "\0").encode("ascii")
            return struct.pack("<i", len(data)) + data

        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "synthetic.m3m"
            manifest = string(json.dumps({"game": "LE1", "files": []}))
            prefix = b"M3MM\x01" + manifest + struct.pack("<i", 1) + b"MMV1"
            path.write_bytes(prefix + string("Example.uc") + struct.pack("<i", 3) + b"abc")
            self.assertEqual(frozen_m3m.decode(path)[1], {"example.uc": b"abc"})
            valid = path.read_bytes()
            for data in (valid + b"extra", valid[:-1], prefix + string("../escape.uc") + struct.pack("<i", 3) + b"abc"):
                path.write_bytes(data)
                with self.assertRaises(ValueError):
                    frozen_m3m.decode(path)

    def test_ordered_completion_requires_exact_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            stage = Path(temporary)
            output = stage / "output"
            output.mkdir()
            (output / "file.pcc").write_bytes(b"output")
            identity = community_patch_test.identity(output, "file.pcc")
            request = {"output_root": str(output), "targets": [{"current": identity}], "jobs": [{}]}
            progress = {"protocol": 1, "type": "progress", "completed": 1, "total": 1}
            complete = {"protocol": 1, "type": "complete", "outputs": [identity]}
            result = subprocess.CompletedProcess([], 0, "\n".join(map(json.dumps, [progress, complete])), "")
            with mock.patch.object(community_patch_m3m_test.subprocess, "run", return_value=result):
                community_patch_m3m_test.transform(["helper"], stage, request, "valid")
                for messages in ([complete], [progress, dict(complete, outputs=[])], [dict(progress, completed=0), complete]):
                    result.stdout = "\n".join(map(json.dumps, messages))
                    with self.assertRaises(ValueError):
                        community_patch_m3m_test.transform(["helper"], stage, request, "invalid")
                result.returncode = 1
                with self.assertRaises(ValueError):
                    community_patch_m3m_test.transform(["helper"], stage, request, "failed", success=False)


class StartupCorpusTests(unittest.TestCase):
    def test_freezes_all_le1_voice_and_text_variants(self):
        self.assertEqual(list(community_patch_startup_test.STARTUPS),
                         ["DE", "ES", "FE", "FR", "GE", "IE", "INT", "IT", "JA", "PL", "PLPC", "RA", "RU"])

    def test_changed_startup_fails_before_transformation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            folder = root / "docs/modTesting"
            folder.mkdir(parents=True)
            (folder / "input").write_bytes(b"changed")
            with mock.patch.object(community_patch_startup_test, "CORPUS", {
                "input": ("input", 7, hashlib.sha256(b"correct").hexdigest())
            }), mock.patch.object(community_patch_startup_test, "transform") as execute:
                with self.assertRaises(ValueError):
                    community_patch_startup_test.run(root, mock.Mock())
                execute.assert_not_called()

    def test_localized_jobs_preserve_manifest_and_change_order(self):
        def decoded(name):
            count = 5 if name.stem == "HUDFixes" else 1
            changes = [{"entryname": f"{name.stem}.Export{index}", "assetupdate": {
                "assetname": "Asset.pcc", "entryname": f"Source{index}"}} for index in range(count)]
            return {"files": [{"filename": "Startup_INT.pcc", "applytoalllocalizations": True,
                               "changes": changes}]}, {"asset.pcc": b"synthetic asset"}

        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            sources = {name: Path(name) for name in ("HUDFixes.m3m", "DebugSaves.m3m")}
            target = "CookedPCConsole/Startup_PLPC.pcc"
            with mock.patch.object(community_patch_startup_test, "decode", side_effect=decoded):
                assets, jobs = community_patch_startup_test.stage_jobs(sources, root, target)
                self.assertEqual(len(assets), 2)
                self.assertEqual([job["entry"] for job in jobs],
                                 [f"HUDFixes.Export{index}" for index in range(5)] + ["DebugSaves.Export0"])
                self.assertTrue(all(job["target"] == target for job in jobs))
                for unknown in ("CookedPCConsole/Startup_XX.pcc", "../Startup_INT.pcc"):
                    with self.assertRaises(ValueError):
                        community_patch_startup_test.stage_jobs(sources, root, unknown)
            manifest, embedded = decoded(Path("HUDFixes.m3m"))
            manifest["files"][0]["applytoalllocalizations"] = False
            with mock.patch.object(community_patch_startup_test, "decode", return_value=(manifest, embedded)):
                with self.assertRaises(ValueError):
                    community_patch_startup_test.stage_jobs(sources, root, target)


class TlkPlotCorpusTests(unittest.TestCase):
    def test_frozen_tlk_decoder_preserves_text_and_targets(self):
        import lzma
        import struct

        def compressed(data):
            encoded = lzma.compress(data, format=lzma.FORMAT_ALONE)
            return encoded[:5] + encoded[13:]

        xml = '<tlkFile><string><id>42</id><data>  Café &amp; 雪\n </data></string></tlkFile>'.encode()
        payload = compressed(xml)
        header = struct.pack('<i', 1) + 'Example.Dialog.tlk.xml\0'.encode('utf-16-le')
        header += struct.pack('<iiiBB', 0, len(xml), len(payload), 255, 0)
        stored = compressed(header + bytes(16))
        data = b'CTMD\x02' + struct.pack('<ii', len(header), len(stored)) + stored + struct.pack('<i', len(payload)) + payload
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'input'
            path.write_bytes(data)
            self.assertEqual(frozen_tlk.decode(path), [{'target': 'CookedPCConsole/Example.pcc', 'export': 'Dialog.tlk',
                                                      'strings': [{'id': 42, 'data': '  Café & 雪\n '}]}])
            for invalid in [b'CTMD\x03' + data[5:], data[:-1], data + b'\0']:
                path.write_bytes(invalid)
                with self.assertRaises(ValueError):
                    frozen_tlk.decode(path)

    def test_changed_tlk_or_plot_corpus_never_starts_a_transformation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with mock.patch.object(community_patch_tlk_plot_test, 'verify_corpus', side_effect=ValueError('changed source')), \
                 mock.patch.object(community_patch_tlk_plot_test, 'transform') as execute:
                for kind in ['tlk', 'plot', 'unknown']:
                    with self.assertRaises(ValueError):
                        community_patch_tlk_plot_test.run(root, mock.Mock(), kind)
                execute.assert_not_called()

    def test_tlk_completion_rejects_missing_progress_and_unexpected_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            stage = Path(temporary)
            output = stage / 'out'
            (output / 'CookedPCConsole').mkdir(parents=True)
            relative = 'CookedPCConsole/Example.pcc'
            (output / relative).write_bytes(b'result')
            request = {'output_root': str(output), 'targets': [{'path': relative}]}
            messages = [{'protocol': 1, 'type': 'progress', 'completed': 1, 'total': 1},
                        {'protocol': 1, 'type': 'complete', 'outputs': [community_patch_test.identity(output, relative)]}]
            result = mock.Mock(returncode=0, stdout='\n'.join(json.dumps(message) for message in messages), stderr='')
            with mock.patch.object(community_patch_tlk_plot_test.subprocess, 'run', return_value=result):
                community_patch_tlk_plot_test.transform(['helper'], stage, request, 'valid', 'tlk', 1)
                (output / 'extra').write_bytes(b'unexpected')
                with self.assertRaises(ValueError):
                    community_patch_tlk_plot_test.transform(['helper'], stage, request, 'extra', 'tlk', 1)
                (output / 'extra').unlink()
                result.stdout = json.dumps(messages[-1])
                with self.assertRaises(ValueError):
                    community_patch_tlk_plot_test.transform(['helper'], stage, request, 'missing-progress', 'tlk', 1)


class CacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.spec = {"size": 4, "sha256": hashlib.sha256(b"data").hexdigest(), "url": "https://example.invalid/source"}

    def test_rejects_a_linked_parent_before_creating_any_output(self):
        outside = self.root / "outside"
        outside.mkdir()
        (self.root / "linked").symlink_to(outside, target_is_directory=True)
        with self.assertRaises(ValueError):
            build_support.directory(self.root / "linked/new-output")
        self.assertEqual(list(outside.iterdir()), [])

    def test_rejects_broad_and_relative_output_directories(self):
        for path in (Path("/"), Path("relative")):
            with self.assertRaises(ValueError):
                build_support.directory(path)

    def test_rejects_size_hash_and_link_mismatches(self):
        candidate = self.root / "candidate"
        for data in (b"dat", b"dataX", b"fake"):
            candidate.write_bytes(data)
            with self.assertRaises(ValueError):
                build_support.verify(candidate, self.spec)
        candidate.write_bytes(b"data")
        link = self.root / "link"
        link.symlink_to(candidate)
        with self.assertRaises(ValueError):
            build_support.verify(link, self.spec)

    def test_reuses_a_verified_archive_offline(self):
        archive = self.root / "archive"
        archive.write_bytes(b"data")
        with mock.patch.object(build_support.urllib.request, "urlopen") as request:
            build_support.download(archive, self.spec)
            request.assert_not_called()

    def test_publishes_only_a_complete_verified_download(self):
        archive = self.root / "archive"
        with mock.patch.object(build_support.urllib.request, "urlopen", return_value=io.BytesIO(b"data")):
            build_support.download(archive, self.spec)
        self.assertEqual(archive.read_bytes(), b"data")
        self.assertEqual(list(self.root.iterdir()), [archive])

    def test_failed_or_oversized_downloads_leave_no_cache_entry(self):
        archive = self.root / "archive"
        for data in (b"dat", b"dataX", b"fake"):
            with mock.patch.object(build_support.urllib.request, "urlopen", return_value=io.BytesIO(data)):
                with self.assertRaises(ValueError):
                    build_support.download(archive, self.spec)
            self.assertEqual(list(self.root.iterdir()), [])
        with mock.patch.object(build_support.urllib.request, "urlopen", side_effect=OSError("offline")):
            with self.assertRaises(OSError):
                build_support.download(archive, self.spec)
        self.assertEqual(list(self.root.iterdir()), [])

    def test_source_extraction_rejects_traversal_and_link_members(self):
        for suffix in ("../../escape", "linked"):
            archive = self.root / "legendary-explorer.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                member = tarfile.TarInfo("LegendaryExplorer-pin/LegendaryExplorer/LegendaryExplorerCore/" + suffix)
                if suffix == "linked":
                    member.type = tarfile.SYMTYPE
                    member.linkname = "../../escape"
                bundle.addfile(member)
            spec = {"commit": "pin", "size": archive.stat().st_size,
                    "sha256": hashlib.sha256(archive.read_bytes()).hexdigest()}
            with self.assertRaises(ValueError):
                build_managed.prepare_source(self.root, self.root, spec)
            self.assertFalse((self.root / "source").exists())
            self.assertEqual(list(self.root.iterdir()), [archive])

    def test_detects_modified_and_added_files_in_the_source_cache(self):
        archive = self.root / "legendary-explorer.tar.gz"
        data = b"pinned source"
        with tarfile.open(archive, "w:gz") as bundle:
            member = tarfile.TarInfo("LegendaryExplorer-pin/LegendaryExplorer/LegendaryExplorerCore/Source.cs")
            member.size = len(data)
            bundle.addfile(member, io.BytesIO(data))
        spec = {"commit": "pin", "size": archive.stat().st_size,
                "sha256": hashlib.sha256(archive.read_bytes()).hexdigest()}
        source = build_managed.prepare_source(self.root, self.root, spec)
        build_managed.verify_source(self.root, source, archive, spec)
        cached = source / "LegendaryExplorerCore/Source.cs"
        cached.write_bytes(b"changed data")
        with self.assertRaises(ValueError):
            build_managed.verify_source(self.root, source, archive, spec)
        cached.write_bytes(data)
        (cached.parent / "Injected.cs").write_bytes(b"new source")
        with self.assertRaisesRegex(ValueError, "Unexpected file"):
            build_managed.verify_source(self.root, source, archive, spec)


class PortTests(unittest.TestCase):
    def test_linux_projection_preserves_required_dependencies_and_resources(self):
        data = b'''<Project><ItemGroup>
            <ContentWithTargetPath Include="Windows/CompressionWrappers.dll" />
            <PackageReference Include="System.Buffers" Version="4.6.1" />
            <PackageReference Include="Newtonsoft.Json" Version="13.0.4" />
            <EmbeddedResource Include="Embedded/**" />
            <EmbeddedResource Include="Embedded/GameResources.zip" />
            </ItemGroup></Project>'''
        projected = ET.fromstring(build_managed.linux_project(data))
        self.assertEqual(projected.findall(".//ContentWithTargetPath"), [])
        self.assertEqual([node.get("Include") for node in projected.findall(".//PackageReference")], ["Newtonsoft.Json"])
        self.assertEqual([node.get("Include") for node in projected.findall(".//EmbeddedResource")], ["Embedded/Infos.zip"])

    def test_port_requires_the_pinned_native_source(self):
        spec = importlib.util.spec_from_file_location("port_mem", ROOT / "helpers/mele/native/port_mem.py")
        port = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(port)
        with self.assertRaises(ValueError):
            port.port("unrecognized source")

    def test_missing_audit_data_is_not_a_clean_report(self):
        for report in ({}, {"version": 1, "projects": []},
                       {"version": 1, "projects": [{}], "logs": [{"level": "error"}]}):
            with self.assertRaises(ValueError):
                build_managed.validate_audit_report(report)

    def test_a_reported_transitive_vulnerability_fails_the_audit(self):
        report = {"version": 1, "projects": [{"path": "helper.csproj", "frameworks": [{"transitivePackages": [
            {"id": "dependency", "vulnerabilities": [{"severity": "high"}]}
        ]}]}]}
        with self.assertRaisesRegex(ValueError, "vulnerability in dependency"):
            build_managed.validate_audit_report(report)

    def test_accepts_a_complete_clean_audit_report(self):
        build_managed.validate_audit_report({"version": 1, "projects": [{"path": "helper.csproj"}]})


class CorpusTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def test_rejects_a_linked_corpus_ancestor_without_reading_the_target(self):
        (self.root / "linked").symlink_to(self.root / "unavailable", target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "Linked corpus"):
            community_patch_test.regular_file(self.root / "linked/Engine.pcc")

    def test_changed_corpus_identity_fails_before_any_helper_call(self):
        source = self.root / "Engine.pcc"
        source.write_bytes(b"changed")
        expected = {"engine": (source.name, 7, hashlib.sha256(b"initial").hexdigest())}
        with mock.patch.object(community_patch_test, "INPUTS", expected), \
                mock.patch.object(subprocess, "run") as execute:
            with self.assertRaisesRegex(ValueError, "checksum"):
                community_patch_test.verify_corpus(self.root)
            execute.assert_not_called()
        self.assertEqual(source.read_bytes(), b"changed")

    def test_copy_does_not_share_mutable_source_storage(self):
        source = self.root / "source"
        source.write_bytes(b"original")
        destination = self.root / "stage/Engine.pcc"
        community_patch_test.copy(source, destination)
        destination.write_bytes(b"modified")
        self.assertEqual(source.read_bytes(), b"original")
        self.assertNotEqual(source.stat().st_ino, destination.stat().st_ino)

    def test_helper_failure_propagates_without_publishing_a_success(self):
        result = subprocess.CompletedProcess([], 1, "", "merge rejected")
        with mock.patch.object(subprocess, "run", return_value=result):
            with self.assertRaisesRegex(ValueError, "merge rejected"):
                community_patch_test.transform(["helper"], self.root, {}, "failure")

    def test_completion_requires_matching_progress_hash_and_file_inventory(self):
        output = self.root / "output"
        package = output / community_patch_test.TARGET
        package.parent.mkdir(parents=True)
        package.write_bytes(b"merged")
        request = {"contributions": [], "output_root": str(output)}
        report = {"protocol": 1, "type": "complete",
                  "outputs": [community_patch_test.identity(output, community_patch_test.TARGET)]}
        with mock.patch.object(subprocess, "run") as execute:
            execute.return_value = subprocess.CompletedProcess([], 0, json.dumps(report), "")
            community_patch_test.transform(["helper"], self.root, request, "valid")
            package.write_bytes(b"edited")
            with self.assertRaisesRegex(ValueError, "manifest disagrees"):
                community_patch_test.transform(["helper"], self.root, request, "hash")
            package.write_bytes(b"merged")
            (output / "unexpected").write_bytes(b"extra")
            with self.assertRaisesRegex(ValueError, "unexpected files"):
                community_patch_test.transform(["helper"], self.root, request, "inventory")
            execute.return_value.stdout = json.dumps({"protocol": 1, "type": "progress"}) + "\n" + json.dumps(report)
            with self.assertRaisesRegex(ValueError, "progress"):
                community_patch_test.transform(["helper"], self.root, request, "progress")


if __name__ == "__main__":
    unittest.main(verbosity=2)
