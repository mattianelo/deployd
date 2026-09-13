"""Exercise license evidence collection without using external packages or services."""

import base64
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock
import zipfile

import license_inventory


class InventoryTests(unittest.TestCase):
    def package(self, license_xml='<license type="expression">MIT</license>', files=None):
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w") as archive:
            archive.writestr("example.nuspec", '<package><metadata><id>Example</id><version>1.0.0</version>'
                             + license_xml + '</metadata></package>')
            for name, text in (files or {}).items():
                archive.writestr(name, text)
        data = output.getvalue()
        return data, {"name": "Example", "version": "1.0.0", "projects": ["helper"],
                      "content_hash": base64.b64encode(hashlib.sha512(data).digest()).decode()}

    def test_preserves_a_declared_license_and_its_evidence_hash(self):
        data, expected = self.package('<license type="file">LICENSE</license>', {"LICENSE": "license evidence"})
        record = license_inventory.inspect_package(data, expected)
        self.assertEqual(record["license"], {"type": "file", "value": "LICENSE"})
        self.assertEqual(record["license_files"][0]["text"], "license evidence")
        self.assertEqual(record["license_files"][0]["sha256"], hashlib.sha256(b"license evidence").hexdigest())

    def test_does_not_assign_a_license_when_metadata_omits_it(self):
        data, expected = self.package("")
        self.assertIsNone(license_inventory.inspect_package(data, expected)["license"])

    def test_rejects_a_missing_declared_license_file(self):
        data, expected = self.package('<license type="file">LICENSE</license>')
        with self.assertRaisesRegex(ValueError, "omits its declared license"):
            license_inventory.inspect_package(data, expected)

    def test_rejects_metadata_for_a_different_package_version(self):
        data, expected = self.package()
        expected["version"] = "2.0.0"
        with self.assertRaisesRegex(ValueError, "locked identity"):
            license_inventory.inspect_package(data, expected)

    def test_accepts_an_exact_unsigned_archive_without_a_subprocess(self):
        data, expected = self.package()
        with mock.patch.object(subprocess, "run") as run:
            license_inventory.verify_archive(data, expected, Path("/sdk/dotnet"))
            run.assert_not_called()

    def test_requires_signed_archive_verification_and_an_exact_content_hash(self):
        data, expected = self.package()
        expected["content_hash"] = "locked-content-hash"
        real_temporary = tempfile.TemporaryDirectory
        def temporary(**kwargs):
            return real_temporary()
        with mock.patch.object(license_inventory.tempfile, "TemporaryDirectory", side_effect=temporary):
            for output in ("", "prefix-locked-content-hash", "Content hash: different"):
                with mock.patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0, output)):
                    with self.assertRaises(ValueError):
                        license_inventory.verify_archive(data, expected, Path("/sdk/dotnet"))
            with mock.patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "Content hash: locked-content-hash\n")) as run:
                license_inventory.verify_archive(data, expected, Path("/sdk/dotnet"))
                self.assertTrue(run.call_args.kwargs["check"])
            with mock.patch.object(subprocess, "run", side_effect=subprocess.CalledProcessError(1, "verify")):
                with self.assertRaises(subprocess.CalledProcessError):
                    license_inventory.verify_archive(data, expected, Path("/sdk/dotnet"))

    def test_rejects_conflicting_package_hashes_between_projects(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            locks = root / "helpers/mele/locks"
            locks.mkdir(parents=True)
            for name, digest in (("one", "first"), ("two", "second")):
                (locks / f"{name}.json").write_text(json.dumps({"dependencies": {"net10.0": {
                    "Example": {"type": "Direct", "resolved": "1.0.0", "contentHash": digest}
                }}}))
            with self.assertRaisesRegex(ValueError, "Conflicting NuGet"):
                license_inventory.locked_packages(root)


if __name__ == "__main__":
    unittest.main(verbosity=2)
