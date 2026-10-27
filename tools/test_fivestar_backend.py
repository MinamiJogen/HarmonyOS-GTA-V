#!/usr/bin/env python3
"""Execute the production Five Star Rust backend against host memory fixtures.

Source game-runtime/scripts/env.sh first. No device, Swift, game AOT compilation
or gameplay is involved. --wasm additionally validates the real pinned Wasm
after rewriting every input hook, using the AOT compiler's patch tests.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]

HOST = r'''
#![allow(dead_code)]
extern crate self as wasmtime;
pub struct SharedMemory { bytes: std::cell::UnsafeCell<Vec<u8>> }
impl SharedMemory {
    fn new(size:usize)->Self {Self{bytes:std::cell::UnsafeCell::new(vec![0;size])}}
    fn u8(&self,at:u64)->u8 {unsafe{(&*self.bytes.get())[at as usize]}}
    fn u32(&self,at:u64)->u32 {
        let (base,_)=mem_base_len(self);
        unsafe{(&*(base.add(at as usize)as*const std::sync::atomic::AtomicU32)).load(std::sync::atomic::Ordering::SeqCst)}
    }
    fn put_u32(&self,at:u64,value:u32) {
        let (base,_)=mem_base_len(self);
        unsafe{(&*(base.add(at as usize)as*const std::sync::atomic::AtomicU32)).store(value,std::sync::atomic::Ordering::SeqCst)}
    }
    fn put_u64(&self,at:u64,value:u64) {
        let (base,_)=mem_base_len(self);
        unsafe{(&*(base.add(at as usize)as*const std::sync::atomic::AtomicU64)).store(value,std::sync::atomic::Ordering::SeqCst)}
    }
}
pub fn mem_base_len(mem:&SharedMemory)->(*mut u8,usize){
    let bytes=unsafe{&mut *mem.bytes.get()};(bytes.as_mut_ptr(),bytes.len())
}
mod ffi {
    pub static GENERATION:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(1);
    pub fn app_presentation_generation()->u64{GENERATION.load(std::sync::atomic::Ordering::Relaxed)}
}
mod host_memory {
    pub fn span(total:usize,at:u64,size:u64,align:u64,_operation:&str)->Option<std::ops::Range<usize>> {
        if align==0||at%align!=0||at.checked_add(size)?>total as u64{return None;}
        Some(at as usize..(at+size)as usize)
    }
}

#[cfg(test)]mod engine_memory_contracts {
    use super::*;
    #[test]fn real_apply_writes_pad_zero_analog_trigger_bits_radio_and_releases_on_engine_samples(){
        let memory=SharedMemory::new(32<<20);let pad=virtual_pad::PAD_ZERO;
        virtual_pad::release();
        // The engine has already cleared/filled its pad before the host hook.
        memory.put_u32(pad+12,1<<3);
        assert!(virtual_pad::update(1<<6,-1.0,1.0,0.0,0.0,0.0,0.15,true));
        virtual_pad::set_radio(0.5,1.0,true);
        virtual_pad::apply(&memory,pad+136);
        assert_eq!(memory.u32(pad+12),1<<3,"other pads cannot consume pad zero's pending sample");
        virtual_pad::apply(&memory,pad);
        assert_eq!(memory.u32(pad+12),(1<<3)|(1<<6)|2,"digital buttons merge and RT above 30 also sets its digital bit");
        assert_eq!(memory.u8(pad+20),0);assert_eq!(memory.u8(pad+21),0);
        assert_eq!(memory.u8(pad+22),128);assert_eq!(memory.u8(pad+23),128);
        assert_eq!(memory.u8(pad+24),0);assert_eq!(memory.u8(pad+25),38);
        for control in [24_660_320u64,24_488_864] {
            assert_eq!(f32::from_bits(memory.u32(control+33_704+72*332+4)),-1.0);
            assert_eq!(f32::from_bits(memory.u32(control+33_704+72*333+4)),0.5);
        }
        let snapshot=fivestar_state::snapshot_json();
        assert_eq!(snapshot["known"],false);assert_eq!(snapshot["hookReady"],true);
        assert_eq!(snapshot["status"],16,"an empty fixture has no local player");
        assert_eq!(snapshot["vehicle"],0);
        memory.put_u64(20_159_720,1000);memory.put_u64(1008,2000);
        memory.put_u64(2000+4328,8000);memory.put_u32(8000+936,0);
        ffi::GENERATION.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        virtual_pad::apply(&memory,pad);
        let snapshot=fivestar_state::snapshot_json();
        assert_eq!(snapshot["known"],true,"hook readiness alone cannot stand in for a local player");
        assert_eq!(snapshot["status"],0);
        fivestar_state::record_control_read(2,2,229,1.0);
        let snapshot=fivestar_state::snapshot_json();
        assert!(snapshot["missionInputs"].as_array().unwrap().contains(&serde_json::json!(229)));
        ffi::GENERATION.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        assert_eq!(fivestar_state::snapshot_json()["known"],false,"another lifecycle generation cannot reuse the old context");
        // Background/focus loss clears transient controls. ioPad itself resets
        // digital history on its next update; the host must not reintroduce it.
        virtual_pad::release();memory.put_u32(pad+12,0);
        virtual_pad::apply(&memory,pad);assert_eq!(memory.u32(pad+12),0);
        for control in [24_660_320u64,24_488_864] {
            assert_eq!(f32::from_bits(memory.u32(control+33_704+72*332+4)),0.0);
            assert_eq!(f32::from_bits(memory.u32(control+33_704+72*333+4)),0.0);
        }
        virtual_pad::request_pause();
        // The app's storage watcher can run after EntryAbility requests the
        // pause. A subsequent focus-loss reset must not cancel that request.
        virtual_pad::release();
        for sample in 0..4 {
            memory.put_u32(pad+12,0);virtual_pad::apply(&memory,pad);
            assert_eq!(memory.u32(pad+12)&(1<<11),if sample<3{1<<11}else{0});
        }
        virtual_pad::release();
    }
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wasm", type=Path, help="Original pinned game.wasm for complete rewrite validation")
    args = parser.parse_args()
    cargo = shutil.which("cargo")
    if cargo is None:
        raise SystemExit("Source game-runtime/scripts/env.sh to select cargo")
    with tempfile.TemporaryDirectory(prefix="gta-fivestar-backend-") as directory:
        project = Path(directory)
        (project / "Cargo.toml").write_text(
            '[package]\nname="gta-fivestar-backend-host-tests"\nversion="0.0.0"\nedition="2021"\n'
            '[dependencies]\nserde={version="1",features=["derive"]}\nserde_json="1"\n[workspace]\n'
        )
        (project / "src").mkdir()
        modules = "\n".join(
            f"#[path={json.dumps(str(ROOT / 'game-runtime/src' / (name + '.rs')), ensure_ascii=False)}] mod {name};"
            for name in ["virtual_pad", "fivestar_state"]
        )
        (project / "src/lib.rs").write_text(HOST + modules)
        env = os.environ.copy()
        env["CARGO_TARGET_DIR"] = str(project / "target")
        subprocess.run([cargo, "test", "--offline", "--manifest-path", str(project / "Cargo.toml"),
                        "--", "--test-threads=1"], check=True, env=env)
    if args.wasm:
        env = os.environ.copy()
        env["MG_PINNED_GAME_WASM"] = str(args.wasm.resolve(strict=True))
        subprocess.run([cargo, "test", "--offline", "--locked", "--manifest-path",
                        str(ROOT / "game-runtime/aot-compiler/Cargo.toml"),
                        "--config", 'source.crates-io.replace-with="rsproxy-sparse"',
                        "--config", 'source.rsproxy-sparse.registry="sparse+https://rsproxy.cn/index/"',
                        "fivestar_tests", "--", "--test-threads=1"], check=True, env=env)


if __name__ == "__main__":
    main()
