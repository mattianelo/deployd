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

    def test_accepts_file_at_limit(self) -> None:
        self.write_lines("src/limit.rs", 3)

        self.assertEqual(
            ci_file_size.validate_file_sizes(self.root, maximum=3),
            [(Path("src/limit.rs"), 3)],
        )

    def test_rejects_existing_or_new_oversized_file(self) -> None:
        self.write_lines("src/domain/oversized.rs", 4)

        with self.assertRaisesRegex(
            ci_file_size.FileSizeError,
            r"src/domain/oversized\.rs: 4 lines \(limit 3\)",
        ):
            ci_file_size.validate_file_sizes(self.root, maximum=3)

    def test_ignores_non_rust_files_and_paths_outside_src(self) -> None:
        self.write_lines("src/within.rs", 2)
        self.write_lines("src/generated.txt", 5)
        self.write_lines("scripts/large.rs", 5)

        self.assertEqual(
            ci_file_size.validate_file_sizes(self.root, maximum=2),
            [(Path("src/within.rs"), 2)],
        )


if __name__ == "__main__":
    unittest.main()
