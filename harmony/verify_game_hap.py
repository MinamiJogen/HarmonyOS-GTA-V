#!/usr/bin/env python3
"""Verify packaged game AOT bytes and their executable ELF load mapping."""
import argparse
import hashlib
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from verify_app_icon import verify_app_icon


def sha256(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(4 << 20), b""):
            result.update(block)
    return result.hexdigest()


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hap", type=Path, help="HAP to verify; defaults to the local signed game package")
    parser.add_argument("--elf", type=Path, help="Matching unstripped libharmony_probe.so")
    parser.add_argument("--aot", type=Path, help="Exact compiler artifact expected in this package; defaults to generic game-ohos.cwasm")
    parser.add_argument("--report", type=Path, help="JSON report output path")
    parser.add_argument("--unsigned", action="store_true", help="Require an unsigned game HAP and verify its application metadata")
    args = parser.parse_args()
    elf = (args.elf or root / "harmony/entry/build/default/intermediates/cmake/default/obj/arm64-v8a/libharmony_probe.so").resolve()
    default_hap = "entry-default-unsigned.hap" if args.unsigned else "entry-default-signed.hap"
    hap = (args.hap or root / "harmony/entry/build/default/outputs/default" / default_hap).resolve()
    deveco = Path(os.environ.get("DEVECO_CONTENTS", "/Applications/DevEco-Studio.app/Contents"))
    reader = deveco / "sdk/default/openharmony/native/llvm/bin/llvm-readelf"
    symbols = subprocess.check_output([str(reader), "-W", "-s", str(elf)], text=True)
    notes = subprocess.check_output([str(reader), "-n", str(elf)], text=True)
    build_id = re.search(r"Build ID:\s*([0-9a-f]+)", notes)
    assert re.search(r"\bmg_game_start$", symbols, re.MULTILINE), "Game runtime is not linked"
    addresses = {}
    for line in symbols.splitlines():
        match = re.search(r"^\s*\d+: ([0-9a-f]+).*\b(mg_game_aot_start|mg_game_aot_end)$", line)
        if match:
            addresses[match[2]] = int(match[1], 16)
    start, end = addresses["mg_game_aot_start"], addresses["mg_game_aot_end"]
    package_metadata = {}
    with zipfile.ZipFile(hap) as package:
        assert package.testzip() is None, "HAP ZIP CRC failure"
        member = next(name for name in package.namelist() if name.endswith("/libharmony_probe.so"))
        native = package.read(member)
        package_metadata.update(verify_app_icon(package, root))
        if args.unsigned:
            app = json.loads(package.read("module.json"))["app"]
            bundle_name = app["bundleName"]
            assert bundle_name == "com.shuyuan.harmonyosgtav", "Unexpected application bundle name"
            pack_info = json.loads(package.read("pack.info"))
            assert pack_info["summary"]["app"]["bundleName"] == bundle_name, "Package metadata bundle name differs from module.json"
            package_metadata.update(
                bundleName=bundle_name, packInfoBundleNameMatches=True,
                versionName=app["versionName"], versionCode=app["versionCode"])
    if args.unsigned:
        assert build_id is not None, "Unstripped native ELF has no Build ID"
        with tempfile.TemporaryDirectory(prefix="game-hap-native-") as temporary:
            packaged_elf = Path(temporary) / "libharmony_probe.so"
            packaged_elf.write_bytes(native)
            packaged_notes = subprocess.check_output([str(reader), "-n", str(packaged_elf)], text=True)
        packaged_build_id = re.search(r"Build ID:\s*([0-9a-f]+)", packaged_notes)
        assert packaged_build_id is not None and packaged_build_id[1] == build_id[1], "Packaged native ELF does not match the supplied unstripped ELF"
        package_metadata["nativeBuildIdMatches"] = True
    assert native[:6] == b"\x7fELF\x02\x01", "Expected little-endian ELF64 native library"
    phoff = struct.unpack_from("<Q", native, 32)[0]
    phsize, phnum = struct.unpack_from("<HH", native, 54)
    segment = None
    for index in range(phnum):
        kind, flags, offset, vaddr, _, size, _, alignment = struct.unpack_from(
            "<IIQQQQQQ", native, phoff + index * phsize)
        if kind == 1 and vaddr <= start < end <= vaddr + size:
            segment = dict(vaddr=vaddr, offset=offset, filesize=size, flags=flags, alignment=alignment)
            break
    assert segment and segment["flags"] & 1, "Game AOT is not within an executable LOAD segment"
    code_offset = start - segment["vaddr"] + segment["offset"]
    packaged_hash = hashlib.sha256(memoryview(native)[code_offset:code_offset + end - start]).hexdigest()
    aot_artifact = (args.aot or root / "game-runtime/artifacts/game-ohos.cwasm").resolve()
    aot_hash = sha256(aot_artifact)
    assert end - start == aot_artifact.stat().st_size, "Packaged game AOT length differs from the selected compiler output"
    assert packaged_hash == aot_hash, "Packaged game AOT differs from the compiler output"
    signature_metadata = {}
    if args.unsigned:
        java = deveco / "jbr/Contents/Home/bin/java"
        signer = deveco / "sdk/default/openharmony/toolchains/lib/hap-sign-tool.jar"
        assert java.is_file() and signer.is_file(), "Official HAP signature verifier is unavailable"
        with tempfile.TemporaryDirectory(prefix="game-hap-signature-") as temporary:
            result = subprocess.run([str(java), "-jar", str(signer), "verify-app", "-inFile", str(hap),
                                     "-outCertChain", str(Path(temporary) / "unused.cer"),
                                     "-outProfile", str(Path(temporary) / "unused.p7b")],
                                    text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        assert result.returncode != 0 and "signature not found" in result.stdout and "No Hap Signing Block" in result.stdout, "Expected an unsigned HAP, but official verifier did not confirm it"
        signature_metadata = dict(
            officialSignatureVerifier="signature not found; No Hap Signing Block",
            officialSignatureVerifierExitCode=result.returncode,
            officialSignatureVerifierOutput=result.stdout)
    report = dict(
        buildMode="release", hapFile=str(hap), unsigned=args.unsigned,
        hapBytes=hap.stat().st_size, zipCrcChecked=True, gameRuntimeLinked=True,
        hapSha256=sha256(hap), nativeMember=member, nativeBytes=len(native),
        nativeSha256=hashlib.sha256(native).hexdigest(),
        nativeBuildId=build_id[1] if build_id else None,
        nativeDebugFile=str(elf),
        runtimeArchiveSha256=sha256(root / "game-runtime/artifacts/libmuguet_game_runtime.a"),
        gameAotStart=hex(start), gameAotEnd=hex(end), gameAotBytes=end - start,
        gameAotSha256=aot_hash, gameAotArtifact=str(aot_artifact), packagedAotBytesMatch=True, executableSegment=segment,
        scope="Static " + ("unsigned" if args.unsigned else "signed") + "-package verification; device gameplay is not established by this report",
        **package_metadata, **signature_metadata)
    if not args.unsigned:
        report["signedHap"] = str(hap)
    output = (args.report or root / "harmony/logs/game-hap-verification.json").resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
