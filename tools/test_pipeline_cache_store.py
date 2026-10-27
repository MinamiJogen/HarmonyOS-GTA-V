#!/usr/bin/env python3
"""Run actual pipeline cache storage code; synthetic blobs never reach a GPU."""
from pathlib import Path
import json
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
env = dict(os.environ)
env['CARGO_HOME'] = str(ROOT / 'runtime-probe/.toolchain/cargo')
env['RUSTUP_HOME'] = str(ROOT / 'runtime-probe/.toolchain/rustup')
env['RUSTFLAGS'] = '-Awarnings'
with tempfile.TemporaryDirectory(prefix='gta-pipeline-cache-store-') as tmp:
    work = Path(tmp)
    (work / 'Cargo.toml').write_text('''[package]
name="pipeline-cache-store-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde_json="1"
libc="0.2"
sha2="=0.10.9"
[lib]
path="lib.rs"
[workspace]
''')
    source = ROOT / 'game-runtime/src/pipeline_cache_store.rs'
    (work / 'lib.rs').write_text(f'#[path={json.dumps(str(source), ensure_ascii=False)}] mod actual;\n')
    subprocess.run([str(Path(env['CARGO_HOME']) / 'bin/cargo'), 'test', '--offline', '--manifest-path',
                    str(work / 'Cargo.toml'), '--target', 'aarch64-apple-darwin', '--', '--test-threads=1'], env=env, check=True)
