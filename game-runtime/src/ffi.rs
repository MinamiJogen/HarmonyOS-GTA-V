use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::path::PathBuf;
#[path = "harmony_keys.rs"]
mod harmony_keys;
#[path = "surface_input_fit.rs"]
mod surface_input_fit;
#[path = "save_validation.rs"]
mod save_validation;
// A single atomic keeps resize callbacks, game setup and pointer mapping from
// observing width from one fold state and height from another.
static PHYSICAL_SIZE: AtomicU64 = AtomicU64::new((1u64 << 32) | 1);
pub(crate) fn physical_size() -> (u32, u32) {
    let packed = PHYSICAL_SIZE.load(Ordering::Relaxed);
    ((packed >> 32) as u32, packed as u32)
}
fn pack_size(w: u32, h: u32) -> u64 {
    ((w as u64) << 32) | h as u64
}
static STARTED: AtomicBool = AtomicBool::new(false);
// The launcher chooses a session before the process-wide guest is constructed.
// 0=new story, 1=load story, 2=demo. A later game session needs a fresh process.
static LAUNCH_ACTION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(2);
static LAUNCH_SAVE_ID: Mutex<String> = Mutex::new(String::new());
static SAVE_LOAD_STATE: Mutex<selected_load::State> = Mutex::new(selected_load::State::new());
static SESSION_RESTART_STATE: Mutex<manual_load_restart::State> = Mutex::new(manual_load_restart::State::new());
static SAVE_VALIDATION_ROOT: Mutex<Option<String>> = Mutex::new(None);
static STOP: AtomicBool = AtomicBool::new(false);
// App visibility is independent of keyboard/pointer focus. A menu may release
// input while its game and frame generation must continue to render.
mod app_lifecycle {
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct State {
        pub foreground: bool,
        pub terminal_stop: bool,
        pub surface_lost: bool,
        pub generation: u64,
    }
    impl State {
        pub const fn new() -> Self {
            Self { foreground: true, terminal_stop: false, surface_lost: false, generation: 0 }
        }
        pub fn allowed(self) -> bool { self.foreground && !self.terminal_stop && !self.surface_lost }
        pub fn foreground(&mut self, value: bool) -> bool {
            if self.foreground == value { return false; }
            self.foreground = value; self.generation = self.generation.wrapping_add(1); true
        }
        pub fn stop(&mut self) {
            if !self.terminal_stop { self.terminal_stop = true; self.generation = self.generation.wrapping_add(1); }
        }
        pub fn lose_surface(&mut self) {
            if !self.surface_lost { self.surface_lost = true; self.generation = self.generation.wrapping_add(1); }
        }
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn background_resume_preserves_session_and_has_a_new_generation() {
            let mut state = State::new();
            assert!(state.allowed());
            assert!(state.foreground(false)); assert!(!state.allowed());
            assert!(!state.terminal_stop); assert_eq!(state.generation, 1);
            assert!(state.foreground(true)); assert!(state.allowed()); assert_eq!(state.generation, 2);
            assert!(!state.foreground(true)); assert_eq!(state.generation, 2);
        }
        #[test]
        fn quick_visibility_roundtrip_still_invalidates_old_capture_generation() {
            let mut state = State::new(); let captured = state.generation;
            state.foreground(false); state.foreground(true);
            assert!(state.allowed()); assert_ne!(captured, state.generation);
        }
        #[test]
        fn foreground_never_resumes_a_terminal_stop_or_a_lost_surface() {
            let mut stopped = State::new(); stopped.stop();
            stopped.foreground(false); stopped.foreground(true);
            assert!(!stopped.allowed()); assert!(stopped.terminal_stop);
            let mut lost = State::new(); lost.lose_surface();
            lost.foreground(false); lost.foreground(true);
            assert!(!lost.allowed()); assert!(!lost.terminal_stop);
        }
    }
}
static APP_LIFECYCLE: Mutex<app_lifecycle::State> = Mutex::new(app_lifecycle::State::new());
static APP_PRESENTATION_ALLOWED: AtomicBool = AtomicBool::new(true);
static APP_PRESENTATION_GENERATION: AtomicU64 = AtomicU64::new(0);
pub(crate) fn app_presentation_allowed() -> bool { APP_PRESENTATION_ALLOWED.load(Ordering::Acquire) }
pub(crate) fn app_presentation_generation() -> u64 { APP_PRESENTATION_GENERATION.load(Ordering::Acquire) }
fn publish_app_lifecycle(state: app_lifecycle::State) {
    // Prevent new worker admission while publishing a generation. In particular
    // foreground=true must never expose the previous background generation.
    APP_PRESENTATION_ALLOWED.store(false, Ordering::Release);
    crate::native_telemetry::reset_source_window();
    APP_PRESENTATION_GENERATION.store(state.generation, Ordering::Release);
    crate::gpu::SUSPENDED.store(!state.allowed(), Ordering::Release);
    crate::gpu::OCCLUDED.store(!state.allowed(), Ordering::Release);
    crate::audio::suspend(!state.allowed());
    APP_PRESENTATION_ALLOWED.store(state.allowed(), Ordering::Release);
}
#[no_mangle]
pub extern "C" fn mg_game_app_foreground(foreground: bool) {
    if !foreground {mg_game_release_inputs();}
    let mut state = APP_LIFECYCLE.lock().unwrap();
    if state.foreground(foreground) {
        publish_app_lifecycle(*state);
        crate::status_sampler::notify();
        eprintln!("[game-lifecycle] foreground={foreground} presentationAllowed={} generation={} terminalStop={} surfaceLost={}",
            state.allowed(), state.generation, state.terminal_stop, state.surface_lost);
    }
}
#[no_mangle]
pub extern "C" fn mg_game_surface_lost() {
    mg_game_release_inputs();
    let mut state = APP_LIFECYCLE.lock().unwrap();
    state.lose_surface(); publish_app_lifecycle(*state);
    // Preserve the guest in memory. A replacement wgpu Surface is not attached
    // yet, so resuming this window or silently restarting would be dishonest.
    set_stage("surface-recovery-required");
    crate::status_sampler::notify();
}
static TOUCHES: Mutex<Vec<(u32,f32,f32)>> = Mutex::new(Vec::new());
static INPUTS: AtomicU64 = AtomicU64::new(0);
static WORLD_PLAYER_OBSERVED: AtomicBool = AtomicBool::new(false);
pub(crate) fn world_player_observed() -> bool { WORLD_PLAYER_OBSERVED.load(Ordering::Acquire) }
pub(crate) fn reset_touch_origin() { TOUCHES.lock().unwrap().clear(); }
static STAGE: Mutex<String> = Mutex::new(String::new());
static ERROR: Mutex<String> = Mutex::new(String::new());
thread_local! { static STATUS: std::cell::RefCell<CString> = std::cell::RefCell::new(CString::new("{}").unwrap()); }
pub fn stop_requested() -> bool { STOP.load(Ordering::Acquire) }

