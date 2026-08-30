#!/usr/bin/env python3
import tempfile
import unittest
from pathlib import Path

import ci_file_size


class FileSizePolicyTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "src").mkdir()

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_lines(self, relative: str, count: int) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("// line\n" * count)

    def write_source(self, relative: str, source: str) -> None:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source)

    def test_accepts_file_at_limit(self) -> None:
        self.write_lines("src/limit.rs", 3)

        self.assertEqual(
            ci_file_size.validate_file_sizes(
                self.root, production_maximum=3, total_maximum=3
            ),
            [(Path("src/limit.rs"), 3, 3)],
        )

    def test_rejects_existing_or_new_oversized_file(self) -> None:
        self.write_lines("src/domain/oversized.rs", 4)

        with self.assertRaisesRegex(
            ci_file_size.FileSizeError,
            r"src/domain/oversized\.rs: 4 production lines \(limit 3\), "
            r"4 total lines \(limit 3\)",
        ):
            ci_file_size.validate_file_sizes(
                self.root, production_maximum=3, total_maximum=3
            )

    def test_ignores_non_rust_files_and_paths_outside_src(self) -> None:
        self.write_lines("src/within.rs", 2)
        self.write_lines("src/generated.txt", 5)
        self.write_lines("scripts/large.rs", 5)

        self.assertEqual(
            ci_file_size.validate_file_sizes(
                self.root, production_maximum=2, total_maximum=2
            ),
            [(Path("src/within.rs"), 2, 2)],
        )

    def test_excludes_one_trailing_test_module_from_production_count(self) -> None:
        self.write_source(
            "src/domain.rs",
            "fn production() {}\n#[cfg(test)]\nmod tests {\n    // test-only\n}\n",
        )

        self.assertEqual(
            ci_file_size.validate_file_sizes(
                self.root, production_maximum=1, total_maximum=5
            ),
            [(Path("src/domain.rs"), 1, 5)],
        )

    def test_retains_total_limit_for_test_heavy_files(self) -> None:
        self.write_source(
            "src/domain.rs",
            "fn production() {}\n#[cfg(test)]\nmod tests {\n    // one\n    // two\n}\n",
        )

        with self.assertRaisesRegex(
            ci_file_size.FileSizeError, r"1 production lines.*6 total lines"
        ):
            ci_file_size.validate_file_sizes(
                self.root, production_maximum=1, total_maximum=5
            )

    def test_rejects_non_module_cfg_test_marker(self) -> None:
        self.write_source(
            "src/domain.rs",
            "#[cfg(test)]\nfn test_helper() {}\n",
        )

        with self.assertRaisesRegex(
            ci_file_size.FileSizeError, r"must introduce a trailing `mod tests"
        ):
            ci_file_size.validate_file_sizes(self.root)


if __name__ == "__main__":
    unittest.main()
