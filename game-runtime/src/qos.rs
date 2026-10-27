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
    fn OH_QoS_ResetThreadQoS() -> i32;
    fn OH_QoS_GetThreadQoS(level: *mut i32) -> i32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReapplyOperation { Get, Reset, Set(i32) }

// Read only the scheduler's selected numeric fields. These are observations,
// never inputs to a private scheduling/uclamp mutation.
fn parse_scheduler(text: &str) -> serde_json::Value {
    let mut fields = serde_json::json!({
        "policy": null, "prio": null, "uclampMin": null, "uclampMax": null,
        "effectiveUclampMin": null, "effectiveUclampMax": null
    });
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue; };
        let field = match key.trim() {
            "policy" => "policy", "prio" => "prio",
            "uclamp.min" => "uclampMin", "uclamp.max" => "uclampMax",
            "effective uclamp.min" => "effectiveUclampMin",
            "effective uclamp.max" => "effectiveUclampMax",
            _ => continue,
        };
        if let Ok(value) = value.trim().parse::<u64>() { fields[field] = value.into(); }
    }
    fields
}

#[cfg(target_env = "ohos")]
fn scheduler_snapshot() -> serde_json::Value {
    use std::io::Read;
    let tid = unsafe { libc::syscall(libc::SYS_gettid) };
    let path = format!("/proc/self/task/{tid}/sched");
    let mut text = String::new();
    let read = std::fs::File::open(&path).and_then(|file| file.take(16 * 1024).read_to_string(&mut text));
    let mut fields = parse_scheduler(&text);
    fields["pid"] = std::process::id().into();
    fields["tid"] = tid.into();
    fields["path"] = path.into();
    fields["readError"] = read.err().map(|error| error.to_string()).into();
    fields
}

fn reapply_with(
    name: &str,
    mut api: impl FnMut(ReapplyOperation) -> (i32, i32),
    mut snapshot: impl FnMut() -> serde_json::Value,
) -> Option<serde_json::Value> {
    if !ENABLED.load(Ordering::Relaxed) { return None; }
    let before = snapshot();
    let (before_get, before_observed) = api(ReapplyOperation::Get);
    // Cancel the cached request first: setting the same logical level alone
    // need not reapply a scheduler policy altered during driver initialization.
    // Even a failed reset is followed by a best-effort set and honest readback.
    let reset_result = api(ReapplyOperation::Reset).0;
    let set_result = api(ReapplyOperation::Set(USER_INTERACTIVE)).0;
    let (get_result, observed) = api(ReapplyOperation::Get);
    let after = snapshot();
    Some(serde_json::json!({
        "thread": name, "stage": "after_gpu_initialization", "requested": USER_INTERACTIVE,
        "before": before, "beforeGetResult": before_get,
        "beforeObserved": (before_get == 0).then_some(before_observed),
        "resetResult": reset_result, "setResult": set_result, "getResult": get_result,
        "observed": (get_result == 0).then_some(observed), "after": after,
        "apiRequestSucceeded": reset_result == 0 && set_result == 0 && get_result == 0 && observed == USER_INTERACTIVE,
        "scope": "one public reset/set after GPU initialization; scheduler fields are immediate read-only observations, not a sustained priority or frequency guarantee"
    }))
}

pub(crate) fn gpu_initialization_begin() {
    if !ENABLED.load(Ordering::Relaxed) { return; }
    #[cfg(target_env = "ohos")]
    eprintln!("[thread-qos-scheduler] {}", serde_json::json!({
        "thread": "gpu", "stage": "before_gpu_initialization", "scheduler": scheduler_snapshot()
    }));
}

pub(crate) fn gpu_initialized() {
    if !ENABLED.load(Ordering::Relaxed) { return; }
    #[cfg(target_env = "ohos")]
    if let Some(report) = reapply_with("gpu", |operation| {
        match operation {
            ReapplyOperation::Reset => (unsafe { OH_QoS_ResetThreadQoS() }, -1),
            ReapplyOperation::Set(level) => (unsafe { OH_QoS_SetThreadQoS(level) }, -1),
            ReapplyOperation::Get => {
                let mut observed = -1;
                let result = unsafe { OH_QoS_GetThreadQoS(&mut observed) };
                (result, observed)
            }
        }
    }, scheduler_snapshot) {
        eprintln!("[thread-qos-reapply] {report}");
    }
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
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unnamed");
    // Thread identity is telemetry, independent of the optional scheduling hint.
    crate::native_telemetry::register_current_thread(name);
    if !ENABLED.load(Ordering::Relaxed) || level.is_none() { return; }
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
        assert!(reapply_with("gpu", |_| panic!("OFF must not call QoS"), || panic!("OFF must not inspect scheduler")).is_none());
        configure(true);
        let mut operations = Vec::new();
        let mut snapshots = 0;
        let report = reapply_with("gpu", |operation| {
            operations.push(operation);
            (0, USER_INTERACTIVE)
        }, || {
            snapshots += 1;
            serde_json::json!({"policy": if snapshots == 1 {0} else {2}, "uclampMax": if snapshots == 1 {300} else {1024}})
        }).unwrap();
        assert_eq!(operations, [ReapplyOperation::Get, ReapplyOperation::Reset, ReapplyOperation::Set(USER_INTERACTIVE), ReapplyOperation::Get]);
        assert_eq!(snapshots, 2);
        assert_eq!(report["before"]["uclampMax"], 300);
        assert_eq!(report["after"]["uclampMax"], 1024);
        assert_eq!(report["apiRequestSucceeded"], true);
        let mut operations = Vec::new();
        let report = reapply_with("gpu", |operation| {
            operations.push(operation);
            match operation { ReapplyOperation::Reset => (-1, -1), ReapplyOperation::Set(_) => (-1, -1), ReapplyOperation::Get => (-1, -1) }
        }, || serde_json::Value::Null).unwrap();
        assert_eq!(operations.len(), 4); // Failed reset still attempts set/get.
        assert_eq!(report["resetResult"], -1);
        assert_eq!(report["apiRequestSucceeded"], false);
        assert!(report["observed"].is_null());
        configure(false);
    }

    #[test]
    fn scheduler_parser_reports_actual_fields_without_fabricated_defaults() {
        let fields = parse_scheduler("gpu (18304)\npolicy : 0\nprio : 30\nuclamp.min : 0\nuclamp.max : 300\neffective uclamp.min : 0\neffective uclamp.max : 300\nse.avg.util_avg : 777\n");
        assert_eq!(fields["policy"], 0);
        assert_eq!(fields["prio"], 30);
        assert_eq!(fields["uclampMax"], 300);
        assert_eq!(fields["effectiveUclampMax"], 300);
        assert!(parse_scheduler("policy : denied\n")["policy"].is_null());
        assert!(parse_scheduler("")["uclampMax"].is_null());
    }
}
