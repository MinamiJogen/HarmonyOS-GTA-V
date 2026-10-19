#!/usr/bin/env python3
"""Small synthetic ZIP fixtures; creates no game data and uses only stdlib.

Run from the workspace root:
  python3 fixtures/resource-audit/test_validate_game_package.py -v
Each test uses TemporaryDirectory and removes its archive afterward. Direct CLI
tests assert exit status, parseable JSON diagnostics and read-only input bytes.
"""

import hashlib
import importlib.util
import json
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
import warnings
import zipfile


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "tools/validate_game_package.py"
SPEC = importlib.util.spec_from_file_location("validate_game_package", SCRIPT)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
WASM = b"\0asm\x01\0\0\0"  # Header-only synthetic core module; not game code.


def members(prefix=""):
    return {
        prefix + "data/manifest.json": json.dumps({"files": [["common/example.bin", 3], ["env_test/ignored.bin", 0]]}),
        prefix + "data/common/example.bin": b"abc",
        prefix + "b/demo/game.wasm": WASM,
        prefix + "b/demo/shaders/index.json": json.dumps({"abc123": {"ok": True, "consts": True}}),
        prefix + "b/demo/shaders/abc123.wgsl": "@compute @workgroup_size(1) fn main() {}",
        prefix + "b/demo/shaders/abc123.consts.json": "[1, 2.5]",
    }


class ResourceAuditTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="muguet-resource-fixture-")
        self.package = Path(self.temp.name) / "game.zip"

    def tearDown(self):
        self.temp.cleanup()

    def write(self, files):
        with zipfile.ZipFile(self.package, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for name, value in files.items():
                archive.writestr(name, value)

    def audit(self, files=None, **kwargs):
        self.write(files if files is not None else members())
        return MODULE.Audit(self.package, **kwargs).run()

    def error_codes(self, report):
        return {error["code"] for error in report["errors"]}

    def test_root_pass_and_streaming_hash(self):
        report = self.audit(hash_wasm=True)
        self.assertEqual(report["status"], "pass", report["errors"])
        self.assertEqual(report["layout"]["selected_prefix"], "")
        wasm = next(item for item in report["key_files"] if item["member"].endswith(".wasm"))
        self.assertEqual(wasm["sha256"], hashlib.sha256(WASM).hexdigest())
        self.assertEqual(wasm["version"], 1)
        self.assertEqual(report["manifest_files"]["upstream_skipped"], ["env_test/ignored.bin"])
        self.assertEqual(report["hooks"]["status"], "not_verified")

    def test_nested_prefix_pass(self):
        report = self.audit(members("download/export/"))
        self.assertEqual(report["status"], "pass", report["errors"])
        self.assertEqual(report["layout"]["selected_prefix"], "download/export/")

    def test_browser_shader_pack_metadata_is_not_a_shader(self):
        files = members()
        index = json.loads(files["b/demo/shaders/index.json"])
        index["_packs"] = [{"file": "pack0.demo.bin", "hashes": 1, "bytes": 10}]
        files["b/demo/shaders/index.json"] = json.dumps(index)
        report = self.audit(files)
        self.assertEqual(report["status"], "pass", report["errors"])
        self.assertEqual(report["builds"][0]["shaders"]["pack_metadata_entries"], 1)
        index["_packs"][0]["file"] = "../outside"
        files["b/demo/shaders/index.json"] = json.dumps(index)
        self.assertIn("shader_pack_metadata", self.error_codes(self.audit(files)))

    def test_shortest_manifest_prefix_matches_upstream(self):
        files = members("outer/nested/")
        files.update(members("x/"))
        report = self.audit(files)
        self.assertEqual(report["status"], "pass", report["errors"])
        self.assertEqual(report["layout"]["selected_prefix"], "x/")
        self.assertIn("multiple_manifests", {item["code"] for item in report["warnings"]})

    def test_missing_manifest_and_wrong_layout(self):
        report = self.audit({"original_install/GTA5.exe": b"not an executable"})
        self.assertEqual(report["status"], "fail")
        self.assertIn("missing_manifest", self.error_codes(report))

    def test_wasm_and_shaders_must_share_direct_build(self):
        files = members()
        files["b/other/game.wasm"] = files.pop("b/demo/game.wasm")
        report = self.audit(files)
        self.assertEqual(report["status"], "fail")
        self.assertEqual(sum(item["code"] == "incomplete_build" for item in report["errors"]), 2)

    def test_missing_data_and_shader_references(self):
        files = members()
        del files["data/common/example.bin"]
        del files["b/demo/shaders/abc123.wgsl"]
        del files["b/demo/shaders/abc123.consts.json"]
        report = self.audit(files)
        self.assertIn("missing_manifest_files", self.error_codes(report))
        self.assertIn("missing_shader_files", self.error_codes(report))
        self.assertEqual(len(report["builds"][0]["shaders"]["missing"]), 2)

    def test_bad_wasm_magic_and_version(self):
        for value in (b"MZBADBAD", b"\0asm\x02\0\0\0", b"\0asm"):
            with self.subTest(value=value):
                files = members()
                files["b/demo/game.wasm"] = value
                self.assertIn("invalid_wasm_header", self.error_codes(self.audit(files)))

    def test_unsafe_archive_paths(self):
        for name in ("../escape", "/absolute", "C:/drive", "folder/../../escape", "..\\escape", "folder//alias", "./alias"):
            with self.subTest(name=name):
                files = members()
                files[name] = "unsafe"
                self.assertIn("unsafe_member_path", self.error_codes(self.audit(files)))
        self.assertFalse((self.package.parent / "escape").exists())

    def test_symlink_duplicate_and_encrypted_member(self):
        self.write(members())
        with warnings.catch_warnings():
            warnings.simplefilter("ignore", UserWarning)
            with zipfile.ZipFile(self.package, "a") as archive:
                info = zipfile.ZipInfo("data/link")
                info.create_system = 3
                info.external_attr = (stat.S_IFLNK | 0o777) << 16
                archive.writestr(info, "../../outside")
                archive.writestr("data/common/example.bin", b"duplicate")
        report = MODULE.Audit(self.package).run()
        self.assertIn("symlink_member", self.error_codes(report))
        self.assertIn("duplicate_member", self.error_codes(report))
        # Toggle encryption metadata in central/local headers without encrypting
        # synthetic bytes; no decrypt attempt should succeed.
        raw = bytearray(self.package.read_bytes())
        for signature, offset in ((b"PK\x01\x02", 8), (b"PK\x03\x04", 6)):
            pos = raw.find(signature)
            raw[pos + offset] |= 1
        self.package.write_bytes(raw)
        self.assertIn("encrypted_member", self.error_codes(MODULE.Audit(self.package).run()))

    def test_malformed_json_and_bounded_json_read(self):
        files = members()
        files["data/manifest.json"] = "{bad"
        self.assertIn("invalid_json", self.error_codes(self.audit(files)))
        self.assertIn("json_size_limit", self.error_codes(self.audit(max_json_bytes=10)))

    def test_utf16_and_nonfinite_json_rejected_like_serde(self):
        for value in ('{"files": [], "x": NaN}', '{"files": []}'.encode("utf-16")):
            with self.subTest(value=value):
                files = members()
                files["data/manifest.json"] = value
                self.assertEqual(self.audit(files)["status"], "fail")

    def test_file_cannot_be_ancestor_of_another_member(self):
        files = members()
        files["data"] = b"file instead of directory"
        self.assertIn("file_directory_conflict", self.error_codes(self.audit(files)))

    def test_unsafe_manifest_and_shader_key(self):
        files = members()
        files["data/manifest.json"] = json.dumps({"files": [["../outside"]]})
        files["b/demo/shaders/index.json"] = json.dumps({"../escape": {"ok": True}})
        report = self.audit(files)
        self.assertIn("unsafe_manifest_path", self.error_codes(report))
        self.assertIn("unsafe_shader_key", self.error_codes(report))

    def test_consts_zero_matches_serde_non_null_non_false(self):
        files = members()
        files["b/demo/shaders/index.json"] = json.dumps({"abc123": {"ok": True, "consts": 0}})
        del files["b/demo/shaders/abc123.consts.json"]
        self.assertIn("missing_shader_files", self.error_codes(self.audit(files)))

    def test_cli_json_exit_codes_and_input_unchanged(self):
        self.write(members())
        before = hashlib.sha256(self.package.read_bytes()).hexdigest()
        result = subprocess.run([sys.executable, str(SCRIPT), str(self.package), "--hash-wasm"], text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)["status"], "pass")
        self.assertEqual(before, hashlib.sha256(self.package.read_bytes()).hexdigest())
        self.assertEqual(result.stderr, "")
        for path in (self.package.parent / "missing.zip", self.package.parent):
            result = subprocess.run([sys.executable, str(SCRIPT), str(path)], text=True, capture_output=True, check=False)
            self.assertEqual(result.returncode, 1)
            self.assertIn("archive_open_failed", self.error_codes(json.loads(result.stdout)))

    def test_not_zip_and_crc_error_are_diagnostic(self):
        self.package.write_bytes(b"not a zip file")
        self.assertIn("archive_open_failed", self.error_codes(MODULE.Audit(self.package).run()))
        # Stored data permits deterministic corruption of manifest bytes.
        with zipfile.ZipFile(self.package, "w", compression=zipfile.ZIP_STORED) as archive:
            for name, value in members().items():
                archive.writestr(name, value)
        raw = self.package.read_bytes()
        marker = b'{"files":'
        pos = raw.find(marker)
        self.assertGreater(pos, 0)
        raw = raw[:pos] + b"!" + raw[pos + 1:]
        self.package.write_bytes(raw)
        self.assertIn("member_read_failed", self.error_codes(MODULE.Audit(self.package).run()))


if __name__ == "__main__":
    unittest.main()
