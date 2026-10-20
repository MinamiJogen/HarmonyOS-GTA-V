#!/usr/bin/env python3
"""Compile production HUD derivation/snapshot tests against original local data.

Uses a tiny isolated host Cargo crate, so no GPU, guest or OHOS linking is needed.
Imported assets are read only. This verifies resource geometry and manifest
consistency; radar movie bars, GPS, Feed and device safe area still need a device.
"""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    root = Path(__file__).resolve().parents[1]
    frontend = root / "resources/game/data/common/data/ui/frontend.xml"
    manifest = root / "resources/game/data/manifest.json"
    inputs = (frontend, manifest)
    before = {path: hashlib.sha256(path.read_bytes()).hexdigest() for path in inputs}
    environment = dict(os.environ)
    bundled = root / "runtime-probe/.toolchain"
    if (bundled / "cargo/bin/cargo").is_file():
        environment.setdefault("CARGO_HOME", str(bundled / "cargo"))
        environment.setdefault("RUSTUP_HOME", str(bundled / "rustup"))
        environment["PATH"] = str(bundled / "cargo/bin") + os.pathsep + environment.get("PATH", "")
    cargo = shutil.which("cargo", path=environment.get("PATH"))
    if cargo is None:
        raise SystemExit("cargo unavailable; source game-runtime/scripts/env.sh first")
    environment["MUGUET_TEST_FRONTEND"] = str(frontend)
    with tempfile.TemporaryDirectory(prefix="muguet-hud-view-") as temporary:
        target = Path(temporary)
        (target / "src").mkdir()
        (target / "Cargo.toml").write_text(
            '[package]\nname="muguet-hud-view-tests"\nversion="0.1.0"\nedition="2021"\n'
            '[dependencies]\nserde_json="1"\nsha2="=0.10.9"\n'
        )
        source = root / "game-runtime/src/resource_view.rs"
        (target / "src/lib.rs").write_text(
            "#[allow(dead_code)]\n#[path = " + json.dumps(str(source), ensure_ascii=False) + "]\nmod resource_view;\n"
        )
        environment["CARGO_TARGET_DIR"] = str(target / "target")
        subprocess.run(
            [cargo, "test", "--offline", "--manifest-path", str(target / "Cargo.toml")],
            cwd=root / "game-runtime", env=environment, check=True,
        )
    after = {path: hashlib.sha256(path.read_bytes()).hexdigest() for path in inputs}
    if before != after:
        raise SystemExit("Imported source resource changed during tests")
    print("Production HUD/manifest tests passed; imported frontend and manifest remain byte-identical.")


if __name__ == "__main__":
    main()
