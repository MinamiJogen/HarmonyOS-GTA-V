#!/usr/bin/env python3
"""Execute production CPU/frame-interval model tests without Vulkan or a device.

Checks exclusive nested accounting, frame-split spans, epoch/dedup, suspend,
clock faults, bounded nonblocking backpressure, cap and actual writer output.
Isolated startup checks exercise exact monitor flags, frozen creation modes under umask 000/077,
trace-off behavior and existing-file/symlink writer failures.
This does not measure device observer overhead, GPU execution or display time.
"""
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
env.pop('MUGUET_FRAME_INTERVAL_TRACE', None)
env.pop('MUGUET_SUPPORT', None)
env.pop('MUGUET_NATIVE_BENCHMARK_MONITOR', None)
cargo = Path(env['CARGO_HOME']) / 'bin/cargo'
with tempfile.TemporaryDirectory(prefix='gta-frame-interval-tests-') as tmp:
    work = Path(tmp)
    (work / 'Cargo.toml').write_text('''[package]
name="frame-interval-tests"
version="0.1.0"
edition="2021"
[dependencies]
serde_json="1"
libc="0.2"
[lib]
path="lib.rs"
[[bin]]
name="trace-integration"
path="main.rs"
test=false
[workspace]
''')
    source = ROOT / 'game-runtime/src/cpu_profile.rs'
    (work / 'lib.rs').write_text(f'#[path={json.dumps(str(source), ensure_ascii=False)}] mod cpu_profile;\n')
    subprocess.run([str(cargo), 'test', '--lib', '--offline', '--manifest-path', str(work / 'Cargo.toml'),
                    '--target', 'aarch64-apple-darwin', '--', '--test-threads=1'], env=env, check=True)
    (work / 'main.rs').write_text(f'#[path={json.dumps(str(source), ensure_ascii=False)}] mod cpu_profile;\n' + r'''
fn main() {
    use cpu_profile::frame_intervals as trace;
    use std::os::unix::{ffi::OsStringExt,fs::{OpenOptionsExt,PermissionsExt}};
    use std::time::{Duration,Instant};
    let mode=std::env::args().nth(1).unwrap();let root=std::path::PathBuf::from(std::env::var_os("MUGUET_SUPPORT").unwrap());
    let mask=if std::env::args().nth(2).as_deref()==Some("077"){0o77}else{0};
    // Each integration case runs alone. Prepare the original file's exact bits
    // first, then apply this case's umask before the actual production startup.
    unsafe{libc::umask(0);}
    let path=root.join(format!("native-frame-intervals-{}.jsonl",std::process::id()));
    if mode=="monitor_non_utf8" {std::env::set_var("MUGUET_NATIVE_BENCHMARK_MONITOR",std::ffi::OsString::from_vec(b"1\xff".to_vec()));}
    let expected_mode=if matches!(mode.as_str(),"monitor_one"|"monitor_flip_one"){0o660}else{0o600};
    let existing=matches!(mode.as_str(),"existing_file"|"existing_symlink");
    let target=root.join("existing-target.jsonl");
    if existing {
        use std::io::Write;
        let existing_path=if mode=="existing_symlink" {&target}else{&path};
        let mut file=std::fs::OpenOptions::new().create_new(true).write(true).mode(0o640).open(existing_path).unwrap();
        file.write_all(b"existing trace\n").unwrap();drop(file);
        if mode=="existing_symlink" {std::os::unix::fs::symlink(&target,&path).unwrap();}
    }
    unsafe{libc::umask(mask);}
    cpu_profile::configure(false);
    trace::initialize();
    if matches!(mode.as_str(),"off"|"off_monitor") {
        assert!(!trace::enabled());assert_eq!(trace::status()["requested"],false);assert!(!root.exists());return;
    }
    if mode=="bad_writer" || existing {
        let began=Instant::now();
        while !trace::status()["counters"]["writerFinished"].as_bool().unwrap_or(false) && began.elapsed()<Duration::from_secs(2) {std::thread::sleep(Duration::from_millis(5));}
        let status=trace::status();assert_eq!(status["enabled"],false);assert_eq!(status["counters"]["writerErrors"],1);
        assert_eq!(status["counters"]["writerFinished"],true);assert_eq!(status["counters"]["intervalsWritten"],0);
        if existing {
            assert_eq!(std::fs::read(&path).unwrap(),b"existing trace\n");
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode()&0o777,0o640);
            if mode=="existing_symlink" {assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());}
        } else {assert!(!root.exists());}
        return;
    }
    if matches!(mode.as_str(),"monitor_flip_default"|"monitor_flip_one") {
        std::env::set_var("MUGUET_NATIVE_BENCHMARK_MONITOR",if expected_mode==0o600{"1"}else{"0"});
        trace::initialize();
    }
    assert!(trace::enabled());assert!(!cpu_profile::enabled());
    trace::lifecycle(1,false);trace::guest_boundary(1,1);trace::source_boundary(1,Some(3),10,1);
    {let sample=cpu_profile::span(cpu_profile::Metric::PipelineCreate);assert!(!sample.is_sampled());std::thread::sleep(Duration::from_millis(2));}
    trace::source_boundary(1,Some(3),11,1);trace::guest_boundary(1,2);
    let began=Instant::now();
    while trace::status()["counters"]["intervalsFlushed"].as_u64().unwrap_or(0)<2 && began.elapsed()<Duration::from_secs(2) {std::thread::sleep(Duration::from_millis(5));}
    let status=trace::status();assert_eq!(status["counters"]["intervalsFlushed"],2);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode()&0o777,expected_mode);
    let text=std::fs::read_to_string(&path).unwrap();let rows:Vec<serde_json::Value>=text.lines().map(|line|serde_json::from_str(line).unwrap()).collect();
    assert_eq!(rows.len(),3);assert_eq!(rows[0]["phaseNames"].as_array().unwrap().len(),19);
    for row in &rows[1..] {assert_eq!(row["coverageComplete"],true);assert_eq!(row["durationNs"],row["accountedWallNs"]);assert!(row["phaseWallNs"]["pipelineCreate"].as_u64().unwrap()>0);}
    assert_eq!(status["counters"]["writerQueueDrops"],0);
    assert_eq!(status["counters"]["writerErrors"],0);
}
''')
    cases = ('off', 'off_monitor', 'on', 'monitor_one', 'monitor_zero',
             'monitor_invalid', 'monitor_non_utf8', 'monitor_flip_default',
             'monitor_flip_one', 'existing_file', 'existing_symlink', 'bad_writer')
    for mode, mask in ((mode, mask) for mask in ('000', '077') for mode in cases):
        run_env = dict(env)
        support = work / ('support-' + mode + '-umask-' + mask)
        if mode not in ('off', 'off_monitor', 'bad_writer'):
            support.mkdir()
        if mode not in ('off', 'off_monitor'):
            run_env['MUGUET_FRAME_INTERVAL_TRACE'] = '1'
        if mode in ('off_monitor', 'monitor_one', 'monitor_flip_one', 'existing_file', 'existing_symlink', 'bad_writer'):
            run_env['MUGUET_NATIVE_BENCHMARK_MONITOR'] = '1'
        elif mode == 'monitor_zero':
            run_env['MUGUET_NATIVE_BENCHMARK_MONITOR'] = '0'
        elif mode == 'monitor_invalid':
            run_env['MUGUET_NATIVE_BENCHMARK_MONITOR'] = 'true'
        run_env['MUGUET_SUPPORT'] = str(support)
        subprocess.run([str(cargo), 'run', '--quiet', '--offline', '--manifest-path', str(work / 'Cargo.toml'),
                        '--target', 'aarch64-apple-darwin', '--bin', 'trace-integration', '--', mode, mask], env=run_env, check=True)
        print('PASS isolated integration:', mode, 'umask=' + mask, flush=True)
