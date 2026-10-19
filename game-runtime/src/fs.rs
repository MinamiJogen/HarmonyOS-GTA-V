// muguet - fs.rs
// Constantin Clerc (c22dev) - 0.1

use crate::{read_bytes, read_cstr, read_u64, write_bytes, write_u64};
use std::collections::HashMap;
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use wasmtime::{SharedMemory, Val};

const EBADF: i64 = 8;
const EEXIST: i64 = 20;
const EINVAL: i64 = 28;
const EISDIR: i64 = 31;
const ENOENT: i64 = 44;
const ENOSYS: i64 = 52;
const ENOTDIR: i64 = 54;
const ENOTEMPTY: i64 = 55;
const ENOTTY: i64 = 59;
const EIO: i64 = 29;
const EFAULT: i64 = 21;

const O_ACCMODE: i64 = 3;
const O_CREAT: i64 = 64;
const O_EXCL: i64 = 128;
const O_TRUNC: i64 = 512;
const O_APPEND: i64 = 1024;
const O_DIRECTORY: i64 = 65536;
const AT_FDCWD: i64 = -100;

const S_IFCHR: u32 = 0o020000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;

enum Entry {
    File { file: File },
    Dir { guest: String, names: Option<Vec<(String, u8)>>, pos: u64 },
}

struct Fs {
    root: PathBuf,
    fds: HashMap<i64, Entry>,
    next_fd: i64,
}

static FS: LazyLock<Mutex<Fs>> = LazyLock::new(|| {
    let root = std::env::var("MUGUET_FSROOT").unwrap_or_else(|_| "fsroot".into());
    let root = PathBuf::from(root);
    for d in ["tmp", "home/web_user", "dev", "proc/self"] {
        let _ = fs::create_dir_all(root.join(d));
    }
    Mutex::new(Fs { root, fds: HashMap::new(), next_fd: 3 })
});

fn guest_abs(fs: &Fs, dirfd: i64, path: &str) -> Result<String, i64> {
    let base = if path.starts_with('/') || dirfd == AT_FDCWD {
        "/".to_string()
    } else {
        match fs.fds.get(&dirfd) {
            Some(Entry::Dir { guest, .. }) => guest.clone(),
            Some(_) => return Err(ENOTDIR),
            None => return Err(EBADF),
        }
    };
    let mut parts: Vec<String> = Vec::new();
    for c in Path::new(&base).join(path).components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::ParentDir => { parts.pop(); }
            _ => {}
        }
    }
    Ok(format!("/{}", parts.join("/")))
}

fn host_path(fs: &Fs, guest: &str) -> PathBuf {
    fs.root.join(guest.trim_start_matches('/'))
}

fn write_stat(mem: &SharedMemory, buf: u64, md: &fs::Metadata, chr: bool) {
    let mode = if chr { S_IFCHR | 0o666 } else if md.is_dir() { S_IFDIR | 0o777 } else { S_IFREG | 0o666 };
    let mut b = [0u8; 112];
    let put32 = |b: &mut [u8], o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    let put64 = |b: &mut [u8], o: usize, v: u64| b[o..o + 8].copy_from_slice(&v.to_le_bytes());
    put32(&mut b, 0, 1);
    put32(&mut b, 4, mode);
    put64(&mut b, 8, 1);
    put64(&mut b, 32, md.len());
    put32(&mut b, 40, 4096);
    put32(&mut b, 44, md.len().div_ceil(512) as u32);
    for (o, s) in [(48, md.atime()), (64, md.mtime()), (80, md.ctime())] {
        put64(&mut b, o, s as u64);
    }
    put64(&mut b, 96, md.ino());
    write_bytes(mem, buf, &b);
}

fn ok_i(results: &mut [Val], v: i64) {
    if let Some(r) = results.get_mut(0) {
        *r = match r { Val::I64(_) => Val::I64(v), _ => Val::I32(v as i32) };
    }
}

fn iovecs(mem: &SharedMemory, iovs: u64, n: u64) -> Vec<(u64, u64)> {
    (0..n).map(|k| (read_u64(mem, iovs + k * 16).unwrap_or(0), read_u64(mem, iovs + k * 16 + 8).unwrap_or(0))).collect()
}

