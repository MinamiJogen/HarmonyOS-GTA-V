#!/usr/bin/env python3
"""Fresh TypeScript build and all four Five Star host-side regression suites.

Run: python3 tools/test_fivestar_controls.py
Uses an installed Node.js/TypeScript toolchain (including DevEco's bundled tools),
never a previously compiled /tmp directory. Does not build or install an app.
"""
from __future__ import annotations

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
CONTROL_DIR = ROOT / "harmony/entry/src/main/ets/controls"
SOURCES = ("FiveStarInput.ts", "FiveStarLayout.ts", "FiveStarSettings.ts", "FiveStarMotionMath.ts")
SUITES = ("test_fivestar_input.js", "test_fivestar_snapshot.js", "test_fivestar_motion.js",
          "test_fivestar_motion_lifecycle.js")


def deveco_contents() -> Path:
    return Path(os.environ.get("DEVECO_CONTENTS", "/Applications/DevEco-Studio.app/Contents")).expanduser()


def find_node(explicit: str | None) -> Path:
    candidates: list[Path] = []
    if explicit:
        candidates.append(Path(explicit).expanduser())
    else:
        if os.environ.get("NODE_HOME"):
            candidates.append(Path(os.environ["NODE_HOME"]) / "bin/node")
        found = shutil.which("node")
        if found:
            candidates.append(Path(found))
        candidates.append(deveco_contents() / "tools/node/bin/node")
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate.resolve()
    raise RuntimeError("Node.js not found. Set NODE_HOME or pass --node /path/to/node.")


def typescript_pair(candidate: Path) -> tuple[Path, Path] | None:
    """Accept a package directory, lib/typescript.js, or a tsc executable."""
    candidate = candidate.expanduser().resolve()
    directories = [candidate] if candidate.is_dir() else [candidate.parent]
    directories += [directory.parent for directory in directories.copy()]
    for directory in directories:
        for lib in (directory, directory / "lib"):
            compiler, library = lib / "tsc.js", lib / "typescript.js"
            if compiler.is_file() and library.is_file():
                return compiler, library
    return None


def find_typescript(node: Path, explicit: str | None) -> tuple[Path, Path]:
    if explicit:
        result = typescript_pair(Path(explicit))
        if result:
            return result
        raise RuntimeError("--typescript must identify an installed TypeScript package, tsc.js, or typescript.js.")
    candidates: list[Path] = []
    configured = os.environ.get("FIVESTAR_TYPESCRIPT_PATH")
    if configured:
        candidates.append(Path(configured))
    candidates += [ROOT / "node_modules/typescript", ROOT / "harmony/node_modules/typescript"]
    contents = deveco_contents()
    candidates += [contents / "tools/ohpm/node_modules/typescript",
                   contents / "tools/hvigor/hvigor/node_modules/typescript",
                   contents / "tools/hvigor/hvigor-ohos-plugin/node_modules/typescript"]
    sdk = Path(os.environ.get("DEVECO_SDK_HOME", str(contents / "sdk"))).expanduser()
    candidates += [sdk / "default/openharmony/ets/build-tools/ets-loader/node_modules/typescript",
                   sdk / "openharmony/ets/build-tools/ets-loader/node_modules/typescript"]
    tsc = shutil.which("tsc")
    if tsc:
        candidates.append(Path(tsc))
    for candidate in candidates:
        result = typescript_pair(candidate)
        if result:
            return result
    # Honor Node's existing package resolution and NODE_PATH without installing anything.
    resolved = subprocess.run([str(node), "-p", "require.resolve('typescript')"], cwd=ROOT,
                              text=True, capture_output=True, timeout=20, check=False)
    if resolved.returncode == 0:
        result = typescript_pair(Path(resolved.stdout.strip()))
        if result:
            return result
    raise RuntimeError("TypeScript not found. Set FIVESTAR_TYPESCRIPT_PATH or pass --typescript /path/to/typescript.")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--node", help="Use this Node.js executable.")
    parser.add_argument("--typescript", help="Use this existing TypeScript package/compiler/library.")
    args = parser.parse_args()
    try:
        node = find_node(args.node)
        compiler, library = find_typescript(node, args.typescript)
        environment = os.environ.copy()
        environment["FIVESTAR_TYPESCRIPT_PATH"] = str(library)
        print(f"Node: {node}\nTypeScript: {compiler}", flush=True)
        with tempfile.TemporaryDirectory(prefix="fivestar-controls-") as temporary:
            output = Path(temporary)
            print(f"Fresh compilation: {output}", flush=True)
            subprocess.run([str(node), str(compiler), "--pretty", "false", "--target", "ES2020",
                            "--module", "commonjs", "--strict", "--noEmitOnError", "--outDir", str(output),
                            *(str(CONTROL_DIR / source) for source in SOURCES)],
                           cwd=ROOT, env=environment, check=True, timeout=60)
            for suite in SUITES:
                subprocess.run([str(node), str(ROOT / "tools" / suite), str(output)],
                               cwd=ROOT, env=environment, check=True, timeout=60)
        print("Five Star controls: fresh compile and all 4 suites passed.", flush=True)
        return 0
    except (RuntimeError, OSError, subprocess.TimeoutExpired) as error:
        print(f"Five Star controls: {error}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as error:
        return error.returncode if error.returncode > 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
