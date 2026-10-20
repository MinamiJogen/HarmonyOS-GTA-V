//! One-shot original-engine save diagnostics in a fresh, isolated filesystem.
//! This module never opens the application's real saves or rewrites its config.
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const REQUEST: &str = "game-save-validation.request.json";
const REQUEST_LIMIT: u64 = 4096;

// Public platform open(2) flags; keep this module directly rustc-testable.
#[cfg(target_os = "macos")]
const READ_FLAGS: i32 = 0x0000_0100 | 0x0000_0004; // O_NOFOLLOW | O_NONBLOCK
#[cfg(target_os = "linux")]
const READ_FLAGS: i32 = 0x0002_0000 | 0x0000_0800; // OHOS/Linux

#[derive(Debug)]
pub struct Validation {
    pub fs_root: PathBuf,
    pub args: Vec<String>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn real_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "validation parent must be a real directory"));
    }
    Ok(())
}

fn same_request(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.dev() == b.dev() && a.ino() == b.ino() && a.len() == b.len()
        && a.mtime() == b.mtime() && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime() && a.ctime_nsec() == b.ctime_nsec()
}

/// Consume an app-private request exactly once into a newly reserved directory.
/// No request means no filesystem mutation. Invalid/existing/symlink requests do
/// not alter the request or any existing files. A later I/O failure may leave a
/// new reserved diagnostic directory; it is deliberately never reused/removed.
/// The caller must initialize `fs_root` and set MUGUET_FSROOT before running the
/// original engine. Only that isolated root may receive the validation save.
pub fn prepare(files_dir: &Path) -> io::Result<Option<Validation>> {
    real_directory(files_dir)?;
    let files_dir = files_dir.canonicalize()?;
    let request = files_dir.join(REQUEST);
    let path_metadata = match fs::symlink_metadata(&request) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "validation request must be a regular non-symlink file"));
    }
    let file = OpenOptions::new().read(true).custom_flags(READ_FLAGS).open(&request)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || !same_request(&metadata, &path_metadata) {
        return Err(invalid("validation request changed before reading"));
    }
    if metadata.len() > REQUEST_LIMIT { return Err(invalid("validation request exceeds 4096 bytes")); }
    let mut bytes = Vec::new();
    (&file).take(REQUEST_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > REQUEST_LIMIT { return Err(invalid("validation request grew beyond limit")); }
    let (id, updates) = RequestParser { bytes: &bytes, index: 0 }.parse()?;
    if !same_request(&metadata, &file.metadata()?) {
        return Err(invalid("validation request changed while reading"));
    }

    let parent = files_dir.join("save-validation");
    match DirBuilder::new().mode(0o700).create(&parent) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => real_directory(&parent)?,
        Err(error) => return Err(error),
    }
    real_directory(&parent)?;
    let directory = parent.join(id);
    // Success of create_dir is mandatory: never reuse an existing diagnostic ID.
    DirBuilder::new().mode(0o700).create(&directory)?;
    let fs_root = directory.join("saves");
    DirBuilder::new().mode(0o700).create(&fs_root)?;

    let current = fs::symlink_metadata(&request)?;
    if current.file_type().is_symlink() || !current.is_file() || !same_request(&metadata, &current) {
        return Err(invalid("validation request changed before consumption"));
    }
    // The destination is in the directory just exclusively created above.
    fs::rename(&request, directory.join("request.json"))?;
    File::open(&directory)?.sync_all()?;
    File::open(&parent)?.sync_all()?;
    File::open(&files_dir)?.sync_all()?;
    Ok(Some(Validation {
        fs_root,
        args: vec![format!("-wasmtestsave={updates}"),
            "-savegame_tty=debug1".into(), "-savegamepc_tty=debug1".into()],
    }))
}

