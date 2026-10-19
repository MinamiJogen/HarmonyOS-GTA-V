//! Opt-in public OHOS thread scheduling hints for the interactive render path.
//! This does not select a device power mode or request fixed CPU/GPU clocks.
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
const USER_INTERACTIVE: i32 = 5; // qos/qos.h QoS_Level, API12+, SDK26.

pub(crate) fn configure(enabled: bool) {
    // Applied at app startup before guest/GPU threads. Restart the app for an
    // on/off comparison; this function does not reclassify already-live threads.
    ENABLED.store(enabled, Ordering::Relaxed);
}

pub(crate) fn engine_thread_level(name: &str) -> Option<i32> {
    // Keep audio decode, streaming and other guest workers at their inherited
    // scheduling policy. Only the engine's named render thread is selected.
    name.contains("RenderThread").then_some(USER_INTERACTIVE)
}

#[cfg(target_env = "ohos")]
#[link(name = "qos")]
unsafe extern "C" {
    fn OH_QoS_SetThreadQoS(level: i32) -> i32;
    fn OH_QoS_GetThreadQoS(level: *mut i32) -> i32;
}

fn apply_with(name: &str, level: Option<i32>, api: impl FnOnce(i32) -> (i32, i32, i32)) {
    if !ENABLED.load(Ordering::Relaxed) { return; }
    let Some(level) = level else { return; };
    let (set_result, get_result, observed) = api(level);
    eprintln!("[thread-qos] {}", serde_json::json!({
        "thread": name,
        "requested": level,
        "setResult": set_result,
        "getResult": get_result,
        "observed": (get_result == 0).then_some(observed),
        "scope": "public thread scheduling request and immediate readback; not a device power-mode or frequency claim"
    }));
}

pub(crate) fn apply(level: Option<i32>) {
    if !ENABLED.load(Ordering::Relaxed) || level.is_none() { return; }
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unnamed");
    #[cfg(target_env = "ohos")]
    apply_with(name, level, |level| {
        let set_result = unsafe { OH_QoS_SetThreadQoS(level) };
        let mut observed = -1;
        let get_result = unsafe { OH_QoS_GetThreadQoS(&mut observed) };
        (set_result, get_result, observed)
    });
    #[cfg(not(target_env = "ohos"))]
    { let _ = name; }
}

pub(crate) fn interactive() {
    apply(Some(USER_INTERACTIVE));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_selected_threads_call_public_api_when_explicitly_enabled() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        configure(false);
        apply_with("gpu", Some(USER_INTERACTIVE), |_| { calls.set(calls.get()+1); (0, 0, USER_INTERACTIVE) });
        assert_eq!(calls.get(), 0);
        configure(true);
        for name in ["[RAGE Audio] Helper", "[RAGE Audio] Mixer", "[RAGE] HDD Stream", "pthread-1", "ResourcePlacement"] {
            assert_eq!(engine_thread_level(name), None);
            apply_with(name, engine_thread_level(name), |_| { calls.set(calls.get()+1); (0, 0, USER_INTERACTIVE) });
        }
        assert_eq!(calls.get(), 0);
        assert_eq!(engine_thread_level("[RAGE] RenderThread"), Some(USER_INTERACTIVE));
        for name in ["gpu", "game-main", "[RAGE] RenderThread"] {
            apply_with(name, Some(USER_INTERACTIVE), |level| { assert_eq!(level, USER_INTERACTIVE); calls.set(calls.get()+1); (0, 0, level) });
        }
        assert_eq!(calls.get(), 3);
        // A failed set is still read back and reported, never upgraded to success.
        apply_with("gpu", Some(USER_INTERACTIVE), |_| { calls.set(calls.get()+1); (-1, -1, -1) });
        assert_eq!(calls.get(), 4);
        configure(false);
        apply_with("gpu", Some(USER_INTERACTIVE), |_| panic!("OFF must not call QoS"));
    }
}
