use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::path::PathBuf;
#[path = "harmony_keys.rs"]
mod harmony_keys;
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
static STOP: AtomicBool = AtomicBool::new(false);
static TOUCHES: Mutex<Vec<(u32,f32,f32)>> = Mutex::new(Vec::new());
static INPUTS: AtomicU64 = AtomicU64::new(0);
static WORLD_PLAYER_OBSERVED: AtomicBool = AtomicBool::new(false);
pub(crate) fn world_player_observed() -> bool { WORLD_PLAYER_OBSERVED.load(Ordering::Acquire) }
pub(crate) fn reset_touch_origin() { TOUCHES.lock().unwrap().clear(); }
static STAGE: Mutex<String> = Mutex::new(String::new());
static ERROR: Mutex<String> = Mutex::new(String::new());
thread_local! { static STATUS: std::cell::RefCell<CString> = std::cell::RefCell::new(CString::new("{}").unwrap()); }
pub fn stop_requested() -> bool { STOP.load(Ordering::Acquire) }
pub(crate) fn observe_engine_line(line: &str) {
    if !WORLD_PLAYER_OBSERVED.load(Ordering::Relaxed) && line.contains("TASK_PLAYER_ON_FOOT constructed") {
        WORLD_PLAYER_OBSERVED.store(true, Ordering::Release);
        eprintln!("[game] world player task observed; game scene may now be reported");
    }
}
pub fn set_error(e: impl ToString) { let e=e.to_string(); eprintln!("[game] ERROR {e}"); *ERROR.lock().unwrap() = e; *STAGE.lock().unwrap() = "error".into(); crate::heapcheck::request_snapshot("game-error"); }
fn render_size(width:u32,height:u32)->(u32,u32) {
    crate::config::render_size(width,height)
}
pub(crate) fn set_stage(s: &str) { eprintln!("[game] {s}"); *STAGE.lock().unwrap()=s.into(); }
unsafe fn string(p:*const c_char)->anyhow::Result<String> { anyhow::ensure!(!p.is_null(),"null string"); Ok(CStr::from_ptr(p).to_str()?.into()) }
#[no_mangle]
pub unsafe extern "C" fn mg_game_start(window:*mut c_void,width:u32,height:u32,files_dir:*const c_char)->i32 {
    if window.is_null() || width==0 || height==0 { set_error("missing native window or size"); return 1; }
    let path=match string(files_dir) {Ok(s)=>s,Err(e)=>{set_error(e);return 1}};
    if STARTED.swap(true,Ordering::SeqCst) { return 2; }
    STOP.store(false,Ordering::Release);
    PHYSICAL_SIZE.store(pack_size(width,height),Ordering::Relaxed);
    let window=window as usize;
    let result=std::thread::Builder::new().name("game-main".into()).stack_size(32<<20).spawn(move|| {
        let run=||->anyhow::Result<()> {
            let root=PathBuf::from(&path);
            std::fs::create_dir_all(root.join("saves"))?;
            if let Ok(log)=std::fs::OpenOptions::new().create(true).append(true).open(root.join("game-runtime.log")) {
                use std::os::fd::AsRawFd; unsafe {libc::dup2(log.as_raw_fd(),1);libc::dup2(log.as_raw_fd(),2);}
            }
            std::panic::set_hook(Box::new(|info| set_error(format!("Rust panic: {info}"))));
            crate::prepare_process_limits();
            set_stage("checking-resources");
            std::env::set_var("MUGUET_SUPPORT",&root);
            std::env::set_var("MUGUET_FSROOT",root.join("saves"));
            std::env::set_var("MUGUET_START","2");
            std::env::set_var("MUGUET_START_MODE","2");
            let startup=crate::config::setup()?;
            let (render_width,render_height)=render_size(width,height);
            anyhow::ensure!(crate::config::game_ready(),"resource root {} is incomplete",root.join("game").display());
            set_stage("creating-vulkan-surface");
            unsafe{crate::native_platform::prepare_surface(window as *mut c_void,width,height)?};
            if crate::native_platform::surface_probe::requested(&root) {
                set_stage("surface-probe-running");
                crate::native_platform::surface_probe::run_requested(&root,render_width,render_height)?;
                set_stage("surface-probe-complete");
                return Ok(());
            }
            crate::input::set_size(render_width as i32,render_height as i32);
            eprintln!("[game] physical {width}x{height}, guest {render_width}x{render_height}, {} FPS target",crate::config::fps_target());
            crate::input::set_focus(true);
            crate::input::set_locked(true);
            set_stage("loading-signed-game-aot");
            let rt=crate::make_runtime(&startup)?;
            // Diagnostic collection is explicitly opt-in through an app-private
            // marker; normal launches do not scan the guest heap or journal writes.
            if root.join("fold-heap-diagnostics.request").is_file() {
                std::env::set_var("MUGUET_HEAPCHECK", "1");
                crate::heapcheck::start();
            }
            let mut args:Vec<String>=["./this.program","-rootdir=/game/","-nodisplaycalibration","-noSocialClub","-pc:nosocialclub","-nonetwork","-output","-forceResolution","-Script_all=warning","-replay_all=warning","-fastscriptcore","-FrameQueueLimit=3"].iter().map(|v|v.to_string()).collect();
            let (guest_width,guest_height)=crate::input::size();
            args.extend(["-wgpu".into(),format!("-width={guest_width}"),format!("-height={guest_height}")]);
            args.extend(startup.engine_args);
            crate::run_engine(&rt,args)?;
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
#[no_mangle] pub extern "C" fn mg_game_stop(){ STOP.store(true,Ordering::Release);crate::native_platform::suspend(true);crate::input::set_focus(false);crate::mp::shutdown();set_stage("stop-requested"); }
#[no_mangle] pub unsafe extern "C" fn mg_game_key_name(name:*const c_char,pressed:bool){if let Ok(name)=string(name){crate::input::key(&name,pressed,false,None,false);if pressed{crate::mp::key_pressed(&name);}INPUTS.fetch_add(1,Ordering::Relaxed);}}
#[no_mangle] pub extern "C" fn mg_game_key(code:u32,pressed:bool){mg_game_key_event(code,pressed,0,false);}
#[no_mangle] pub extern "C" fn mg_game_key_event(code:u32,pressed:bool,unicode:u32,suppress_text:bool)->bool {
    let Some(name)=harmony_keys::key_name(code) else { return false; };
    let repeat=pressed && crate::input::is_held(&name);
    let text=char::from_u32(unicode).filter(|c| c.is_ascii_graphic() || *c==' ').map(|c|c.to_string());
    crate::input::key(&name,pressed,repeat,text.as_deref(),suppress_text);
    if pressed && !repeat {crate::mp::key_pressed(&name);}
    INPUTS.fetch_add(1,Ordering::Relaxed);
    true
}
#[no_mangle] pub extern "C" fn mg_game_focus(focused:bool) {
    TOUCHES.lock().unwrap().clear();
    crate::input::set_focus(focused);
    crate::input::set_locked(focused);
    INPUTS.fetch_add(1,Ordering::Relaxed);
}
#[no_mangle] pub extern "C" fn mg_game_pointer(x:f32,y:f32,dx:f32,dy:f32,look:bool) {
    if ![x,y,dx,dy].iter().all(|v|v.is_finite()) {return;}
    let (rw,rh)=crate::input::size();
    let (pw,ph)=physical_size();
    crate::input::set_locked(look);
    if look {crate::input::mouse_delta_f(dx as f64,dy as f64);}
    crate::input::mouse_absolute((x*rw as f32/pw.max(1) as f32) as i32,
        (y*rh as f32/ph.max(1) as f32) as i32);
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
    let x=x*rw as f32/pw.max(1) as f32;let y=y*rh as f32/ph.max(1) as f32;
    crate::input::set_locked(true);
    let mut touches=TOUCHES.lock().unwrap();
    if action==2 { if let Some(t)=touches.iter_mut().find(|t|t.0==pointer_id){crate::input::mouse_delta_f((x-t.1) as f64,(y-t.2) as f64);t.1=x;t.2=y;} }
    else { touches.retain(|t|t.0!=pointer_id);if action==0 {touches.push((pointer_id,x,y));} }
    crate::input::mouse_absolute(x as i32,y as i32);
    INPUTS.fetch_add(1,Ordering::Relaxed);
}
#[no_mangle] pub extern "C" fn mg_game_control(control:i32,value:f32){let mut inject=crate::pad::INJECT.lock().unwrap();inject.retain(|(c,_)|*c!=control);if value!=0.0{inject.push((control,value.clamp(-1.0,1.0)));}crate::pad::ACTIVE.store(true,Ordering::Relaxed);INPUTS.fetch_add(1,Ordering::Relaxed);}
#[no_mangle] pub extern "C" fn mg_game_status_json()->*const c_char {
    let input_health = crate::input::health_json();
    let gameplay_observed = WORLD_PLAYER_OBSERVED.load(Ordering::Acquire);
    let mut value=serde_json::json!({"stage":STAGE.lock().unwrap().clone(),"error":ERROR.lock().unwrap().clone(),"started":STARTED.load(Ordering::Relaxed),"stopRequested":stop_requested(),"presents":crate::gpu::PRESENTS.load(Ordering::Relaxed),"guestPresents":crate::gpu::GUEST_PRESENTS.load(Ordering::Relaxed),"gpuStarted":crate::gpu::STARTED.load(Ordering::Relaxed),"renderSize":crate::input::size(),"inputEvents":INPUTS.load(Ordering::Relaxed),"inputHealth":input_health,"rssMB":crate::footprint_mb(),"heapVirtualMB":crate::RT.get().map(|rt|rt.mem.data_size()>>20),"heapResidentMB":crate::heap_resident_mb(),"gameHostCalls":crate::RT.get().map(|rt|rt.counts.iter().map(|c|c.load(Ordering::Relaxed)).sum::<u64>()),"target":"aarch64-unknown-linux-ohos","source":"muguet-1a0300d"});
    value["worldPlayerObserved"] = gameplay_observed.into();
    value["spatialUpscale"] = crate::spatial_upscale::status();
    value["worldPlayerObservationSource"] = "engine TASK_PLAYER_ON_FOOT constructed log".into();
    STATUS.with(|s|{*s.borrow_mut()=CString::new(value.to_string()).unwrap();s.borrow().as_ptr()})
}
