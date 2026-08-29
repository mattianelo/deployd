#!/usr/bin/env python3

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

import release_metadata


VERSION = "2.4.0"


class ReleaseMetadataTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "snap").mkdir()
        (self.root / "data").mkdir()
        self._write_valid_metadata()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def _write_valid_metadata(self) -> None:
        (self.root / "Cargo.toml").write_text(
            f'[package]\nname = "deployd"\nversion = "{VERSION}"\n'
        )
        (self.root / "Cargo.lock").write_text(
            f'[[package]]\nname = "deployd"\nversion = "{VERSION}"\n'
        )
        (self.root / "snap" / "snapcraft.yaml").write_text(
            f"name: deployd\nversion: '{VERSION}'\nadopt-info: deployd\n"
        )
        (self.root / "data" / "io.mattianelo.deployd.metainfo.xml").write_text(
            "<component><releases>"
            f'<release version="{VERSION}" date="2026-08-29" />'
            '<release version="2.3.3" date="2026-07-30" />'
            "</releases></component>"
        )
        (self.root / "README.md").write_text(
            f"![Version](https://img.shields.io/badge/version-{VERSION}-blue)\n"
            f"> **v{VERSION}** Current release.\n"
        )
        (self.root / "CHANGELOG.md").write_text(
            f"# Changelog\n\n## [Unreleased]\n\n## [{VERSION}]\n"
        )

    def _replace(self, relative: str, old: str, new: str) -> None:
        path = self.root / relative
        path.write_text(path.read_text().replace(old, new, 1))

    def test_accepts_exact_stable_release_tag(self) -> None:
        metadata = release_metadata.validate_metadata(self.root, f"v{VERSION}")

        self.assertTrue(metadata.release)
        self.assertEqual(metadata.version, VERSION)

    def test_tagless_preflight_validates_current_metadata(self) -> None:
        metadata = release_metadata.validate_metadata(self.root)

        self.assertFalse(metadata.release)
        self.assertEqual(metadata.version, VERSION)

    def test_unrelated_tags_are_non_releases(self) -> None:
        for tag in ("v2.4.0-beta", "v2.4.0suffix", "v2.4", "nightly"):
            with self.subTest(tag=tag):
                metadata = release_metadata.validate_metadata(self.root, tag)
                self.assertFalse(metadata.release)
                self.assertEqual(metadata.version, "")

    def test_rejects_tag_that_disagrees_with_package(self) -> None:
        with self.assertRaisesRegex(release_metadata.MetadataError, "Cargo.toml"):
            release_metadata.validate_metadata(self.root, "v2.4.1")

    def test_rejects_lockfile_version_drift(self) -> None:
        self._replace("Cargo.lock", VERSION, "2.3.3")

        with self.assertRaisesRegex(release_metadata.MetadataError, "Cargo.lock"):
            release_metadata.validate_metadata(self.root)

    def test_rejects_snap_version_drift(self) -> None:
        self._replace("snap/snapcraft.yaml", VERSION, "2.3.3")

        with self.assertRaisesRegex(release_metadata.MetadataError, "snap/snapcraft.yaml"):
            release_metadata.validate_metadata(self.root)

    def test_rejects_appstream_adopt_info_drift(self) -> None:
        self._replace("data/io.mattianelo.deployd.metainfo.xml", VERSION, "2.3.3")

        with self.assertRaisesRegex(release_metadata.MetadataError, "AppStream newest release"):
            release_metadata.validate_metadata(self.root)

    def test_rejects_readme_badge_drift(self) -> None:
        self._replace("README.md", f"version-{VERSION}", "version-2.3.3")

        with self.assertRaisesRegex(release_metadata.MetadataError, "README version badge"):
            release_metadata.validate_metadata(self.root)

    def test_rejects_readme_announcement_drift(self) -> None:
        self._replace("README.md", f"v{VERSION}", "v2.3.3")

        with self.assertRaisesRegex(release_metadata.MetadataError, "README release announcement"):
            release_metadata.validate_metadata(self.root)

    def test_rejects_missing_changelog_release(self) -> None:
        self._replace("CHANGELOG.md", f"## [{VERSION}]", "## [2.3.3]")

        with self.assertRaisesRegex(release_metadata.MetadataError, "CHANGELOG.md"):
            release_metadata.validate_metadata(self.root)

    def test_writes_github_outputs_without_credentials(self) -> None:
        output = self.root / "github-output"
        status = release_metadata.main(
            ["--root", str(self.root), "--tag", f"v{VERSION}", "--github-output", str(output)]
        )

        self.assertEqual(status, 0)
        self.assertEqual(output.read_text(), f"release=true\nversion={VERSION}\n")


if __name__ == "__main__":
    unittest.main()