fn read_dir_names(host: &Path) -> Vec<(String, u8)> {
    let mut v = vec![(".".to_string(), 4u8), ("..".to_string(), 4u8)];
    if let Ok(rd) = fs::read_dir(host) {
        for e in rd.flatten() {
            let t = if e.file_type().map(|t| t.is_dir()).unwrap_or(false) { 4 } else { 8 };
            v.push((e.file_name().to_string_lossy().into_owned(), t));
        }
    }
    v
}

fn statfs_bytes(host: &Path) -> Result<[u8; 120], i64> {
    let path = CString::new(host.as_os_str().as_bytes()).map_err(|_| EINVAL)?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) } != 0 {
        let error = std::io::Error::last_os_error();
        return Err(match error.kind() {
            std::io::ErrorKind::NotFound => ENOENT,
            std::io::ErrorKind::PermissionDenied => 2,
            _ => EIO,
        });
    }
    let stats = unsafe { stats.assume_init() };
    // The memory64 guest uses 8-byte statfs fields, not the host's struct layout.
    let mut bytes = [0u8; 120];
    for (offset, value) in [
        (8, stats.f_bsize as u64), (16, stats.f_blocks as u64),
        (24, stats.f_bfree as u64), (32, stats.f_bavail as u64),
        (40, stats.f_files as u64), (48, stats.f_ffree as u64),
        (56, stats.f_fsid as u64), (64, stats.f_namemax as u64),
        (72, stats.f_frsize as u64), (80, stats.f_flag as u64),
    ] {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }
    Ok(bytes)
}

