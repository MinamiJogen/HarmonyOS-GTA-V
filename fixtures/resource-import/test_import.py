#!/usr/bin/env python3
"""Portable synthetic ZIP/ZIP64 tests. Does not use or import real game content."""
import binascii
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import struct
import subprocess
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[2]
BUILD = "8b0b5899ed"
MARKER = b"HarmonyGtaResourceImportV1\n8b0b5899ed\n"


def files(prefix="", payload=b"abc"):
    return {
        prefix + "data/manifest.json": json.dumps({"files": [["common/sample.bin", len(payload)]]}).encode(),
        prefix + "data/common/sample.bin": payload,
        prefix + f"b/{BUILD}/shaders/index.json": b'{"abc":{"ok":true,"consts":true}}',
        prefix + f"b/{BUILD}/shaders/abc.wgsl": b"@compute @workgroup_size(1) fn main() {}",
        prefix + f"b/{BUILD}/shaders/abc.consts.json": b"[1,2.5,-3,1e-2]",
    }


def write64(path, entries, padding=0):
    """Real ZIP64 EOCD, per-member ZIP64 sizes/offsets; optionally sparse >4GiB."""
    records = []
    with path.open("wb") as out:
        out.seek(padding)
        for name, payload in entries.items():
            name = name.encode(); offset = out.tell(); crc = binascii.crc32(payload)
            extra = struct.pack("<HHQQ", 1, 16, len(payload), len(payload))
            out.write(struct.pack("<IHHHHHIIIHH", 0x04034B50, 45, 0, 0, 0, 0, crc, 0xFFFFFFFF, 0xFFFFFFFF, len(name), len(extra)))
            out.write(name + extra + payload)
            records.append((name, payload, crc, offset))
        start = out.tell()
        for name, payload, crc, offset in records:
            extra = struct.pack("<HHQQQ", 1, 24, len(payload), len(payload), offset)
            out.write(struct.pack("<IHHHHHHIIIHHHHHII", 0x02014B50, (3 << 8) | 45, 45, 0, 0, 0, 0, crc, 0xFFFFFFFF, 0xFFFFFFFF, len(name), len(extra), 0, 0, 0, 0o100600 << 16, 0xFFFFFFFF))
            out.write(name + extra)
        size = out.tell() - start; end = out.tell()
        out.write(struct.pack("<IQHHIIQQQQ", 0x06064B50, 44, 45, 45, 0, 0, len(records), len(records), size, start))
        out.write(struct.pack("<IIQI", 0x07064B50, 0, end, 1))
        out.write(struct.pack("<IHHHHIIH", 0x06054B50, 0, 0, 0xFFFF, 0xFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0))


class ImportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.build = tempfile.TemporaryDirectory(prefix="resource-import-host-")
        cls.binary = Path(cls.build.name) / "import-test"
        subprocess.run([os.environ.get("CXX", "clang++"), "-std=c++17", "-Wall", "-Wextra", "-Werror", "-O1", "-g",
                        str(ROOT / "fixtures/resource-import/import_cli.cpp"),
                        str(ROOT / "harmony/entry/src/main/cpp/game_resource_import_core.cpp"),
                        "-lz", "-o", str(cls.binary)], check=True)

    @classmethod
    def tearDownClass(cls): cls.build.cleanup()

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="资源导入 fixture ")
        self.base = Path(self.temp.name)
        self.app = self.base / "应用 files"
        self.app.mkdir()
        (self.app / "saves").mkdir()
        (self.app / "saves/save.bin").write_bytes(b"keep save")
        (self.app / "config.json").write_bytes(b"keep config")
        (self.app / "game").mkdir()
        (self.app / "game/old.bin").write_bytes(b"keep old resources")
        self.package = self.base / "用户 游戏包.zip"

    def tearDown(self): self.temp.cleanup()

    def archive(self, entries=None, compression=zipfile.ZIP_DEFLATED):
        with zipfile.ZipFile(self.package, "w", compression=compression) as archive:
            for name, value in (entries if entries is not None else files()).items(): archive.writestr(name, value)

    def invoke(self, operation="import", *options):
        args = [str(self.binary), operation]
        if operation == "import": args += [str(self.package)]
        args += [str(self.app), *options]
        result = subprocess.run(args, text=True, capture_output=True)
        self.assertIn(result.returncode, (0, 2), result.stderr)
        return json.loads(result.stdout)

    def unchanged(self):
        self.assertEqual((self.app / "game/old.bin").read_bytes(), b"keep old resources")
        self.assertEqual((self.app / "saves/save.bin").read_bytes(), b"keep save")
        self.assertEqual((self.app / "config.json").read_bytes(), b"keep config")
        self.assertFalse(list(self.app.glob(".game-import-stage-*")))
        self.assertFalse((self.app / "game.import-backup").exists())

    def test_streaming_deflate_import_and_read_only_source(self):
        entries = files(payload=b"A\0B" * 400000)
        entries.update({f"b/{BUILD}/game.wasm": b"not imported", "config.json": b"wrong config", "install.command": b"exit 1", "other.hap": b"not imported"})
        self.archive(entries)
        before = hashlib.sha256(self.package.read_bytes()).digest()
        result = self.invoke()
        self.assertTrue(result["ready"], result)
        self.assertTrue(self.invoke("check")["ready"])
        self.assertEqual(hashlib.sha256(self.package.read_bytes()).digest(), before)
        self.assertFalse((self.app / f"game/b/{BUILD}/game.wasm").exists())
        self.assertFalse((self.app / "game/install.command").exists())
        self.assertEqual((self.app / "config.json").read_bytes(), b"keep config")
        self.assertEqual((self.app / "saves/save.bin").read_bytes(), b"keep save")
        self.assertFalse((self.app / "game/old.bin").exists())

    def test_game_prefixed_store_layout(self):
        self.archive(files("game/"), zipfile.ZIP_STORED)
        self.assertTrue(self.invoke()["ready"])

    def test_zip64_sizes_and_directory(self):
        write64(self.package, files())
        self.assertTrue(self.invoke()["ready"])

    def test_zip64_offsets_over_four_gibibytes(self):
        write64(self.package, files(), (1 << 32) + 4096)
        self.assertGreater(self.package.stat().st_size, 1 << 32)
        self.assertLess(self.package.stat().st_blocks * 512, 1 << 20)
        self.assertTrue(self.invoke()["ready"])

    def test_traversal_rejected(self):
        entries = files(); entries["../outside.txt"] = b"bad"; self.archive(entries)
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_backslash_rejected(self):
        entries = files(); entries["data\\outside.txt"] = b"bad"; self.archive(entries)
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_case_collision_rejected(self):
        entries = files(); entries["data/COMMON/sample.bin"] = b"bad"; self.archive(entries)
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_symlink_rejected(self):
        self.archive()
        with zipfile.ZipFile(self.package, "a") as archive:
            info = zipfile.ZipInfo("data/link"); info.create_system = 3; info.external_attr = (stat.S_IFLNK | 0o777) << 16
            archive.writestr(info, "../../outside")
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_crc_corruption_preserves_previous(self):
        self.archive(compression=zipfile.ZIP_STORED)
        with zipfile.ZipFile(self.package) as archive:
            info = archive.getinfo("data/common/sample.bin")
        with self.package.open("r+b") as stream:
            stream.seek(info.header_offset + 26); name_len, extra_len = struct.unpack("<HH", stream.read(4))
            stream.seek(info.header_offset + 30 + name_len + extra_len); stream.write(b"X")
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_truncated_zip_preserves_previous(self):
        self.archive(); self.package.write_bytes(self.package.read_bytes()[:-9])
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_wrong_build_rejected(self):
        entries = {name.replace(BUILD, "anotherbuild"): value for name, value in files().items()}; self.archive(entries)
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_manifest_missing_file_rejected(self):
        entries = files(); del entries["data/common/sample.bin"]; self.archive(entries)
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_manifest_wrong_size_rejected(self):
        entries = files(); entries["data/manifest.json"] = b'{"files":[["common/sample.bin",4]]}'; self.archive(entries)
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_missing_shader_constants_rejected(self):
        entries = files(); del entries[f"b/{BUILD}/shaders/abc.consts.json"]; self.archive(entries)
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_duplicate_json_keys_rejected(self):
        entries = files(); entries["data/manifest.json"] = b'{"files":[],"files":[["common/sample.bin",3]]}'; self.archive(entries)
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_cancel_before_and_during_stream(self):
        self.archive(files(payload=b"abc" * 400000), zipfile.ZIP_STORED)
        self.assertTrue(self.invoke("import", "cancel-before")["cancelled"]); self.unchanged()
        self.assertTrue(self.invoke("import", "cancel=300000")["cancelled"]); self.unchanged()

    def test_commit_failure_rolls_back(self):
        self.archive()
        result = self.invoke("import", "throw-commit")
        self.assertFalse(result["ready"], result); self.unchanged()

    def test_size_limits_rejected_before_extract(self):
        self.archive()
        self.assertFalse(self.invoke("import", "total=10")["ready"]); self.unchanged()
        self.assertFalse(self.invoke("import", "file=2")["ready"]); self.unchanged()

    def test_check_restores_crash_backup(self):
        self.archive(); self.assertTrue(self.invoke()["ready"])
        (self.app / "game").rename(self.app / "game.import-backup")
        self.assertTrue(self.invoke("check")["ready"])
        self.assertTrue((self.app / "game/data/manifest.json").is_file())

    def test_owned_crash_stage_is_cleaned(self):
        stage = self.app / ".game-import-stage-Abc123"; stage.mkdir()
        (stage / ".resource-import-owner").write_bytes(MARKER)
        (stage / "partial.bin").write_bytes(b"partial")
        self.archive(); self.assertTrue(self.invoke()["ready"])
        self.assertFalse(stage.exists())

    def test_unknown_stage_is_preserved_and_blocks_retry(self):
        stage = self.app / ".game-import-stage-Abc123"; stage.mkdir()
        (stage / "user.bin").write_bytes(b"keep unknown")
        self.archive(); self.assertFalse(self.invoke()["ready"])
        self.assertEqual((stage / "user.bin").read_bytes(), b"keep unknown")
        self.assertEqual((self.app / "game/old.bin").read_bytes(), b"keep old resources")

    def test_empty_crash_stage_is_cleaned(self):
        stage = self.app / ".game-import-stage-Abc123"; stage.mkdir()
        self.archive(); self.assertTrue(self.invoke()["ready"])
        self.assertFalse(stage.exists())

    def test_half_written_crash_marker_is_cleaned(self):
        stage = self.app / ".game-import-stage-Abc123"; stage.mkdir()
        (stage / ".resource-import-owner").write_bytes(MARKER[:10])
        self.archive(); self.assertTrue(self.invoke()["ready"])
        self.assertFalse(stage.exists())

    def test_unknown_stage_warns_without_blocking_valid_game(self):
        self.archive(); self.assertTrue(self.invoke()["ready"])
        stage = self.app / ".game-import-stage-Abc123"; stage.mkdir()
        (stage / "user.bin").write_bytes(b"preserve")
        result = self.invoke("check")
        self.assertTrue(result["ready"])
        self.assertTrue(result["warning"])
        self.assertTrue(stage.exists())

    def test_encrypted_flag_rejected(self):
        self.archive()
        payload = bytearray(self.package.read_bytes())
        central = payload.index(b"PK\x01\x02")
        payload[central + 8] |= 1
        self.package.write_bytes(payload)
        self.assertFalse(self.invoke()["ready"]); self.unchanged()

    def test_cancel_during_deflate(self):
        self.archive(files(payload=b"abc" * 400000), zipfile.ZIP_DEFLATED)
        self.assertTrue(self.invoke("import", "cancel=300000")["cancelled"]); self.unchanged()


if __name__ == "__main__": unittest.main(verbosity=2)
