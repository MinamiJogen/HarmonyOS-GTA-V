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

#[path = "save_management.rs"]
mod save_management;
pub use save_management::{
    bootstrap_save_pending, bootstrap_selected_relative_id, ensure_save_directories,
    finish_bootstrap_save, select_bootstrap_save,
};

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
    File { file: File, guest: String, sync_path: Option<PathBuf> },
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
    if let Err(error) = ensure_save_directories(&root) {
        eprintln!("[fs] save directory initialization failed: {error}");
    }
    let root = root.canonicalize().unwrap_or(root);
    Mutex::new(Fs { root, fds: HashMap::new(), next_fd: 3 })
});

fn guest_abs(fs: &Fs, dirfd: i64, path: &str) -> Result<String, i64> {
    // The original engine joins save paths using Windows separators even on
    // this POSIX guest. Normalize before resolving components so `..` remains
    // confined to the guest root and imported slash-layout slots are reachable.
    let path = path.replace('\\', "/");
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
    for c in Path::new(&base).join(&path).components() {
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
    for (o, s, ns) in [(48, md.atime(), md.atime_nsec()), (64, md.mtime(), md.mtime_nsec()), (80, md.ctime(), md.ctime_nsec())] {
        put64(&mut b, o, s as u64);
        put64(&mut b, o + 8, ns as u64);
    }
    put64(&mut b, 96, md.ino());
    write_bytes(mem, buf, &b);
}

fn ok_i(results: &mut [Val], v: i64) {
    if let Some(r) = results.get_mut(0) {
        *r = match r { Val::I64(_) => Val::I64(v), _ => Val::I32(v as i32) };
    }
}

fn memory_span(mem: &SharedMemory, addr: u64, len: u64) -> bool {
    addr.checked_add(len).is_some_and(|end| end <= mem.data().len() as u64)
}

fn iovecs(mem: &SharedMemory, iovs: u64, n: u64) -> Result<Vec<(u64, u64)>, i64> {
    if n > 1024 { return Err(EINVAL); }
    if !memory_span(mem, iovs, n * 16) { return Err(EFAULT); }
    (0..n).map(|k| {
        let ptr = read_u64(mem, iovs + k * 16).ok_or(EFAULT)?;
        let len = read_u64(mem, iovs + k * 16 + 8).ok_or(EFAULT)?;
        if !memory_span(mem, ptr, len) { return Err(EFAULT); }
        Ok((ptr, len))
    }).collect()
}

fn io_errno(error: &std::io::Error) -> i64 {
    match error.raw_os_error() {
        Some(libc::ENOSPC) => 51,
        Some(libc::EROFS) => 69,
        Some(libc::EFBIG) => 22,
        Some(libc::EXDEV) => 75,
        Some(libc::ELOOP) => 32,
        Some(libc::ENAMETOOLONG) => 37,
        Some(libc::EBADF) => EBADF,
        Some(libc::ENOTDIR) => ENOTDIR,
        Some(libc::EISDIR) => EISDIR,
        Some(libc::ENOTEMPTY) => ENOTEMPTY,
        _ => match error.kind() {
            std::io::ErrorKind::NotFound => ENOENT,
            std::io::ErrorKind::PermissionDenied => 2,
            std::io::ErrorKind::AlreadyExists => EEXIST,
            std::io::ErrorKind::InvalidInput => EINVAL,
            std::io::ErrorKind::Interrupted => 27,
            _ => EIO,
        },
    }
}

fn read_dir_names(host: &Path) -> Result<Vec<(String, u8)>, i64> {
    let mut v = vec![(".".to_string(), 4u8), ("..".to_string(), 4u8)];
    for entry in fs::read_dir(host).map_err(|error| io_errno(&error))? {
        let entry = entry.map_err(|error| io_errno(&error))?;
        let kind = entry.file_type().map_err(|error| io_errno(&error))?;
        let t = if kind.is_dir() { 4 } else if kind.is_symlink() { 10 } else { 8 };
        v.push((entry.file_name().to_string_lossy().into_owned(), t));
    }
    Ok(v)
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
            let acc = flags & O_ACCMODE;
            if acc == O_ACCMODE { return Err(-EINVAL); }
            if acc == 0 && !save_management::bootstrap_read_visible(&fs.root, &guest) { return Err(-ENOENT); }
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
            let file = OpenOptions::new()
                .read(acc == 0 || acc == 2)
                .write(acc == 1 || acc == 2)
                .append(flags & O_APPEND != 0)
                .create(mode_creat)
                .create_new(mode_creat && flags & O_EXCL != 0)
                .truncate(flags & O_TRUNC != 0)
                .open(&host)
                .map_err(|error| -io_errno(&error))?;
            let fd = fs.next_fd; fs.next_fd += 1;
            let sync_path = (acc != 0 && save_management::is_persistent_guest_path(&guest)).then_some(host);
            fs.fds.insert(fd, Entry::File { file, guest, sync_path });
            Ok(fd)
        })().unwrap_or_else(|e| e),
        "__syscall_stat64" | "__syscall_lstat64" | "__syscall_newfstatat" => {
            let (dirfd, path, buf) = if name == "__syscall_newfstatat" { (a(0), a(1), a(2)) } else { (AT_FDCWD, a(0), a(1)) };
            match guest_abs(&fs, dirfd, &read_cstr(mem, path as u64)) {
                Ok(g) if !save_management::bootstrap_read_visible(&fs.root, &g) => -ENOENT,
                Ok(_) if !memory_span(mem, buf as u64, 112) => -EFAULT,
                Ok(g) => match if name == "__syscall_lstat64" { fs::symlink_metadata(host_path(&fs, &g)) } else { fs::metadata(host_path(&fs, &g)) } {
                    Ok(md) => { write_stat(mem, buf as u64, &md, g.starts_with("/dev/")); 0 }
                    Err(error) => -io_errno(&error),
                },
                Err(e) => -e,
            }
        }
        "__syscall_fstat64" => {
            let fd = a(0);
            if !memory_span(mem, a(1) as u64, 112) { -EFAULT } else if fd <= 2 {
                let md = fs::metadata("/dev/null").unwrap();
                write_stat(mem, a(1) as u64, &md, true); 0
            } else {
                match fs.fds.get(&fd) {
                    Some(Entry::File { file, .. }) => match file.metadata() { Ok(md) => { write_stat(mem, a(1) as u64, &md, false); 0 } Err(_) => -EBADF },
                Some(Entry::Dir { guest, .. }) => match fs::metadata(host_path(&fs, guest)) {
                    Ok(md) => { write_stat(mem, a(1) as u64, &md, false); 0 }
                    Err(error) => -io_errno(&error),
                },
                    None => -EBADF,
                }
            }
        }
        "__syscall_mkdirat" => match guest_abs(&fs, a(0), &read_cstr(mem, a(1) as u64)) {
            Ok(g) => {
                let h = host_path(&fs, &g);
                fs::create_dir(&h).and_then(|_| if save_management::is_persistent_guest_path(&g) {
                    save_management::sync_parent(&h)
                } else { Ok(()) }).map(|_| 0).unwrap_or_else(|error| -io_errno(&error))
            }
            Err(e) => -e,
        },
        "__syscall_unlinkat" => match guest_abs(&fs, a(0), &read_cstr(mem, a(1) as u64)) {
            Ok(g) => {
                let h = host_path(&fs, &g);
                let res = if a(2) & 0x200 != 0 { fs::remove_dir(&h) } else { fs::remove_file(&h) };
                res.and_then(|_| if save_management::is_persistent_guest_path(&g) {
                    save_management::sync_parent(&h)
                } else { Ok(()) }).map(|_| 0).unwrap_or_else(|error| -io_errno(&error))
            }
            Err(e) => -e,
        },
        "__syscall_rmdir" => match guest_abs(&fs, AT_FDCWD, &read_cstr(mem, a(0) as u64)) {
            Ok(g) => {
                let h = host_path(&fs, &g);
                fs::remove_dir(&h).and_then(|_| if save_management::is_persistent_guest_path(&g) {
                    save_management::sync_parent(&h)
                } else { Ok(()) }).map(|_| 0).unwrap_or_else(|error| -io_errno(&error))
            }
            Err(e) => -e,
        },
        "__syscall_renameat" => {
            let from = guest_abs(&fs, a(0), &read_cstr(mem, a(1) as u64));
            let to = guest_abs(&fs, a(2), &read_cstr(mem, a(3) as u64));
            match (from, to) {
                (Ok(f), Ok(t)) => (|| {
                    let from = host_path(&fs, &f);
                    let to = host_path(&fs, &t);
                    fs::rename(&from, &to).map_err(|error| -io_errno(&error))?;
                    if save_management::is_persistent_guest_path(&f) || save_management::is_persistent_guest_path(&t) {
                        File::open(&to).and_then(|file| file.sync_all()).map_err(|error| -io_errno(&error))?;
                        save_management::sync_parent(&to).map_err(|error| -io_errno(&error))?;
                        if from.parent() != to.parent() {
                            save_management::sync_parent(&from).map_err(|error| -io_errno(&error))?;
                        }
                    }
                    Ok(0)
                })().unwrap_or_else(|error| error),
                (Err(e), _) | (_, Err(e)) => -e,
            }
        }
        "__syscall_ftruncate64" => match fs.fds.get(&a(0)) {
            Some(Entry::File { file, .. }) => if a(1) < 0 { -EINVAL } else {
                file.set_len(a(1) as u64).map(|_| 0).unwrap_or_else(|error| -io_errno(&error))
            },
            _ => -EBADF,
        },
        "__syscall_getcwd" => {
            let (buf, size) = (a(0) as u64, a(1) as u64);
            if size < 2 { -EINVAL } else if !write_bytes(mem, buf, b"/\0") { -EFAULT } else { 2 }
        }
        "__syscall_getdents64" => (|| {
            let (fd, dirp, count) = (a(0), a(1) as u64, a(2) as u64);
            if !memory_span(mem, dirp, count) { return Err(-EFAULT); }
            let root = fs.root.clone();
            match fs.fds.get_mut(&fd) {
                Some(Entry::Dir { guest, names, pos }) => {
                    if names.is_none() { *names = Some(read_dir_names(&root.join(guest.trim_start_matches('/'))).map_err(|error| -error)?); }
                    // Cache the real directory, not a filtered copy. A later engine
                    // menu enumeration sees all slots after finish_bootstrap_save.
                    let names = names.as_ref().unwrap();
                    let start = (*pos / 280) as usize;
                    let mut off = 0u64;
                    for (idx, (n, t)) in names.iter().enumerate().skip(start) {
                        let child = format!("{}/{n}", guest.trim_end_matches('/'));
                        if !save_management::bootstrap_read_visible(&root, &child) { *pos = ((idx + 1) * 280) as u64; continue; }
                        if count - off < 280 { break; }
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
                        *pos = ((idx + 1) * 280) as u64;
                    }
                    if off == 0 && (*pos / 280) < names.len() as u64 && count < 280 { Err(-EINVAL) } else { Ok(off as i64) }
                }
                Some(_) => Err(-ENOTDIR),
                None => Err(-EBADF),
            }
        })().unwrap_or_else(|error| error),
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

        "fd_read" | "fd_pread" => (|| {
            let fd = a(0);
            let root = fs.root.clone();
            let iov = iovecs(mem, a(1) as u64, a(2) as u64)?;
            let (offset, nread_ptr) = if name == "fd_pread" { (Some(a(3) as u64), a(4) as u64) } else { (None, a(3) as u64) };
            if !memory_span(mem, nread_ptr, 8) { return Err(EFAULT); }
            write_u64(mem, nread_ptr, 0);
            match fs.fds.get_mut(&fd) {
                Some(Entry::File { file, guest, .. }) => {
                    let mut total = 0u64;
                    let mut off = offset;
                    for (ptr, len) in iov {
                        if off.is_some_and(|o| o.checked_add(len).is_none()) { return Err(EINVAL); }
                        let mut done = 0u64;
                        let mut buf = vec![0u8; len.min(64 * 1024) as usize];
                        while done < len {
                            let chunk = ((len - done) as usize).min(buf.len());
                            let result = match off {
                                Some(o) => file.read_at(&mut buf[..chunk], o),
                                None => file.read(&mut buf[..chunk]),
                            };
                            let n = match result {
                                Ok(n) => n,
                                Err(error) => {
                                    write_u64(mem, nread_ptr, total);
                                    return if total > 0 { Ok(0) } else { Err(io_errno(&error)) };
                                }
                            };
                            if !write_bytes(mem, ptr + done, &buf[..n]) { return Err(EFAULT); }
                            total += n as u64;
                            save_management::observe_bootstrap_read(&root, guest, n as u64);
                            done += n as u64;
                            if let Some(o) = off.as_mut() { *o += n as u64; }
                            write_u64(mem, nread_ptr, total);
                            if n < chunk { return Ok(0); }
                        }
                    }
                    write_u64(mem, nread_ptr, total);
                    Ok(0)
                }
                Some(Entry::Dir { .. }) => Err(EISDIR),
                None if fd == 0 => Ok(0),
                None => Err(EBADF),
            }
        })().unwrap_or_else(|error| error),
        "fd_write" | "fd_pwrite" if a(0) > 2 => (|| {
            let fd = a(0);
            let iov = iovecs(mem, a(1) as u64, a(2) as u64)?;
            let (offset, nw_ptr) = if name == "fd_pwrite" { (Some(a(3) as u64), a(4) as u64) } else { (None, a(3) as u64) };
            if !memory_span(mem, nw_ptr, 8) { return Err(EFAULT); }
            write_u64(mem, nw_ptr, 0);
            match fs.fds.get_mut(&fd) {
                Some(Entry::File { file, .. }) => {
                    let mut total = 0u64;
                    let mut off = offset;
                    for (ptr, len) in iov {
                        if off.is_some_and(|o| o.checked_add(len).is_none()) { return Err(EINVAL); }
                        let mut done = 0u64;
                        while done < len {
                            let data = read_bytes(mem, ptr + done, (len - done).min(64 * 1024));
                            if data.is_empty() { return Err(EFAULT); }
                            let result = match off { Some(o) => file.write_at(&data, o), None => file.write(&data) };
                            let n = match result {
                                Ok(0) => {
                                    write_u64(mem, nw_ptr, total);
                                    return if total > 0 { Ok(0) } else { Err(EIO) };
                                }
                                Ok(n) => n,
                                Err(error) => {
                                    write_u64(mem, nw_ptr, total);
                                    return if total > 0 { Ok(0) } else { Err(io_errno(&error)) };
                                }
                            };
                            total += n as u64;
                            done += n as u64;
                            if let Some(o) = off.as_mut() { *o += n as u64; }
                            write_u64(mem, nw_ptr, total);
                        }
                    }
                    write_u64(mem, nw_ptr, total);
                    Ok(0)
                }
                Some(Entry::Dir { .. }) => Err(EISDIR),
                _ => Err(EBADF),
            }
        })().unwrap_or_else(|error| error),
        "fd_sync" | "fd_datasync" | "__syscall_fsync" | "__syscall_fdatasync" => {
            let result = match fs.fds.get(&a(0)) {
                Some(Entry::File { file, .. }) => {
                    let result = if name.ends_with("datasync") { file.sync_data() } else { file.sync_all() };
                    result.map(|_| 0).unwrap_or_else(|error| io_errno(&error))
                }
                _ => EBADF,
            };
            if name.starts_with("__syscall_") { -result } else { result }
        }
        "fd_seek" => {
            let (fd, offset, whence, out) = (a(0), a(1), a(2), a(3) as u64);
            if !memory_span(mem, out, 8) { EFAULT } else { match fs.fds.get_mut(&fd) {
                Some(Entry::File { file, .. }) => {
                    if whence == 0 && offset < 0 { return { ok_i(results, EINVAL); true }; }
                    let sf = match whence { 0 => SeekFrom::Start(offset as u64), 1 => SeekFrom::Current(offset), 2 => SeekFrom::End(offset), _ => return { ok_i(results, EINVAL); true } };
                    match file.seek(sf) { Ok(p) => { write_u64(mem, out, p); 0 } Err(_) => EINVAL }
                }
                Some(Entry::Dir { pos, .. }) => { if whence != 0 || offset < 0 { EINVAL } else { *pos = offset as u64; write_u64(mem, out, *pos); 0 } }
                None => EBADF,
            } }
        }
        "fd_close" => match fs.fds.remove(&a(0)) {
            Some(Entry::File { file, sync_path: Some(path), .. }) => file.sync_all()
                .and_then(|_| save_management::sync_parent(&path)).map(|_| 0).unwrap_or_else(|error| io_errno(&error)),
            Some(_) => 0,
            None if a(0) <= 2 => 0,
            None => EBADF,
        },
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
        let _serial = save_management::TEST_SERIAL.lock().unwrap();
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
        let directory = "userdata/Documents/Rockstar Games/GTA V/Default";
        let selected = format!("{directory}/SGTA50001");
        let other = format!("{directory}/SGTA50002");
        // Exercise the original engine's Windows-separator spelling on write.
        write_bytes(&mem, 32, format!("/{}\0", format!("{directory}/SGTA50000").replace('/', "\\")).as_bytes());
        write_bytes(&mem, 128, format!("/{selected}\0").as_bytes());
        let fd = call("__syscall_openat", &[AT_FDCWD, 32, 2 | O_CREAT | O_TRUNC, 0]);
        assert!(fd >= 3);
        let payload = b"save round trip";
        write_bytes(&mem, 512, payload);
        write_u64(&mem, 256, 512);
        write_u64(&mem, 264, payload.len() as u64);
        assert_eq!(call("fd_write", &[fd, 256, 1, 320]), 0);
        assert_eq!(read_u64(&mem, 320), Some(payload.len() as u64));
        assert_eq!(call("__syscall_ftruncate64", &[fd, -1]), -EINVAL);
        assert_eq!(call("fd_sync", &[fd]), 0);
        assert_eq!(call("__syscall_fsync", &[fd]), 0);
        assert_eq!(call("fd_datasync", &[fd]), 0);
        assert_eq!(call("fd_close", &[fd]), 0);
        assert_eq!(call("__syscall_renameat", &[AT_FDCWD, 32, AT_FDCWD, 128]), 0);
        // Read from disk independently, then reopen through the guest interface.
        assert_eq!(fs::read(root.join(&selected)).unwrap(), payload);
        let windows_selected = format!("/{}\0", selected.replace('/', "\\"));
        write_bytes(&mem, 128, windows_selected.as_bytes());
        let fd = call("__syscall_openat", &[AT_FDCWD, 128, 0, 0]);
        assert!(fd >= 3);
        assert_eq!(call("fd_read", &[fd, 256, 1, 320]), 0);
        assert_eq!(call("fd_close", &[fd]), 0);
        assert_eq!(guest_abs(&FS.lock().unwrap(), AT_FDCWD, "\\..\\..\\userdata\\Documents").unwrap(), "/userdata/Documents");
        assert_eq!(call("__syscall_openat", &[AT_FDCWD, 128, 2 | O_CREAT | O_EXCL, 0]), -EEXIST);
        let fd = call("__syscall_openat", &[AT_FDCWD, 128, 0, 0]);
        assert!(fd >= 3);
        write_bytes(&mem, 512, &[0; 15]);
        assert_eq!(call("fd_read", &[fd, 256, 1, 320]), 0);
        assert_eq!(crate::read_bytes(&mem, 512, payload.len() as u64), payload);
        assert_eq!(call("fd_write", &[fd, 256, 1, 320]), EBADF);
        write_u64(&mem, 264, 65536);
        assert_eq!(call("fd_read", &[fd, 256, 1, 320]), EFAULT);
        write_u64(&mem, 264, payload.len() as u64);
        assert_eq!(call("fd_close", &[fd]), 0);
        assert_eq!(call("fd_close", &[fd]), EBADF);

        // Selection filters guest lookup/enumeration, never disk bytes or writes.
        fs::write(root.join(&other), b"other existing slot").unwrap();
        let lowercase = format!("{directory}/sgta50003");
        fs::write(root.join(&lowercase), b"legacy lowercase slot").unwrap();
        let sandbox = "sandbox/Documents/Rockstar Games/GTA V/Default/SGTA50001";
        fs::write(root.join(sandbox), b"separate sandbox slot").unwrap();
        select_bootstrap_save(&root, &selected).unwrap();
        write_bytes(&mem, 128, format!("/{}\0", other.replace('/', "\\")).as_bytes());
        assert_eq!(call("__syscall_stat64", &[128, 1024]), -ENOENT);
        assert_eq!(call("__syscall_openat", &[AT_FDCWD, 128, 0, 0]), -ENOENT);
        let writable = call("__syscall_openat", &[AT_FDCWD, 128, 1, 0]);
        assert!(writable >= 3);
        assert_eq!(call("fd_close", &[writable]), 0);
        write_bytes(&mem, 128, format!("/{lowercase}\0").as_bytes());
        assert_eq!(call("__syscall_stat64", &[128, 1024]), -ENOENT);
        assert_eq!(call("__syscall_openat", &[AT_FDCWD, 128, 0, 0]), -ENOENT);
        write_bytes(&mem, 128, format!("/{selected}\0").as_bytes());
        assert_eq!(call("__syscall_stat64", &[128, 1024]), 0);
        let fd = call("__syscall_openat", &[AT_FDCWD, 128, 0, 0]);
        assert!(fd >= 3);
        assert_eq!(call("fd_read", &[fd, 256, 1, 320]), 0);
        assert_eq!(call("fd_close", &[fd]), 0);
        assert!(bootstrap_save_pending()); // A successful read is not a loaded game.
        write_bytes(&mem, 32, format!("/{directory}\0").as_bytes());
        let dirfd = call("__syscall_openat", &[AT_FDCWD, 32, O_DIRECTORY, 0]);
        assert!(dirfd >= 3);
        let count = call("__syscall_getdents64", &[dirfd, 4096, 4096]);
        assert!(count > 0);
        let names = |count: i64| -> Vec<String> {
            (0..count as u64).step_by(280).map(|offset| read_cstr(&mem, 4096 + offset + 19)).collect()
        };
        let filtered = names(count);
        assert!(filtered.iter().any(|name| name == "SGTA50001"));
        assert!(!filtered.iter().any(|name| name == "SGTA50002"));
        assert!(!filtered.iter().any(|name| name == "sgta50003"));
        finish_bootstrap_save();
        assert!(!bootstrap_save_pending());
        assert_eq!(call("fd_seek", &[dirfd, 0, 0, 320]), 0);
        let count = call("__syscall_getdents64", &[dirfd, 4096, 4096]);
        let unfiltered = names(count);
        assert!(unfiltered.iter().any(|name| name == "SGTA50002"));
        assert!(unfiltered.iter().any(|name| name == "sgta50003"));
        assert_eq!(call("fd_close", &[dirfd]), 0);
        write_bytes(&mem, 128, format!("/{other}\0").as_bytes());
        assert_eq!(call("__syscall_stat64", &[128, 1024]), 0);
        assert_eq!(fs::read(root.join(&other)).unwrap(), b"other existing slot");
        assert_eq!(fs::read(root.join(sandbox)).unwrap(), b"separate sandbox slot");
        assert_eq!(fs::read(root.join(&selected)).unwrap(), payload);
        assert_eq!(fs::read(root.join(lowercase)).unwrap(), b"legacy lowercase slot");
        write_bytes(&mem, 96, b"/tmp\0");
        write_bytes(&mem, 1024, &[0xaa; 128]);
        assert_eq!(call("__syscall_statfs64", &[96, 120, 1024]), 0);
        assert!(read_u64(&mem, 1056).unwrap() > 0);
        assert_eq!(crate::read_bytes(&mem, 1144, 8), [0xaa; 8]);
        assert_eq!(call("__syscall_statfs64", &[96, 88, 1024]), -EINVAL);
        fs::remove_dir_all(root).unwrap();
    }
}
