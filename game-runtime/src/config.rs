// muguet - config.rs
// Constantin Clerc (c22dev) - 0.1

use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg_attr(target_os = "ios", allow(dead_code))]
#[derive(Clone)]
pub struct Startup {
    pub wasm_path: PathBuf,
    pub cache_dir: PathBuf,

    pub engine_args: Vec<String>,
}

const DEFAULT_CONFIG: &str = r#"{
  "scale": 0.4,
  "mouse_sensitivity": 2.0,
  "fullscreen": true,
  "fps_limit": 30,
  "start_mode": 2,
  "mute": false,
  "no_astc": false,
  "full_mips": false,
  "dedicated_atlas": false,
  "dedicated_images": false,
  "dedicated_buffers": false,
  "sync_submit": false,
  "gpu_diagnostics": false,
  "cpu_profile": false,
  "gpu_profile": false,
  "thread_qos": true,
  "depth_reduce_opt": true,
  "depth_reduce_no_zero": true,
  "skip_empty_init_transition": false,
  "readback_pool": false,
  "luminance_no_zero": false,
  "spatial_upscale_mode": "off",
  "spatial_upscale_sharpness": 0.2,
  "multiplayer": false,
  "host": false,
  "server": "",
  "player_name": "",
  "extra_args": []
}
"#;

fn support_dir() -> PathBuf {
    PathBuf::from(std::env::var("MUGUET_SUPPORT").unwrap_or_else(|_| "/data/storage/el2/base/haps/entry/files".into()))
}

pub fn game_dir() -> PathBuf {
    let dir = support_dir().join("game");
    let old = dir.join("gamedat");
    if !dir.join("data").exists() && old.join("data/manifest.json").exists() { old } else { dir }
}

pub fn game_ready() -> bool {
    let dir = game_dir();
    dir.join("data/manifest.json").exists() && build_dir(&dir).is_some_and(|b| !cfg!(feature = "jit") || crate::code_embedded() || b.join("game.wasm").exists())
}

pub fn bundled_resources() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let res = exe.parent()?.parent()?.join("Resources");
    (exe.parent()?.ends_with("Contents/MacOS") && res.is_dir()).then_some(res)
}

pub(crate) fn build_dir(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root.join("b")).ok()?.flatten().map(|e| e.path()).find(|p| p.join("shaders/index.json").exists())
}

fn set_default(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    if std::env::var_os(key).is_none() {
        std::env::set_var(key, value);
    }
}

#[derive(Clone, Copy)]
struct DisplaySettings {
    resolution: Option<(u32, u32)>,
    scale: f64,
    fps_limit: f64,
}

static DISPLAY_SETTINGS: Mutex<DisplaySettings> = Mutex::new(DisplaySettings {
    resolution: None,
    scale: 0.4,
    fps_limit: 30.0,
});

fn parse_resolution(value: &str) -> Option<(u32, u32)> {
    let (w, h) = value.trim().split_once('x')?;
    let (w, h) = (w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?);
    (w > 0 && h > 0 && w <= i32::MAX as u32 && h <= i32::MAX as u32).then_some((w, h))
}

fn configure_display(cfg: &serde_json::Value) {
    let number = |key: &str| cfg.get(key).and_then(|v| v.as_f64()).filter(|v| v.is_finite());
    let env_number = |key: &str| std::env::var(key).ok().and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite());
    let resolution = std::env::var("MUGUET_RES").ok().as_deref().and_then(parse_resolution)
        .or_else(|| cfg.get("resolution").and_then(|v| v.as_str()).and_then(parse_resolution));
    let scale = env_number("MUGUET_SCALE").filter(|v| *v > 0.0)
        .or_else(|| number("scale").filter(|v| *v > 0.0)).unwrap_or(0.4);
    let fps_limit = env_number("MUGUET_FPS").or_else(|| number("fps_limit")).unwrap_or(30.0);
    // Do not export JSON values to the environment: a later setup must read the
    // current file rather than mistake values from a previous setup for overrides.
    *DISPLAY_SETTINGS.lock().unwrap() = DisplaySettings { resolution, scale, fps_limit };
}

