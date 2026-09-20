#!/usr/bin/env python3
"""Validate isolation-critical identities in the development Snap recipe."""

from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parent.parent
DEVELOPMENT_ID = "io.mattianelo.deployd.Devel"


def required_match(source: str, pattern: str) -> str:
    match = re.search(pattern, source, re.MULTILINE)
    if match is None:
        raise AssertionError(f"missing recipe pattern: {pattern}")
    return match.group(1)


class DevelopmentSnapRecipeTests(unittest.TestCase):
    def setUp(self) -> None:
        self.recipe = (ROOT / "snap" / "snapcraft-dev.yaml").read_text()
        self.desktop = (
            ROOT / "data" / "io.mattianelo.deployd.Devel.desktop"
        ).read_text()

    def test_snap_and_application_names_match(self) -> None:
        snap_name = required_match(self.recipe, r"^name:\s*([^\s]+)$")
        app_name = required_match(self.recipe, r"^apps:\n  ([^:]+):$")

        self.assertEqual(snap_name, "deployd-dev")
        self.assertEqual(app_name, snap_name)
        self.assertNotIn("deployd_dev", self.recipe)

    def test_dbus_and_compiled_application_ids_match(self) -> None:
        dbus_name = required_match(
            self.recipe,
            r"^    name:\s*(io\.mattianelo\.deployd\.Devel)$",
        )
        build_id = required_match(
            self.recipe,
            r"^      - DEPLOYD_APPLICATION_ID:\s*([^\s]+)$",
        )

        self.assertEqual(dbus_name, DEVELOPMENT_ID)
        self.assertEqual(build_id, DEVELOPMENT_ID)

    def test_desktop_launches_only_the_development_command(self) -> None:
        self.assertIn("Name=Deployd Development\n", self.desktop)
        self.assertIn("Exec=deployd-dev %u\n", self.desktop)
        self.assertNotIn("MimeType=x-scheme-handler/nxm", self.desktop)
        self.assertIn(
            "desktop: usr/share/applications/io.mattianelo.deployd.Devel.desktop",
            self.recipe,
        )

    def test_recipe_retains_strict_confinement(self) -> None:
        self.assertIn("grade: devel\n", self.recipe)
        self.assertIn("confinement: strict\n", self.recipe)
        self.assertNotIn("personal-files", self.recipe)
        self.assertNotIn("system-files", self.recipe)


if __name__ == "__main__":
    unittest.main()
