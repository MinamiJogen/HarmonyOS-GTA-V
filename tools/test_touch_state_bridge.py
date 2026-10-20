#!/usr/bin/env python3
"""Run production touch observations, ownership, and control transport on host.

Source game-runtime/scripts/env.sh first. Guest native queries are fakes; these
tests verify missing-query handling, state decoding, freshness/generation,
bounded sampling, source ownership, text, and short input pulses, not devices.
"""
from pathlib import Path
import json
import os
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    cargo = shutil.which("cargo")
    if cargo is None:
        raise SystemExit("Source game-runtime/scripts/env.sh to select cargo")
    with tempfile.TemporaryDirectory(prefix="gta-touch-state-") as directory:
        project = Path(directory)
        (project / "Cargo.toml").write_text(
            '[package]\nname="gta-touch-bridge-host-tests"\nversion="0.0.0"\nedition="2021"\n'
            '[dependencies]\nserde_json="1"\n[workspace]\n'
        )
        (project / "src").mkdir()
        modules = "\n".join(
            f"#[path={json.dumps(str(ROOT / 'game-runtime/src' / (name + '.rs')), ensure_ascii=False)}] mod {name};"
            for name in ["touch_state", "touch_controls", "input_ownership", "harmony_keys"]
        )
        input_source=(ROOT / "game-runtime/src/input.rs").read_text()
        key_mapping=input_source[input_source.index("fn vk("):input_source.index("fn dom_code(")]
        codes=json.loads((ROOT / "docs/research/gta-v-mobile-touch-layout-v1.json").read_text())["keyboard"]["unique_codes"]
        key_test="\n".join(["#[test] fn all_104_named_keys_are_mapped_distinctly() {", "let mut codes=std::collections::HashSet::new();"]+[f'assert!(codes.insert(vk({json.dumps(code)}).expect("mapped")));' for code in codes]+["assert_eq!(codes.len(),104);}"])
        (project / "src/lib.rs").write_text(
            "#![allow(dead_code)]\nmod ffi {\n"
            "pub fn app_presentation_allowed()->bool {true}\n"
            "pub fn app_presentation_generation()->u64 {0}\n}\n" + modules + key_mapping + key_test
        )
        env = os.environ.copy()
        env["CARGO_TARGET_DIR"] = str(project / "target")
        subprocess.run([cargo, "test", "--offline", "--manifest-path", str(project / "Cargo.toml")],
                       check=True, env=env)


if __name__ == "__main__":
    main()
