//! Native persistence primitives. Browser userdata callbacks are not the disk backend.
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

struct BootstrapSave {
    root: PathBuf,
    relative_id: String,
    read_observed: bool,
}

static BOOTSTRAP_SAVE: Mutex<Option<BootstrapSave>> = Mutex::new(None);

#[cfg(test)]
pub(super) static TEST_SERIAL: Mutex<()> = Mutex::new(());

fn save_slot_relative(relative: &str) -> bool {
    let parts: Vec<_> = relative.split('/').collect();
    if parts.len() != 6
        || !matches!(parts[0], "userdata" | "sandbox")
        || parts[1..5] != ["Documents", "Rockstar Games", "GTA V", "Default"]
    {
        return false;
    }
    let name = parts[5].as_bytes();
    name.len() == 9 && name.starts_with(b"SGTA5")
        && name[5..].iter().all(u8::is_ascii_digit)
        && parts[5][5..].parse::<u32>().is_ok_and(|slot| slot <= 15)
}

/// Constrain the game's initial save enumeration/read view to one existing slot.
/// `relative_id` is relative to filesDir/saves; no file bytes or timestamps change.
/// The caller releases this view on a confirmed engine result or explicit
/// failure/cancellation. Only original-engine deserialization confirms success.
pub fn select_bootstrap_save(fs_root: &Path, relative_id: &str) -> io::Result<()> {
    if !save_slot_relative(relative_id) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a canonical story/sandbox progress slot"));
    }
    let root_metadata = fs::symlink_metadata(fs_root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "save root is not a real directory"));
    }
    let root = fs_root.canonicalize()?;
    let mut path = root.clone();
    let parts: Vec<_> = relative_id.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        path.push(part);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink()
            || (index + 1 == parts.len() && !metadata.is_file())
            || (index + 1 != parts.len() && !metadata.is_dir())
        {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "save slot contains a symlink or wrong file type"));
        }
    }
    if path.canonicalize()? != root.join(relative_id) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "save slot escaped the canonical root"));
    }
    // Only publish a validated selection. A failed selection preserves the prior view.
    *BOOTSTRAP_SAVE.lock().unwrap() = Some(BootstrapSave {
        root, relative_id: relative_id.to_owned(), read_observed: false,
    });
    Ok(())
}

/// Clear the guest-only filter. This never deletes, renames or edits a save.
pub fn finish_bootstrap_save() {
    *BOOTSTRAP_SAVE.lock().unwrap() = None;
}

pub fn bootstrap_save_pending() -> bool {
    BOOTSTRAP_SAVE.lock().unwrap().is_some()
}

pub fn bootstrap_selected_relative_id() -> Option<String> {
    BOOTSTRAP_SAVE.lock().unwrap().as_ref().map(|save| save.relative_id.clone())
}

pub(super) fn bootstrap_read_visible(fs_root: &Path, guest: &str) -> bool {
    let state = BOOTSTRAP_SAVE.lock().unwrap();
    let Some(save) = state.as_ref().filter(|save| save.root == fs_root) else { return true; };
    let relative = guest.trim_start_matches('/');
    let Some((directory, name)) = relative.rsplit_once('/') else { return true; };
    let progress_directory = matches!(directory,
        "userdata/Documents/Rockstar Games/GTA V/Default" |
        "sandbox/Documents/Rockstar Games/GTA V/Default");
    // Also hide SGTA5 backup/candidate names during autoload. Settings and all
    // other files retain their ordinary guest view; writes are never filtered.
    let progress_candidate = name.as_bytes().get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"SGTA5"));
    !progress_directory || !progress_candidate || relative == save.relative_id
}

pub(super) fn observe_bootstrap_read(fs_root: &Path, guest: &str, bytes: u64) {
    if bytes == 0 { return; }
    let mut state = BOOTSTRAP_SAVE.lock().unwrap();
    if let Some(save) = state.as_mut().filter(|save| save.root == fs_root
        && guest.trim_start_matches('/') == save.relative_id && !save.read_observed)
    {
        save.read_observed = true;
        eprintln!("[save] selected bootstrap read observed path={} bytes={bytes}; awaiting engine deserialization success", save.relative_id);
    }
}

