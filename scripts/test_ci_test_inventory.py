#!/usr/bin/env python3
import tempfile
import unittest
from pathlib import Path

import ci_test_inventory


class TestInventoryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        (self.root / "src").mkdir()
        (self.root / "src" / "sample.rs").write_text(
            "// @variants: snap\n#[test]\nfn handles_snap() {}\n"
            "// @variants: both\n#[test]\nfn engine_boundary() {}\n"
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def write_inventory(self, variant: str = "snap") -> Path:
        inventory = self.root / "inventory.toml"
        inventory.write_text(
            "[[variant_tests]]\n"
            'path = "src/sample.rs"\n'
            'name = "handles_snap"\n'
            f'variant = "{variant}"\n'
            'interface = "home"\n'
            'interface_state = "connected"\n\n'
            "[[variant_tests]]\n"
            'path = "src/sample.rs"\n'
            'name = "engine_boundary"\n'
            'variant = "both"\n'
            'interface = "home"\n'
            'interface_state = "disconnected"\n\n'
            + "".join(
                "[[engine_tests]]\n"
                'path = "src/sample.rs"\n'
                'name = "engine_boundary"\n'
                f'engine = "{engine}"\n'
                f'case = "{case}"\n\n'
                for engine in sorted(ci_test_inventory.ENGINES)
                for case in sorted(ci_test_inventory.CASES)
            )
        )
        return inventory

    def test_accepts_paired_variant_and_engine_coverage(self) -> None:
        inventory = self.write_inventory()
        data = ci_test_inventory.validate_inventory(self.root, inventory)
        expression = ci_test_inventory.filter_expression("variant:snap", data)
        self.assertIn("test(handles_snap)", expression)

    def test_includes_integration_test_annotations(self) -> None:
        directory = self.root / "tests" / "generations"
        directory.mkdir(parents=True)
        (directory / "activation.rs").write_text(
            "// @variants: both\n#[tokio::test]\nasync fn recovers_activation() {}\n"
        )
        inventory = self.write_inventory()
        with inventory.open("a") as stream:
            stream.write(
                '\n[[variant_tests]]\npath = "tests/generations/activation.rs"\n'
                'name = "recovers_activation"\nvariant = "both"\n'
                'interface_state = "not_applicable"\n'
            )
        data = ci_test_inventory.validate_inventory(self.root, inventory)
        expression = ci_test_inventory.filter_expression("variant:snap", data)
        self.assertIn("test(recovers_activation)", expression)

    def test_rejects_variant_annotation_drift(self) -> None:
        inventory = self.write_inventory("both")
        with self.assertRaisesRegex(ci_test_inventory.InventoryError, "inventory drift"):
            ci_test_inventory.validate_inventory(self.root, inventory)

    def test_rejects_unpaired_interface_coverage(self) -> None:
        inventory = self.write_inventory()
        content = inventory.read_text().replace(
            'interface_state = "disconnected"', 'interface_state = "connected"'
        )
        inventory.write_text(content)
        with self.assertRaisesRegex(ci_test_inventory.InventoryError, "paired coverage"):
            ci_test_inventory.validate_inventory(self.root, inventory)


if __name__ == "__main__":
    unittest.main()
