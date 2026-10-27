#!/usr/bin/env python3
"""Host fixtures for FG build guards and NPU-only bridge behavior."""
import importlib.util
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("fg_guard", ROOT / "tools/verify_frame_generation_build.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


def elf(enabled=True, markers=True, machine=183, body=None):
    code = body or bytes.fromhex("20008052c0035fd6" if enabled else "e0031f2ac0035fd6")
    names = b"\0" + guard.ABI + b"\0"
    symbols = bytes(24) + struct.pack("<IBBHQQ", 1, 0x12, 0, 1, 0, len(code))
    symbol_offset = 64 + len(code) + len(names)
    section_offset = symbol_offset + len(symbols)
    header = b"\x7fELF\x02\x01\x01" + bytes(9)
    header += struct.pack("<HHIQQQIHHHHHH", 1, machine, 1, 0, 0, section_offset, 0, 64, 0, 0, 64, 4, 0)
    sections = bytes(64)
    sections += struct.pack("<IIQQQQIIQQ", 0, 1, 6, 0, 64, len(code), 0, 0, 4, 0)
    sections += struct.pack("<IIQQQQIIQQ", 0, 3, 0, 0, 64 + len(code), len(names), 0, 0, 1, 0)
    sections += struct.pack("<IIQQQQIIQQ", 0, 2, 0, 0, symbol_offset, len(symbols), 2, 1, 8, 24)
    return header + code + names + symbols + sections + (b"\0".join(guard.MARKERS) if markers else b"")


def archive(payload, bsd=False):
    name = b"runtime.o/"
    if bsd:
        name = b"#1/24"
        payload = b"long-runtime-object-name" + payload
    header = name.ljust(16) + b"0".ljust(12) + b"0".ljust(6) + b"0".ljust(6)
    header += b"100644".ljust(8) + str(len(payload)).encode().ljust(10) + b"`\n"
    return b"!<arch>\n" + header + payload + (b"\n" if len(payload) % 2 else b"")


class BuildGuard(unittest.TestCase):
    def test_true_code_and_markers_required(self):
        self.assertTrue(guard.capability(archive(elf()), archive=True)["frameInterpolationAvailable"])
        with self.assertRaises(ValueError):
            guard.capability(archive(elf(True, False)), archive=True)

    def test_symbol_and_markers_cannot_disguise_stub_zero(self):
        with self.assertRaises(ValueError):
            guard.capability(archive(elf(False, True)), archive=True)
        self.assertFalse(guard.capability(archive(elf(False, False)), archive=True)["frameInterpolationAvailable"])

    def test_nonconstant_or_wrong_arch_code_is_rejected(self):
        for payload in (elf(body=bytes.fromhex("00000014c0035fd6")), elf(machine=62)):
            with self.assertRaises(ValueError):
                guard.capability(archive(payload), archive=True)

    def test_bsd_archive_and_bti_prefix(self):
        payload = elf(body=bytes.fromhex("5f2403d520008052c0035fd6"))
        self.assertTrue(guard.capability(archive(payload, bsd=True), archive=True)["frameInterpolationAvailable"])

    def test_truncated_or_thin_archive_rejected(self):
        for data in (b"!<thin>\n", archive(elf())[:-2], archive(elf())[:70]):
            with self.assertRaises((ValueError, struct.error)):
                guard.capability(data, archive=True)

    def test_manifest_hash_and_code_are_bound(self):
        with tempfile.TemporaryDirectory() as directory:
            a, m = Path(directory)/"runtime.a", Path(directory)/"features.json"
            a.write_bytes(archive(elf()))
            guard.record(a, m, "frame-interpolation", "default")
            self.assertTrue(guard.check_archive(a, m)["frameInterpolationAvailable"])
            a.write_bytes(a.read_bytes() + b"changed")
            with self.assertRaises(ValueError): guard.check_archive(a, m)

    def test_forged_manifest_or_false_feature_cannot_pass(self):
        with tempfile.TemporaryDirectory() as directory:
            a, m = Path(directory)/"runtime.a", Path(directory)/"features.json"
            a.write_bytes(archive(elf(False, False)))
            guard.record(a, m, "", "explicit")
            with self.assertRaises(ValueError): guard.check_archive(a, m)
            value = json.loads(m.read_text()); value["frameInterpolationAvailable"] = True
            m.write_text(json.dumps(value))
            with self.assertRaises(ValueError): guard.check_archive(a, m)

    def test_requested_feature_cannot_claim_another_binary(self):
        with tempfile.TemporaryDirectory() as directory:
            a, m = Path(directory)/"runtime.a", Path(directory)/"features.json"
            for enabled, features in [(True, ""), (False, "frame-interpolation")]:
                a.write_bytes(archive(elf(enabled, enabled)))
                with self.assertRaises(ValueError): guard.record(a, m, features, "explicit")

    def test_packaged_elf_checked_independently(self):
        with tempfile.TemporaryDirectory() as directory:
            a, m, h = (Path(directory)/name for name in ("runtime.a", "features.json", "app.hap"))
            a.write_bytes(archive(elf())); guard.record(a, m, "frame-interpolation", "default")
            with zipfile.ZipFile(h, "w") as package: package.writestr(guard.NATIVE_ENTRY, elf())
            self.assertTrue(guard.check_hap(h, a, m)["frameInterpolationAvailable"])
            with zipfile.ZipFile(h, "w") as package: package.writestr(guard.NATIVE_ENTRY, elf(False, False))
            with self.assertRaises(ValueError): guard.check_hap(h, a, m)
            # Error/help text containing unsupported is legitimate in an ON build.
            with zipfile.ZipFile(h, "w") as package: package.writestr(guard.NATIVE_ENTRY, elf()+b"unsupported on this device")
            self.assertTrue(guard.check_hap(h, a, m)["frameInterpolationAvailable"])

    def test_actual_shell_selection_default_and_explicit(self):
        text = (ROOT/"game-runtime/scripts/build-ohos.sh").read_text()
        start, end = text.index("task_feature_args=()"), text.index("# A failed build")
        snippet = text[start:end] + '\nprintf "selection=%s\\nfeatures=%s\\nargc=%s\\n" "$task_feature_selection" "$task_features" "${#task_feature_args[@]}"\n'
        for requested, expected, origin, count in [(None, "frame-interpolation", "default", 2), ("", "", "explicit", 0), ("frame-interpolation", "frame-interpolation", "explicit", 2), ("jit", "jit", "explicit", 2)]:
            env = os.environ.copy(); env.pop("MUGUET_RUNTIME_FEATURES", None)
            if requested is not None: env["MUGUET_RUNTIME_FEATURES"] = requested
            output = subprocess.check_output(["bash", "-c", "set -euo pipefail\n"+snippet], env=env, text=True)
            self.assertIn("selection="+origin+"\n", output)
            self.assertIn("features="+expected+"\n", output)
            self.assertIn("argc="+str(count)+"\n", output)

    def test_actual_wrapper_preflight_shell_probe_exempt_and_game_requires_sdk(self):
        text = (ROOT/"harmony/build_unsigned.sh").read_text()
        prefix = text[:text.index('mkdir -p "$task_stage_dir"')]
        fake_python = 'python3() { if [[ "$*" == *verify_frame_generation_build.py* ]]; then print checked-runtime; fi; return 0; }\n'
        with tempfile.TemporaryDirectory() as directory:
            sdk = Path(directory)/"sdk"; (sdk/"include").mkdir(parents=True); (sdk/"lib").mkdir()
            (sdk/"include/harmony_rife_c.h").write_text("fixture"); (sdk/"lib/libharmony_rife_full.a").write_text("fixture")
            for shell, probe, supplied_sdk, npu_only, succeeds, checked in [("1","0","","0",True,False),("1","1","","0",True,False),("0","0","","0",False,True),("0","0",str(sdk),"0",True,True),("0","0","","1",True,True),("0","0",str(sdk),"1",False,False),("0","0","","bad",False,False)]:
                env=os.environ.copy(); env.update(HARMONY_BUILD_STAGE=directory+"/stage",HARMONY_SHELL_ONLY=shell,HARMONY_FG_PROBE_ONLY=probe,HARMONY_COPY_BUILD_OUTPUT="0",HARMONY_GAME_EXPERIMENT_BUNDLE="",HARMONY_RIFE_SDK=supplied_sdk,HARMONY_FG_NPU_ONLY=npu_only)
                result=subprocess.run(["zsh","-c",fake_python+prefix,str(ROOT/"harmony/build_unsigned.sh")],env=env,text=True,capture_output=True)
                self.assertEqual(result.returncode==0,succeeds,result.stderr)
                self.assertEqual("checked-runtime" in result.stdout,checked)
                self.assertFalse((Path(directory)/"stage").exists())

    def test_actual_profile_pins_checked_archive_over_stale_cmake_value(self):
        text=(ROOT/"harmony/build_unsigned.sh").read_text()
        script=text.split("<<'PYSHELL'\n",1)[1].split("\nPYSHELL",1)[0]
        with tempfile.TemporaryDirectory() as directory:
            profile=Path(directory)/"profile.json"
            runtime=Path(directory)/"stage/game-runtime/artifacts/libmuguet_game_runtime.a"
            aot=Path(directory)/"stage/game-runtime/artifacts/game-embedded.o"
            old={"buildOption":{"externalNativeOptions":{"arguments":"-DKEEP=ON -DMUGUET_RIFE_SDK=/old -DGAME_RUNTIME_LIBRARY:FILEPATH=/stale/runtime.a -DGAME_AOT_OBJECT:FILEPATH=/stale/game.o -DMUGUET_SHELL_ONLY=ON"}}}
            profile.write_text(json.dumps(old))
            subprocess.run(["python3","-c",script,str(profile),"0",directory+"/sdk",str(runtime),str(aot),"0"],check=True)
            arguments=json.loads(profile.read_text())["buildOption"]["externalNativeOptions"]["arguments"]
            self.assertIn("-DKEEP=ON",arguments)
            self.assertIn("-DMUGUET_SHELL_ONLY=OFF",arguments)
            self.assertIn("-DMUGUET_FG_NPU_ONLY=OFF",arguments)
            self.assertEqual(arguments.count("-DGAME_RUNTIME_LIBRARY="),1)
            self.assertIn("-DGAME_RUNTIME_LIBRARY="+str(runtime),arguments)
            self.assertEqual(arguments.count("-DGAME_AOT_OBJECT="),1)
            self.assertIn("-DGAME_AOT_OBJECT="+str(aot),arguments)
            self.assertNotIn("/stale/",arguments)
            subprocess.run(["python3","-c",script,str(profile),"1","",str(runtime),str(aot),"1"],check=True)
            arguments=json.loads(profile.read_text())["buildOption"]["externalNativeOptions"]["arguments"]
            self.assertNotIn("GAME_RUNTIME_LIBRARY",arguments)
            self.assertNotIn("GAME_AOT_OBJECT",arguments)
            self.assertIn("-DMUGUET_SHELL_ONLY=ON",arguments)
            self.assertEqual(arguments.count("-DMUGUET_FG_NPU_ONLY="),1)
            self.assertIn("-DMUGUET_FG_NPU_ONLY=ON",arguments)

    def test_npu_only_bridge_rejects_rife_and_keeps_npu(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory)/"npu-only-test"
            subprocess.run(["c++", "-std=c++17", "-DMUGUET_INTERPOLATION_HAS_RIFE=0",
                "-I"+str(ROOT/"native-platform/include"), "-I"+str(ROOT/"native-diagnostics/src"),
                str(ROOT/"native-platform/src/fg_interpolation_bridge.cpp"),
                str(ROOT/"tools/test_fg_npu_only.cpp"), "-o", str(binary)], check=True)
            subprocess.run([str(binary)], check=True)


if __name__ == "__main__":
    unittest.main(verbosity=2)
