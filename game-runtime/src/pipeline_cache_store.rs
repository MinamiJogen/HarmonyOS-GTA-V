//! Opt-in storage for UNCHANGED wgpu PipelineCache::get_data bytes. This module
//! makes no GPU calls and never makes arbitrary imported cache data trustworthy.
//! GPU ownership, feature gating and serialized create/get_data stay with gpu.rs.
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

const MAGIC: &[u8; 8] = b"MGPCACHE";
const VERSION: u32 = 1;
const HEADER: usize = 56;
pub(crate) const MAX_PAYLOAD: usize = 64 << 20;
const MAX_METADATA: usize = 16 << 10;
const DIRECTORY: &str = "native-pipeline-cache-v1";
const CACHE_FILE: &str = "cache.bin";
const TEMP_FILE: &str = "pending.bin";
const FIRST_CHECKPOINT: Duration = Duration::from_secs(15);
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(60);
static WRITER_ACTIVE: AtomicBool = AtomicBool::new(false);

fn read_only_requested() -> bool {
    static READ_ONLY: OnceLock<bool> = OnceLock::new();
    *READ_ONLY.get_or_init(|| std::env::var_os("MUGUET_PIPELINE_CACHE_READ_ONLY").is_some_and(|value| value == "1"))
}
fn monotonic_ns() -> Option<u64> {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0 { return None; }
    u64::try_from(ts.tv_sec).ok()?.checked_mul(1_000_000_000)?.checked_add(u64::try_from(ts.tv_nsec).ok()?)
}
#[derive(Default)]
struct CaptureStats {
    calls: u64, total_ns: u64, max_ns: u64, last_ns: u64, last_bytes: usize,
    begin_monotonic_ns: Option<u64>, end_monotonic_ns: Option<u64>,
}

#[derive(Clone, Debug)]
pub(crate) struct Identity {
    pub vendor: u32, pub device: u32, pub adapter_name: String,
    pub driver: String, pub driver_info: String, pub driver_version: u32,
    // None, or an all-zero/unobserved UUID, disables BOTH disk read and write.
    pub pipeline_cache_uuid: Option<[u8; 16]>,
    pub runtime_source_sha256: String, pub wgpu_build_sha256: String,
    pub resource_build: String, pub shader_index_sha256: String,
    pub target_abi: String,
}
impl Identity {
    fn metadata(&self) -> Result<Vec<u8>, Error> {
        if self.pipeline_cache_uuid.is_none_or(|uuid| uuid == [0; 16]) {
            return Err(Error::Invalid("missing_actual_pipeline_cache_uuid"));
        }
        if self.target_abi != "aarch64-unknown-linux-ohos" {
            return Err(Error::Invalid("unsupported_target_abi"));
        }
        for digest in [&self.runtime_source_sha256, &self.wgpu_build_sha256, &self.shader_index_sha256] {
            if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
                return Err(Error::Invalid("missing_build_or_resource_digest"));
            }
        }
        for text in [&self.adapter_name, &self.driver, &self.driver_info, &self.resource_build] {
            if text.is_empty() || text.len() > 512 { return Err(Error::Invalid("missing_or_oversize_identity")); }
        }
        let metadata = serde_json::to_vec(&serde_json::json!({
            "schema": VERSION, "backend": "Vulkan", "wgpu": "30.0.1",
            "vendor": self.vendor, "device": self.device, "adapterName": self.adapter_name,
            "driver": self.driver, "driverInfo": self.driver_info, "driverVersion": self.driver_version,
            "pipelineCacheUUID": self.pipeline_cache_uuid,
            "runtimeSourceSHA256": self.runtime_source_sha256, "wgpuBuildSHA256": self.wgpu_build_sha256,
            "resourceBuild": self.resource_build, "shaderIndexSHA256": self.shader_index_sha256,
            "targetABI": self.target_abi, "origin": "unchanged_wgpu_get_data_in_app_private_storage"
        })).map_err(|_| Error::Invalid("identity_serialization"))?;
        if metadata.len() > MAX_METADATA { return Err(Error::Invalid("metadata_limit")); }
        Ok(metadata)
    }
}

