// muguet - import.rs
// Constantin Clerc (c22dev) - 0.1

use anyhow::{bail, Context, Result};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const SKIPPED_DIRS: &[&str] = &["env_test", "verityradio"];

pub fn import(zip_path: &Path, dest: &Path, keep_code: bool, mut progress: impl FnMut(u64, u64)) -> Result<()> {
    let file = File::open(zip_path).with_context(|| format!("{}", zip_path.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::with_capacity(1 << 20, file)).context("not a zip file")?;

    let names: Vec<String> = zip.file_names().map(String::from).collect();
    let manifest = names
        .iter()
        .filter(|n| n.ends_with("data/manifest.json") && (n.len() == "data/manifest.json".len() || n.ends_with("/data/manifest.json")))
        .min_by_key(|n| n.len())
        .context("this zip has no game data (data/manifest.json)")?;
    let prefix = manifest[..manifest.len() - "data/manifest.json".len()].to_string();
    if !names.iter().any(|n| n.starts_with(&format!("{prefix}b/")) && n.ends_with("/shaders/index.json")) {
        bail!("this zip has no shaders (b/<build>/shaders)");
    }
    if keep_code && !names.iter().any(|n| n.starts_with(&format!("{prefix}b/")) && n.ends_with("/game.wasm")) {
        bail!("this zip has no game code (b/<build>/game.wasm)");
    }

    let wanted = |rel: &str| -> bool {
        if !(rel.starts_with("data/") || rel.starts_with("b/")) || rel.ends_with('/') {
            return false;
        }
        if rel.split('/').any(|c| SKIPPED_DIRS.contains(&c.to_ascii_lowercase().as_str())) {
            return false;
        }
        if let Some(build) = rel.strip_prefix("b/") {
            let inner = build.split_once('/').map(|(_, r)| r).unwrap_or("");
            return inner.starts_with("shaders/") || (keep_code && inner == "game.wasm");
        }
        true
    };

    let mut jobs = Vec::new();
    let mut total = 0u64;
    for k in 0..zip.len() {
        let entry = zip.by_index_raw(k)?;
        let Some(name) = entry.enclosed_name() else { continue };
        let name = name.to_string_lossy().replace('\\', "/");
        let Some(rel) = name.strip_prefix(&prefix) else { continue };
        if entry.is_dir() || !wanted(rel) {
            continue;
        }
        total += entry.size();
        jobs.push((k, rel.to_string()));
    }

    let staging = dest.with_extension("importing");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let mut done = 0u64;
    let mut reported = 0u64;
    progress(0, total);
    let mut buf = vec![0u8; 1 << 20];
    for (k, rel) in &jobs {
        let mut entry = zip.by_index(*k)?;
        let out_path = staging.join(rel);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&out_path).with_context(|| format!("{}", out_path.display()))?;
        loop {
            let n = entry.read(&mut buf).with_context(|| format!("reading {rel} from the zip"))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            if done - reported >= 64 << 20 {
                reported = done;
                progress(done, total);
            }
        }
    }

    tidy_lists(&staging.join("data"))?;
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&staging, dest)?;
    progress(total, total);
    Ok(())
}

fn tidy_lists(data: &Path) -> Result<()> {
    let dlc = data.join("common/data/dlcList.xml");
    if let Ok(text) = std::fs::read_to_string(&dlc) {
        let kept: Vec<&str> = text.lines().filter(|l| !SKIPPED_DIRS.iter().any(|d| l.to_ascii_lowercase().contains(d))).collect();
        std::fs::write(&dlc, kept.join("\n") + "\n")?;
    }
    let manifest_path = data.join("manifest.json");
    let mut manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(&manifest_path)?).context("manifest.json")?;
    let present: HashSet<PathBuf> = walk(data);
    if let Some(files) = manifest.get_mut("files").and_then(|f| f.as_array_mut()) {
        files.retain(|f| f.get(0).and_then(|p| p.as_str()).is_some_and(|p| present.contains(&data.join(p))));
    }
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest)?)?;
    Ok(())
}

fn walk(dir: &Path) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p);
            }
        }
    }
    out
}

#[cfg(target_os = "ios")]
mod ffi {
    use std::ffi::{c_char, CStr, CString};
    use std::sync::Mutex;

    static ERROR: Mutex<Option<CString>> = Mutex::new(None);

    #[no_mangle]
    pub extern "C" fn muguet_import(zip: *const c_char, progress: extern "C" fn(u64, u64)) -> bool {
        let path = unsafe { CStr::from_ptr(zip) }.to_string_lossy().into_owned();
        match super::import(std::path::Path::new(&path), &crate::config::game_dir(), false, |d, t| progress(d, t)) {
            Ok(()) => true,
            Err(e) => {
                *ERROR.lock().unwrap() = CString::new(format!("{e:#}")).ok();
                false
            }
        }
    }

    #[no_mangle]
    pub extern "C" fn muguet_import_error() -> *const c_char {
        ERROR.lock().unwrap().as_ref().map(|s| s.as_ptr()).unwrap_or(std::ptr::null())
    }

    #[no_mangle]
    pub extern "C" fn muguet_game_ready() -> bool {
        crate::config::game_ready()
    }

    #[no_mangle]
    pub extern "C" fn muguet_has_game_code() -> bool {
        crate::has_game_code()
    }
}
