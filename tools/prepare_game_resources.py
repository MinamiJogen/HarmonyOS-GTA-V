#!/usr/bin/env python3
"""Extract only Muguet runtime assets from a preflighted ZIP, with CRC and SHA.

Never runs scripts shipped in the archive. Original ZIP remains untouched.
"""
import hashlib
import json
import os
from pathlib import Path
import sys
import time
import zipfile

from validate_game_package import Audit

package = Path(sys.argv[1])
destination = Path(sys.argv[2]).resolve()
report_path = Path(sys.argv[3])
audit = Audit(package, hash_wasm=True).run()
if audit["status"] != "pass":
    raise SystemExit("Resource preflight failed; inspect the preflight report.")
prefix = audit["layout"]["selected_prefix"]
destination.mkdir(parents=True, exist_ok=True)
records = []
total = 0
started = time.monotonic()
with zipfile.ZipFile(package) as archive:
    jobs = []
    for info in archive.infolist():
        if not info.filename.startswith(prefix) or info.is_dir():
            continue
        relative = info.filename[len(prefix):]
        if any(p.lower() in {"env_test", "verityradio"} for p in relative.split("/")):
            continue
        if relative.startswith("b/"):
            parts = relative.split("/", 2)
            if len(parts) != 3 or not (parts[2] == "game.wasm" or parts[2].startswith("shaders/")):
                continue
        elif not relative.startswith("data/"):
            continue
        jobs.append((info, relative))
    expected = sum(i.file_size for i, _ in jobs)
    print(f"Extracting {len(jobs)} assets, {expected} bytes", flush=True)
    for index, (info, relative) in enumerate(jobs):
        output = destination / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        temporary = output.with_name(output.name + ".extracting")
        digest = hashlib.sha256()
        count = 0
        with archive.open(info) as source, temporary.open("wb") as target:
            while data := source.read(1 << 20):
                digest.update(data)
                target.write(data)
                count += len(data)
        if count != info.file_size:
            raise RuntimeError(f"Short member: {relative}")
        os.replace(temporary, output)
        total += count
        records.append({"path": relative, "bytes": count, "sha256": digest.hexdigest(), "zip_crc32": f"{info.CRC:08x}"})
        if index % 300 == 0 or index == len(jobs) - 1:
            print(f"{index + 1}/{len(jobs)} assets; {total}/{expected} bytes; {time.monotonic()-started:.1f}s", flush=True)

# Match upstream import.rs tidy_lists, only in the extracted working copy.
dlc = destination / "data/common/data/dlcList.xml"
if dlc.is_file():
    dlc.write_text("\n".join(l for l in dlc.read_text().splitlines() if not any(s in l.lower() for s in ("env_test", "verityradio"))) + "\n")
manifest_path = destination / "data/manifest.json"
manifest = json.loads(manifest_path.read_text())
before = len(manifest.get("files", []))
manifest["files"] = [r for r in manifest.get("files", []) if isinstance(r, list) and r and (destination / "data" / r[0]).is_file()]
refreshed = []
for row in manifest["files"]:
    actual_size = (destination / "data" / row[0]).stat().st_size
    if len(row) > 1 and isinstance(row[1], int) and row[1] != actual_size:
        refreshed.append({"path": row[0], "before": row[1], "after": actual_size})
        row[1] = actual_size
manifest_path.write_text(json.dumps(manifest, separators=(",", ":"), ensure_ascii=False))
normalizations = {"manifest_files_before": before, "manifest_files_after": len(manifest["files"]), "file_sizes_refreshed": refreshed, "manifest_sha256_after": hashlib.sha256(manifest_path.read_bytes()).hexdigest(), "reason": "upstream optional-directory filtering, with manifest lengths matched to actual extracted files"}
report_path.parent.mkdir(parents=True, exist_ok=True)
report_path.write_text(json.dumps({"package": str(package.resolve()), "prefix": prefix, "destination": str(destination), "member_crc_checked": True, "extracted_bytes": total, "elapsed_seconds": time.monotonic()-started, "normalizations": normalizations, "files_before_normalization": records}, indent=2) + "\n")
print(json.dumps(normalizations), flush=True)
