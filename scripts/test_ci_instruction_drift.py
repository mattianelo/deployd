#!/usr/bin/env python3
import sys
import tempfile
import unittest
from pathlib import Path

sys.dont_write_bytecode = True

from ci_instruction_drift import REQUIRED_FILES, instruction_issues


class InstructionDriftTests(unittest.TestCase):
    def setUp(self) -> None:
        self._temp = tempfile.TemporaryDirectory()
        self.root = Path(self._temp.name)
        for relative in REQUIRED_FILES:
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("")

    def tearDown(self) -> None:
        self._temp.cleanup()

    def test_accepts_current_instruction_paths(self) -> None:
        (self.root / "README.md").write_text(
            "Run `./check.sh check`; implementation lives in "
            "`scripts/rust-command.sh`.\n"
        )

        self.assertEqual(instruction_issues(self.root), [])

    def test_rejects_missing_required_file(self) -> None:
        (self.root / "check.sh").unlink()

        self.assertIn(
            "missing required file: check.sh",
            instruction_issues(self.root),
        )

    def test_rejects_stale_repository_path(self) -> None:
        (self.root / "README.md").write_text("See `src/paths.rs`.\n")

        issues = instruction_issues(self.root)

        self.assertTrue(any("stale repository path" in issue for issue in issues))

    def test_rejects_missing_referenced_path(self) -> None:
        (self.root / "README.md").write_text("Run `scripts/missing.sh`.\n")

        self.assertIn(
            "README.md: referenced repository path is missing: scripts/missing.sh",
            instruction_issues(self.root),
        )

    def test_rejects_raw_cargo_suggestion(self) -> None:
        (self.root / "README.md").write_text("Run `cargo check`.\n")

        self.assertTrue(
            any("raw Cargo suggestion" in issue for issue in instruction_issues(self.root))
        )

    def test_rejects_missing_pipeline_command(self) -> None:
        (self.root / ".gitlab-ci.yml").write_text(
            "script:\n  - python3 scripts/missing.py\n"
        )

        self.assertIn(
            ".gitlab-ci.yml: command path is missing: scripts/missing.py",
            instruction_issues(self.root),
        )


if __name__ == "__main__":
    unittest.main()