#[derive(Debug)]
pub(crate) enum Error { Invalid(&'static str), Io(std::io::Error) }
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self { Self::Invalid(reason) => f.write_str(reason), Self::Io(error) => write!(f, "cache_io: {error}") }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error { fn from(error: std::io::Error) -> Self { Self::Io(error) } }

/// Only returned after complete envelope, checksum, identity AND both native
/// cache headers match. Bytes remain immutable and include the wgpu header.
pub(crate) struct ValidatedData { envelope: Vec<u8>, start: usize }
impl ValidatedData { pub(crate) fn bytes(&self) -> &[u8] { &self.envelope[self.start..] } }

fn sha256(metadata: &[u8], payload: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new(); digest.update(metadata); digest.update(payload); digest.finalize().into()
}
fn be32(bytes: &[u8]) -> u32 { u32::from_be_bytes(bytes.try_into().expect("checked field size")) }
fn be64(bytes: &[u8]) -> u64 { u64::from_be_bytes(bytes.try_into().expect("checked field size")) }
fn le32(bytes: &[u8]) -> u32 { u32::from_le_bytes(bytes.try_into().expect("checked field size")) }

// Conservative validator tied to the PINNED wgpu30 core/HAL format. It reads
// fields without rewriting them. Unknown future versions fail closed.
fn validate_wgpu_data(identity: &Identity, data: &[u8]) -> Result<(), Error> {
    if !(96..=MAX_PAYLOAD).contains(&data.len()) { return Err(Error::Invalid("payload_size")); }
    if &data[..8] != b"WGPUPLCH" || be32(&data[8..12]) != 1 || be32(&data[12..16]) != 8 || data[16] != 1 {
        return Err(Error::Invalid("wgpu_header_version_abi_or_backend"));
    }
    let vendor = identity.vendor.to_be_bytes(); let device = identity.device.to_be_bytes();
    let expected_adapter = [255, 255, 255, vendor[0], vendor[1], vendor[2], vendor[3], device[0], device[1], device[2], device[3], 255, 255, 255, 255];
    if data[17..32] != expected_adapter { return Err(Error::Invalid("wgpu_adapter_mismatch")); }
    if data[32..36] != identity.driver_version.to_be_bytes() || data[36..48] != [0; 12] {
        return Err(Error::Invalid("wgpu_driver_or_validation_key_mismatch"));
    }
    if be64(&data[48..56]) != (data.len() - 64) as u64 || be64(&data[56..64]) != 0xFEDCBA9_876543210 {
        return Err(Error::Invalid("wgpu_payload_length_or_sentinel"));
    }
    let native = &data[64..];
    if le32(&native[0..4]) != 32 || le32(&native[4..8]) != 1 ||
        le32(&native[8..12]) != identity.vendor || le32(&native[12..16]) != identity.device ||
        Some(native[16..32].try_into().expect("checked UUID length")) != identity.pipeline_cache_uuid {
        return Err(Error::Invalid("vulkan_cache_header_or_uuid_mismatch"));
    }
    Ok(())
}

fn envelope_header(identity: &Identity, metadata: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    validate_wgpu_data(identity, data)?;
    let mut bytes = Vec::with_capacity(HEADER);
    bytes.extend_from_slice(MAGIC); bytes.extend_from_slice(&VERSION.to_be_bytes());
    bytes.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&(data.len() as u64).to_be_bytes());
    bytes.extend_from_slice(&sha256(metadata, data));
    Ok(bytes)
}
#[cfg(test)]
fn encode(identity: &Identity, metadata: &[u8], data: &[u8]) -> Result<Vec<u8>, Error> {
    let mut bytes = envelope_header(identity, metadata, data)?;
    bytes.extend_from_slice(metadata); bytes.extend_from_slice(data); Ok(bytes)
}
fn validate(identity: &Identity, expected_metadata: &[u8], bytes: Vec<u8>) -> Result<ValidatedData, Error> {
    if bytes.len() < HEADER || &bytes[..8] != MAGIC || be32(&bytes[8..12]) != VERSION {
        return Err(Error::Invalid("envelope_magic_or_version"));
    }
    let metadata_len = be32(&bytes[12..16]) as usize;
    let payload_len = usize::try_from(be64(&bytes[16..24])).map_err(|_| Error::Invalid("payload_length_overflow"))?;
    if metadata_len > MAX_METADATA || !(96..=MAX_PAYLOAD).contains(&payload_len) {
        return Err(Error::Invalid("envelope_size_limit"));
    }
    let start = HEADER.checked_add(metadata_len).ok_or(Error::Invalid("envelope_length_overflow"))?;
    if start.checked_add(payload_len) != Some(bytes.len()) { return Err(Error::Invalid("envelope_exact_length")); }
    if &bytes[HEADER..start] != expected_metadata { return Err(Error::Invalid("identity_mismatch")); }
    if bytes[24..56] != sha256(&bytes[HEADER..start], &bytes[start..]) { return Err(Error::Invalid("sha256_mismatch")); }
    validate_wgpu_data(identity, &bytes[start..])?;
    Ok(ValidatedData { envelope: bytes, start })
}

struct Directory(File);
fn private_metadata(metadata: &fs::Metadata, directory: bool) -> bool {
    (if directory { metadata.is_dir() } else { metadata.is_file() }) &&
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o077 == 0
}
impl Directory {
    fn open(root: &Path, create: bool) -> Result<Option<Self>, Error> {
        if create {
            match fs::DirBuilder::new().mode(0o700).create(root) {
                Ok(()) => {}, Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}, Err(error) => return Err(error.into()),
            }
        }
        let opened = OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(root);
        let file = match opened {
            Ok(file) => file, Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None), Err(error) => return Err(error.into()),
        };
        if !private_metadata(&file.metadata()?, true) { return Err(Error::Invalid("non_private_cache_directory")); }
        Ok(Some(Self(file)))
    }
    fn file(&self, name: &str, flags: i32) -> Result<File, Error> {
        let name = CString::new(name).map_err(|_| Error::Invalid("invalid_internal_filename"))?;
        let fd = unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(), flags | libc::O_NOFOLLOW | libc::O_CLOEXEC, 0o600) };
        if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
        let file = unsafe { File::from_raw_fd(fd) };
        if !private_metadata(&file.metadata()?, false) { return Err(Error::Invalid("non_private_cache_file")); }
        Ok(file)
    }
    fn unlink(&self, name: &str) {
        if let Ok(name) = CString::new(name) { unsafe { libc::unlinkat(self.0.as_raw_fd(), name.as_ptr(), 0); } }
    }
    fn publish_parts(&self, parts: &[&[u8]]) -> Result<(), Error> {
        // Exactly one writer is admitted per process. A private stale temporary
        // from an interrupted prior process is disposable; last-good cache isn't.
        match self.file(TEMP_FILE, libc::O_RDONLY | libc::O_NONBLOCK) {
            Ok(_) => self.unlink(TEMP_FILE),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(error),
        }
        let write = (|| -> Result<(), Error> {
            let mut file = self.file(TEMP_FILE, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NONBLOCK)?;
            for bytes in parts { file.write_all(bytes)?; }
            file.sync_all()?; drop(file);
            let old = CString::new(TEMP_FILE).expect("constant name"); let new = CString::new(CACHE_FILE).expect("constant name");
            if unsafe { libc::renameat(self.0.as_raw_fd(), old.as_ptr(), self.0.as_raw_fd(), new.as_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            self.0.sync_all()?; Ok(())
        })();
        if write.is_err() { self.unlink(TEMP_FILE); }
        write
    }
    #[cfg(test)]
    fn publish(&self, bytes: &[u8]) -> Result<(), Error> { self.publish_parts(&[bytes]) }
}

#[derive(Default)]
struct WriterStats { published: AtomicU64, published_generation: AtomicU64, errors: AtomicU64, full: AtomicU64 }
struct Job { data: Vec<u8>, generation: u64 }
struct Writer { sender: mpsc::SyncSender<Job>, stats: Arc<WriterStats> }
struct WriterLease;
impl Drop for WriterLease { fn drop(&mut self) { WRITER_ACTIVE.store(false, Ordering::Release); } }
impl Writer {
    fn new(root: PathBuf, identity: Identity, metadata: Arc<Vec<u8>>) -> Result<Self, Error> {
        if WRITER_ACTIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
            return Err(Error::Invalid("another_pipeline_cache_writer_active"));
        }
        let (sender, receiver) = mpsc::sync_channel::<Job>(1);
        let stats = Arc::new(WriterStats::default()); let worker_stats = stats.clone();
        let spawned = std::thread::Builder::new().name("pipeline-cache-io".into()).spawn(move || {
            let _lease = WriterLease;
            while let Ok(job) = receiver.recv() {
                // Hash/write the owned payload in place. No second payload-sized
                // envelope allocation or GPU object crosses to this IO worker.
                let result = envelope_header(&identity, &metadata, &job.data).and_then(|header|
                    Directory::open(&root, true)?.ok_or(Error::Invalid("missing_cache_directory"))?
                        .publish_parts(&[&header, &metadata, &job.data]));
                match result {
                    Ok(()) => {
                        worker_stats.published_generation.store(job.generation, Ordering::Release);
                        worker_stats.published.fetch_add(1, Ordering::Relaxed);
                    },
                    Err(error) => { worker_stats.errors.fetch_add(1, Ordering::Release); eprintln!("[pipeline-cache] checkpoint failed: {error}"); },
                }
            }
        });
        if let Err(error) = spawned { WRITER_ACTIVE.store(false, Ordering::Release); return Err(error.into()); }
        Ok(Self { sender, stats })
    }
}