/// Explicit resolution takes precedence over scale. Both paths retain the OHOS
/// minimum, wide-aspect workaround and even-size rules used by the guest.
pub(crate) fn render_size(width: u32, height: u32) -> (u32, u32) {
    if let Some(size) = crate::spatial_upscale::render_size_override() { return size; }
    let settings = *DISPLAY_SETTINGS.lock().unwrap();
    let (w, h) = settings.resolution.map(|(w, h)| (w as f64, h as f64))
        .unwrap_or((width as f64 * settings.scale, height as f64 * settings.scale));
    let mut w = w.max(320.0);
    let h = h.max(240.0);
    if w <= h * 1.5 { w = h * 1.5 + 2.0; }
    // Input dimensions are signed i32 values in the shared guest block.
    ((w.min(i32::MAX as f64) as u32) & !1, (h.min(i32::MAX as f64) as u32) & !1)
}

pub(crate) fn fps_target() -> f64 {
    DISPLAY_SETTINGS.lock().unwrap().fps_limit
}

/// Explicit opt-in for the synchronous-submit compatibility diagnostic.
/// Reapplying false also clears a previous mode's environment and trace path.
pub(crate) fn configure_sync_submit(enabled: bool, trace_dir: &Path) {
    if enabled {
        std::env::set_var("MUGUET_SYNC_SUBMIT", "1");
        std::env::set_var("MUGUET_SUBMIT_TRACE_DIR", trace_dir);
    } else {
        std::env::remove_var("MUGUET_SYNC_SUBMIT");
        std::env::remove_var("MUGUET_SUBMIT_TRACE_DIR");
    }
}

const GAME_DIAGNOSTIC_PATHS: &[(&str, &str)] = &[
    ("MUGUET_GPU_UPLOAD_TRACE", "gpu-buffer-upload-last.txt"),
    ("MUGUET_VK_COPY_TRACE", "vk-buffer-copy-last.txt"),
    ("MUGUET_VK_POOL_TRACE", "vk-command-pool-last.txt"),
    ("MUGUET_IMAGE_SYNC_TRACE", "gpu-image-sync-first.txt"),
    ("MUGUET_ATLAS_SYNC_TRACE", "gpu-atlas-sync.txt"),
    ("MUGUET_PIPELINE_TRACE_DIR", "gpu-pipeline-trace"),
];

fn configure_gpu_diagnostics(enabled: bool) {
    if enabled {
        std::env::set_var("MUGUET_GPU_DIAGNOSTICS", "1");
    } else {
        std::env::remove_var("MUGUET_GPU_DIAGNOSTICS");
    }
}

fn configure_gpu_profile(enabled: bool) {
    if enabled {
        std::env::set_var("MUGUET_GPU_PROFILE", "1");
        std::env::set_var("MUGUET_GPU_PROFILE_DIR", support_dir().join("gpu-profile"));
    } else {
        std::env::remove_var("MUGUET_GPU_PROFILE");
        std::env::remove_var("MUGUET_GPU_PROFILE_DIR");
    }
}

/// Normal-game diagnostic paths only. Explicit probe requests own their paths.
pub(crate) fn configure_game_diagnostic_paths(root: &Path) {
    let enabled = std::env::var("MUGUET_GPU_DIAGNOSTICS").as_deref() == Ok("1");
    for &(key, name) in GAME_DIAGNOSTIC_PATHS {
        if enabled { std::env::set_var(key, root.join(name)); }
        else { std::env::remove_var(key); }
    }
    // Fault collection is passive until device loss and remains available.
    std::env::set_var("MUGUET_DEVICE_FAULT_DIR", root.join("gpu-device-fault"));
    // This independent compatibility switch keeps its original trace behavior.
    if std::env::var_os("MUGUET_SYNC_SUBMIT").is_some() {
        std::env::set_var("MUGUET_SUBMIT_TRACE_DIR", root.join("gpu-submit-trace"));
    } else {
        std::env::remove_var("MUGUET_SUBMIT_TRACE_DIR");
    }
}

