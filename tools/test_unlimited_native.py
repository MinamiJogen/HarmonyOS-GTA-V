#!/usr/bin/env python3
"""Exercise actual config/argument and native-pacing policies without GPU/device."""
from pathlib import Path
import hashlib
import json
import os
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'evidence/unlimited-button-controls-20261021'


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    sources = [ROOT / 'game-runtime/src' / name for name in
               ['frame_interpolation.rs', 'gpu.rs', 'config.rs', 'engine_args.rs']]
    before = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sources}
    frame = sources[0].read_text()
    # The executable tests below run the actual pure scheduling implementation.
    # These wiring guards additionally reject accidentally reconnecting native
    # selection to the legacy bucket or inference-readback admission.
    unlimited = frame[frame.index('        fn record_unlimited('):frame.index('        fn encode(', frame.index('        fn record_unlimited('))]
    assert 'self.encode_native_snapshot(device,source)' in unlimited
    assert 'self.native_pacer.offer(ready,elapsed_ns)' in unlimited
    assert 'self.native_pacer.offer' in unlimited and 'source_ns' not in unlimited
    tick = frame[frame.index('        fn tick_unlimited('):frame.index('        pub(crate) fn prepare_output(', frame.index('        fn tick_unlimited('))]
    assert tick.index('if !self.native_pacer.urgent(elapsed)') < tick.index('self.collect(device)')
    assert tick.index('self.native_pacer.take(elapsed)') < tick.index('self.last_ai_emit')
    assert 'next_slot' not in tick and 'DELAY' not in tick
    assert 'else if ready.kind=="source" {self.native_pacer.retry(ready);' in frame
    gpu = sources[1].read_text()
    assert 'preserve_native_sr&&self.interpolation_spatial.is_none()' in gpu
    assert 'unlimited native SR recording failed' in gpu
    assert 'interpolation.presented(&display,false,None,None)' in gpu
    env = dict(os.environ)
    env['CARGO_HOME'] = str(ROOT / 'runtime-probe/.toolchain/cargo')
    env['RUSTUP_HOME'] = str(ROOT / 'runtime-probe/.toolchain/rustup')
    env['PATH'] = env['CARGO_HOME'] + '/bin:' + env['PATH']
    runs = []
    for script in ['tools/test_frame_interpolation_pairing.py', 'tools/test_frame_generation_live_state.py']:
        run = subprocess.run(['python3', str(ROOT / script)], env=env, capture_output=True, text=True, check=True)
        path = OUT / (Path(script).stem + '.log')
        path.write_text(run.stdout + run.stderr)
        runs.append({'test': script, 'log': str(path.relative_to(ROOT)), 'status': 'pass'})
    with tempfile.TemporaryDirectory(prefix='unlimited-native-host-') as temporary:
        directory = Path(temporary)
        source = r'''
#![allow(dead_code,unexpected_cfgs)]
fn code_embedded()->bool{true}
mod gpu {pub fn max_msaa()->u32{4}}
mod cpu_profile {pub fn configure(_:bool){}}
mod qos {pub fn configure(_:bool){}}
mod spatial_upscale {
 pub fn configure(_: &serde_json::Value){}
 pub fn render_size_override()->Option<(u32,u32)>{None}
 pub fn status()->serde_json::Value{serde_json::json!({})}
}
#[path=ENGINE_ARGS]
mod engine_args;
mod actual_config {
 include!(CONFIG);
 #[cfg(test)] mod unlimited_contract {
  use super::*;
  #[test] fn defaults_missing_and_stale_caps_preserve_one_zero_and_queue3() {
   let directory=std::env::temp_dir().join(format!("unlimited-config-{}",std::process::id()));
   std::fs::create_dir_all(&directory).unwrap();std::env::set_var("MUGUET_SUPPORT",&directory);
   for key in ["MUGUET_FPS","MUGUET_RES","MUGUET_SCALE"]{std::env::remove_var(key);}
   let path=directory.join("config.json");
   let defaults:serde_json::Value=serde_json::from_str(DEFAULT_CONFIG).unwrap();assert_eq!(defaults["fps_limit"],0);
   for config in [defaults,serde_json::json!({}),serde_json::json!({"fps_limit":0,"extra_args":["-FRAMELIMIT=2","-frameLimit","1","-frameLimitOther=keep"]}),serde_json::json!({"fps_limit":-1})] {
    std::fs::write(&path,serde_json::to_vec(&config).unwrap()).unwrap();let args=settings(&path);
    assert!(fps_target()<=0.0);assert_eq!(render_size(3000,2000),(1202,800));
    assert_eq!(args.iter().filter(|arg|arg.to_ascii_lowercase().starts_with("-framelimit=")).collect::<Vec<_>>(),vec![&String::from("-frameLimit=0")]);
    assert_eq!(args.iter().filter(|arg|arg.starts_with("-FrameQueueLimit=")).collect::<Vec<_>>(),vec![&String::from("-FrameQueueLimit=3")]);
   }
   std::fs::write(&path,r#"{"fps_limit":24}"#).unwrap();let args=settings(&path);assert_eq!(fps_target(),24.0);assert!(args.iter().any(|arg|arg=="-frameLimit=2"));
   std::fs::write(&path,r#"{"fps_limit":60}"#).unwrap();let args=settings(&path);assert_eq!(fps_target(),60.0);assert!(args.iter().any(|arg|arg=="-frameLimit=1"));
   std::fs::remove_dir_all(directory).unwrap();
  }
 }
}
'''
        source = source.replace('ENGINE_ARGS', json.dumps(str(sources[3]), ensure_ascii=False)).replace('include!(CONFIG)', 'include!(' + json.dumps(str(sources[2]), ensure_ascii=False) + ')')
        (directory / 'lib.rs').write_text(source)
        (directory / 'Cargo.toml').write_text('''[package]\nname="unlimited-native-contract"\nversion="0.1.0"\nedition="2021"\n[lib]\npath="lib.rs"\n[dependencies]\nanyhow="1"\nregex="1"\nserde_json="1"\n[workspace]\n''')
        env['CARGO_TARGET_DIR'] = str(ROOT / 'game-runtime/logs/unlimited-native-host/target')
        run = subprocess.run([str(Path(env['CARGO_HOME']) / 'bin/cargo'), 'test', '--offline', '--manifest-path', str(directory / 'Cargo.toml'), '--target', 'aarch64-apple-darwin', 'unlimited_contract', '--', '--test-threads=1'], env=env, capture_output=True, text=True)
        (OUT / 'actual-config-tests.log').write_text(run.stdout + run.stderr)
        if run.returncode:
            raise SystemExit(run.stdout + run.stderr)
        runs.append({'test': 'actual_config::unlimited_contract', 'status': 'pass', 'log': str((OUT / 'actual-config-tests.log').relative_to(ROOT))})
        binary=directory/'engine-tests'
        subprocess.run([str(Path(env['CARGO_HOME'])/'bin/rustc'),'--edition=2021','--test',str(sources[3]),'-o',str(binary)],env=env,check=True)
        run = subprocess.run([str(binary)],env=env,capture_output=True,text=True,check=True)
        (OUT/'engine-argument-tests.log').write_text(run.stdout+run.stderr)
        runs.append({'test':'actual engine_args.rs all 4 tests','status':'pass','log':str((OUT/'engine-argument-tests.log').relative_to(ROOT))})
    assert before == {str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sources}, 'production changed during validation'
    report={'status':'pass','sourceSHA256':before,'tests':runs,'sourceStableDuringRun':True,
            'boundaries':['Host tests exercise production scheduling, configuration and lifecycle policy, not a real Vulkan/NNRT/compositor.',
                          '80/120/240Hz model checks prove no artificial clock bucket drops in normal consume order; real GPU throughput still needs device acceptance.',
                          'SR submission fences remain required natural backpressure; permanent SR setup/record failure explicitly disables interpolation for original presentation recovery.',
                          'Unlimited native path has at most one pending source plus one ready source; maximum scheduling hold 50ms, excluding actual GPU backpressure.',
                          'AI is best effort, may be absent at high native rates, and cannot delay native snapshots or present behind already shown content.']}
    (OUT/'native-contract-tests.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps({'status':'pass','report':str(OUT/'native-contract-tests.json')},ensure_ascii=False))

if __name__=='__main__':main()
