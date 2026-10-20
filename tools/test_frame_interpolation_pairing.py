#!/usr/bin/env python3
"""Run the production pure pairing policy's Rust tests without a GPU/NNRT.

Source game-runtime/scripts/env.sh first to select the existing Rust toolchain.
Only a temporary test binary/source is created; production inputs are read-only.
"""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile


def main():
    root = Path(__file__).resolve().parents[1]
    source = (root / "game-runtime/src/frame_interpolation.rs").read_text()
    begin = source.index("mod pairing {")
    end = source.index("pub(crate) fn monotonic_ns()", begin)
    key_begin = source.index("/// The content identity survives")
    key_end = source.index("#[cfg_attr", key_begin)
    key_tests = '''
#[test]
fn immutable_content_identity_does_not_change_on_repeat() {
    let source=OutputContentKey {epoch:1,class:"source",older_sequence:77,older_slot:12,
        newer_sequence:None,newer_slot:None};
    let repeat=source;assert_eq!(repeat,source);
    let ai=OutputContentKey {class:"ai",newer_sequence:Some(78),newer_slot:Some(15),..source};
    assert_ne!(source,ai);assert_eq!(ai,ai.clone());
    assert_ne!(ai,OutputContentKey {epoch:2,..ai});
    assert_ne!(ai,OutputContentKey {newer_slot:Some(16),..ai});
    assert_ne!(ai,OutputContentKey {newer_sequence:Some(79),..ai});
    assert_ne!(source,OutputContentKey {older_sequence:78,..source});
}
'''
    rustc = shutil.which("rustc")
    if rustc is None:
        raise SystemExit("rustc unavailable: source game-runtime/scripts/env.sh first")
    with tempfile.TemporaryDirectory(prefix="frame-pair-contract-") as temporary:
        test_source = Path(temporary) / "pairing.rs"
        binary = Path(temporary) / "pairing-test"
        test_source.write_text("#![allow(dead_code)]\n" + source[key_begin:key_end] +
                               source[begin:end] + key_tests)
        subprocess.run([rustc, "--edition=2021", "--test", str(test_source), "-o", str(binary)], check=True)
        subprocess.run([str(binary)], check=True, env=os.environ.copy())


if __name__ == "__main__":
    main()