// Only original-engine deserialization confirms a selected load. Reads, a
// rendered world and a completed ManualLoad operation are not that event.
mod selected_load {
    use std::time::{Duration, Instant};
    pub const TIMEOUT_SECONDS: u64 = 300;
    const FAILURE: &str = "原引擎未能加载所选存档，请返回首页后重试。";
    const NO_ELIGIBLE: &str = "原引擎未找到可加载的所选存档，可能外层头或内容不兼容；请返回首页重试。";
    const TIMEOUT: &str = "5 分钟内未收到原引擎反序列化成功确认，本次加载按失败处理；请返回首页重试。";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Completion { Loaded, Failed(&'static str) }
    pub struct State {
        started: Option<Instant>,
        pub succeeded: bool,
        pub error: &'static str,
        pub elapsed_ms: u64,
    }
    impl State {
        pub const fn new() -> Self {
            Self { started: None, succeeded: false, error: "", elapsed_ms: 0 }
        }
        pub fn begin(&mut self, now: Instant) { *self = Self { started: Some(now), ..Self::new() }; }
        fn finish(&mut self, result: Completion, now: Instant) -> Option<Completion> {
            let start = self.started.take()?;
            self.elapsed_ms = now.saturating_duration_since(start).as_millis().min(u64::MAX as u128) as u64;
            self.succeeded = result == Completion::Loaded;
            self.error = match result { Completion::Loaded => "", Completion::Failed(error) => error };
            Some(result)
        }
        pub fn poll(&mut self, now: Instant) -> Option<Completion> {
            let start = self.started?;
            self.elapsed_ms = now.saturating_duration_since(start).as_millis().min(u64::MAX as u128) as u64;
            if now.saturating_duration_since(start) >= Duration::from_secs(TIMEOUT_SECONDS) {
                self.finish(Completion::Failed(TIMEOUT), now)
            } else { None }
        }
        pub fn abort(&mut self, reason: &'static str, now: Instant) -> Option<Completion> {
            self.finish(Completion::Failed(reason), now)
        }
        pub fn observe(&mut self, line: &str, now: Instant) -> Option<Completion> {
            if let Some(timeout) = self.poll(now) { return Some(timeout); }
            self.started?;
            if line.contains("CSavegameLoad::DeSerialize - Game successfully loaded") {
                return self.finish(Completion::Loaded, now);
            }
            // #45261 prints this only after CheckEnumeration has completed.
            // Header-rejected files can disappear here without BeginLoad ever
            // being called; do not silently treat the resulting new world as a load.
            if line.split_once("ms_NumberOfSaveGameSlotsForAutoload =")
                .is_some_and(|(_, tail)| tail.split_ascii_whitespace().next() == Some("0"))
                || line.contains("Bypassing autoload (and disabling autosave)")
            {
                return self.finish(Completion::Failed(NO_ELIGIBLE), now);
            }
            // Original #45280/#45281 and their session/queue callers. The
            // savegame channel is explicitly enabled at debug1 for selected loads.
            if [
                "CSavegameLoad::DeSerialize - LoadBlockData failed",
                "CSavegameLoad::DoLoad - BeginLoad failed",
                "CSavegameLoad::DoLoad - BeginGetCreator failed",
                "CSavegameLoad::DoLoad - SAVE_GAME_DIALOG_CHECK_GET_CREATOR_FAILED1",
                "CSavegameLoad::DoLoad - SAVE_GAME_DIALOG_CHECK_GET_CREATOR_FAILED2",
                "CGameSessionStateMachine::ProcessAutoLoadSaveGameState - CSavegameLoad::DeSerialize failed",
                "CGameSessionStateMachine::ProcessHandleLoadedSaveGameState - CSavegameLoad::DeSerialize failed",
                "CSavegameQueuedOperation_LoadMostRecentSave::Update - CSavegameLoad::GenericLoad() has failed",
            ].iter().any(|marker| line.contains(marker)) {
                self.finish(Completion::Failed(FAILURE), now)
            } else { None }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        const SUCCESS: &str = "Debug1: [savegame] CSavegameLoad::DeSerialize - Game successfully loaded \n";

        #[test]
        fn only_original_deserialization_confirms_loading() {
            let now = Instant::now();
            let mut state = State::new();
            assert_eq!(state.observe(SUCCESS, now), None);
            state.begin(now);
            for line in ["selected bootstrap read observed", "TASK_PLAYER_ON_FOOT constructed",
                "CSavegameQueue::Update - popping ManualLoad operation MEM_CARD_COMPLETE",
                "CSavegameQueue::Update - popping Autosave operation MEM_CARD_COMPLETE"]
            {
                assert_eq!(state.observe(line, now), None);
                assert!(!state.succeeded);
                assert!(state.error.is_empty());
            }
            assert_eq!(state.observe(SUCCESS, now + Duration::from_secs(20)), Some(Completion::Loaded));
            assert!(state.succeeded);
            assert_eq!(state.elapsed_ms, 20_000);
            assert_eq!(state.poll(now + Duration::from_secs(1000)), None);
            assert!(state.error.is_empty());
        }

        #[test]
        fn exact_engine_failures_are_terminal_and_late_success_cannot_override() {
            let now = Instant::now();
            for marker in [
                "ms_NumberOfSaveGameSlotsForAutoload = 0 Time = 100",
                "Bypassing autoload (and disabling autosave)",
                "CSavegameLoad::DoLoad - BeginGetCreator failed",
                "CSavegameLoad::DoLoad - SAVE_GAME_DIALOG_CHECK_GET_CREATOR_FAILED1",
                "CSavegameLoad::DoLoad - SAVE_GAME_DIALOG_CHECK_GET_CREATOR_FAILED2",
                "CSavegameLoad::DoLoad - BeginLoad failed",
                "CSavegameLoad::DeSerialize - LoadBlockData failed for some reason",
                "CGameSessionStateMachine::ProcessAutoLoadSaveGameState - CSavegameLoad::DeSerialize failed",
                "CGameSessionStateMachine::ProcessHandleLoadedSaveGameState - CSavegameLoad::DeSerialize failed",
                "CSavegameQueuedOperation_LoadMostRecentSave::Update - CSavegameLoad::GenericLoad() has failed",
            ] {
                let mut state = State::new();
                state.begin(now);
                assert!(matches!(state.observe(marker, now), Some(Completion::Failed(_))), "{marker}");
                assert!(!state.succeeded);
                assert!(!state.error.is_empty());
                assert_eq!(state.observe(SUCCESS, now), None);
                assert!(!state.succeeded);
            }
        }

        #[test]
        fn enumeration_nonzero_or_unrelated_count_is_not_failure() {
            let now = Instant::now();
            let mut state = State::new();
            state.begin(now);
            for line in ["ms_NumberOfSaveGameSlotsForAutoload = 1 Time = 100",
                "ms_NumberOfSaveGameSlotsForAutoload = 10 Time = 100",
                "ms_NumberOfSaveGameSlots = 0 Time = 100"]
            {
                assert_eq!(state.observe(line, now), None);
            }
        }

        #[test]
        fn timeout_is_explicit_failure_at_boundary_and_never_late_success() {
            let now = Instant::now();
            let mut state = State::new();
            state.begin(now);
            assert_eq!(state.poll(now + Duration::from_secs(TIMEOUT_SECONDS - 1)), None);
            assert!(matches!(state.observe(SUCCESS, now + Duration::from_secs(TIMEOUT_SECONDS)),
                Some(Completion::Failed(_))));
            assert!(!state.succeeded);
            assert_eq!(state.error, TIMEOUT);
            assert_eq!(state.observe(SUCCESS, now + Duration::from_secs(301)), None);
            assert!(!state.succeeded);
        }

        #[test]
        fn stop_or_runtime_error_cannot_leave_an_active_load_or_mark_success() {
            let now = Instant::now();
            let mut state = State::new();
            state.begin(now);
            assert_eq!(state.abort("runtime stopped", now), Some(Completion::Failed("runtime stopped")));
            assert_eq!(state.error, "runtime stopped");
            assert_eq!(state.abort("other error", now), None);
            assert_eq!(state.observe(SUCCESS, now), None);
            assert!(!state.succeeded);
        }
    }
}

// The native guest cannot reliably reconstruct its process-wide state after a
// pause-menu load. Publish the completed original operation to the launcher;
// only a new process's DeSerialize can confirm that the requested save loaded.
mod manual_load_restart {
    const PUSH: &str = "CSavegameQueue::Push - pushing ManualLoad operation. Setting its status to MEM_CARD_BUSY";
    const NAME: &str = "CSavegameFilenames::MakeValidSaveNameForLocalFile - ms_FilenameOfLocalFile is now ";
    const LOAD_COMPLETE: &str = "CSavegameQueuedOperation_ManualLoad::Update - setting status to MEM_CARD_COMPLETE (1)";
    const OTHER_COMPLETE: &str = "CSavegameQueuedOperation_ManualLoad::Update - setting status to MEM_CARD_COMPLETE (2)";
    const OP_ERROR: &str = "CSavegameQueuedOperation_ManualLoad::Update - setting status to MEM_CARD_ERROR";
    const POP: &str = "CSavegameQueue::Update - popping ManualLoad operation ";
    const NO_SLOT: &str = "原游戏手动加载已完成读取，但未能确认有效的所选存档编号；请返回首页选择存档。";

    #[derive(Default)]
    struct Operation { slot: Option<String>, loaded: bool }
    pub struct State {
        operation: Option<Operation>,
        pub id: u64,
        pub action: &'static str,
        pub save_id: String,
        pub error: &'static str,
    }
    impl State {
        pub const fn new() -> Self {
            Self { operation: None, id: 0, action: "", save_id: String::new(), error: "" }
        }
        pub fn cancel_active(&mut self) { self.operation = None; }
        // Returns true only when a new launcher request is published. Requests
        // remain stable until the process exits, including repeated log/status calls.
        pub fn observe(&mut self, line: &str, story: bool, bootstrap_pending: bool) -> bool {
            if !story || bootstrap_pending {
                self.cancel_active();
                return false;
            }
            if !self.action.is_empty() { return false; }
            if exact_suffix(line, PUSH).is_some() {
                // A new queue start cannot inherit a previous operation's slot.
                self.operation = Some(Operation::default());
                self.error = "";
                return false;
            }
            let Some(operation) = self.operation.as_mut() else { return false; };
            if let Some((_, name)) = line.split_once(NAME) {
                // No enumeration/header log, path, backup or trailing token is
                // accepted. The last exact filename in this operation owns it.
                operation.slot = normal_slot(name.trim_end()).map(str::to_owned);
                operation.loaded = false;
                return false;
            }
            if exact_suffix(line, LOAD_COMPLETE).is_some() {
                operation.loaded = true;
                return false;
            }
            // COMPLETE (2) belongs to the delete/dialog path in original #45385,
            // whereas COMPLETE (1) follows GenericLoad + HandleLoadedSaveGame.
            if exact_suffix(line, OTHER_COMPLETE).is_some() || line.contains(OP_ERROR) {
                self.cancel_active();
                return false;
            }
            if let Some((_, result)) = line.split_once(POP) {
                let operation = self.operation.take().unwrap();
                if result.trim_end() != "MEM_CARD_COMPLETE" || !operation.loaded { return false; }
                let Some(slot) = operation.slot else {
                    self.error = NO_SLOT;
                    return false;
                };
                self.id += 1;
                self.action = "load";
                self.save_id = format!("userdata/Documents/Rockstar Games/GTA V/Default/{slot}");
                self.error = "";
                return true;
            }
            false
        }
    }
    fn exact_suffix<'a>(line: &'a str, marker: &str) -> Option<&'a str> {
        let (_, tail) = line.split_once(marker)?;
        if tail.trim_end().is_empty() { Some(tail) } else { None }
    }
    fn normal_slot(name: &str) -> Option<&str> {
        let bytes = name.as_bytes();
        if bytes.len() != 9 || !bytes.starts_with(b"SGTA5") || !bytes[5..].iter().all(u8::is_ascii_digit) {
            return None;
        }
        let slot = bytes[5..].iter().fold(0u32, |n, d| n * 10 + (d - b'0') as u32);
        (slot <= 15).then_some(name)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        fn observe(state: &mut State, line: &str) -> bool { state.observe(line, true, false) }
        fn filename(slot: &str) -> String { format!("[savegame] {NAME}{slot}\n") }
        fn pop(status: &str) -> String { format!("[savegame_queue] {POP}{status}") }
        fn successful_operation(state: &mut State, slot: &str) -> bool {
            observe(state, PUSH);
            observe(state, &filename(slot));
            observe(state, LOAD_COMPLETE);
            observe(state, &pop("MEM_CARD_COMPLETE"))
        }
        #[test]
        fn requires_original_load_branch_and_queue_terminal_then_publishes_once() {
            let mut state = State::new();
            assert!(!observe(&mut state, &filename("SGTA50002")));
            assert!(!observe(&mut state, &pop("MEM_CARD_COMPLETE")));
            observe(&mut state, PUSH);
            observe(&mut state, &filename("SGTA50002"));
            assert!(!observe(&mut state, LOAD_COMPLETE));
            assert!(state.action.is_empty());
            assert!(observe(&mut state, &pop("MEM_CARD_COMPLETE")));
            assert_eq!(state.id, 1);
            assert_eq!(state.action, "load");
            assert_eq!(state.save_id, "userdata/Documents/Rockstar Games/GTA V/Default/SGTA50002");
            assert!(!successful_operation(&mut state, "SGTA50001"));
            assert_eq!(state.id, 1);
            assert!(state.save_id.ends_with("SGTA50002"));
        }
        #[test]
        fn cancel_error_delete_completion_and_successless_pop_never_restart() {
            for terminal in [pop("MEM_CARD_ERROR"), OP_ERROR.into(), OTHER_COMPLETE.into(), pop("MEM_CARD_COMPLETE")] {
                let mut state = State::new();
                observe(&mut state, PUSH);
                observe(&mut state, &filename("SGTA50002"));
                assert!(!observe(&mut state, &terminal));
                assert!(!observe(&mut state, LOAD_COMPLETE));
                assert!(!observe(&mut state, &pop("MEM_CARD_COMPLETE")));
                assert!(state.action.is_empty());
            }
        }
        #[test]
        fn missing_or_malformed_latest_slot_is_explicit_error_and_can_retry() {
            for name in [None, Some("SGTA50016"), Some("SGTA50002.bak"), Some("../SGTA50002"),
                Some("SGTA50002 extra"), Some("SGTA5０００２"), Some("SGTA5002")] {
                let mut state = State::new();
                observe(&mut state, PUSH);
                observe(&mut state, &filename("SGTA50001"));
                // A duplicate/new start must discard this previous selection.
                observe(&mut state, PUSH);
                if let Some(name) = name { observe(&mut state, &filename(name)); }
                observe(&mut state, LOAD_COMPLETE);
                assert!(!observe(&mut state, &pop("MEM_CARD_COMPLETE")));
                assert_eq!(state.error, NO_SLOT);
                assert!(state.action.is_empty());
                assert!(successful_operation(&mut state, "SGTA50015"));
                assert_eq!(state.error, "");
            }
        }
        #[test]
        fn only_latest_filename_and_current_story_operation_are_used() {
            let mut state = State::new();
            observe(&mut state, PUSH);
            observe(&mut state, &filename("SGTA50000"));
            observe(&mut state, "CSavegameList::PrintFilenames - slot 0 contains SGTA50015");
            observe(&mut state, &filename("SGTA50002"));
            observe(&mut state, LOAD_COMPLETE);
            assert!(observe(&mut state, &pop("MEM_CARD_COMPLETE")));
            assert!(state.save_id.ends_with("SGTA50002"));
            for (story, pending) in [(false, false), (true, true)] {
                let mut state = State::new();
                for line in [PUSH.to_owned(), filename("SGTA50002"), LOAD_COMPLETE.into(), pop("MEM_CARD_COMPLETE")] {
                    assert!(!state.observe(&line, story, pending));
                }
                assert!(state.action.is_empty());
            }
        }
        #[test]
        fn pending_bootstrap_or_stop_discards_active_context_and_marker_suffixes_are_exact() {
            let mut state = State::new();
            observe(&mut state, PUSH);
            observe(&mut state, &filename("SGTA50002"));
            state.observe("bootstrap still loading", true, true);
            assert!(!observe(&mut state, LOAD_COMPLETE));
            assert!(!observe(&mut state, &pop("MEM_CARD_COMPLETE")));
            observe(&mut state, PUSH);
            observe(&mut state, &filename("SGTA50002"));
            state.cancel_active();
            assert!(!observe(&mut state, LOAD_COMPLETE));
            assert!(!observe(&mut state, &pop("MEM_CARD_COMPLETE")));
            observe(&mut state, &format!("{PUSH} unexpected"));
            observe(&mut state, &filename("SGTA50002"));
            observe(&mut state, LOAD_COMPLETE);
            assert!(!observe(&mut state, &pop("MEM_CARD_COMPLETE")));
        }
    }
}

fn publish_selected_load_completion(completion: Option<selected_load::Completion>) {
    if let Some(completion) = completion {
        crate::fs::finish_bootstrap_save();
        match completion {
            selected_load::Completion::Loaded => eprintln!("[game-launch] selected save deserialized successfully; full save list restored"),
            selected_load::Completion::Failed(error) => eprintln!("[game-launch] selected save load failed; full save list restored; {error}"),
        }
    }
}
fn abort_selected_load(reason: &'static str) {
    let mut state = SAVE_LOAD_STATE.lock().unwrap();
    publish_selected_load_completion(state.abort(reason, std::time::Instant::now()));
}

pub(crate) fn observe_engine_line(line: &str) {
    let action = LAUNCH_ACTION.load(Ordering::Acquire);
    if action == 1 {
        let mut state = SAVE_LOAD_STATE.lock().unwrap();
        publish_selected_load_completion(state.observe(line, std::time::Instant::now()));
    }
    let bootstrap_pending = crate::fs::bootstrap_save_pending();
    let mut restart = SESSION_RESTART_STATE.lock().unwrap();
    if restart.observe(line, action <= 1 && !stop_requested(), bootstrap_pending) {
        eprintln!("[game-launch] original ManualLoad completed reading {}; requesting fresh-process load id={}; deserialization is not yet confirmed",restart.save_id,restart.id);
    }
    drop(restart);
    if !WORLD_PLAYER_OBSERVED.load(Ordering::Relaxed) && line.contains("TASK_PLAYER_ON_FOOT constructed") {
        WORLD_PLAYER_OBSERVED.store(true, Ordering::Release);
        eprintln!("[game] world player task observed; game scene may now be reported");
    }
}
pub fn set_error(e: impl ToString) { abort_selected_load("加载期间游戏运行时发生错误，未确认恢复成功；请返回首页重试。"); SESSION_RESTART_STATE.lock().unwrap().cancel_active(); let e=e.to_string(); eprintln!("[game] ERROR {e}"); *ERROR.lock().unwrap() = e; *STAGE.lock().unwrap() = "error".into(); crate::heapcheck::request_snapshot("game-error"); crate::status_sampler::notify(); }
fn render_size(width:u32,height:u32)->(u32,u32) {
    crate::config::render_size(width,height)
}
pub(crate) fn set_stage(s: &str) { eprintln!("[game] {s}"); *STAGE.lock().unwrap()=s.into(); }
unsafe fn string(p:*const c_char)->anyhow::Result<String> { anyhow::ensure!(!p.is_null(),"null string"); Ok(CStr::from_ptr(p).to_str()?.into()) }
/// Compile capability only: model/device initialization is checked at runtime.
#[no_mangle]
pub extern "C" fn mg_game_frame_generation_available()->i32 {
    if cfg!(feature="frame-interpolation") {1}else{0}
}
#[no_mangle]
pub unsafe extern "C" fn mg_game_set_launch(action:*const c_char,save_id:*const c_char)->i32 {
    if STARTED.load(Ordering::Acquire) { return 2; }
    let action=match string(action).as_deref() { Ok("new")=>0,Ok("load")=>1,Ok("demo")=>2,_=>return 1 };
    let id=match string(save_id) { Ok(s)=>s,Err(_)=>return 1 };
    if action==1 && (id.is_empty() || id.len()>1024 || std::path::Path::new(&id).is_absolute() ||
        std::path::Path::new(&id).components().any(|p| !matches!(p,std::path::Component::Normal(_))) ||
        id.contains('\\') || id.contains(':')) { return 1; }
    // Demo starts its own sandbox flow and does not enter story autoload.
    // Never accept a selection that would silently start an unrelated world.
    if action==1 && !id.starts_with("userdata/") { return 1; }
    if action!=1 && !id.is_empty() { return 1; }
    *LAUNCH_SAVE_ID.lock().unwrap()=id;
    LAUNCH_ACTION.store(action,Ordering::Release);
    0
}
#[no_mangle]
pub unsafe extern "C" fn mg_game_start(window:*mut c_void,width:u32,height:u32,files_dir:*const c_char)->i32 {
    if window.is_null() || width==0 || height==0 { set_error("missing native window or size"); return 1; }
    let path=match string(files_dir) {Ok(s)=>s,Err(e)=>{set_error(e);return 1}};
    if mg_game_frame_generation_available()==0 {
        let cfg=std::fs::read(PathBuf::from(&path).join("config.json")).ok()
            .and_then(|bytes|serde_json::from_slice::<serde_json::Value>(&bytes).ok()).unwrap_or(serde_json::Value::Null);
        let root=PathBuf::from(&path);
        if crate::config::select_interpolation_backend(&cfg,root.join("frame-interpolation-npu.request").is_file(),root.join("frame-interpolation-rife.request").is_file()).is_some() {
            eprintln!("[frame-interpolation] startup rejected: runtime was built without frame-interpolation feature; requested config preserved");return 4;
        }
    }
    if STARTED.swap(true,Ordering::SeqCst) { return 2; }
    STOP.store(false,Ordering::Release);
    PHYSICAL_SIZE.store(pack_size(width,height),Ordering::Relaxed);
    let window=window as usize;
    let result=std::thread::Builder::new().name("game-main".into()).stack_size(32<<20).spawn(move|| {
        // Owned by game-main, including startup failure and normal return. Its
        // Drop stops/joins off the UI thread and exports a final cached state.
        let _status_worker = crate::status_sampler::start(PathBuf::from(&path), sample_status_metrics,
            status_sampling_mode, poll_selected_load, status_value).map_err(|error| {
                eprintln!("[game-status] sampler unavailable: {error}");
            }).ok();
        let run=||->anyhow::Result<()> {
            let root=PathBuf::from(&path);
            if let Ok(log)=std::fs::OpenOptions::new().create(true).append(true).open(root.join("game-runtime.log")) {
                use std::os::fd::AsRawFd; unsafe {libc::dup2(log.as_raw_fd(),1);libc::dup2(log.as_raw_fd(),2);}
            }
            std::panic::set_hook(Box::new(|info| set_error(format!("Rust panic: {info}"))));
            crate::prepare_process_limits();
            set_stage("checking-resources");
            std::env::set_var("MUGUET_SUPPORT",&root);
            let validation=save_validation::prepare(&root)?;
            let fs_root=validation.as_ref().map(|v|v.fs_root.clone()).unwrap_or_else(||root.join("saves"));
            crate::fs::ensure_save_directories(&fs_root)?;
            if validation.is_some() {
                let settings="userdata/Documents/Rockstar Games/GTA V/settings.xml";
                let original=root.join("saves").join(settings);
                if original.is_file() { std::fs::copy(original,fs_root.join(settings))?; }
                *SAVE_VALIDATION_ROOT.lock().unwrap()=Some(fs_root.to_string_lossy().into_owned());
                eprintln!("[save-validation] isolated fs_root={} original saves/config preserved",fs_root.display());
            }
            std::env::set_var("MUGUET_FSROOT",&fs_root);
            let action=LAUNCH_ACTION.load(Ordering::Acquire);
            let selected=LAUNCH_SAVE_ID.lock().unwrap().clone();
            if action==1 {
                let mut state = SAVE_LOAD_STATE.lock().unwrap();
                crate::fs::select_bootstrap_save(&fs_root, &selected)?;
                state.begin(std::time::Instant::now());
            }
            let mode=if action==2 { "2" } else { "1" };
            std::env::set_var("MUGUET_START",mode);
            std::env::set_var("MUGUET_START_MODE",mode);
            crate::fivestar_state::set_story_mode(mode == "1");
            eprintln!("[game-launch] action={} mode={} save={}",action,mode,LAUNCH_SAVE_ID.lock().unwrap());
            // Read before setup can create a default config: a missing field
            // retains marker-only opt-in, while explicit true defaults to NPU.
            let interpolation_cfg = match std::fs::read(root.join("config.json")) {
                Ok(bytes) => serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_else(|error| {
                    eprintln!("[frame-interpolation] startup config JSON: {error}"); serde_json::Value::Null
                }),
                Err(error) => {
                    if error.kind() != std::io::ErrorKind::NotFound { eprintln!("[frame-interpolation] startup config read: {error}"); }
                    serde_json::Value::Null
                }
            };
            let interpolation = crate::config::select_interpolation_backend(&interpolation_cfg,
                root.join("frame-interpolation-npu.request").is_file(),
                root.join("frame-interpolation-rife.request").is_file());
            if let Some(backend) = interpolation {
                std::env::set_var("MG_FRAME_INTERPOLATION", backend);
                std::env::set_var("MG_RIFE_MODEL_DIR", root.join("rife-v4.6"));
            } else {
                std::env::remove_var("MG_FRAME_INTERPOLATION");
                std::env::remove_var("MG_RIFE_MODEL_DIR");
            }
            let mut startup=crate::config::setup()?;
            if let Some(validation)=validation { startup.engine_args.extend(validation.args); }
            if action==0 { startup.engine_args.push("-noautoload".into()); }
            if action!=2 { startup.engine_args.push("-savegame_tty=debug1".into()); }
            let (render_width,render_height)=render_size(width,height);
            anyhow::ensure!(crate::config::game_ready(),"resource root {} is incomplete",root.join("game").display());
            set_stage("creating-vulkan-surface");
            unsafe{crate::native_platform::prepare_surface(window as *mut c_void,width,height)?};
            // prepare_surface's initial unsuspend must respect a background
            // notification which raced the asynchronous startup worker.
            publish_app_lifecycle(*APP_LIFECYCLE.lock().unwrap());
            if crate::native_platform::surface_probe::requested(&root) {
                set_stage("surface-probe-running");
                crate::native_platform::surface_probe::run_requested(&root,render_width,render_height)?;
                set_stage("surface-probe-complete");
                return Ok(());
            }
            crate::input::set_size(render_width as i32,render_height as i32);
            eprintln!("[game] physical {width}x{height}, guest {render_width}x{render_height}, {} FPS target",crate::config::fps_target());
            crate::input::set_focus(app_presentation_allowed());
            crate::input::set_locked(true);
            set_stage("loading-signed-game-aot");
            let rt=crate::make_runtime(&startup)?;
            // Diagnostic collection is explicitly opt-in through an app-private
            // marker; normal launches do not scan the guest heap or journal writes.
            if root.join("fold-heap-diagnostics.request").is_file() {
                std::env::set_var("MUGUET_HEAPCHECK", "1");
                crate::heapcheck::start();
            }
            let mut args:Vec<String>=["./this.program","-rootdir=/game/","-nodisplaycalibration","-noSocialClub","-pc:nosocialclub","-nonetwork","-output","-forceResolution","-Script_all=warning","-replay_all=warning","-fastscriptcore"].iter().map(|v|v.to_string()).collect();
            let (guest_width,guest_height)=crate::input::size();
            args.extend(["-wgpu".into(),format!("-width={guest_width}"),format!("-height={guest_height}")]);
            args.extend(startup.engine_args);
            crate::run_engine(&rt,args)?;
            abort_selected_load("原引擎已结束运行但未确认所选存档恢复成功；请返回首页重试。");
            SESSION_RESTART_STATE.lock().unwrap().cancel_active();
            if !stop_requested() && ERROR.lock().unwrap().is_empty(){ set_stage("game-main-returned"); }
            Ok(())
        };
        if let Err(e)=run(){set_error(format!("{e:#}"));}
    });
    match result {Ok(_)=>0,Err(e)=>{set_error(e);STARTED.store(false,Ordering::SeqCst);3}}
}
#[no_mangle] pub extern "C" fn mg_game_resize(w:u32,h:u32){
    // Temporary zero bounds during a display transition are not a game size.
    if w < 2 || h < 2 { return; }
    let changed = PHYSICAL_SIZE.swap(pack_size(w,h),Ordering::Relaxed) != pack_size(w,h);
    crate::native_platform::resize(w,h);
    if changed {
        // Keep the guest and SR extent selected from the stable landscape
        // startup window for this session. Layout changes only remap input.
        reset_touch_origin();
    }
}
#[no_mangle] pub extern "C" fn mg_game_stop(){
    abort_selected_load("本次加载在成功确认前已停止；请返回首页重试。");
    SESSION_RESTART_STATE.lock().unwrap().cancel_active();
    let mut state = APP_LIFECYCLE.lock().unwrap();
    STOP.store(true,Ordering::Release);state.stop();publish_app_lifecycle(*state);drop(state);
    mg_game_release_inputs();crate::input::set_focus(false);crate::mp::shutdown();set_stage("stop-requested");
    crate::status_sampler::notify();
}
#[no_mangle] pub unsafe extern "C" fn mg_game_key_name(name:*const c_char,pressed:bool){if let Ok(name)=string(name){let accepted=crate::input::key(&name,pressed,false,None,false);if pressed&&accepted{crate::mp::key_pressed(&name);}INPUTS.fetch_add(1,Ordering::Relaxed);}}
#[no_mangle] pub unsafe extern "C" fn mg_game_raw_key(name:*const c_char,pressed:bool,source:*const c_char)->bool {
    let (Ok(name),Ok(source))=(string(name),string(source)) else {return false};
    let accepted=crate::input::raw_key(&name,pressed,&source);
    if accepted {INPUTS.fetch_add(1,Ordering::Relaxed);} accepted
}
#[no_mangle] pub unsafe extern "C" fn mg_game_mouse_button_source(button:u32,pressed:bool,source:*const c_char) {
    if let Ok(source)=string(source) {crate::input::button_source(button as usize,pressed,&source);INPUTS.fetch_add(1,Ordering::Relaxed);}
}
#[no_mangle] pub unsafe extern "C" fn mg_game_named_control(name:*const c_char,value:f32,group:i32)->bool {
    let Ok(name)=string(name) else {return false};
    let accepted=crate::touch_controls::send(&name,value,group);
    if accepted {INPUTS.fetch_add(1,Ordering::Relaxed);} accepted
}
#[no_mangle] pub extern "C" fn mg_game_release_semantic_inputs() {
    crate::touch_controls::release();
    crate::virtual_pad::release();
}
#[no_mangle] pub extern "C" fn mg_game_release_inputs() {
    crate::virtual_pad::release();
    TOUCHES.lock().unwrap().clear(); crate::pad::release_all();
    crate::pad::HORN_TOUCH.store(false,Ordering::Relaxed);
    crate::pad::INJECT.lock().unwrap().clear();
    *crate::pad::RADIO_WHEEL.lock().unwrap()=None; crate::pad::REQUESTS.lock().unwrap().clear();
    crate::touch_controls::release(); crate::input::release_all();
}
#[no_mangle] pub extern "C" fn mg_game_touch_state_json()->*const c_char {
    thread_local! {static TOUCH_STATUS:std::cell::RefCell<CString>=std::cell::RefCell::new(CString::new("{}").unwrap());}
    let json=crate::touch_state::snapshot().to_string();
    TOUCH_STATUS.with(|slot|{*slot.borrow_mut()=CString::new(json).unwrap();slot.borrow().as_ptr()})
}
#[no_mangle] pub extern "C" fn mg_game_key(code:u32,pressed:bool){mg_game_key_event(code,pressed,0,false);}
#[no_mangle] pub extern "C" fn mg_game_virtual_pad(buttons:u32,lx:f32,ly:f32,rx:f32,ry:f32,lt:f32,rt:f32,active:bool)->bool {
    if !active {crate::virtual_pad::release();return true;}
    if !crate::pad::FOCUSED.load(Ordering::Relaxed) || ![lx,ly,rx,ry,lt,rt].iter().all(|v|v.is_finite()) {return false;}
    let accepted=crate::virtual_pad::update(buttons,lx,ly,rx,ry,lt,rt,true);
    if accepted {INPUTS.fetch_add(1,Ordering::Relaxed);} accepted
}
#[no_mangle] pub extern "C" fn mg_game_virtual_pad_state_json()->*const c_char {
    thread_local! {static FIVE_STAR_STATUS:std::cell::RefCell<CString>=std::cell::RefCell::new(CString::new("{}").unwrap());}
    let mut value=crate::fivestar_state::snapshot_json();
    value["transport"]=crate::virtual_pad::health_json();
    if crate::RT.get().is_some() {
        if let Some(rect)=crate::resource_view::minimap_rect() {value["minimapRect"]=rect;}
    }
    FIVE_STAR_STATUS.with(|slot|{*slot.borrow_mut()=CString::new(value.to_string()).unwrap();slot.borrow().as_ptr()})
}
#[no_mangle] pub extern "C" fn mg_game_release_virtual_pad(){crate::virtual_pad::release();}
#[no_mangle] pub extern "C" fn mg_game_virtual_radio(x:f32,y:f32,active:bool){crate::virtual_pad::set_radio(x,y,active);}
#[no_mangle] pub extern "C" fn mg_game_camera_swipe(x:f32,y:f32){
    if crate::pad::FOCUSED.load(Ordering::Relaxed){crate::virtual_pad::queue_camera_swipe(x,y);}
}
#[no_mangle] pub extern "C" fn mg_game_virtual_pause(){
    if crate::RT.get().is_some() {
        let snapshot=crate::fivestar_state::snapshot_json();
        if snapshot["hookReady"].as_bool()==Some(true) && snapshot["pauseMenu"].as_bool()!=Some(true) {
            crate::virtual_pad::request_pause();
        }
    }
}
#[no_mangle] pub extern "C" fn mg_game_virtual_mouse_mode(active:bool){crate::input::set_locked(active);}
#[no_mangle] pub extern "C" fn mg_game_next_aircraft_weapon(){crate::virtual_pad::request_next_aircraft_weapon();}
#[no_mangle] pub extern "C" fn mg_game_virtual_targeting_mode(mode:i32)->bool{crate::virtual_pad::set_targeting_mode(mode)}
#[no_mangle] pub extern "C" fn mg_game_key_event(code:u32,pressed:bool,unicode:u32,suppress_text:bool)->bool {
    let Some(name)=harmony_keys::key_name(code) else { return false; };
    let repeat=pressed && crate::input::is_held(&name);
    let text=char::from_u32(unicode).filter(|c| c.is_ascii_graphic() || *c==' ').map(|c|c.to_string());
    let accepted=crate::input::key(&name,pressed,repeat,text.as_deref(),suppress_text);
    if accepted && pressed && !repeat {crate::mp::key_pressed(&name);}
    INPUTS.fetch_add(1,Ordering::Relaxed);
    true
}
#[no_mangle] pub extern "C" fn mg_game_focus(focused:bool) {
    TOUCHES.lock().unwrap().clear();
    if !focused {mg_game_release_inputs();}
    crate::input::set_focus(focused);
    crate::input::set_locked(focused);
    INPUTS.fetch_add(1,Ordering::Relaxed);
}
#[no_mangle] pub extern "C" fn mg_game_pointer(x:f32,y:f32,dx:f32,dy:f32,look:bool) {
    if ![x,y,dx,dy].iter().all(|v|v.is_finite()) {return;}
    let (rw,rh)=crate::input::size();
    let (pw,ph)=physical_size();
    let Some((x,y))=surface_input_fit::map_point(x as f64,y as f64,(rw,rh),(pw,ph)) else {return;};
    crate::input::set_locked(look);
    if look {crate::input::mouse_delta_f(dx as f64,dy as f64);}
    crate::input::mouse_absolute(x as i32,y as i32);
    INPUTS.fetch_add(1,Ordering::Relaxed);
}
#[no_mangle] pub extern "C" fn mg_game_wheel(notches:f32) {
    crate::input::wheel_fractional(notches);
    INPUTS.fetch_add(1,Ordering::Relaxed);
}
#[no_mangle] pub extern "C" fn mg_game_mouse_delta(dx:f32,dy:f32){crate::input::mouse_delta_f(dx as f64,dy as f64);INPUTS.fetch_add(1,Ordering::Relaxed);}
#[no_mangle] pub extern "C" fn mg_game_mouse_button(button:u32,pressed:bool){crate::input::button(button as usize,pressed);INPUTS.fetch_add(1,Ordering::Relaxed);}
#[no_mangle] pub extern "C" fn mg_game_touch(action:u32,x:f32,y:f32,pointer_id:u32){
    let (rw,rh)=crate::input::size();let (pw,ph)=physical_size();
    let Some((x,y))=surface_input_fit::map_point(x as f64,y as f64,(rw,rh),(pw,ph)) else {
        if action!=2 {TOUCHES.lock().unwrap().retain(|t|t.0!=pointer_id);}
        return;
    };
    let (x,y)=(x as f32,y as f32);
    crate::input::set_locked(true);
    let mut touches=TOUCHES.lock().unwrap();
    if action==2 { if let Some(t)=touches.iter_mut().find(|t|t.0==pointer_id){crate::input::mouse_delta_f((x-t.1) as f64,(y-t.2) as f64);t.1=x;t.2=y;} }
    else { touches.retain(|t|t.0!=pointer_id);if action==0 {touches.push((pointer_id,x,y));} }
    crate::input::mouse_absolute(x as i32,y as i32);
    INPUTS.fetch_add(1,Ordering::Relaxed);
}
#[no_mangle] pub extern "C" fn mg_game_control(control:i32,value:f32){let mut inject=crate::pad::INJECT.lock().unwrap();inject.retain(|(c,_)|*c!=control);if value!=0.0{inject.push((control,value.clamp(-1.0,1.0)));}crate::pad::ACTIVE.store(true,Ordering::Relaxed);INPUTS.fetch_add(1,Ordering::Relaxed);}
fn status_sampling_mode() -> crate::status_sampler::Mode {
    let lifecycle = *APP_LIFECYCLE.lock().unwrap();
    crate::status_sampler::Mode { foreground: lifecycle.allowed(), generation: lifecycle.generation,
        terminal: stop_requested() || !ERROR.lock().unwrap().is_empty() }
}
fn poll_selected_load() {
    let completion = SAVE_LOAD_STATE.lock().unwrap().poll(std::time::Instant::now());
    publish_selected_load_completion(completion);
}
fn sample_status_metrics() -> Result<serde_json::Value, String> {
    let rss = crate::sample_footprint_mb().map_err(|error| format!("VmRSS sample: {error}"))?;
    let resident = crate::sample_heap_resident_mb().map_err(|error| format!("mincore sample: {error}"))?;
    Ok(serde_json::json!({"rssMB":rss,"heapVirtualMB":crate::RT.get().map(|rt|rt.mem.data_size()>>20),
        "heapResidentMB":resident,"gameHostCalls":crate::RT.get().map(|rt|rt.counts.iter().map(|count|count.load(Ordering::Relaxed)).sum::<u64>())}))
}
fn status_value() -> serde_json::Value {
    let (save_pending, save_succeeded, save_error, save_elapsed_ms) = {
        let state = SAVE_LOAD_STATE.lock().unwrap();
        (crate::fs::bootstrap_save_pending(), state.succeeded, state.error, state.elapsed_ms)
    };
    let input_health = crate::input::health_json();
    let gameplay_observed = WORLD_PLAYER_OBSERVED.load(Ordering::Acquire);
    let lifecycle = *APP_LIFECYCLE.lock().unwrap();
    let error = ERROR.lock().unwrap().clone();
    let stopped = stop_requested();
    let diagnostics = crate::status_sampler::snapshot(crate::status_sampler::Mode {
        foreground:lifecycle.allowed(), generation:lifecycle.generation, terminal:stopped || !error.is_empty(),
    });
    let mut value=serde_json::json!({"stage":STAGE.lock().unwrap().clone(),"error":error,"started":STARTED.load(Ordering::Relaxed),"stopRequested":stopped,"presents":crate::gpu::PRESENTS.load(Ordering::Relaxed),"guestPresents":crate::gpu::GUEST_PRESENTS.load(Ordering::Relaxed),"gpuStarted":crate::gpu::STARTED.load(Ordering::Relaxed),"renderSize":crate::input::size(),"inputEvents":INPUTS.load(Ordering::Relaxed),"inputHealth":input_health,"rssMB":diagnostics["values"]["rssMB"],"heapVirtualMB":diagnostics["values"]["heapVirtualMB"],"heapResidentMB":diagnostics["values"]["heapResidentMB"],"gameHostCalls":diagnostics["values"]["gameHostCalls"],"target":"aarch64-unknown-linux-ohos","source":"muguet-1a0300d"});
    value["diagnostics"] = diagnostics;
    value["nativeTelemetry"] = crate::native_telemetry::snapshot();
    value["frameIntervalTrace"] = crate::cpu_profile::frame_intervals::status();
    value["nativeCapabilities"] = crate::native_capabilities::snapshot();
    value["worldPlayerObserved"] = gameplay_observed.into();
    value["processId"] = std::process::id().into();
    value["frameGenerationAvailable"] = (mg_game_frame_generation_available()==1).into();
    value["frameGenerationLive"] = crate::frame_interpolation::live_status();
    value["touchControlTransport"] = crate::touch_controls::health_json();
    value["virtualPadTransport"] = crate::virtual_pad::health_json();
    {
        value["appForeground"] = lifecycle.foreground.into();
        value["presentationPaused"] = (!lifecycle.allowed()).into();
        value["presentationGeneration"] = lifecycle.generation.into();
        value["surfaceRecoveryRequired"] = lifecycle.surface_lost.into();
        value["surfaceRecoveryError"] = if lifecycle.surface_lost {
            "游戏窗口已失效，当前进度已暂停且尚未恢复；重新开始不等于恢复未保存的进度。"
        } else { "" }.into();
        value["pauseScope"] = "app visibility pauses future GPU/AI work; an already-running public synchronous driver call is not interrupted".into();
    }
    value["launchAction"] = LAUNCH_ACTION.load(Ordering::Acquire).into();
    value["requestedSaveId"] = LAUNCH_SAVE_ID.lock().unwrap().clone().into();
    value["saveLoadPending"] = save_pending.into();
    value["saveLoadSucceeded"] = save_succeeded.into();
    value["saveLoadError"] = save_error.into();
    value["saveLoadElapsedMs"] = save_elapsed_ms.into();
    value["saveLoadTimeoutSeconds"] = selected_load::TIMEOUT_SECONDS.into();
    value["saveValidationRoot"] = SAVE_VALIDATION_ROOT.lock().unwrap().clone().into();
    {
        let restart = SESSION_RESTART_STATE.lock().unwrap();
        value["sessionRestartId"] = restart.id.into();
        value["sessionRestartAction"] = restart.action.into();
        value["sessionRestartSaveId"] = restart.save_id.clone().into();
        value["sessionRestartError"] = restart.error.into();
    }
    value["spatialUpscale"] = crate::spatial_upscale::status();
    value["worldPlayerObservationSource"] = "engine TASK_PLAYER_ON_FOOT constructed log".into();
    value
}
#[no_mangle] pub extern "C" fn mg_game_status_json()->*const c_char {
    let value = status_value();
    STATUS.with(|s|{*s.borrow_mut()=CString::new(value.to_string()).unwrap();s.borrow().as_ptr()})
}
