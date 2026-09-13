"""Exercise codec boundaries and byte-preserving round trips in an isolated process."""

import concurrent.futures
import ctypes
import hashlib
from pathlib import Path
import random
import sys
import unittest


def bind_native():
    native = ctypes.CDLL(sys.argv[1])
    native.mele_oodle_load_verified.argtypes = [ctypes.c_void_p, ctypes.c_uint64]
    native.mele_oodle_load_verified.restype = ctypes.c_int
    native.mele_oodle_bound.argtypes = [ctypes.c_uint64]
    native.mele_oodle_bound.restype = ctypes.c_int64
    for name in ("compress", "decompress"):
        function = getattr(native, f"mele_oodle_{name}")
        function.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_uint64]
        function.restype = ctypes.c_int64
    return native


class UnloadedAdapterTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = bind_native()

    def test_rejects_invalid_codec_sizes_before_loading(self):
        self.assertLess(self.native.mele_oodle_load_verified(None, 1007616), 0)
        self.assertLess(self.native.mele_oodle_load_verified(b"MZ", 2), 0)

    def test_refuses_operations_without_a_loaded_codec(self):
        output = ctypes.create_string_buffer(32)
        self.assertLess(self.native.mele_oodle_bound(32), 0)
        self.assertLess(self.native.mele_oodle_compress(b"data", 4, output, 32), 0)
        self.assertLess(self.native.mele_oodle_decompress(b"data", 4, output, 32), 0)


class AdapterTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.native = bind_native()
        cls.fixture = Path("/workspace/.ci-artifacts/mele-fixtures/oo2core_8_win64.dll")
        if not cls.fixture.is_file():
            raise RuntimeError("The verified, game-owned Oodle test fixture is unavailable")
        codec = cls.fixture.read_bytes()
        if len(codec) != 1007616 or hashlib.sha256(codec).hexdigest() != "d42940381611cda3b8555f6eb9fcb1bc3b1a3b96d7e24cb98738f4b71653d415":
            raise RuntimeError("Refusing to execute an unverified codec fixture")
        cls.codec_bytes = codec
        if cls.native.mele_oodle_load_verified(codec, len(codec)) != 0:
            raise RuntimeError("Native loader could not initialize the verified codec")

    def round_trip(self, data):
        capacity = self.native.mele_oodle_bound(len(data))
        self.assertGreater(capacity, 0)
        encoded = ctypes.create_string_buffer(capacity)
        size = self.native.mele_oodle_compress(data, len(data), encoded, capacity)
        self.assertGreater(size, 0)
        decoded = ctypes.create_string_buffer(len(data))
        self.assertEqual(self.native.mele_oodle_decompress(encoded, size, decoded, len(data)), len(data))
        self.assertEqual(decoded.raw, data)

    def test_round_trips_small_unaligned_and_package_sized_blocks(self):
        for size in (1, 17, 65535, 65536, 262144, 1048576):
            self.round_trip((b"Legendary Edition package data\x00" * (size // 30 + 1))[:size])
        self.round_trip(random.Random(42).randbytes(262144))

    def test_serializes_calls_from_different_linux_threads(self):
        with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
            list(executor.map(self.round_trip, [bytes([index]) * 262144 for index in range(16)]))

    def test_refuses_invalid_lengths_and_insufficient_output(self):
        for size in (0, 64 * 1024 * 1024 + 1, 2**64 - 1):
            self.assertLess(self.native.mele_oodle_bound(size), 0)
        output = ctypes.create_string_buffer(32)
        self.assertLess(self.native.mele_oodle_compress(b"content", 7, output, 1), 0)
        self.assertLess(self.native.mele_oodle_decompress(b"x", 1, output, 32), 0)
        self.assertLess(self.native.mele_oodle_decompress(b"invalid content", 15, output, 32), 0)

    def test_refuses_codec_replacement_and_preserves_the_source(self):
        self.assertEqual(self.native.mele_oodle_load_verified(self.codec_bytes, len(self.codec_bytes)), -2)
        self.assertLess(self.native.mele_oodle_load_verified(b"MZ", 2), 0)
        self.assertEqual(self.fixture.read_bytes(), self.codec_bytes)


if __name__ == "__main__":
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(UnloadedAdapterTests)
    if len(sys.argv) == 3 and sys.argv[2] == "--with-codec":
        suite.addTests(unittest.defaultTestLoader.loadTestsFromTestCase(AdapterTests))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    sys.exit(0 if result.wasSuccessful() else 1)
