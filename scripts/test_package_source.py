#!/usr/bin/env python3
"""Packaging source boundaries, including repeated Snapcraft pull hooks."""

import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import package_source


ROOT = Path(__file__).resolve().parent.parent


class PackageSourceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.source = self.root / "project"
        self.source.mkdir()
        self.output = self.root / "build/source"
        for relative in package_source.INPUTS:
            path = self.source / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            if relative in {"src", "data", "helpers/mele"}:
                path.mkdir(exist_ok=True)
                (path / "input.txt").write_text("original")
            else:
                path.write_text("fixture")
        (self.source / "scripts/package_source.py").write_bytes((ROOT / "scripts/package_source.py").read_bytes())

    def tearDown(self):
        self.temporary.cleanup()

    def test_excludes_game_trees_caches_artifacts_and_private_files_without_reading_them(self):
        for directory in ("docs", "modTesting", ".git", "target", "out", "parts", ".ci-artifacts"):
            (self.source / directory).symlink_to(self.root / "unavailable", target_is_directory=True)
        (self.source / "local-secret").write_text("private")
        (self.source / "old.snap").write_text("artifact")
        count = package_source.copy_source(self.source, self.output)
        self.assertEqual(count, len(package_source.INPUTS))
        self.assertEqual((self.output / "src/input.txt").read_text(), "original")
        for name in ("docs", "modTesting", ".git", "target", "out", "parts", ".ci-artifacts", "local-secret", "old.snap"):
            self.assertFalse((self.output / name).exists())
            self.assertFalse((self.output / name).is_symlink())
        (self.output / "src/input.txt").write_text("changed copy")
        self.assertEqual((self.source / "src/input.txt").read_text(), "original")

    def test_repeated_copy_replaces_changed_inputs_and_removes_deleted_sources(self):
        (self.source / "src/deleted.rs").write_text("old")
        package_source.copy_source(self.source, self.output)
        (self.source / "src/deleted.rs").unlink()
        (self.source / "src/input.txt").write_text("new")
        package_source.copy_source(self.source, self.output)
        self.assertFalse((self.output / "src/deleted.rs").exists())
        self.assertEqual((self.output / "src/input.txt").read_text(), "new")

    def test_missing_input_preserves_the_previous_source_copy(self):
        package_source.copy_source(self.source, self.output)
        (self.source / "Cargo.lock").unlink()
        with self.assertRaises(OSError):
            package_source.copy_source(self.source, self.output)
        self.assertTrue((self.output / "Cargo.lock").is_file())

    def test_container_root_cannot_publish_through_the_workspace(self):
        with patch.object(package_source.os, "getuid", return_value=0):
            with self.assertRaisesRegex(ValueError, "shared workspace"):
                package_source.copy_source(self.source, Path("/workspace/parts/deployd/src"))

    def test_rejects_linked_inputs_before_publication(self):
        (self.source / "src/escape").symlink_to(self.root)
        with self.assertRaises(ValueError):
            package_source.copy_source(self.source, self.output)
        self.assertFalse(self.output.exists())

    def test_rejects_linked_input_ancestors(self):
        (self.source / "helpers").rename(self.source / "actual-helpers")
        (self.source / "helpers").symlink_to(self.source / "actual-helpers")
        with self.assertRaises(ValueError):
            package_source.copy_source(self.source, self.output)
        self.assertFalse(self.output.exists())

    def test_rejects_unowned_and_overlapping_destinations(self):
        self.output.mkdir(parents=True)
        (self.output / "user-file").write_text("preserve")
        for destination in (self.output, self.source, self.root, self.source / "src/build"):
            with self.assertRaises(ValueError):
                package_source.copy_source(self.source, destination)
        self.assertEqual((self.output / "user-file").read_text(), "preserve")

    def test_rejects_linked_destination_and_marker(self):
        self.output.parent.mkdir(parents=True)
        self.output.symlink_to(self.source, target_is_directory=True)
        with self.assertRaises(ValueError):
            package_source.copy_source(self.source, self.output)
        self.output.unlink()
        self.output.mkdir()
        (self.output / package_source.MARKER).symlink_to(self.source / "Cargo.toml")
        with self.assertRaises(ValueError):
            package_source.copy_source(self.source, self.output)

    def test_failed_copy_keeps_previous_sources(self):
        package_source.copy_source(self.source, self.output)
        with patch.object(package_source.shutil, "copy2", side_effect=OSError("disk full")):
            with self.assertRaises(OSError):
                package_source.copy_source(self.source, self.output)
        self.assertEqual((self.output / "src/input.txt").read_text(), "original")

    def test_snap_pull_hook_uses_the_same_filter_on_initial_and_updated_sources(self):
        for recipe_name in ("snapcraft.yaml", "snapcraft-dev.yaml"):
            with self.subTest(recipe=recipe_name):
                recipe = (ROOT / "snap" / recipe_name).read_text()
                hook = re.search(r"    override-pull: \|\n((?:      [^\n]*\n)+)", recipe)
                self.assertIsNotNone(hook)
                command = "\n".join(line[6:] for line in hook[1].splitlines())
                self.output = self.source / f"parts/{recipe_name}/src"
                self.output.mkdir(parents=True)
                environment = dict(
                    os.environ,
                    CRAFT_PROJECT_DIR=str(self.source),
                    CRAFT_PART_SRC=str(self.output),
                )
                docs = self.source / "docs"
                if not docs.exists() and not docs.is_symlink():
                    docs.symlink_to(self.root / "unavailable")
                for value in ("first", "second"):
                    (self.source / "src/input.txt").write_text(value)
                    subprocess.run(
                        ["bash", "-eu", "-c", command],
                        cwd=self.output,
                        env=environment,
                        check=True,
                        capture_output=True,
                    )
                    self.assertEqual((self.output / "src/input.txt").read_text(), value)
                    self.assertFalse((self.output / "docs").exists())


if __name__ == "__main__":
    unittest.main()
