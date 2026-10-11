"""Regression coverage for the verified texture source projection."""

import json
from pathlib import Path
import unittest
import xml.etree.ElementTree as ET

from build_managed import linux_project
from image_source import project_image_source


class ImageSourceTests(unittest.TestCase):
    def test_replaces_image_dependencies_without_removing_compression(self):
        project = linux_project(b'''<Project><ItemGroup>
          <PackageReference Include="SixLabors.ImageSharp" Version="3.1.12" />
          <PackageReference Include="BCnEncoder.Net.ImageSharp" Version="1.1.2" />
          <PackageReference Include="BCnEncoder.Net" Version="2.2.1" />
        </ItemGroup></Project>''')
        packages = {node.get("Include"): node.get("Version") for node in ET.fromstring(project).iter("PackageReference")}
        self.assertEqual(packages, {"BCnEncoder.Net": "2.2.1", "SkiaSharp": "[4.153.1]"})

    def test_rejects_changed_or_ambiguous_upstream_source(self):
        replacements = json.loads(Path(__file__).with_name("image_source_replacements.json").read_text())
        source = "\n".join(before for before, _ in replacements)
        self.assertNotIn(b"SixLabors", project_image_source(source.encode()))
        for changed in (source.replace(replacements[0][0], "", 1), source + replacements[0][0]):
            with self.assertRaisesRegex(ValueError, "no longer matches"):
                project_image_source(changed.encode())


if __name__ == "__main__":
    unittest.main()
