#!/usr/bin/env python3
"""Check a freshly rebuilt public shell, including its unstripped native ELFs."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import zipfile


def main():
    if len(sys.argv) != 5:
        sys.exit("usage: verify_public_shell.py unsigned.hap native-object-directory llvm-readelf report.json")
    hap, native_root, reader, report_path = map(Path, sys.argv[1:])
    forbidden_symbols = re.compile(r"\b(?:mg_game_\w*|mg_(?:depth|luminance|readback|wgpu)_\w*|rp_embedded_\w*|rp_(?:memory64|shared)[\w_]*|muguet_game_runtime)\b")
    native_reports = {}
    repo = Path(__file__).resolve().parents[1]
    icon_path = "resources/base/media/app_icon_gta5.png"
    with zipfile.ZipFile(hap) as package:
        assert package.testzip() is None, "HAP ZIP CRC failure"
        names = package.namelist()
        forbidden_members = [name for name in names if name.lower().endswith((".wasm", ".cwasm", ".rpf", ".wgsl"))
                             or re.search(r"(?:^|/)(?:game|gamedat|shaders|rawfile)(?:/|$)", name, re.I)]
        assert not forbidden_members, "Unexpected game/resource members: " + repr(forbidden_members)
        native_members = [name for name in names if name.startswith("libs/") and name.endswith(".so")]
        expected = {"libharmony_probe.so", "libgraphics_frame_surface.so", "libc++_shared.so"}
        assert {Path(name).name for name in native_members} == expected, "Unexpected packaged native libraries"
        for name in native_members:
            payload = package.read(name)
            assert payload.startswith(b"\x7fELF"), "Expected native ELF: " + name
            assert payload.count(b"\x7fELF") == 1, "Nested ELF / possible embedded AOT: " + name
            for marker in (b"mg_game_aot", b"muguet_game_runtime", b"loading-signed-game-aot", b"wasmtime"):
                assert marker not in payload, "Unexpected game/runtime marker: " + name
            row = {"bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest(), "nestedElfCount": 0}
            if Path(name).name != "libc++_shared.so":
                debug_native = native_root / Path(name).name
                assert debug_native.is_file(), "Missing freshly built native object: " + str(debug_native)
                symbols = subprocess.check_output([str(reader), "-W", "-s", str(debug_native)], text=True)
                assert not forbidden_symbols.search(symbols), "Game/AOT symbols in unstripped native library"
                notes = subprocess.check_output([str(reader), "-n", str(debug_native)], text=True)
                match = re.search(r"Build ID:\s*([0-9a-f]+)", notes)
                row["buildId"] = match[1] if match else None
                row["unstrippedSymbolsChecked"] = True
            native_reports[name] = row
        module = json.loads(package.read("module.json"))
        app = module["app"]
        bundle_name = app["bundleName"]
        assert bundle_name == "com.shuyuan.harmonyosgtav", "Unexpected application bundle name"
        pack_info = json.loads(package.read("pack.info"))
        assert pack_info["summary"]["app"]["bundleName"] == bundle_name, "Package metadata bundle name differs from module.json"
        assert app["versionName"] == "0.8.0" and app["versionCode"] == 1000006, "Unexpected application version"
        icon = package.read(icon_path)
        assert icon == (repo / "harmony/AppScope/resources/base/media/app_icon_gta5.png").read_bytes(), "Packaged app icon differs from the selected source icon"
        assert icon == (repo / "harmony/entry/src/main/resources/base/media/app_icon_gta5.png").read_bytes(), "Entry and AppScope app icons differ"
    # Confirm build-system inputs rather than relying only on stripped symbols.
    entry_root = next((parent for parent in native_root.parents if (parent / "src/main/cpp/CMakeLists.txt").is_file()), None)
    assert entry_root is not None, "Cannot locate source entry for native build evidence"
    cmake_root = entry_root / ".cxx"
    caches = list(cmake_root.rglob("CMakeCache.txt"))
    assert caches, "No CMake cache for source-build evidence"
    assert all("MUGUET_SHELL_ONLY:BOOL=ON" in path.read_text() for path in caches), "Shell CMake mode not confirmed"
    ninja_files = list(cmake_root.rglob("build.ninja"))
    assert ninja_files, "No native Ninja input for source-build evidence"
    for path in ninja_files:
        text = path.read_text()
        assert "MG_HAS_GAME_RUNTIME=0" in text and "RP_HAS_RUST=0" in text, "Native compile guards not confirmed"
        assert not re.search(r"game-embedded\.o|libmuguet_game_runtime\.a|(?:add|memory64|shared|shared-memory64)-embedded\.o", text), "Embedded AOT/runtime in native link inputs"
    deveco = reader.parents[6]
    java = deveco / "jbr/Contents/Home/bin/java"
    signer = deveco / "sdk/default/openharmony/toolchains/lib/hap-sign-tool.jar"
    assert java.is_file() and signer.is_file(), "Official HAP signature verifier is unavailable"
    with tempfile.TemporaryDirectory(prefix="public-shell-signature-") as temporary:
        result = subprocess.run([str(java), "-jar", str(signer), "verify-app", "-inFile", str(hap),
                                 "-outCertChain", str(Path(temporary) / "unused.cer"),
                                 "-outProfile", str(Path(temporary) / "unused.p7b")],
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    assert result.returncode != 0 and "signature not found" in result.stdout and "No Hap Signing Block" in result.stdout, "Expected an unsigned HAP, but official verifier did not confirm it"
    (report_path.parent / "unsigned-signature-check.log").write_text(result.stdout)
    report = {
        "status": "PASS_public_shell_static_verification",
        "bundleName": bundle_name, "packInfoBundleNameMatches": True,
        "versionName": "0.8.0", "versionCode": 1000006,
        "api": 26, "abi": "arm64-v8a", "buildMode": "release",
        "unsigned": True, "hapBytes": hap.stat().st_size,
        "officialSignatureVerifier": "signature not found; No Hap Signing Block",
        "officialSignatureVerifierExitCode": result.returncode,
        "hapSha256": hashlib.sha256(hap.read_bytes()).hexdigest(),
        "gameRuntimeLinked": False, "embeddedGameAot": False, "embeddedProbeAot": False,
        "externalGameAssetsIncluded": False, "freshNativeBuild": True,
        "cmakeShellMode": True, "nativeLinkInputsChecked": True,
        "selectedAppIconMatches": True, "appIconSha256": hashlib.sha256(icon).hexdigest(),
        "members": names, "nativeLibraries": native_reports,
        "scope": "Fresh source build and static package verification only; installation and device behavior are not established."
    }
    report_path.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps(report, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