pub fn handle(name: &str, mem: &SharedMemory, p: &[Val], results: &mut [Val]) -> bool {
    let a = |i: usize| -> i64 { match p.get(i) { Some(Val::I32(x)) => *x as i64, Some(Val::I64(x)) => *x, _ => 0 } };
    let mut fs = FS.lock().unwrap();
    let r: i64 = match name {
        "__syscall_openat" => (|| {
            let flags = a(2);
            let mode_creat = flags & O_CREAT != 0;
            let guest = guest_abs(&fs, a(0), &read_cstr(mem, a(1) as u64)).map_err(|e| -e)?;
            let host = host_path(&fs, &guest);
            let exists = host.exists();
            if exists && mode_creat && flags & O_EXCL != 0 { return Err(-EEXIST); }
            if host.is_dir() {
                if flags & O_ACCMODE != 0 { return Err(-EISDIR); }
                let fd = fs.next_fd; fs.next_fd += 1;
                fs.fds.insert(fd, Entry::Dir { guest, names: None, pos: 0 });
                return Ok(fd);
            }
            if flags & O_DIRECTORY != 0 { return Err(if exists { -ENOTDIR } else { -ENOENT }); }
            if !exists && !mode_creat { return Err(-ENOENT); }
            let acc = flags & O_ACCMODE;
            let file = OpenOptions::new()
                .read(acc == 0 || acc == 2)
                .write(acc == 1 || acc == 2)
                .append(flags & O_APPEND != 0)
                .create(mode_creat)
                .truncate(flags & O_TRUNC != 0)
                .open(&host)
                .map_err(|_| -ENOENT)?;
            let fd = fs.next_fd; fs.next_fd += 1;
            fs.fds.insert(fd, Entry::File { file });
            Ok(fd)
        })().unwrap_or_else(|e| e),
        "__syscall_stat64" | "__syscall_lstat64" | "__syscall_newfstatat" => {
            let (dirfd, path, buf) = if name == "__syscall_newfstatat" { (a(0), a(1), a(2)) } else { (AT_FDCWD, a(0), a(1)) };
            match guest_abs(&fs, dirfd, &read_cstr(mem, path as u64)) {
                Ok(g) => match fs::metadata(host_path(&fs, &g)) {
                    Ok(md) => { write_stat(mem, buf as u64, &md, g.starts_with("/dev/")); 0 }
                    Err(_) => -ENOENT,
                },
                Err(e) => -e,
            }
        }
        "__syscall_fstat64" => {
            let fd = a(0);
            if fd <= 2 {
                let md = fs::metadata("/dev/null").unwrap();
                write_stat(mem, a(1) as u64, &md, true); 0
            } else {
                match fs.fds.get(&fd) {
                    Some(Entry::File { file, .. }) => match file.metadata() { Ok(md) => { write_stat(mem, a(1) as u64, &md, false); 0 } Err(_) => -EBADF },
                    Some(Entry::Dir { guest, .. }) => { let md = fs::metadata(host_path(&fs, guest)).unwrap(); write_stat(mem, a(1) as u64, &md, false); 0 }
                    None => -EBADF,
                }
            }
        }
        "__syscall_mkdirat" => match guest_abs(&fs, a(0), &read_cstr(mem, a(1) as u64)) {
            Ok(g) => { let h = host_path(&fs, &g); if h.exists() { -EEXIST } else if fs::create_dir(&h).is_ok() { 0 } else { -ENOENT } }
            Err(e) => -e,
        },
        "__syscall_unlinkat" => match guest_abs(&fs, a(0), &read_cstr(mem, a(1) as u64)) {
            Ok(g) => {
                let h = host_path(&fs, &g);
                let res = if a(2) & 0x200 != 0 { fs::remove_dir(&h) } else { fs::remove_file(&h) };
                if res.is_ok() { 0 } else if !h.exists() { -ENOENT } else { -ENOTEMPTY }
            }
            Err(e) => -e,
        },
        "__syscall_rmdir" => match guest_abs(&fs, AT_FDCWD, &read_cstr(mem, a(0) as u64)) {
            Ok(g) => { let h = host_path(&fs, &g); if fs::remove_dir(&h).is_ok() { 0 } else if !h.exists() { -ENOENT } else { -ENOTEMPTY } }
            Err(e) => -e,
        },
        "__syscall_renameat" => {
            let from = guest_abs(&fs, a(0), &read_cstr(mem, a(1) as u64));
            let to = guest_abs(&fs, a(2), &read_cstr(mem, a(3) as u64));
            match (from, to) {
                (Ok(f), Ok(t)) => if fs::rename(host_path(&fs, &f), host_path(&fs, &t)).is_ok() { 0 } else { -ENOENT },
                (Err(e), _) | (_, Err(e)) => -e,
            }
        }
        "__syscall_ftruncate64" => match fs.fds.get(&a(0)) {
            Some(Entry::File { file, .. }) => if file.set_len(a(1) as u64).is_ok() { 0 } else { -EINVAL },
            _ => -EBADF,
        },
        "__syscall_getcwd" => {
            let (buf, size) = (a(0) as u64, a(1) as u64);
            if size < 2 { -EINVAL } else { write_bytes(mem, buf, b"/\0"); 2 }
        }
        "__syscall_getdents64" => {
            let (fd, dirp, count) = (a(0), a(1) as u64, a(2) as u64);
            let root = fs.root.clone();
            match fs.fds.get_mut(&fd) {
                Some(Entry::Dir { guest, names, pos }) => {
                    let names = names.get_or_insert_with(|| read_dir_names(&root.join(guest.trim_start_matches('/'))));
                    let start = (*pos / 280) as usize;
                    let end = names.len().min(start + (count / 280) as usize);
                    let mut off = 0u64;
                    for (idx, (n, t)) in names.iter().enumerate().take(end).skip(start) {
                        let mut rec = [0u8; 280];
                        rec[0..8].copy_from_slice(&((idx as u64) + 1).to_le_bytes());
                        rec[8..16].copy_from_slice(&(((idx + 1) * 280) as u64).to_le_bytes());
                        rec[16..18].copy_from_slice(&280u16.to_le_bytes());
                        rec[18] = *t;
                        let nb = n.as_bytes();
                        let l = nb.len().min(255);
                        rec[19..19 + l].copy_from_slice(&nb[..l]);
                        write_bytes(mem, dirp + off, &rec);
                        off += 280;
                    }
                    *pos = (end * 280) as u64;
                    off as i64
                }
                Some(_) => -ENOTDIR,
                None => -EBADF,
            }
        }
        "__syscall_fcntl64" => if a(0) <= 2 || fs.fds.contains_key(&a(0)) { 0 } else { -EBADF },
        "__syscall_ioctl" => -ENOTTY,
        "__syscall_statfs64" => (|| {
            if a(1) != 120 { return Err(-EINVAL); }
            let guest = guest_abs(&fs, AT_FDCWD, &read_cstr(mem, a(0) as u64)).map_err(|e| -e)?;
            let host = host_path(&fs, &guest);
            let bytes = statfs_bytes(host.ancestors().find(|p| p.exists()).unwrap_or(&host)).map_err(|e| -e)?;
            if !write_bytes(mem, a(2) as u64, &bytes) { return Err(-EFAULT); }
            Ok(0)
        })().unwrap_or_else(|e| e),

        "__syscall_socket" | "__syscall_connect" | "__syscall_accept4" | "__syscall_sendto"
        | "__syscall_recvfrom" | "__syscall_setsockopt" | "__syscall_getsockopt"
        | "__syscall_getsockname" | "__syscall_shutdown" | "__syscall_poll" => -ENOSYS,

        "fd_read" | "fd_pread" => {
            let fd = a(0);
            let iov = iovecs(mem, a(1) as u64, a(2) as u64);
            let (offset, nread_ptr) = if name == "fd_pread" { (Some(a(3) as u64), a(4) as u64) } else { (None, a(3) as u64) };
            match fs.fds.get_mut(&fd) {
                Some(Entry::File { file, .. }) => {
                    let mut total = 0u64;
                    let mut off = offset;
                    for (ptr, len) in iov {
                        let mut buf = vec![0u8; len as usize];
                        let n = match off {
                            Some(o) => file.read_at(&mut buf, o).unwrap_or(0),
                            None => file.read(&mut buf).unwrap_or(0),
                        };
                        write_bytes(mem, ptr, &buf[..n]);
                        total += n as u64;
                        if let Some(o) = off.as_mut() { *o += n as u64; }
                        if (n as u64) < len { break; }
                    }
                    write_u64(mem, nread_ptr, total);
                    0
                }
                Some(Entry::Dir { .. }) => EISDIR,
                None if fd == 0 => { write_u64(mem, nread_ptr, 0); 0 }
                None => EBADF,
            }
        }
        "fd_write" | "fd_pwrite" if a(0) > 2 => {
            let fd = a(0);
            let iov = iovecs(mem, a(1) as u64, a(2) as u64);
            let (offset, nw_ptr) = if name == "fd_pwrite" { (Some(a(3) as u64), a(4) as u64) } else { (None, a(3) as u64) };
            match fs.fds.get_mut(&fd) {
                Some(Entry::File { file, .. }) => {
                    let mut total = 0u64;
                    let mut off = offset;
                    for (ptr, len) in iov {
                        let data = read_bytes(mem, ptr, len);
                        let ok = match off { Some(o) => file.write_all_at(&data, o).is_ok(), None => file.write_all(&data).is_ok() };
                        if !ok { break; }
                        total += len;
                        if let Some(o) = off.as_mut() { *o += len; }
                    }
                    write_u64(mem, nw_ptr, total);
                    0
                }
                _ => EBADF,
            }
        }
        "fd_seek" => {
            let (fd, offset, whence, out) = (a(0), a(1), a(2), a(3) as u64);
            match fs.fds.get_mut(&fd) {
                Some(Entry::File { file, .. }) => {
                    let sf = match whence { 0 => SeekFrom::Start(offset as u64), 1 => SeekFrom::Current(offset), 2 => SeekFrom::End(offset), _ => return { ok_i(results, EINVAL); true } };
                    match file.seek(sf) { Ok(p) => { write_u64(mem, out, p); 0 } Err(_) => EINVAL }
                }
                Some(Entry::Dir { pos, .. }) => { if whence == 0 { *pos = offset as u64; } write_u64(mem, out, *pos); 0 }
                None => EBADF,
            }
        }
        "fd_close" => if fs.fds.remove(&a(0)).is_some() || a(0) <= 2 { 0 } else { EBADF },
        _ => return false,
    };
    drop(fs);
    ok_i(results, r);
    true
}

