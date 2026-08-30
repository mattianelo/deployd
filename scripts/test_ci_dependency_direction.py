#!/usr/bin/env python3
import tempfile
import unittest
from pathlib import Path

import ci_dependency_direction


class DependencyDirectionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        for layer in ("app", "core", "models", "ui", "utils"):
            (self.root / "src" / layer).mkdir(parents=True)

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write(self, layer: str, source: str) -> None:
        (self.root / "src" / layer / "mod.rs").write_text(source)

    def test_accepts_downward_dependencies(self) -> None:
        self.write("app", "use crate::core::tracker;\nuse crate::ui::Dialog;\n")
        self.write("ui", "use crate::core::game;\nuse crate::models::game;\n")
        self.write("core", "use crate::models::game;\nuse crate::utils::paths;\n")

        ci_dependency_direction.validate(self.root)

    def test_rejects_ui_dependency_on_app(self) -> None:
        self.write("ui", "use crate::app::messages::AppMsg;\n")

        with self.assertRaisesRegex(
            ci_dependency_direction.DependencyDirectionError,
            r"src/ui/mod\.rs:1: ui must not depend on app",
        ):
            ci_dependency_direction.validate(self.root)

    def test_rejects_utils_dependency_on_core(self) -> None:
        self.write("utils", "use crate::core::game;\n")

        with self.assertRaisesRegex(
            ci_dependency_direction.DependencyDirectionError,
            r"utils must not depend on core",
        ):
            ci_dependency_direction.validate(self.root)

    def test_rejects_any_project_dependency_from_models(self) -> None:
        self.write("models", "use crate::utils::paths;\n")

        with self.assertRaisesRegex(
            ci_dependency_direction.DependencyDirectionError,
            r"models must not depend on utils",
        ):
            ci_dependency_direction.validate(self.root)


if __name__ == "__main__":
    unittest.main()