// Deliberately narrow JSON grammar for the two-field private diagnostic request.
// Escaped strings, unknown/duplicate fields, floats and exponents are rejected.
// All accepted requests are valid JSON; ID bytes need no escape representation.
struct RequestParser<'a> { bytes: &'a [u8], index: usize }
impl RequestParser<'_> {
    fn whitespace(&mut self) {
        while self.bytes.get(self.index).is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n')) {
            self.index += 1;
        }
    }
    fn take(&mut self, byte: u8) -> bool {
        self.whitespace();
        if self.bytes.get(self.index) == Some(&byte) { self.index += 1; true } else { false }
    }
    fn expect(&mut self, byte: u8) -> io::Result<()> {
        if self.take(byte) { Ok(()) } else { Err(invalid("malformed validation request JSON")) }
    }
    fn string(&mut self) -> io::Result<&[u8]> {
        self.expect(b'"')?;
        let start = self.index;
        while let Some(&byte) = self.bytes.get(self.index) {
            if byte == b'"' {
                let end = self.index;
                self.index += 1;
                return Ok(&self.bytes[start..end]);
            }
            if !(0x20..=0x7e).contains(&byte) || byte == b'\\' { break; }
            self.index += 1;
        }
        Err(invalid("validation request requires unescaped ASCII strings"))
    }
    fn updates(&mut self) -> io::Result<u32> {
        self.whitespace();
        if !self.bytes.get(self.index).is_some_and(|b| matches!(b, b'1'..=b'9')) {
            return Err(invalid("saveAfterUpdates must be an integer from 1 to 100000"));
        }
        let mut value = 0u32;
        while let Some(byte @ b'0'..=b'9') = self.bytes.get(self.index) {
            value = value.checked_mul(10).and_then(|n| n.checked_add((byte - b'0') as u32))
                .ok_or_else(|| invalid("saveAfterUpdates overflow"))?;
            self.index += 1;
        }
        if value > 100000 { return Err(invalid("saveAfterUpdates exceeds 100000")); }
        Ok(value)
    }
    fn parse(mut self) -> io::Result<(String, u32)> {
        self.expect(b'{')?;
        let mut id = None;
        let mut updates = None;
        loop {
            let key = self.string()?.to_vec();
            self.expect(b':')?;
            match key.as_slice() {
                b"id" if id.is_none() => {
                    let value = self.string()?;
                    if value.is_empty() || value.len() > 64
                        || !value.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
                    { return Err(invalid("id must contain 1 to 64 ASCII alphanumeric, underscore or hyphen bytes")); }
                    id = Some(String::from_utf8(value.to_vec()).unwrap());
                }
                b"saveAfterUpdates" if updates.is_none() => updates = Some(self.updates()?),
                _ => return Err(invalid("unknown or duplicate validation request field")),
            }
            if self.take(b'}') { break; }
            self.expect(b',')?;
        }
        self.whitespace();
        if self.index != self.bytes.len() { return Err(invalid("trailing validation request data")); }
        Ok((id.ok_or_else(|| invalid("missing id"))?, updates.ok_or_else(|| invalid("missing saveAfterUpdates"))?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn temporary() -> PathBuf {
        let path = std::env::temp_dir().join(format!("muguet-isolated-save-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).unwrap();
        path
    }
    fn request(root: &Path, id: &str) {
        fs::write(root.join(REQUEST), format!(r#"{{"id":"{id}","saveAfterUpdates":1500}}"#)).unwrap();
    }
    #[test]
    fn no_request_has_no_side_effect() {
        let root = temporary();
        assert!(prepare(&root).unwrap().is_none());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn fresh_root_consumes_once_and_preserves_real_save_and_config() {
        let root = temporary();
        fs::create_dir(root.join("saves")).unwrap();
        fs::write(root.join("saves/SGTA50015"), b"real progress").unwrap();
        fs::write(root.join("config.json"), b"real config").unwrap();
        request(&root, "test_01-safe");
        let original = fs::read(root.join(REQUEST)).unwrap();
        let validation = prepare(&root).unwrap().unwrap();
        assert_eq!(validation.fs_root, root.canonicalize().unwrap().join("save-validation/test_01-safe/saves"));
        assert!(validation.fs_root.is_dir());
        assert_eq!(validation.args, ["-wasmtestsave=1500", "-savegame_tty=debug1", "-savegamepc_tty=debug1"]);
        assert_eq!(fs::read(validation.fs_root.parent().unwrap().join("request.json")).unwrap(), original);
        assert!(!root.join(REQUEST).exists());
        assert!(prepare(&root).unwrap().is_none());
        request(&root, "test_01-safe");
        assert_eq!(prepare(&root).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        assert!(root.join(REQUEST).is_file());
        assert_eq!(fs::read(root.join("saves/SGTA50015")).unwrap(), b"real progress");
        assert_eq!(fs::read(root.join("config.json")).unwrap(), b"real config");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_request_never_creates_namespace_or_consumes_request() {
        let root = temporary();
        for invalid in [r#"{}"#, r#"{"id":"../escape","saveAfterUpdates":1}"#,
            r#"{"id":"x","saveAfterUpdates":0}"#, r#"{"id":"x","saveAfterUpdates":100001}"#,
            r#"{"id":"x","saveAfterUpdates":01}"#, r#"{"id":"x","saveAfterUpdates":1.0}"#,
            r#"{"id":"x","saveAfterUpdates":1,"id":"y"}"#,
            r#"{"id":"x","saveAfterUpdates":1,"other":true}"#,
            r#"{"id":"x","saveAfterUpdates":1,}"#, r#"{"id":"x","saveAfterUpdates":1}x"#,
            r#"{"id":"\u0078","saveAfterUpdates":1}"#,
            r#"{"id":"x","saveAfterUpdates":999999999999999999999999999999999}"#]
        {
            fs::write(root.join(REQUEST), invalid).unwrap();
            assert!(prepare(&root).is_err(), "{invalid}");
            assert_eq!(fs::read(root.join(REQUEST)).unwrap(), invalid.as_bytes());
            assert!(!root.join("save-validation").exists());
        }
        let id64 = "a".repeat(64);
        let valid = format!(r#" {{"saveAfterUpdates":100000,"id":"{id64}"}} "#);
        assert!(RequestParser { bytes: valid.as_bytes(), index: 0 }.parse().is_ok());
        let invalid = format!(r#"{{"id":"{}","saveAfterUpdates":1}}"#, "a".repeat(65));
        assert!(RequestParser { bytes: invalid.as_bytes(), index: 0 }.parse().is_err());
        assert!(RequestParser { bytes: br#"{"id":"_","saveAfterUpdates":1}"#, index: 0 }.parse().is_ok());
        fs::write(root.join(REQUEST), vec![b' '; REQUEST_LIMIT as usize + 1]).unwrap();
        assert!(prepare(&root).is_err());
        assert!(!root.join("save-validation").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn symlink_parent_id_and_request_are_rejected_without_touching_targets() {
        let root = temporary();
        let outside = temporary();
        fs::write(outside.join("request.json"), br#"{"id":"x","saveAfterUpdates":1}"#).unwrap();
        std::os::unix::fs::symlink(outside.join("request.json"), root.join(REQUEST)).unwrap();
        assert!(prepare(&root).is_err());
        fs::remove_file(root.join(REQUEST)).unwrap();
        request(&root, "x");
        std::os::unix::fs::symlink(&outside, root.join("save-validation")).unwrap();
        assert!(prepare(&root).is_err());
        assert!(!outside.join("x").exists());
        fs::remove_file(root.join("save-validation")).unwrap();
        fs::create_dir(root.join("save-validation")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("save-validation/x")).unwrap();
        assert!(prepare(&root).is_err());
        assert!(!outside.join("saves").exists());
        assert!(root.join(REQUEST).is_file());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
