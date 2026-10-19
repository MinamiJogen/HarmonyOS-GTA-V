#!/usr/bin/env python3
"""Decode a completed little-endian OHOS AArch64 snapshot, never host fixtures."""
import argparse
import hashlib
import json
import struct
from pathlib import Path


def decode(data):
    if len(data) < 472:
        raise ValueError("Truncated snapshot header")
    prefix = struct.unpack_from("<8s16I2Q", data)
    magic = prefix[0]
    fields = dict(zip([
        "version", "headerBytes", "siginfoBytes", "ucontextBytes",
        "loadCount", "previousFlags", "machine", "pcOrLrFilter",
        "mcontextOffset", "faultAddressOffset", "regsOffset", "spOffset",
        "pcOffset", "pstateOffset", "lrOffset", "siginfoAddressOffset",
        "processId", "driverLoadBias"], prefix[1:]))
    if magic != b"MGSEGV2\0" or fields["version"] != 2:
        raise ValueError("Unknown snapshot version")
    if fields["machine"] != 183:
        raise ValueError("Not an OHOS AArch64 record; host fixtures are not device evidence")
    if (fields["headerBytes"], fields["siginfoBytes"], fields["ucontextBytes"]) != (472, 128, 4560):
        raise ValueError("Unexpected SDK ABI layout")
    if not 1 <= fields["loadCount"] <= 16:
        raise ValueError("Invalid PT_LOAD count")
    expected = fields["headerBytes"] + fields["siginfoBytes"] + fields["ucontextBytes"] + 8
    if len(data) != expected or data[-8:] != b"MGDONE2\0":
        raise ValueError("Incomplete snapshot: no exact size/completion trailer")
    loads = []
    for index in range(fields["loadCount"]):
        start, end, flags, _ = struct.unpack_from("<QQII", data, 88 + index * 24)
        if end <= start:
            raise ValueError("Invalid PT_LOAD bounds")
        loads.append({"begin": hex(start), "end": hex(end), "flags": flags})
    start = fields["headerBytes"]
    info = data[start:start + fields["siginfoBytes"]]
    context = data[start + len(info):-8]

    def u64(buffer, offset, count=1):
        if offset < 0 or offset + 8 * count > len(buffer):
            raise ValueError("Register offset outside fixed ABI frame")
        return struct.unpack_from("<" + "Q" * count, buffer, offset)

    registers = u64(context, fields["regsOffset"], 31)
    pc = u64(context, fields["pcOffset"])[0]
    lr = u64(context, fields["lrOffset"])[0]
    in_driver = lambda value: any(int(x["begin"], 16) <= value < int(x["end"], 16)
                                  and x["flags"] & 1 for x in loads)
    signo, signal_errno, code = struct.unpack_from("<iii", info)
    return {
        "scope": "Raw fixed kernel siginfo/ucontext snapshot; predecessor chaining is not established by this file",
        "snapshotSha256": hashlib.sha256(data).hexdigest(),
        "processId": fields["processId"], "machine": "AArch64",
        "signal": signo, "signalErrno": signal_errno, "signalCode": code,
        "siginfoFaultAddress": hex(u64(info, fields["siginfoAddressOffset"])[0]),
        "mcontextFaultAddress": hex(u64(context, fields["faultAddressOffset"])[0]),
        "pc": hex(pc), "lr": hex(lr),
        "sp": hex(u64(context, fields["spOffset"])[0]),
        "pstate": hex(u64(context, fields["pstateOffset"])[0]),
        "registers": {"x" + str(i): hex(value) for i, value in enumerate(registers)},
        "pcInExecutableDriverLoad": in_driver(pc), "lrInExecutableDriverLoad": in_driver(lr),
        "driverLoadBias": hex(fields["driverLoadBias"]), "loads": loads,
        "previousFlags": hex(fields["previousFlags"]),
        "rawSiginfoSha256": hashlib.sha256(info).hexdigest(),
        "rawUcontextSha256": hashlib.sha256(context).hexdigest(),
        "extendedContextPointersFollowed": False, "complete": True,
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("snapshot", type=Path)
    arguments = parser.parse_args()
    print(json.dumps(decode(arguments.snapshot.read_bytes()), indent=2))