struct Schedule { began: Instant, last_attempt: Option<Instant>, captured_generation: u64 }
impl Schedule {
    fn due(&self, now: Instant, generation: u64) -> bool {
        generation != self.captured_generation && now.saturating_duration_since(self.began) >= FIRST_CHECKPOINT &&
            self.last_attempt.is_none_or(|last| now.saturating_duration_since(last) >= CHECKPOINT_INTERVAL)
    }
}

pub(crate) struct Store {
    requested: bool, identity: Identity, metadata: Option<Arc<Vec<u8>>>, reason: Option<String>,
    root: PathBuf, schedule: Schedule, generation: u64, writer: Option<Writer>,
    observed_writer_errors: u64, read_only: bool, captures: CaptureStats,
}
impl Store {
    /// OFF performs no directory read/create, cache read, allocation of GPU data,
    /// or worker spawn. `files_dir` must be the current app's private filesDir.
    pub(crate) fn new(requested: bool, files_dir: &Path, identity: Identity) -> Self {
        Self::new_with_policy(requested, read_only_requested(), files_dir, identity)
    }
    fn new_with_policy(requested: bool, read_only: bool, files_dir: &Path, identity: Identity) -> Self {
        let (metadata, reason) = if !requested { (None, Some("disabled".into())) } else {
            match identity.metadata() { Ok(metadata) => (Some(Arc::new(metadata)), None), Err(error) => (None, Some(error.to_string())) }
        };
        Self { requested, identity, metadata, reason, root: files_dir.join(DIRECTORY),
            schedule: Schedule { began: Instant::now(), last_attempt: None, captured_generation: 0 },
            generation: 0, writer: None, observed_writer_errors: 0, read_only, captures: CaptureStats::default() }
    }
    pub(crate) fn enabled(&self) -> bool { self.metadata.is_some() }
    /// Cold/missing returns None; rejected cache returns Err and NEVER yields data.
    /// Caller must catch Err and use a scoped fresh cache or cache=None.
    pub(crate) fn load(&self) -> Result<Option<ValidatedData>, Error> {
        let Some(metadata) = &self.metadata else { return Ok(None); };
        let Some(directory) = Directory::open(&self.root, false)? else { return Ok(None); };
        let mut file = match directory.file(CACHE_FILE, libc::O_RDONLY | libc::O_NONBLOCK) {
            Ok(file) => file, Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None), Err(error) => return Err(error),
        };
        let before = file.metadata()?;
        if before.len() < HEADER as u64 || before.len() > (HEADER + MAX_METADATA + MAX_PAYLOAD) as u64 {
            return Err(Error::Invalid("cache_file_size_limit"));
        }
        let mut bytes = Vec::with_capacity(before.len() as usize);
        (&mut file).take((HEADER + MAX_METADATA + MAX_PAYLOAD + 1) as u64).read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        if before.len() != after.len() || before.mtime() != after.mtime() || before.mtime_nsec() != after.mtime_nsec() {
            return Err(Error::Invalid("cache_changed_during_read"));
        }
        validate(&self.identity, metadata, bytes).map(Some)
    }
    pub(crate) fn note_pipeline_created(&mut self) { self.generation = self.generation.saturating_add(1); }
    /// Invoke from the existing serialized GPU thread at a low-frequency safe
    /// checkpoint. `capture` MUST be exactly cache.get_data(); never a file/import.
    /// The closure is not invoked when OFF, unchanged, throttled or ineligible.
    /// Only owned bytes cross to the capacity-one IO worker; no GPU handle does.
    pub(crate) fn checkpoint(&mut self, capture: impl FnOnce() -> Option<Vec<u8>>) -> Result<bool, Error> {
        self.checkpoint_at(Instant::now(), capture)
    }
    fn checkpoint_at(&mut self, now: Instant, capture: impl FnOnce() -> Option<Vec<u8>>) -> Result<bool, Error> {
        // Read-only is a frozen startup policy, including cold/rejected loads.
        // Do not export, mutate the schedule, create a worker/directory, enqueue,
        // remove stale temps, or replace the last-good/corrupt file in this mode.
        if self.read_only { return Ok(false); }
        let Some(metadata) = &self.metadata else { return Ok(false); };
        if let Some(writer) = &self.writer {
            let errors = writer.stats.errors.load(Ordering::Acquire);
            if errors != self.observed_writer_errors {
                // A failed queued generation remains dirty. Retry at the normal
                // 60-second bound; never require another pipeline to be created.
                self.schedule.captured_generation = writer.stats.published_generation.load(Ordering::Acquire);
                self.observed_writer_errors = errors;
            }
        }
        if !self.schedule.due(now, self.generation) { return Ok(false); }
        self.schedule.last_attempt = Some(now);
        self.captures.begin_monotonic_ns = monotonic_ns();
        let began = Instant::now();
        let data = capture(); // Synchronous cache.get_data() on the GPU caller.
        let elapsed_ns = began.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        self.captures.end_monotonic_ns = monotonic_ns();
        self.captures.calls = self.captures.calls.saturating_add(1);
        self.captures.total_ns = self.captures.total_ns.saturating_add(elapsed_ns);
        self.captures.max_ns = self.captures.max_ns.max(elapsed_ns);
        self.captures.last_ns = elapsed_ns;
        self.captures.last_bytes = data.as_ref().map_or(0, Vec::len);
        eprintln!("[pipeline-cache] get-data: {}", serde_json::json!({
            "pid":std::process::id(),"generation":self.generation,"call":self.captures.calls,
            "beginMonotonicNs":self.captures.begin_monotonic_ns,"endMonotonicNs":self.captures.end_monotonic_ns,
            "wallNs":elapsed_ns,"bytes":self.captures.last_bytes,
            "scope":"serialized GPU caller capture closure host wall; not GPU execution or cache-hit feedback"
        }));
        let Some(data) = data else { return Ok(false); };
        validate_wgpu_data(&self.identity, &data)?;
        if self.writer.is_none() { self.writer = Some(Writer::new(self.root.clone(), self.identity.clone(), metadata.clone())?); }
        let writer = self.writer.as_ref().expect("writer created");
        match writer.sender.try_send(Job { data, generation: self.generation }) {
            Ok(()) => { self.schedule.captured_generation = self.generation; Ok(true) },
            Err(mpsc::TrySendError::Full(_)) => { writer.stats.full.fetch_add(1, Ordering::Relaxed); Ok(false) },
            Err(mpsc::TrySendError::Disconnected(_)) => Err(Error::Invalid("cache_writer_disconnected")),
        }
    }
    pub(crate) fn status(&self) -> serde_json::Value {
        serde_json::json!({"requested":self.requested,"persistenceEnabled":self.enabled(),"disabledReason":self.reason,
            "readOnly":self.read_only,"loadEnabled":self.enabled(),"writeEnabled":self.enabled()&&!self.read_only,
            "getDataCalls":self.captures.calls,"getDataWallTotalNs":self.captures.total_ns,
            "getDataWallMaxNs":self.captures.max_ns,"getDataWallLastNs":self.captures.last_ns,"getDataLastBytes":self.captures.last_bytes,
            "getDataLastBeginMonotonicNs":self.captures.begin_monotonic_ns,"getDataLastEndMonotonicNs":self.captures.end_monotonic_ns,
            "pipelineGeneration":self.generation,"capturedGeneration":self.schedule.captured_generation,
            "published":self.writer.as_ref().map(|w|w.stats.published.load(Ordering::Relaxed)).unwrap_or(0),
            "writeErrors":self.writer.as_ref().map(|w|w.stats.errors.load(Ordering::Relaxed)).unwrap_or(0),
            "queueFull":self.writer.as_ref().map(|w|w.stats.full.load(Ordering::Relaxed)).unwrap_or(0),
            "checkpointMinimumSeconds":CHECKPOINT_INTERVAL.as_secs(),"firstCheckpointDelaySeconds":FIRST_CHECKPOINT.as_secs(),
            "checkpointIntervalSeconds":CHECKPOINT_INTERVAL.as_secs(),"payloadCapBytes":MAX_PAYLOAD,"writerQueueCapacity":1,
            "scope":"app-private exact wgpu bytes; readOnly suppresses exports/writer without weakening load validation; getData wall is host time, not GPU execution/cache-hit feedback; checkpointMinimumSeconds is the repeat interval, initial delay is separate"})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn identity() -> Identity {
        Identity { vendor: 0x19e5, device: 900, adapter_name:"fixture-adapter".into(), driver:"fixture-driver".into(),
            driver_info:"fixture-driver-info".into(), driver_version:77, pipeline_cache_uuid:Some([0x71;16]),
            runtime_source_sha256:"a".repeat(64), wgpu_build_sha256:"b".repeat(64), resource_build:"fixture-build".into(),
            shader_index_sha256:"c".repeat(64), target_abi:"aarch64-unknown-linux-ohos".into() }
    }
    // Structural fixtures exercise storage only; they are NEVER sent to a GPU.
    fn payload(id: &Identity) -> Vec<u8> {
        let mut bytes = b"WGPUPLCH".to_vec(); bytes.extend(1u32.to_be_bytes()); bytes.extend(8u32.to_be_bytes()); bytes.push(1);
        bytes.extend([255;3]); bytes.extend(id.vendor.to_be_bytes()); bytes.extend(id.device.to_be_bytes()); bytes.extend([255;4]);
        bytes.extend(id.driver_version.to_be_bytes()); bytes.extend([0;12]); bytes.extend(40u64.to_be_bytes()); bytes.extend(0xFEDCBA9_876543210u64.to_be_bytes());
        bytes.extend(32u32.to_le_bytes()); bytes.extend(1u32.to_le_bytes()); bytes.extend(id.vendor.to_le_bytes()); bytes.extend(id.device.to_le_bytes());
        bytes.extend(id.pipeline_cache_uuid.unwrap()); bytes.extend([0x35;8]); bytes
    }
    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("pipeline-cache-store-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap(); path
    }
    #[test]
    fn off_and_missing_uuid_do_not_read_create_or_capture() {
        let root = root(); let mut id = identity(); id.pipeline_cache_uuid = None;
        for requested in [false,true] {
            let mut store = Store::new_with_policy(requested, false, &root, id.clone()); store.note_pipeline_created();
            assert!(!store.enabled()); assert!(store.load().unwrap().is_none());
            assert!(!store.checkpoint_at(Instant::now()+Duration::from_secs(100), || panic!("must not capture")).unwrap());
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(),0); fs::remove_dir(root).unwrap();
    }
    #[test]
    fn cold_and_warm_cache_preserve_exact_payload_bytes() {
        let root = root(); let id = identity(); let data = payload(&id);
        let store = Store::new_with_policy(true, false, &root, id.clone()); assert!(store.load().unwrap().is_none());
        let directory = Directory::open(&store.root,true).unwrap().unwrap(); directory.publish(&encode(&id,&id.metadata().unwrap(),&data).unwrap()).unwrap();
        assert_eq!(store.load().unwrap().unwrap().bytes(),data); fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn all_envelope_header_corruption_truncation_and_trailing_data_fail_closed() {
        let id=identity(); let metadata=id.metadata().unwrap(); let good=encode(&id,&metadata,&payload(&id)).unwrap();
        for at in [0,8,12,16,24,HEADER,good.len()-1] {
            let mut bad=good.clone(); bad[at]^=1; assert!(validate(&id,&metadata,bad).is_err(),"offset{at}");
        }
        for length in [0,7,HEADER-1,HEADER,good.len()-1] { assert!(validate(&id,&metadata,good[..length].to_vec()).is_err()); }
        let mut extra=good; extra.push(0); assert!(validate(&id,&metadata,extra).is_err());
    }
    #[test]
    fn every_identity_component_invalidates_existing_data() {
        let id=identity(); let data=encode(&id,&id.metadata().unwrap(),&payload(&id)).unwrap();
        let mut variants=Vec::new();
        for field in 0..12 {
            let mut changed=id.clone(); match field {
                0=>changed.vendor+=1,1=>changed.device+=1,2=>changed.adapter_name.push('x'),3=>changed.driver.push('x'),
                4=>changed.driver_info.push('x'),5=>changed.driver_version+=1,6=>changed.pipeline_cache_uuid=Some([9;16]),
                7=>changed.runtime_source_sha256="d".repeat(64),8=>changed.wgpu_build_sha256="e".repeat(64),
                9=>changed.resource_build.push('x'),10=>changed.shader_index_sha256="f".repeat(64),11=>changed.target_abi="other-abi".into(),_=>unreachable!(),
            } variants.push(changed);
        }
        for variant in variants {
            match variant.metadata() { Ok(metadata)=>assert!(validate(&variant,&metadata,data.clone()).is_err()),Err(_)=>{} }
        }
    }
    #[test]
    fn valid_envelope_checksum_cannot_bypass_native_header_identity_checks() {
        let id=identity(); let metadata=id.metadata().unwrap(); let good=payload(&id);
        for at in [8,12,16,17,32,36,48,56,64,68,72,76,80] {
            let mut bad=good.clone(); bad[at]^=1; assert!(validate_wgpu_data(&id,&bad).is_err(),"offset{at}");
        }
        assert!(validate_wgpu_data(&id,b"raw-vulkan-or-imported-blob").is_err());
        let mut envelope=encode(&id,&metadata,&good).unwrap(); let start=HEADER+metadata.len(); envelope[start+80]^=1;
        let digest=sha256(&envelope[HEADER..start],&envelope[start..]); envelope[24..56].copy_from_slice(&digest);
        assert!(validate(&id,&metadata,envelope).is_err());
    }
    #[test]
    fn declared_oversize_lengths_and_actual_sparse_oversize_file_are_rejected() {
        let id=identity(); let metadata=id.metadata().unwrap(); let mut bytes=encode(&id,&metadata,&payload(&id)).unwrap();
        bytes[16..24].copy_from_slice(&u64::MAX.to_be_bytes()); assert!(validate(&id,&metadata,bytes).is_err());
        let root=root(); let store=Store::new_with_policy(true, false, &root,id); let directory=Directory::open(&store.root,true).unwrap().unwrap();
        let file=directory.file(CACHE_FILE,libc::O_WRONLY|libc::O_CREAT|libc::O_EXCL).unwrap(); file.set_len((MAX_PAYLOAD+MAX_METADATA+HEADER+1) as u64).unwrap();
        assert!(store.load().is_err()); fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn symbolic_links_and_non_private_storage_are_rejected() {
        let root=root(); let outside=root.join("outside"); fs::DirBuilder::new().mode(0o700).create(&outside).unwrap();
        std::os::unix::fs::symlink(&outside,root.join(DIRECTORY)).unwrap(); let store=Store::new_with_policy(true, false, &root,identity());
        assert!(store.load().is_err()); fs::remove_file(root.join(DIRECTORY)).unwrap();
        fs::DirBuilder::new().mode(0o755).create(root.join(DIRECTORY)).unwrap(); assert!(store.load().is_err());
        fs::set_permissions(root.join(DIRECTORY),std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(&outside,root.join(DIRECTORY).join(CACHE_FILE)).unwrap(); assert!(store.load().is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn schedule_prevents_per_pipeline_get_data_and_requires_new_work() {
        let now=Instant::now(); let mut schedule=Schedule{began:now,last_attempt:None,captured_generation:0};
        assert!(!schedule.due(now+Duration::from_secs(14),1)); assert!(!schedule.due(now+Duration::from_secs(100),0));
        assert!(schedule.due(now+Duration::from_secs(15),1)); schedule.last_attempt=Some(now+Duration::from_secs(15)); schedule.captured_generation=1;
        assert!(!schedule.due(now+Duration::from_secs(74),2)); assert!(schedule.due(now+Duration::from_secs(75),2));
        assert!(!schedule.due(now+Duration::from_secs(1000),1));
    }
    #[test]
    fn atomic_publish_failure_keeps_last_good_and_stale_temp_recovers() {
        let root=root(); let directory=Directory::open(&root.join(DIRECTORY),true).unwrap().unwrap();
        directory.publish(b"last-good").unwrap();
        let stale=directory.file(TEMP_FILE,libc::O_WRONLY|libc::O_CREAT|libc::O_EXCL).unwrap(); drop(stale);
        directory.publish(b"new-good").unwrap();
        assert_eq!(fs::read(root.join(DIRECTORY).join(CACHE_FILE)).unwrap(),b"new-good");
        fs::create_dir(root.join(DIRECTORY).join(TEMP_FILE)).unwrap();
        assert!(directory.publish(b"must-not-replace").is_err());
        assert_eq!(fs::read(root.join(DIRECTORY).join(CACHE_FILE)).unwrap(),b"new-good");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn owned_snapshot_is_published_on_one_io_worker_and_never_recaptured_when_unchanged() {
        let root=root(); let id=identity(); let mut store=Store::new_with_policy(true, false, &root,id.clone()); store.note_pipeline_created();
        assert!(store.checkpoint_at(Instant::now()+Duration::from_secs(16),||Some(payload(&id))).unwrap());
        let until=Instant::now()+Duration::from_secs(3);
        while store.writer.as_ref().unwrap().stats.published.load(Ordering::Relaxed)==0 && Instant::now()<until { std::thread::yield_now(); }
        assert_eq!(store.writer.as_ref().unwrap().stats.published.load(Ordering::Relaxed),1);
        assert_eq!(store.load().unwrap().unwrap().bytes(),payload(&id));
        assert!(!store.checkpoint_at(Instant::now()+Duration::from_secs(200),||panic!("unchanged must not capture")).unwrap());
        drop(store); let until=Instant::now()+Duration::from_secs(3);
        while WRITER_ACTIVE.load(Ordering::Acquire) && Instant::now()<until { std::thread::yield_now(); }
        assert!(!WRITER_ACTIVE.load(Ordering::Acquire)); fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn failed_async_checkpoint_remains_dirty_and_retries_after_interval() {
        let root=root(); fs::write(root.join(DIRECTORY),b"directory creation fails safely").unwrap();
        let id=identity(); let mut store=Store::new_with_policy(true, false, &root,id.clone()); store.note_pipeline_created();
        let began=Instant::now();
        assert!(store.checkpoint_at(began+Duration::from_secs(16),||Some(payload(&id))).unwrap());
        let until=Instant::now()+Duration::from_secs(3);
        while store.writer.as_ref().unwrap().stats.errors.load(Ordering::Acquire)==0 && Instant::now()<until { std::thread::yield_now(); }
        assert_eq!(store.writer.as_ref().unwrap().stats.errors.load(Ordering::Acquire),1);
        assert!(!store.checkpoint_at(began+Duration::from_secs(50),||panic!("failed save must stay throttled")).unwrap());
        fs::remove_file(root.join(DIRECTORY)).unwrap();
        assert!(store.checkpoint_at(began+Duration::from_secs(76),||Some(payload(&id))).unwrap());
        let until=Instant::now()+Duration::from_secs(3);
        while store.writer.as_ref().unwrap().stats.published.load(Ordering::Relaxed)==0 && Instant::now()<until { std::thread::yield_now(); }
        assert_eq!(store.load().unwrap().unwrap().bytes(),payload(&id)); drop(store);
        let until=Instant::now()+Duration::from_secs(3);
        while WRITER_ACTIVE.load(Ordering::Acquire) && Instant::now()<until { std::thread::yield_now(); }
        assert!(!WRITER_ACTIVE.load(Ordering::Acquire)); fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn read_only_dirty_missing_cache_never_captures_or_creates_writer_or_files() {
        let root=root();let mut store=Store::new_with_policy(true,true,&root,identity());
        assert!(store.enabled());assert!(store.load().unwrap().is_none());
        for seconds in [0,16,100,1000] {
            store.note_pipeline_created();
            assert!(!store.checkpoint_at(Instant::now()+Duration::from_secs(seconds),||panic!("read-only must not invoke GPU export")).unwrap());
        }
        assert!(store.writer.is_none());assert!(store.schedule.last_attempt.is_none());
        assert_eq!(store.captures.calls,0);assert_eq!(fs::read_dir(&root).unwrap().count(),0);
        let status=store.status();assert_eq!(status["readOnly"],true);assert_eq!(status["loadEnabled"],true);
        assert_eq!(status["writeEnabled"],false);assert_eq!(status["getDataCalls"],0);fs::remove_dir(root).unwrap();
    }
    #[test]
    fn read_only_warm_load_keeps_payload_and_stale_temporary_exactly_unchanged() {
        let root=root();let id=identity();let data=payload(&id);
        let mut store=Store::new_with_policy(true,true,&root,id.clone());
        let directory=Directory::open(&store.root,true).unwrap().unwrap();
        let encoded=encode(&id,&id.metadata().unwrap(),&data).unwrap();directory.publish(&encoded).unwrap();
        fs::write(store.root.join(TEMP_FILE),b"preserve-stale-temp").unwrap();
        let before=fs::metadata(store.root.join(CACHE_FILE)).unwrap();
        assert_eq!(store.load().unwrap().unwrap().bytes(),data);store.note_pipeline_created();
        assert!(!store.checkpoint_at(Instant::now()+Duration::from_secs(100),||panic!("no rewrite in read-only")).unwrap());
        let after=fs::metadata(store.root.join(CACHE_FILE)).unwrap();
        assert_eq!((before.ino(),before.mtime(),before.mtime_nsec()),(after.ino(),after.mtime(),after.mtime_nsec()));
        assert_eq!(fs::read(store.root.join(CACHE_FILE)).unwrap(),encoded);
        assert_eq!(fs::read(store.root.join(TEMP_FILE)).unwrap(),b"preserve-stale-temp");
        assert!(store.writer.is_none());assert_eq!(store.captures.calls,0);fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn read_only_rejects_corrupt_or_wrong_identity_without_deletion_or_repair() {
        let root=root();let id=identity();let mut store=Store::new_with_policy(true,true,&root,id.clone());
        let directory=Directory::open(&store.root,true).unwrap().unwrap();
        let good=encode(&id,&id.metadata().unwrap(),&payload(&id)).unwrap();
        let mut corrupt=good.clone();*corrupt.last_mut().unwrap()^=1;
        for bytes in [b"bad-cache".to_vec(),corrupt] {
            directory.publish(&bytes).unwrap();assert!(store.load().is_err());store.note_pipeline_created();
            assert!(!store.checkpoint_at(Instant::now()+Duration::from_secs(100),||panic!("rejected cache must not export")).unwrap());
            assert_eq!(fs::read(store.root.join(CACHE_FILE)).unwrap(),bytes);
        }
        directory.publish(&good).unwrap();let mut changed=id;changed.driver.push('x');
        let different=Store::new_with_policy(true,true,&root,changed);assert!(different.load().is_err());
        assert_eq!(fs::read(store.root.join(CACHE_FILE)).unwrap(),good);fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn capture_wall_stats_measure_only_due_prime_attempts_and_none_results() {
        let root=root();let mut store=Store::new_with_policy(true,false,&root,identity());store.note_pipeline_created();
        let began=Instant::now();assert!(!store.checkpoint_at(began+Duration::from_secs(14),||panic!("too early")).unwrap());
        assert_eq!(store.captures.calls,0);
        assert!(!store.checkpoint_at(began+Duration::from_secs(16),||{std::thread::sleep(Duration::from_millis(2));None}).unwrap());
        assert_eq!(store.captures.calls,1);assert!(store.captures.last_ns>=1_000_000);assert_eq!(store.captures.last_bytes,0);
        assert_eq!(store.captures.total_ns,store.captures.max_ns);assert!(store.writer.is_none());
        assert!(!store.checkpoint_at(began+Duration::from_secs(60),||panic!("throttled export")).unwrap());
        assert_eq!(store.status()["firstCheckpointDelaySeconds"],15);assert_eq!(store.status()["checkpointIntervalSeconds"],60);
        assert!(!store.status()["readOnly"].as_bool().unwrap());fs::remove_dir(root).unwrap();
    }
    #[test]
    fn startup_read_only_flag_is_strict_and_frozen_in_isolated_processes() {
        if let Ok(expected)=std::env::var("MG_CACHE_POLICY_TEST_CHILD") {
            let expected=expected=="1";let root=root();let store=Store::new(true,&root,identity());
            assert_eq!(store.read_only,expected);
            std::env::set_var("MUGUET_PIPELINE_CACHE_READ_ONLY",if expected{"0"}else{"1"});
            let again=Store::new(true,&root,identity());assert_eq!(again.read_only,expected);
            fs::remove_dir(root).unwrap();return;
        }
        for value in [None,Some("0"),Some("1"),Some("true"),Some("01"),Some("")] {
            // Discover the module-qualified test name rather than assume the
            // surrounding harness's #[path] module identifier.
            let listed=std::process::Command::new(std::env::current_exe().unwrap()).arg("--list").output().unwrap();
            let list=String::from_utf8(listed.stdout).unwrap();
            let name=list.lines().find(|line|line.contains("startup_read_only_flag_is_strict_and_frozen_in_isolated_processes: test")).unwrap().trim_end_matches(": test");
            let mut command=std::process::Command::new(std::env::current_exe().unwrap());command.arg(name).arg("--exact");
            command.env("MG_CACHE_POLICY_TEST_CHILD",if value==Some("1"){"1"}else{"0"}).env_remove("MUGUET_PIPELINE_CACHE_READ_ONLY");
            if let Some(value)=value {command.env("MUGUET_PIPELINE_CACHE_READ_ONLY",value);}
            let result=command.output().unwrap();assert!(result.status.success()&&String::from_utf8_lossy(&result.stdout).contains("1 passed"),"{:?}: {} {}",value,String::from_utf8_lossy(&result.stdout),String::from_utf8_lossy(&result.stderr));
        }
    }
}
