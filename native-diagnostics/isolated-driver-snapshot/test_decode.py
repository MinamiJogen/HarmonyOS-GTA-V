"""Synthetic ABI fixture tests; none are a device-generated snapshot."""
import struct
import unittest
from decode_snapshot import decode


def synthetic_record():
    # OHOS SDK offsets, checked separately by the AArch64 compiler.
    fields = [2, 472, 128, 4560, 1, 0x48000004, 183, 1,
              176, 176, 184, 432, 440, 448, 424, 16]
    header = bytearray(472)
    struct.pack_into("<8s16I2Q", header, 0, b"MGSEGV2\0", *fields, 12345, 0x1000)
    struct.pack_into("<QQII", header, 88, 0x1000, 0x2000, 5, 0)
    info = bytearray(128)
    struct.pack_into("<iii", info, 0, 11, 0, 1)
    struct.pack_into("<Q", info, 16, 0x454c414d4554)
    context = bytearray(4560)
    struct.pack_into("<Q", context, 176, 0x454c414d4554)
    for i in range(31):
        struct.pack_into("<Q", context, 184 + i * 8, 0xa000 + i)
    struct.pack_into("<Q", context, 424, 0x1abc)  # x30/LR, indirect callback case.
    struct.pack_into("<QQQ", context, 432, 0xabcdef, 4, 0x60000000)
    return bytes(header + info + context + b"MGDONE2\0")


class SnapshotDecoderTest(unittest.TestCase):
    def test_pc_four_lr_driver_and_all_registers(self):
        result = decode(synthetic_record())
        self.assertEqual(result["pc"], "0x4")
        self.assertFalse(result["pcInExecutableDriverLoad"])
        self.assertTrue(result["lrInExecutableDriverLoad"])
        self.assertEqual(result["registers"]["x29"], "0xa01d")
        self.assertEqual(len(result["registers"]), 31)
        self.assertEqual(result["lr"], "0x1abc")
        self.assertEqual(result["siginfoFaultAddress"], "0x454c414d4554")

    def test_truncated_file_rejected(self):
        with self.assertRaises(ValueError):
            decode(synthetic_record()[:-1])

    def test_corrupt_completion_rejected(self):
        with self.assertRaises(ValueError):
            decode(synthetic_record()[:-8] + b"BAD-DONE")

    def test_host_fixture_marker_rejected(self):
        data = bytearray(synthetic_record())
        struct.pack_into("<I", data, 32, 0x53545542)
        with self.assertRaisesRegex(ValueError, "host fixtures"):
            decode(data)

    def test_register_offset_outside_frame_rejected(self):
        data = bytearray(synthetic_record())
        struct.pack_into("<I", data, 48, 10000)
        with self.assertRaisesRegex(ValueError, "Register offset"):
            decode(data)


if __name__ == "__main__":
    unittest.main()