fn redirect_logs_if_detached() {
    extern "C" {
        fn isatty(fd: i32) -> i32;
        fn dup2(old: i32, new: i32) -> i32;
    }

    if cfg!(target_os = "ios") {
        return;
    }
    let in_bundle = std::env::current_exe().map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/")).unwrap_or(false);
    if !in_bundle || unsafe { isatty(2) } != 0 {
        return;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let path = PathBuf::from(home).join("Library/Logs/Muguet.log");
    if let Ok(f) = std::fs::File::create(&path) {
        use std::os::fd::IntoRawFd;
        let fd = f.into_raw_fd();
        unsafe {
            dup2(fd, 2);
            dup2(fd, 1);
        }
    }
}

pub fn setup() -> anyhow::Result<Startup> {
    redirect_logs_if_detached();
    let support = support_dir();
    std::fs::create_dir_all(support.join("saves"))?;

    let cfg_path = support.join("config.json");
    if !cfg_path.exists() {
        std::fs::write(&cfg_path, DEFAULT_CONFIG)?;
    }
    let engine_args = settings(&cfg_path);

    set_default("MUGUET_FSROOT", support.join("saves"));
    let build = locate();
    #[cfg(not(target_os = "ios"))]
    let build = build?;
    #[cfg(target_os = "ios")]
    let build = build.unwrap_or_default();
    Ok(Startup { wasm_path: build.join("game.wasm"), cache_dir: support, engine_args })
}

fn locate() -> anyhow::Result<PathBuf> {
    let root = game_dir();
    let build = build_dir(&root).filter(|_| game_ready()).ok_or_else(|| anyhow::anyhow!("no game data: import your game .zip from the launcher"))?;
    std::env::set_var("MUGUET_DATA", root.join("data"));
    std::env::set_var("MUGUET_SHADERS", build.join("shaders"));
    eprintln!("[config] game {}", root.display());
    Ok(build)
}

const SETTING_VARS: &[&str] = &["MUGUET_SCALE", "MUGUET_MOUSE_SENS", "MUGUET_TOUCH_SENS", "MUGUET_RES", "MUGUET_FULLSCREEN", "MUGUET_MUTE", "MUGUET_HOST", "MUGUET_SERVER", "MUGUET_NAME", "MUGUET_SHOW_FPS"];

#[cfg(target_os = "ios")]
pub fn reload() -> Vec<String> {
    for v in SETTING_VARS { std::env::remove_var(v); }
    let args = settings(&support_dir().join("config.json"));
    if let Err(e) = locate() {
        eprintln!("[config] {e:#}");
    }
    eprintln!("[config] launcher settings applied: {}", args.join(" "));
    args
}

fn settings(cfg_path: &Path) -> Vec<String> {
    let _ = SETTING_VARS;
    let cfg: serde_json::Value = std::fs::read_to_string(&cfg_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).map_err(|e| eprintln!("[config] {}: {e}", cfg_path.display())).ok())
        .unwrap_or_else(|| serde_json::from_str(DEFAULT_CONFIG).unwrap());
    let num = |k: &str| cfg.get(k).and_then(|v| v.as_f64());
    // Newly qualified defaults also apply to existing configs missing these
    // fields. An explicit false remains an A/B override; invalid types stay off.
    let flag = |k: &str| match cfg.get(k) {
        Some(value) => value.as_bool().unwrap_or(false),
        None => matches!(k, "thread_qos" | "depth_reduce_opt" | "depth_reduce_no_zero"),
    };
    configure_display(&cfg);
    crate::spatial_upscale::configure(&cfg);
    if let Some(v) = num("mouse_sensitivity") { set_default("MUGUET_MOUSE_SENS", v.to_string()); }
    if let Some(v) = num("touch_sensitivity") { set_default("MUGUET_TOUCH_SENS", v.to_string()); }
    if flag("fullscreen") { set_default("MUGUET_FULLSCREEN", "1"); }
    if flag("mute") { set_default("MUGUET_MUTE", "1"); }
    if flag("show_fps") { set_default("MUGUET_SHOW_FPS", "1"); }
    // These are opt-in diagnostic switches. GPU code checks environment-variable
    // presence, so false/missing must leave the variable absent, never set "0".
    if flag("no_astc") { set_default("MUGUET_NO_ASTC", "1"); }
    if flag("full_mips") { set_default("MUGUET_FULL_MIPS", "1"); }
    if flag("dedicated_atlas") { set_default("MUGUET_DEDICATED_ATLAS", "1"); }
    if flag("dedicated_images") { set_default("MUGUET_DEDICATED_IMAGES", "1"); }
    if flag("dedicated_buffers") { set_default("MUGUET_DEDICATED_BUFFERS", "1"); }
    configure_sync_submit(flag("sync_submit"), &support_dir().join("gpu-submit-trace"));
    configure_gpu_diagnostics(flag("gpu_diagnostics"));
    crate::cpu_profile::configure(flag("cpu_profile"));
    configure_gpu_profile(flag("gpu_profile"));
    crate::qos::configure(flag("thread_qos"));
    if flag("depth_reduce_opt") { std::env::set_var("MUGUET_DEPTH_REDUCE_OPT", "1"); }
    else { std::env::remove_var("MUGUET_DEPTH_REDUCE_OPT"); }
    if flag("depth_reduce_no_zero") { std::env::set_var("MUGUET_DEPTH_REDUCE_NO_ZERO", "1"); }
    else { std::env::remove_var("MUGUET_DEPTH_REDUCE_NO_ZERO"); }
    if flag("skip_empty_init_transition") { std::env::set_var("MUGUET_SKIP_EMPTY_INIT_TRANSITION", "1"); }
    else { std::env::remove_var("MUGUET_SKIP_EMPTY_INIT_TRANSITION"); }
    if flag("readback_pool") { std::env::set_var("MUGUET_READBACK_POOL", "1"); }
    else { std::env::remove_var("MUGUET_READBACK_POOL"); }
    if flag("luminance_no_zero") { std::env::set_var("MUGUET_LUMINANCE_NO_ZERO", "1"); }
    else { std::env::remove_var("MUGUET_LUMINANCE_NO_ZERO"); }

    let text = |k: &str| cfg.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|v| !v.is_empty());
    if flag("multiplayer") {
        if flag("host") { set_default("MUGUET_HOST", "1"); }
        else if let Some(v) = text("server") { set_default("MUGUET_SERVER", v); }
    }
    if let Some(v) = text("player_name") { set_default("MUGUET_NAME", v); }

    let mut engine_args = Vec::new();

    let fps = fps_target();
    engine_args.push(format!("-frameLimit={}", if fps <= 0.0 { 0 } else if fps <= 30.0 { 2 } else { 1 }));

    let start_mode = std::env::var("MUGUET_START").ok().and_then(|v| v.parse::<f64>().ok()).or_else(|| num("start_mode")).unwrap_or(2.0) as i64;
    let start_mode = if (1..=2).contains(&start_mode) { start_mode } else { 2 };
    if !std::env::args().any(|a| a.starts_with("-wasmstartchoice")) {
        engine_args.push(format!("-wasmstartchoice={start_mode}"));
    }
    if let Some(extra) = cfg.get("extra_args").and_then(|v| v.as_array()) {
        engine_args.extend(extra.iter().filter_map(|a| a.as_str().map(String::from)));
    }
    if !engine_args.iter().any(|a| a.to_ascii_lowercase().starts_with("-framequeuelimit")) {
        engine_args.push("-FrameQueueLimit=3".into());
    }
    fix_settings(num("texture_quality").map(|q| q as i64), flag("no_msaa"));
    engine_args
}

