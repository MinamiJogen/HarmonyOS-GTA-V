#!/usr/bin/env python3
"""Package only runtime data/shaders for the complete HAP's resource importer.

The matching game AOT must already be embedded in the HAP. Source files are
read-only; Wasm, standalone AOT, HAPs and local signing material are excluded.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import stat
import time
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BUILD = "8b0b5899ed"
BLOCK = 8 * 1024 * 1024


def package(game, output):
    game = game.resolve()
    output = output.resolve()
    partial = output.with_name(output.name + ".partial")
    if output.exists() or partial.exists():
        raise ValueError("输出或临时文件已存在，请指定新的输出路径。")
    if output.is_relative_to(game):
        raise ValueError("输出不能位于游戏资源目录内。")
    checksums = {}
    for line in (game / "asset-sha256.txt").read_text().splitlines():
        sha, name = line.split("  ", 1)
        if name in checksums or len(sha) != 64 or any(c not in "0123456789abcdef" for c in sha):
            raise ValueError("游戏资源校验表格式错误。")
        checksums[name] = sha
    sources = []
    folded = set()
    for subdir in ("data", f"b/{BUILD}/shaders"):
        folder = game / subdir
        if not folder.is_dir() or folder.is_symlink():
            raise ValueError("缺少资源目录或目录为符号链接：" + subdir)
        for path in sorted(folder.rglob("*")):
            if path.is_symlink():
                raise ValueError("资源中不能含符号链接：" + str(path))
            if path.is_dir():
                continue
            if not path.is_file():
                raise ValueError("资源中只能含普通文件：" + str(path))
            name = path.relative_to(game).as_posix()
            if name.casefold() in folded or name not in checksums:
                raise ValueError("资源路径冲突或缺少校验值：" + name)
            folded.add(name.casefold())
            sources.append((path, name, checksums[name]))
    names = {name for _, name, _ in sources}
    if not {"data/manifest.json", f"b/{BUILD}/shaders/index.json"} <= names:
        raise ValueError("缺少游戏数据清单或着色器索引。")
    total = sum(path.stat().st_size for path, _, _ in sources)
    output.parent.mkdir(parents=True, exist_ok=True)
    if shutil.disk_usage(output.parent).free < total + 1024 ** 3:
        raise ValueError("剩余磁盘空间不足以生成 ZIP。")
    start = time.monotonic()
    last = start

    def progress(phase, done, force=False):
        nonlocal last
        now = time.monotonic()
        if force or now - last >= 5:
            print(json.dumps({"phase": phase, "bytes": done, "total": total,
                              "percent": round(done * 100 / total, 1),
                              "seconds": round(now - start, 1)}, ensure_ascii=False), flush=True)
            last = now

    print(json.dumps({"output": str(output), "build": BUILD, "files": len(sources),
                      "uncompressedBytes": total, "compression": "deflate-level-1"}), flush=True)
    records = {}
    done = 0
    with zipfile.ZipFile(partial, "x", compression=zipfile.ZIP_DEFLATED,
                         compresslevel=1, allowZip64=True) as archive:
        for path, name, expected in sources:
            before = path.stat()
            info = zipfile.ZipInfo.from_file(path, arcname=name)
            info.compress_type = zipfile.ZIP_DEFLATED
            info._compresslevel = 1
            info.external_attr = (stat.S_IFREG | 0o644) << 16
            sha = hashlib.sha256()
            with path.open("rb") as source, archive.open(info, "w", force_zip64=True) as target:
                while block := source.read(BLOCK):
                    target.write(block)
                    sha.update(block)
                    done += len(block)
                    progress("compress", done)
            after = path.stat()
            if (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns) != (
                    after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns):
                raise ValueError("打包期间源文件发生变化：" + name)
            if sha.hexdigest() != expected:
                raise ValueError("游戏资源 SHA-256 不匹配：" + name)
            records[name] = {"bytes": before.st_size, "sha256": expected}
    progress("compress", done, True)
    done = 0
    with zipfile.ZipFile(partial) as archive:
        if set(archive.namelist()) != names or len(archive.infolist()) != len(names):
            raise ValueError("ZIP 文件列表与运行资源不一致。")
        for info in archive.infolist():
            sha = hashlib.sha256()
            with archive.open(info) as source:
                while block := source.read(BLOCK):
                    sha.update(block)
                    done += len(block)
                    progress("verify-crc-and-sha256", done)
            expected = records[info.filename]
            if info.file_size != expected["bytes"] or sha.hexdigest() != expected["sha256"]:
                raise ValueError("ZIP 文件校验失败：" + info.filename)
    progress("verify-crc-and-sha256", done, True)
    sha = hashlib.sha256()
    with partial.open("rb") as source:
        while block := source.read(BLOCK):
            sha.update(block)
    partial.rename(output)
    result = {"build": BUILD, "files": len(sources), "uncompressedBytes": total,
              "archiveBytes": output.stat().st_size, "sha256": sha.hexdigest(),
              "layout": ["data/", f"b/{BUILD}/shaders/"],
              "containsWasm": False, "containsStandaloneAot": False, "containsHap": False,
              "sourceChecksumsVerified": True, "zipCrcAndSha256Verified": True}
    output.with_name(output.name + ".sha256").write_text(sha.hexdigest() + "  " + output.name + "\n")
    output.with_name(output.name + ".verification.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, ensure_ascii=False), flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="生成完整 HAP 可直接导入的精简资源 ZIP；不含 Wasm、AOT 或 HAP。")
    parser.add_argument("--game-dir", type=Path, default=ROOT / "resources/game")
    parser.add_argument("--output", type=Path, default=ROOT / "dist/private-import-v0.8.0/HarmonyOS-GTA-V-v0.8.0-runtime-resources.zip")
    args = parser.parse_args()
    package(args.game_dir, args.output)