pub fn httpfs_manifest(mem: &SharedMemory, p: &[Val], results: &mut [Val]) {
    static MANIFEST: LazyLock<Option<Vec<u8>>> = LazyLock::new(|| {
        let dir = std::env::var("MUGUET_DATA").unwrap_or_default();
        fs::read(Path::new(&dir).join("manifest.json")).ok()
    });
    let a = |i: usize| -> i64 { match p.get(i) { Some(Val::I32(x)) => *x as i64, Some(Val::I64(x)) => *x, _ => 0 } };
    let r = match MANIFEST.as_ref() {
        None => -1,
        Some(bytes) => {
            if a(1) as usize > bytes.len() {
                write_bytes(mem, a(0) as u64, bytes);
                write_bytes(mem, a(0) as u64 + bytes.len() as u64, &[0]);
            }
            bytes.len() as i64
        }
    };
    ok_i(results, r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statfs_reports_real_available_space_in_guest_layout() {
        let host = std::env::temp_dir();
        let bytes = statfs_bytes(&host).unwrap();
        let field = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        let path = CString::new(host.as_os_str().as_bytes()).unwrap();
        let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        assert_eq!(unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) }, 0);
        let stats = unsafe { stats.assume_init() };
        assert_eq!(field(8), stats.f_bsize as u64);
        assert_eq!(field(16), stats.f_blocks as u64);
        assert!(field(8) > 0);
        assert!(field(32) <= field(24));
        assert!(field(24) <= field(16));
        assert_eq!(field(72), stats.f_frsize as u64);
        // A zero-filled response previously made this writable disk look full.
        assert!(field(32) * field(72) > 1024 * 1024);
        assert_eq!(statfs_bytes(&host.join(format!("muguet-missing-{}", std::process::id()))), Err(ENOENT));
    }

    #[test]
    fn guest_save_survives_close_and_reopen() {
        let root = std::env::temp_dir().join(format!("muguet-save-test-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        std::env::set_var("MUGUET_FSROOT", &root);
        let mut config = wasmtime::Config::new();
        config.shared_memory(true);
        let engine = wasmtime::Engine::new(&config).unwrap();
        let mem = SharedMemory::new(&engine, wasmtime::MemoryType::shared(1, 1)).unwrap();
        let call = |name: &str, args: &[i64]| {
            let params: Vec<_> = args.iter().map(|v| Val::I64(*v)).collect();
            let mut result = [Val::I32(-999)];
            assert!(handle(name, &mem, &params, &mut result));
            result[0].unwrap_i32() as i64
        };
        write_bytes(&mem, 32, b"/tmp/SGTA50000\0");
        write_bytes(&mem, 64, b"/tmp/SGTA50001\0");
        let fd = call("__syscall_openat", &[AT_FDCWD, 32, 2 | O_CREAT | O_TRUNC, 0]);
        assert!(fd >= 3);
        let payload = b"save round trip";
        write_bytes(&mem, 512, payload);
        write_u64(&mem, 256, 512);
        write_u64(&mem, 264, payload.len() as u64);
        assert_eq!(call("fd_write", &[fd, 256, 1, 320]), 0);
        assert_eq!(read_u64(&mem, 320), Some(payload.len() as u64));
        assert_eq!(call("fd_close", &[fd]), 0);
        assert_eq!(call("__syscall_renameat", &[AT_FDCWD, 32, AT_FDCWD, 64]), 0);
        // Read from disk independently, then reopen through the guest interface.
        assert_eq!(fs::read(root.join("tmp/SGTA50001")).unwrap(), payload);
        let fd = call("__syscall_openat", &[AT_FDCWD, 64, 0, 0]);
        assert!(fd >= 3);
        write_bytes(&mem, 512, &[0; 15]);
        assert_eq!(call("fd_read", &[fd, 256, 1, 320]), 0);
        assert_eq!(crate::read_bytes(&mem, 512, payload.len() as u64), payload);
        assert_eq!(call("fd_close", &[fd]), 0);
        write_bytes(&mem, 96, b"/tmp\0");
        write_bytes(&mem, 1024, &[0xaa; 128]);
        assert_eq!(call("__syscall_statfs64", &[96, 120, 1024]), 0);
        assert!(read_u64(&mem, 1056).unwrap() > 0);
        assert_eq!(crate::read_bytes(&mem, 1144, 8), [0xaa; 8]);
        assert_eq!(call("__syscall_statfs64", &[96, 88, 1024]), -EINVAL);
        fs::remove_dir_all(root).unwrap();
    }
}