fn fix_settings(texture_quality: Option<i64>, no_msaa: bool) {
    let path = support_dir().join("saves/userdata/Documents/Rockstar Games/GTA V/settings.xml");
    let Ok(xml) = std::fs::read_to_string(&path) else { return };
    let mut new = xml.clone();
    let mut set = |tag: &str, fix: &dyn Fn(i64) -> i64| {
        let re = regex::Regex::new(&format!(r#"<{tag} value="(\d+)" />"#)).unwrap();
        let Some(v) = re.captures(&new).and_then(|c| c[1].parse::<i64>().ok()) else { return };
        let to = fix(v);
        if to != v {
            new = re.replace(&new, format!(r#"<{tag} value="{to}" />"#).as_str()).into_owned();
            eprintln!("[config] {tag} {v} -> {to}");
        }
    };
    if let Some(q) = texture_quality {
        set("TextureQuality", &|_| q.clamp(0, 2));
    }
    set("ShaderQuality", &|v| v.max(1));
    set("MSAA", &|v| if v > 4 { v.min(crate::gpu::max_msaa() as i64) } else { v });
    if no_msaa {
        for tag in ["MSAA", "MSAAFragments", "MSAAQuality", "ReflectionMSAA"] {
            set(tag, &|_| 0);
        }
        if new.contains(r#"<TXAA_Enabled value="true" />"#) {
            new = new.replace(r#"<TXAA_Enabled value="true" />"#, r#"<TXAA_Enabled value="false" />"#);
            eprintln!("[config] TXAA_Enabled true -> false");
        }
    }
    if new != xml {
        let _ = std::fs::write(&path, new.as_bytes());
    }
}
