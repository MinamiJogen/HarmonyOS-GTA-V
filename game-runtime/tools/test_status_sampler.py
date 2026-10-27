#!/usr/bin/env python3
"""Exercise the actual production sampler off-device without GPU/guest linking."""
from pathlib import Path
import re
import subprocess
import tempfile


def main():
    runtime = Path(__file__).resolve().parents[1]
    lock = (runtime / 'Cargo.lock').read_text()
    version = re.search(r'\[\[package\]\]\nname = "serde_json"\nversion = "([^"]+)"', lock).group(1)
    with tempfile.TemporaryDirectory(prefix='game-native-status-contract-') as directory:
        root = Path(directory)
        (root / 'src').mkdir()
        (root / 'Cargo.toml').write_text('[package]\nname="game-native-status-contract"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nserde_json="=' + version + '"\n')
        source = (runtime / 'src/status_sampler.rs').read_text()
        (root / 'src/lib.rs').write_text('#![allow(dead_code)]\n' + source)
        subprocess.run(['cargo', 'test', '--offline', '--manifest-path', str(root / 'Cargo.toml')], check=True)
    ffi = (runtime / 'src/ffi.rs').read_text()
    status = ffi[ffi.index('fn status_value()'):]
    for forbidden in ['heap_resident_mb(', 'footprint_mb(', 'std::fs::', 'std::thread::', 'state.poll(']:
        assert forbidden not in status, f'UI status path must not contain {forbidden}'
    assert 'sample_status_metrics,\n            status_sampling_mode, poll_selected_load, status_value' in ffi
    assert 'crate::status_sampler::snapshot(crate::status_sampler::Mode {' in status
    assert 'foreground:lifecycle.allowed(), generation:lifecycle.generation, terminal:stopped || !error.is_empty()' in status
    print('Status sampler host contracts and UI-path checks passed; device latency remains unmeasured.')


if __name__ == '__main__':
    main()
