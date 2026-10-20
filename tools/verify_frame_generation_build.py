#!/usr/bin/env python3
"""Bind OHOS runtime feature metadata to actual ARM64 code and packaged ELF.

No target code is executed. A capability symbol alone is insufficient: inspect
its constant-return body, the real interpolation implementation markers, and
the exact archive hash. An unfamiliar compiler encoding fails closed.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import sys
import zipfile

ABI = b"mg_game_frame_generation_available"
MARKERS = (b"active_experimental_game_interpolation", b"game-frame-ai")
SCHEMA = 1
NATIVE_ENTRY = "libs/arm64-v8a/libharmony_probe.so"


def digest(data):
    return hashlib.sha256(data).hexdigest()


def bounded(data, offset, size):
    if offset < 0 or size < 0 or offset + size > len(data):
        raise ValueError("invalid ELF/archive extent")
    return data[offset:offset + size]


def elf_returns(data):
    if not data.startswith(b"\x7fELF") or ABI + b"\0" not in data:
        return []
    if data[4:6] != b"\x02\x01" or len(data) < 64:
        raise ValueError("capability requires little-endian ELF64")
    header = struct.unpack_from("<HHIQQQIHHHHHH", data, 16)
    if header[1] != 183:
        raise ValueError("capability is not AArch64")
    shoff, stride, count = header[5], header[10], header[11]
    if stride < 64 or not count:
        raise ValueError("missing ELF section headers")
    sections = [struct.unpack("<IIQQQQIIQQ", bounded(data, shoff + n * stride, 64)) for n in range(count)]
    found = []
    for section in sections:
        if section[1] not in (2, 11):  # SYMTAB / DYNSYM
            continue
        if section[6] >= count or section[9] < 24:
            raise ValueError("invalid ELF symbol table")
        names = sections[section[6]]
        strings = bounded(data, names[4], names[5])
        symbols = bounded(data, section[4], section[5])
        for at in range(0, len(symbols), section[9]):
            name, info, _, index, value, size = struct.unpack("<IBBHQQ", bounded(symbols, at, 24))
            if name >= len(strings):
                raise ValueError("invalid ELF symbol name")
            end = strings.find(b"\0", name)
            if end < 0 or strings[name:end] != ABI or index == 0:
                continue
            if index >= count or info >> 4 not in (1, 2):
                raise ValueError("capability must be a defined global function")
            target = sections[index]
            if target[1] == 8 or value < target[3] or not 8 <= size <= 32:
                raise ValueError("invalid capability function extent")
            body = bounded(data, target[4] + value - target[3], size)
            # Permit harmless AArch64 NOP / BTI-C prefixes, then a pure return.
            while body[:4] in (bytes.fromhex("1f2003d5"), bytes.fromhex("5f2403d5")):
                body = body[4:]
            if body == bytes.fromhex("20008052c0035fd6"):  # mov w0,#1; ret
                found.append(True)
            elif body in (bytes.fromhex("00008052c0035fd6"), bytes.fromhex("e0031f2ac0035fd6")):
                found.append(False)
            else:
                raise ValueError("unrecognized capability return encoding; rebuild/check validator")
    return found


def archive_members(data):
    if not data.startswith(b"!<arch>\n"):
        raise ValueError("expected a self-contained regular archive")
    offset = 8
    while offset < len(data):
        header = bounded(data, offset, 60)
        if header[58:60] != b"`\n":
            raise ValueError("invalid archive header")
        size = int(header[48:58].strip())
        body = bounded(data, offset + 60, size)
        name = header[:16].strip()
        if name.startswith(b"#1/"):  # BSD long-name prefix
            body = bounded(body, int(name[3:]), size - int(name[3:]))
        yield body
        offset += 60 + size + size % 2
    if offset != len(data):
        raise ValueError("invalid archive alignment")


def capability(data, archive=False):
    returns = []
    for member in archive_members(data) if archive else [data]:
        returns.extend(elf_returns(member))
    if not returns or len(set(returns)) != 1:
        raise ValueError("missing or conflicting frame-generation capability implementation")
    enabled = returns[0]
    markers = {marker.decode(): marker in data for marker in MARKERS}
    if enabled != all(markers.values()) or (not enabled and any(markers.values())):
        raise ValueError("capability return disagrees with actual interpolation implementation markers")
    return {"frameInterpolationAvailable": enabled, "abi": ABI.decode(),
            "capabilityReturnCodeVerified": True, "implementationMarkers": markers}


def record(archive, manifest, features, selection):
    data = archive.read_bytes()
    result = capability(data, archive=True)
    requested = "frame-interpolation" in re.split(r"[\s,]+", features.strip())
    if requested != result["frameInterpolationAvailable"]:
        raise ValueError("requested Cargo features disagree with actual runtime capability")
    value = {"schemaVersion": SCHEMA, "archiveSha256": digest(data), "archiveBytes": len(data),
             "cargoFeaturesRequested": features, "featureSelection": selection, **result}
    temporary = manifest.with_name(manifest.name + ".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(manifest)
    return value


def check_archive(archive, manifest):
    value = json.loads(manifest.read_text())
    data = archive.read_bytes()
    if value.get("schemaVersion") != SCHEMA or value.get("archiveSha256") != digest(data) or value.get("archiveBytes") != len(data):
        raise ValueError("runtime feature manifest is missing/stale or archive hash changed; rebuild native runtime")
    actual = capability(data, archive=True)
    for key, fact in actual.items():
        if value.get(key) != fact:
            raise ValueError("runtime feature metadata disagrees with archive code")
    if not actual["frameInterpolationAvailable"]:
        raise ValueError("full-game HAP requires frame-interpolation; explicit feature-off native builds are diagnostic only")
    return value


def check_hap(hap, archive, manifest):
    runtime = check_archive(archive, manifest)
    with zipfile.ZipFile(hap) as package:
        entries = [entry for entry in package.infolist() if entry.filename == NATIVE_ENTRY]
        if len(entries) != 1 or entries[0].file_size > 1536 * 1024 * 1024:
            raise ValueError("missing/duplicate/oversized game native library in HAP")
        native = package.read(entries[0])
    actual = capability(native)
    if not actual["frameInterpolationAvailable"]:
        raise ValueError("packaged native library is feature-off")
    return {"schemaVersion": SCHEMA, "archiveSha256": runtime["archiveSha256"],
            "hapSha256": digest(hap.read_bytes()), "nativeEntry": NATIVE_ENTRY,
            "nativeSha256": digest(native), **actual,
            "scope": "compiled capability only; device support, model initialization and FPS remain unverified"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("record", "check-archive", "check-hap"))
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--features", default="")
    parser.add_argument("--selection", choices=("default", "explicit"), default="explicit")
    parser.add_argument("--hap", type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    try:
        if args.mode == "record":
            result = record(args.archive, args.manifest, args.features, args.selection)
        elif args.mode == "check-archive":
            result = check_archive(args.archive, args.manifest)
        else:
            if args.hap is None:
                parser.error("check-hap requires --hap")
            result = check_hap(args.hap, args.archive, args.manifest)
        if args.report:
            args.report.write_text(json.dumps(result, indent=2) + "\n")
        print("Frame generation build verified: " + ("ON" if result["frameInterpolationAvailable"] else "OFF (explicit native diagnostic)"))
    except (OSError, ValueError, KeyError, struct.error, zipfile.BadZipFile) as error:
        parser.exit(2, "Frame generation build check failed: " + str(error) + "\n")


if __name__ == "__main__":
    main()
