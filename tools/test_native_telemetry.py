#!/usr/bin/env python3
"""Run production native telemetry and QoS registration tests on the host."""
from pathlib import Path
import json
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
NATIVE = ROOT / 'game-runtime/src/native_telemetry.rs'
QOS = ROOT / 'game-runtime/src/qos.rs'
env = dict(os.environ)
env['CARGO_HOME'] = str(ROOT / 'runtime-probe/.toolchain/cargo')
env['RUSTUP_HOME'] = str(ROOT / 'runtime-probe/.toolchain/rustup')
env['RUSTFLAGS'] = '-Awarnings'
cargo = Path(env['CARGO_HOME']) / 'bin/cargo'

with tempfile.TemporaryDirectory(prefix='gta-native-telemetry-') as tmp:
    work = Path(tmp)
    (work / 'Cargo.toml').write_text('''[package]
name="native-telemetry-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde_json="1"
libc="0.2"
[lib]
path="lib.rs"
[workspace]
''')
    (work / 'lib.rs').write_text(f'#[path={json.dumps(str(NATIVE), ensure_ascii=False)}] mod actual_native_telemetry;\n' +
        f'#[path={json.dumps(str(QOS), ensure_ascii=False)}] mod qos;\n' + '''
mod native_telemetry {
    pub static REGISTRATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    pub fn register_current_thread(_: &str) { REGISTRATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }
}
#[test]
fn disabling_qos_does_not_disable_native_identity_registration() {
    use std::sync::atomic::Ordering;
    qos::configure(false);
    qos::apply(Some(5)); qos::apply(None);
    assert_eq!(native_telemetry::REGISTRATIONS.load(Ordering::Relaxed), 2);
}
''')
    subprocess.run([str(cargo), 'test', '--offline', '--manifest-path', str(work / 'Cargo.toml'),
                    '--target', 'aarch64-apple-darwin', '--', '--test-threads=1'], env=env, check=True)
