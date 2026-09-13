"""Boundaries for helper packaging without building an application artifact."""

from pathlib import Path
import tempfile
import unittest

from package import copy_tree, copy_application, copy_runtime, validate_native_files, TRACE_PROVIDER


class PackageTests(unittest.TestCase):
    def test_keeps_linux_native_assets_and_managed_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "app"
            source.mkdir()
            (source / "managed.dll").write_bytes(b"managed")
            for rid in ("linux-x64", "unix", "android-arm", "linux-arm64", "win-x64", "osx-x64"):
                native = source / "runtimes" / rid / "native"
                native.mkdir(parents=True)
                (native / "library.so").write_bytes(b"fixture")
            copy_application(source, root / "output")
            self.assertEqual({p.name for p in (root / "output/runtimes").iterdir()}, {"linux-x64", "unix"})
            self.assertEqual((root / "output/managed.dll").read_bytes(), b"managed")
            self.assertTrue((source / "runtimes/android-arm/native/library.so").exists())

    def test_omits_optional_tracing_without_changing_the_cached_runtime(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "runtime"
            source.mkdir()
            for name in (TRACE_PROVIDER, "libcoreclr.so", "System.Runtime.dll"):
                (source / name).write_bytes(b"fixture")
            copy_runtime(source, root / "output")
            self.assertTrue((source / TRACE_PROVIDER).exists())
            self.assertFalse((root / "output" / TRACE_PROVIDER).exists())
            self.assertTrue((root / "output/libcoreclr.so").exists())
            self.assertTrue((root / "output/System.Runtime.dll").exists())

    def test_rejects_foreign_elf_architecture_even_under_a_linux_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "library.so"
            header = bytearray(20)
            header[:6] = b"\x7fELF\x02\x01"
            header[18:20] = b"\x3e\x00"
            binary.write_bytes(header)
            validate_native_files(root)
            header[18:20] = b"\xb7\x00"
            binary.write_bytes(header)
            with self.assertRaises(ValueError):
                validate_native_files(root)

    def test_copies_independent_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            (source / "example.dll").write_bytes(b"synthetic helper")
            copy_tree(source, root / "output")
            (root / "output/example.dll").write_bytes(b"changed")
            self.assertEqual((source / "example.dll").read_bytes(), b"synthetic helper")

    def test_rejects_links_in_package_inputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            (source / "link").symlink_to(root)
            with self.assertRaises(ValueError):
                copy_tree(source, root / "output")
            self.assertFalse((root / "output").exists())


if __name__ == "__main__":
    unittest.main()