/// `fs_root` is the native `MUGUET_FSROOT` (normally app filesDir/saves).
/// Keep story/userdata and sandbox namespaces separate; never replace user files.
pub fn ensure_save_directories(fs_root: &Path) -> io::Result<()> {
    fs::create_dir_all(fs_root)?;
    if fs::symlink_metadata(fs_root)?.file_type().is_symlink() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "save root is a symlink"));
    }
    for relative in [
        "tmp", "home/web_user", "dev", "proc/self",
        "userdata/Documents/Rockstar Games/GTA V/Default",
        "sandbox/Documents/Rockstar Games/GTA V/Default",
    ] {
        let mut path = fs_root.to_path_buf();
        for component in Path::new(relative).components() {
            let Component::Normal(name) = component else { unreachable!() };
            path.push(name);
            match fs::create_dir(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let metadata = fs::symlink_metadata(&path)?;
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                                                  "save directory is a symlink or a file"));
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

pub(super) fn is_persistent_guest_path(guest: &str) -> bool {
    guest.starts_with("/userdata/") || guest.starts_with("/sandbox/")
}

/// Flush a successful save/rename to disk; no copy, upload or content rewrite.
pub(super) fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no parent directory"))?;
    File::open(parent)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn temporary() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("muguet-save-layout-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn layout_is_idempotent_and_preserves_both_namespaces() {
        let path = temporary();
        ensure_save_directories(&path).unwrap();
        let story = path.join("userdata/Documents/Rockstar Games/GTA V/settings.xml");
        let sandbox = path.join("sandbox/Documents/Rockstar Games/GTA V/settings.xml");
        fs::write(&story, b"story settings").unwrap();
        fs::write(&sandbox, b"sandbox settings").unwrap();
        ensure_save_directories(&path).unwrap();
        assert_eq!(fs::read(&story).unwrap(), b"story settings");
        assert_eq!(fs::read(&sandbox).unwrap(), b"sandbox settings");
        assert!(story.parent().unwrap().join("Default").is_dir());
        assert!(sandbox.parent().unwrap().join("Default").is_dir());
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn existing_file_or_symlink_is_rejected_without_replacing_it() {
        let path = temporary();
        fs::write(path.join("userdata"), b"user content").unwrap();
        assert!(ensure_save_directories(&path).is_err());
        assert_eq!(fs::read(path.join("userdata")).unwrap(), b"user content");
        fs::remove_file(path.join("userdata")).unwrap();
        let outside = temporary();
        std::os::unix::fs::symlink(&outside, path.join("userdata")).unwrap();
        assert!(ensure_save_directories(&path).is_err());
        assert!(!outside.join("Documents").exists());
        fs::remove_dir_all(path).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }

    #[test]
    fn selected_slot_is_canonical_existing_and_read_view_only() {
        let _serial = TEST_SERIAL.lock().unwrap();
        let path = temporary();
        ensure_save_directories(&path).unwrap();
        let slot = "userdata/Documents/Rockstar Games/GTA V/Default/SGTA50015";
        fs::write(path.join(slot), b"unmodified engine slot").unwrap();
        select_bootstrap_save(&path, slot).unwrap();
        assert!(bootstrap_save_pending());
        assert_eq!(bootstrap_selected_relative_id().as_deref(), Some(slot));
        let canonical = path.canonicalize().unwrap();
        assert!(bootstrap_read_visible(&canonical, &format!("/{slot}")));
        assert!(!bootstrap_read_visible(&canonical, "/userdata/Documents/Rockstar Games/GTA V/Default/SGTA50000"));
        assert!(!bootstrap_read_visible(&canonical, "/sandbox/Documents/Rockstar Games/GTA V/Default/SGTA50015"));
        assert!(!bootstrap_read_visible(&canonical, "/userdata/Documents/Rockstar Games/GTA V/Default/SGTA50015.bak"));
        assert!(!bootstrap_read_visible(&canonical, "/userdata/Documents/Rockstar Games/GTA V/Default/sgta50001"));
        assert!(!bootstrap_read_visible(&canonical, "/userdata/Documents/Rockstar Games/GTA V/Default/sGtA50015.bak"));
        assert!(bootstrap_read_visible(&canonical, "/userdata/Documents/Rockstar Games/GTA V/settings.xml"));
        for invalid in ["/userdata/Documents/Rockstar Games/GTA V/Default/SGTA50015",
            "userdata/Documents/Rockstar Games/GTA V/Default/../SGTA50015",
            "userdata/Documents/Rockstar Games/GTA V/Default/SGTA50016",
            "userdata/Documents/Rockstar Games/GTA V/Default/SGTA50000",
            "userdata\\Documents\\Rockstar Games\\GTA V\\Default\\SGTA50015"]
        {
            assert!(select_bootstrap_save(&path, invalid).is_err(), "{invalid}");
            assert_eq!(bootstrap_selected_relative_id().as_deref(), Some(slot));
        }
        let symlink_slot = "userdata/Documents/Rockstar Games/GTA V/Default/SGTA50001";
        std::os::unix::fs::symlink(path.join(slot), path.join(symlink_slot)).unwrap();
        assert!(select_bootstrap_save(&path, symlink_slot).is_err());
        let directory_slot = "userdata/Documents/Rockstar Games/GTA V/Default/SGTA50002";
        fs::create_dir(path.join(directory_slot)).unwrap();
        assert!(select_bootstrap_save(&path, directory_slot).is_err());
        finish_bootstrap_save();
        assert!(!bootstrap_save_pending());
        assert!(bootstrap_read_visible(&canonical, &format!("/{symlink_slot}")));
        assert_eq!(fs::read(path.join(slot)).unwrap(), b"unmodified engine slot");
        fs::remove_dir_all(path).unwrap();
    }
}
