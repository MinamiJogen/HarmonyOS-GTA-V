#!/usr/bin/env python3
"""Run the fixed baseline's actual Rust lifecycle state and direct FFI APIs.

Source game-runtime/scripts/env.sh first. Only a temporary host test is built;
GPU/audio are fakes. This does not verify C++/ArkTS event forwarding, native
surface recovery, or device behavior.
"""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def main():
    ffi = (ROOT / "game-runtime/src/ffi.rs").read_text()
    start = ffi.index("mod app_lifecycle {")
    end = ffi.index("static TOUCHES:", start)
    # Includes the production State's three embedded tests, publication logic,
    # atomic getters and mg_game_app_foreground/mg_game_surface_lost verbatim.
    policy_and_api = ffi[start:end]
    harness = r'''
use std::sync::{Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};
mod gpu {
    pub static SUSPENDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    pub static OCCLUDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
}
mod audio {
    use super::*;
    pub static PAUSED: AtomicBool = AtomicBool::new(false);
    pub static CALLS: AtomicU64 = AtomicU64::new(0);
    pub static GENERATION_AT_LAST_CALL: AtomicU64 = AtomicU64::new(0);
    pub static ALLOWED_DURING_CALL: AtomicBool = AtomicBool::new(false);
    pub fn suspend(value: bool) {
        PAUSED.store(value, Ordering::Release);
        CALLS.fetch_add(1, Ordering::Relaxed);
        GENERATION_AT_LAST_CALL.store(app_presentation_generation(), Ordering::Release);
        if app_presentation_allowed() { ALLOWED_DURING_CALL.store(true, Ordering::Release); }
    }
}
// The direct visibility/loss APIs must never set/clear the engine stop flag.
static STOP: AtomicBool = AtomicBool::new(false);
static STAGE: Mutex<String> = Mutex::new(String::new());
static TEST_SERIAL: Mutex<()> = Mutex::new(());
// Input clearing is outside this lifecycle-only harness.
fn mg_game_release_inputs() {}
fn set_stage(stage: &str) { *STAGE.lock().unwrap() = stage.into(); }
fn fresh_lifecycle() {
    STOP.store(false, Ordering::Release);
    set_stage("game-running");
    let mut state = APP_LIFECYCLE.lock().unwrap();
    *state = app_lifecycle::State::new();
    publish_app_lifecycle(*state);
    audio::CALLS.store(0, Ordering::Release);
    audio::ALLOWED_DURING_CALL.store(false, Ordering::Release);
}
fn assert_published(allowed: bool) {
    let state = *APP_LIFECYCLE.lock().unwrap();
    assert_eq!(state.allowed(), allowed);
    assert_eq!(app_presentation_allowed(), allowed);
    assert_eq!(app_presentation_generation(), state.generation);
    assert_eq!(gpu::SUSPENDED.load(Ordering::Acquire), !allowed);
    assert_eq!(gpu::OCCLUDED.load(Ordering::Acquire), !allowed);
    assert_eq!(audio::PAUSED.load(Ordering::Acquire), !allowed);
    assert_eq!(audio::GENERATION_AT_LAST_CALL.load(Ordering::Acquire), state.generation);
    assert!(!audio::ALLOWED_DURING_CALL.load(Ordering::Acquire));
}
'''
    integration = r'''
#[test]
fn direct_foreground_api_has_new_generations_and_gates_gpu_audio() {
    let _serial = TEST_SERIAL.lock().unwrap(); fresh_lifecycle();
    assert_published(true);
    let captured = app_presentation_generation();
    mg_game_app_foreground(false); assert_published(false);
    assert_eq!(app_presentation_generation(), captured + 1);
    let calls = audio::CALLS.load(Ordering::Acquire);
    mg_game_app_foreground(false);
    assert_eq!(app_presentation_generation(), captured + 1);
    assert_eq!(audio::CALLS.load(Ordering::Acquire), calls);
    mg_game_app_foreground(true); assert_published(true);
    assert_eq!(app_presentation_generation(), captured + 2);
    mg_game_app_foreground(false); mg_game_app_foreground(true);
    assert_published(true);
    assert_eq!(app_presentation_generation(), captured + 4);
    assert!(!STOP.load(Ordering::Acquire));
    assert_eq!(&*STAGE.lock().unwrap(), "game-running");
    // Startup republishes the real lifecycle after an independent unsuspend.
    mg_game_app_foreground(false);
    let paused_generation = app_presentation_generation();
    gpu::SUSPENDED.store(false, Ordering::Release);
    publish_app_lifecycle(*APP_LIFECYCLE.lock().unwrap());
    assert_published(false);
    assert_eq!(app_presentation_generation(), paused_generation);
}

#[test]
fn direct_surface_lost_api_requires_recovery_and_foreground_cannot_clear_it() {
    let _serial = TEST_SERIAL.lock().unwrap(); fresh_lifecycle();
    mg_game_surface_lost(); assert_published(false);
    let lost_generation = app_presentation_generation();
    assert!(APP_LIFECYCLE.lock().unwrap().surface_lost);
    assert_eq!(&*STAGE.lock().unwrap(), "surface-recovery-required");
    mg_game_surface_lost();
    assert_eq!(app_presentation_generation(), lost_generation);
    mg_game_app_foreground(false); mg_game_app_foreground(true);
    assert_published(false);
    assert_eq!(app_presentation_generation(), lost_generation + 2);
    assert!(APP_LIFECYCLE.lock().unwrap().surface_lost);
    assert!(!STOP.load(Ordering::Acquire));
    assert_eq!(&*STAGE.lock().unwrap(), "surface-recovery-required");
}

#[test]
fn terminal_state_cannot_be_resumed_by_direct_foreground_api() {
    let _serial = TEST_SERIAL.lock().unwrap(); fresh_lifecycle();
    // Exercise actual State::stop/publication without pretending the rest of
    // mg_game_stop's save/input/shutdown dependencies run in this harness.
    {
        let mut state = APP_LIFECYCLE.lock().unwrap();
        STOP.store(true, Ordering::Release); state.stop(); publish_app_lifecycle(*state);
    }
    let stopped_generation = app_presentation_generation();
    mg_game_app_foreground(false); mg_game_app_foreground(true);
    assert_published(false);
    assert_eq!(app_presentation_generation(), stopped_generation + 2);
    assert!(APP_LIFECYCLE.lock().unwrap().terminal_stop);
    assert!(STOP.load(Ordering::Acquire));
}

#[test]
fn concurrent_direct_foreground_calls_publish_one_consistent_final_state() {
    let _serial = TEST_SERIAL.lock().unwrap(); fresh_lifecycle();
    std::thread::scope(|scope| {
        for _ in 0..2 { scope.spawn(|| {
            for _ in 0..100 { mg_game_app_foreground(false); mg_game_app_foreground(true); }
        }); }
    });
    assert_published(true);
    assert!(app_presentation_generation() > 0);
    assert!(!STOP.load(Ordering::Acquire));
}
'''
    rustc = os.environ.get("RUSTC") or shutil.which("rustc")
    if not rustc:
        raise SystemExit("rustc unavailable: source game-runtime/scripts/env.sh first")
    with tempfile.TemporaryDirectory(prefix="muguet-app-lifecycle-", dir="/private/tmp") as temp:
        source = Path(temp) / "app_lifecycle.rs"
        binary = Path(temp) / "app_lifecycle_tests"
        source.write_text(harness + policy_and_api + integration)
        subprocess.run([rustc, "--edition=2021", "--test", str(source), "-o", str(binary)], check=True)
        subprocess.run([str(binary)], check=True)
    print("Actual Rust lifecycle state/direct FFI host contracts passed; "
          "C++/ArkTS forwarding, surface reattachment and device behavior are not tested.")


if __name__ == "__main__":
    main()
