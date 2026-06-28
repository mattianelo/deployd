#!/usr/bin/env python3

from __future__ import annotations

import tempfile
import unittest
import zipfile
from pathlib import Path

import package_nexus_release


class PackageNexusReleaseTests(unittest.TestCase):
    def test_archive_contains_only_the_executable_appimage(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            appimage = root / "Deployd-2.3.1-x86_64.AppImage"
            appimage.write_bytes(b"appimage")
            appimage.chmod(0o755)
            output = root / "Deployd-2.3.1-x86_64.zip"

            package_nexus_release.create_archive(appimage, output)

            with zipfile.ZipFile(output) as archive:
                self.assertEqual(archive.namelist(), [appimage.name])
                self.assertEqual(archive.read(appimage.name), b"appimage")
                archived_mode = archive.getinfo(appimage.name).external_attr >> 16
                self.assertEqual(archived_mode & 0o777, 0o755)

    def test_rejects_non_zip_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            appimage = root / "Deployd.AppImage"
            appimage.write_bytes(b"appimage")

            with self.assertRaisesRegex(
                package_nexus_release.PackagingError, "must end in .zip"
            ):
                package_nexus_release.create_archive(
                    appimage, root / "Deployd.AppImage"
                )


if __name__ == "__main__":
    unittest.main()
