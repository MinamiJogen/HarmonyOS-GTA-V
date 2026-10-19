#!/usr/bin/env python3
"""Validate the exact SPIR-V exported by the device, without changing it."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("trace", type=Path)
    parser.add_argument("report", type=Path)
    parser.add_argument("--target-env", default="vulkan1.3")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    binaries = root / ".tools/SPIRV-Tools/build/tools"
    results = []
    for source in sorted(args.trace.rglob("*.spv")):
        validation = subprocess.run(
            [str(binaries / "spirv-val"), "--target-env", args.target_env, str(source)],
            capture_output=True, text=True, check=False,
        )
        assembly = source.with_suffix(".spvasm")
        disassembly = subprocess.run(
            [str(binaries / "spirv-dis"), "--offsets", str(source), "-o", str(assembly)],
            capture_output=True, text=True, check=False,
        )
        results.append({
            "file": str(source), "bytes": source.stat().st_size,
            "sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
            "validationExit": validation.returncode,
            "validationOutput": validation.stdout + validation.stderr,
            "assembly": str(assembly), "disassemblyExit": disassembly.returncode,
            "disassemblyOutput": disassembly.stdout + disassembly.stderr,
        })
    report = {
        "targetEnvironment": args.target_env, "files": results,
        "result": "pass" if results and all(r["validationExit"] == 0 for r in results) else "fail",
        "scope": "Offline SPIR-V validation; a pass does not establish driver compilation or gameplay.",
    }
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"result": report["result"], "files": len(results), "failed": [r["file"] for r in results if r["validationExit"]]}))
    return 0 if report["result"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
