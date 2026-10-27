//! Bounded, app-private descriptions of pipelines that reached an actual draw.
//! These are ordinary validated JSON recipes, never driver cache bytes. Loading
//! and replay scheduling belong to gpu.rs; this module makes no GPU calls.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::CString;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

pub(crate) const LIMIT: usize = 4096;
const FILE_LIMIT: usize = 16 << 20;
const VERSION: u32 = 1;
const DIRECTORY: &str = "native-pso-warmup-v1";
const FILENAME: &str = "recipes.json";
const INTERVAL: Duration = Duration::from_secs(2);
static TEMP_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Shader {
    pub hash: String,
    pub pause: bool,
    pub source_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Recipe {
    pub vs: Shader,
    pub ps: Option<Shader>,
    pub signature: Vec<[u32; 3]>,
    pub elements: Vec<[u32; 6]>,
    pub strides: [u32; 16],
    pub topology: u32,
    pub strip: u8,
    pub blend: Option<Vec<u32>>,
    pub depth: Option<Vec<u32>>,
    pub raster: Option<Vec<u32>>,
    pub colors: Vec<Option<wgpu::TextureFormat>>,
    pub depth_format: Option<wgpu::TextureFormat>,
    pub samples: u32,
    pub read_only_depth: bool,
    pub read_only_stencil: bool,
    // Per stage: texture low/high 64 bits, then sampler low/high 64 bits.
    pub masks: [[u64; 4]; 2],
}

fn hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
impl Shader {
    fn valid(&self) -> bool { hex(&self.hash, 16) && hex(&self.source_sha256, 64) }
}
impl Recipe {
    pub(crate) fn valid(&self) -> bool {
        self.vs.valid() && self.ps.as_ref().is_none_or(Shader::valid) &&
            self.signature.len() <= 32 && self.signature.iter().all(|s| s[2] < 32) &&
            self.elements.len() <= 32 && self.elements.iter().all(|e| e[3] < 16 && e[5] <= 1) &&
            (1..=5).contains(&self.topology) && self.strip <= 2 &&
            self.colors.len() <= 8 && self.samples.is_power_of_two() && self.samples <= 64 &&
            [&self.blend, &self.depth, &self.raster].iter()
                .all(|state| state.as_ref().is_none_or(|words| words.len() <= 128))
    }
    fn size_bound(&self) -> usize {
        // Conservative compact-JSON upper bound for the pinned wgpu30 schema.
        // Avoid serializing a newly seen recipe on the drawing thread. The
        // fixed allowance covers hashes, strides, masks, keys and punctuation;
        // 128 bytes covers even the structured ASTC texture-format variant.
        2048 + self.signature.len() * 40 + self.elements.len() * 80 + self.colors.len() * 128 +
            [&self.blend, &self.depth, &self.raster].iter()
                .map(|state| state.as_ref().map_or(0, |words| words.len() * 11)).sum::<usize>()
    }
}

#[derive(Serialize)]
struct Payload<'a> { version: u32, identity: &'a str, recipes: &'a [Recipe] }
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope { version: u32, identity: String, recipes: Vec<Recipe>, sha256: String }

fn checksum(version: u32, identity: &str, recipes: &[Recipe]) -> Result<String, String> {
    let bytes = serde_json::to_vec(&Payload { version, identity, recipes }).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn encode(identity: &str, recipes: Vec<Recipe>) -> Result<Vec<u8>, String> {
    let sha256 = checksum(VERSION, identity, &recipes)?;
    let bytes = serde_json::to_vec(&Envelope { version: VERSION, identity: identity.into(), recipes, sha256 })
        .map_err(|e| e.to_string())?;
    if bytes.len() > FILE_LIMIT { return Err("recipe_file_limit".into()); }
    Ok(bytes)
}
fn decode(identity: &str, bytes: &[u8]) -> Result<Vec<Recipe>, String> {
    if bytes.len() > FILE_LIMIT { return Err("recipe_file_limit".into()); }
    let envelope: Envelope = serde_json::from_slice(bytes).map_err(|e| format!("recipe_json: {e}"))?;
    if envelope.version != VERSION { return Err("recipe_version".into()); }
    if envelope.identity != identity { return Err("recipe_identity".into()); }
    if envelope.recipes.len() > LIMIT || !envelope.recipes.iter().all(Recipe::valid) {
        return Err("recipe_validation".into());
    }
    if !hex(&envelope.sha256, 64) ||
        envelope.sha256 != checksum(envelope.version, &envelope.identity, &envelope.recipes)? {
        return Err("recipe_checksum".into());
    }
    Ok(envelope.recipes)
}

// The provided application root is the trust boundary. Open its descriptor
// without following a symlink and verify its effective owner. HarmonyOS app
// roots can grant their application group write access (0770/0771); permit
// that only when the group matches this process's effective group. World write
// is always rejected. The dedicated directory and files stay 0700/0600.
struct Directory(File);
fn owner(metadata: &Metadata) -> bool {
    // SAFETY: geteuid takes no pointers, has no failure mode and changes no state.
    metadata.uid() == unsafe { libc::geteuid() }
}
fn private_file(metadata: &Metadata) -> bool {
    metadata.is_file() && owner(metadata) && metadata.mode() & 0o777 == 0o600 && metadata.nlink() == 1
}
fn component(parent: &File, name: &str, flags: i32, mode: u32) -> Result<File, String> {
    let name = CString::new(name).map_err(|_| "recipe_internal_filename".to_string())?;
    // SAFETY: parent owns a live directory descriptor; name is a terminated,
    // fixed internal component. The returned descriptor is adopted exactly once.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(),
        flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK, mode as libc::c_uint) };
    if fd < 0 { return Err(std::io::Error::last_os_error().to_string()); }
    Ok(unsafe { File::from_raw_fd(fd) })
}
impl Directory {
    fn open(root: &Path, create: bool) -> Result<Option<Self>, String> {
        let parent = OpenOptions::new().read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC).open(root)
            .map_err(|e| format!("recipe_root: {e}"))?;
        let metadata = parent.metadata().map_err(|e| e.to_string())?;
        // SAFETY: getegid, like geteuid above, takes no pointers and changes no
        // state. Matching the effective group keeps the writable app-root group
        // within the process's existing filesystem authority.
        let own_group = metadata.gid() == unsafe { libc::getegid() };
        if !metadata.is_dir() || !owner(&metadata) || metadata.mode() & 0o002 != 0 ||
            (metadata.mode() & 0o020 != 0 && !own_group) {
            return Err("recipe_non_private_root".into());
        }
        let name = CString::new(DIRECTORY).expect("constant component");
        if create {
            // SAFETY: parent is live; name is a terminated constant component.
            if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::AlreadyExists { return Err(error.to_string()); }
            }
        }
        // Keep the original io::Error here to distinguish an absent cold cache.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if !create && error.kind() == std::io::ErrorKind::NotFound { return Ok(None); }
            return Err(error.to_string());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !metadata.is_dir() || !owner(&metadata) || metadata.mode() & 0o777 != 0o700 {
            return Err("recipe_non_private_directory".into());
        }
        Ok(Some(Self(file)))
    }
    fn read(&self) -> Result<Option<Vec<u8>>, String> {
        let name = CString::new(FILENAME).expect("constant component");
        let fd = unsafe { libc::openat(self.0.as_raw_fd(), name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK) };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound { return Ok(None); }
            return Err(error.to_string());
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if !private_file(&metadata) { return Err("recipe_non_private_file".into()); }
        if metadata.len() > FILE_LIMIT as u64 { return Err("recipe_file_limit".into()); }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(FILE_LIMIT as u64 + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() > FILE_LIMIT { return Err("recipe_file_limit".into()); }
        Ok(Some(bytes))
    }
    fn publish(&self, bytes: &[u8]) -> Result<(), String> {
        // Reject an existing symlink, non-private file or other unusual target.
        // read() is size-bounded and also proves the target is a regular file.
        self.read()?;
        let temporary = format!(".recipes-{}-{}.tmp", std::process::id(), TEMP_ID.fetch_add(1, Ordering::Relaxed));
        let temporary_c = CString::new(temporary.as_str()).expect("generated component");
        let destination = CString::new(FILENAME).expect("constant component");
        let result = (|| {
            let mut file = component(&self.0, &temporary, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)?;
            if !private_file(&file.metadata().map_err(|e| e.to_string())?) { return Err("recipe_non_private_file".into()); }
            file.write_all(bytes).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            drop(file);
            // SAFETY: both names are terminated fixed/generated components;
            // descriptors refer to the same verified private directory.
            if unsafe { libc::renameat(self.0.as_raw_fd(), temporary_c.as_ptr(),
                self.0.as_raw_fd(), destination.as_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            self.0.sync_all().map_err(|e| e.to_string())
        })();
        if result.is_err() {
            unsafe { libc::unlinkat(self.0.as_raw_fd(), temporary_c.as_ptr(), 0); }
        }
        result
    }
}

struct Job { generation: u64, recipes: Vec<Recipe> }
struct Completion { generation: u64, result: Result<(), String> }
struct Worker { sender: SyncSender<Job>, receiver: Receiver<Completion> }
impl Worker {
    fn new(root: PathBuf, identity: String) -> Result<Self, String> {
        let (sender, jobs) = mpsc::sync_channel::<Job>(1);
        let (done, receiver) = mpsc::channel();
        std::thread::Builder::new().name("pso-recipe-io".into()).spawn(move || {
            while let Ok(job) = jobs.recv() {
                let result = encode(&identity, job.recipes).and_then(|bytes| {
                    Directory::open(&root, true)?.ok_or_else(|| "recipe_missing_directory".to_string())?.publish(&bytes)
                });
                // A dropped Store also drops the completion receiver. Still
                // drain its one possible final snapshot before exiting.
                let _ = done.send(Completion { generation: job.generation, result });
            }
        }).map_err(|e| e.to_string())?;
        Ok(Self { sender, receiver })
    }
}

pub(crate) struct Store {
    root: PathBuf,
    identity: String,
    read_only: bool,
    recipes: HashSet<Recipe>,
    bytes_bound: usize,
    loaded: usize,
    generation: u64,
    saved_generation: u64,
    queued_generation: u64,
    in_flight: bool,
    worker: Option<Worker>,
    last_capture: Option<Instant>,
    writes: u64,
    errors: u64,
    rejected: u64,
    limit_drops: u64,
    last_error: Option<String>,
}
impl Store {
    pub(crate) fn new(root: &Path, identity: String, read_only: bool) -> Self {
        let bytes_bound = 1024usize.saturating_add(identity.len().saturating_mul(6));
        Self { root: root.into(), identity, read_only, recipes: HashSet::new(), loaded: 0,
            bytes_bound,
            generation: 0, saved_generation: 0, queued_generation: 0, in_flight: false,
            worker: None, last_capture: None, writes: 0, errors: 0, rejected: 0,
            limit_drops: 0, last_error: None }
    }
    pub(crate) fn load(&mut self) -> Result<Vec<Recipe>, String> {
        let result = (|| {
            if self.identity.is_empty() || self.identity.len() > 16384 { return Err("recipe_identity_size".into()); }
            let Some(directory) = Directory::open(&self.root, false)? else { return Ok(Vec::new()); };
            let Some(bytes) = directory.read()? else { return Ok(Vec::new()); };
            decode(&self.identity, &bytes)
        })();
        match result {
            Ok(recipes) => {
                // A file contains prior actual draws. Never discard them when
                // this session observes only a subset of the previous scene.
                let unique: HashSet<_> = recipes.into_iter().collect();
                let loaded: Vec<_> = unique.into_iter().collect();
                for recipe in &loaded {
                    if self.recipes.len() < LIMIT && !self.recipes.contains(recipe) {
                        // Existing records were already size checked as one
                        // whole file; exact serialization is loading work.
                        self.bytes_bound += serde_json::to_vec(recipe).expect("validated recipe serializes").len() + 1;
                        self.recipes.insert(recipe.clone());
                    }
                }
                self.loaded = loaded.len();
                Ok(loaded)
            }
            Err(error) => { self.last_error = Some(error.clone()); self.errors += 1; Err(error) }
        }
    }
    /// Call only after an actual draw accepted this exact pipeline. In-memory
    /// hashing deduplicates the complete recipe without per-draw JSON encoding.
    pub(crate) fn record(&mut self, recipe: &Recipe) {
        if !recipe.valid() { self.rejected += 1; return; }
        if self.recipes.contains(recipe) { return; }
        if self.recipes.len() >= LIMIT || self.bytes_bound.saturating_add(recipe.size_bound()) > FILE_LIMIT {
            self.limit_drops += 1; return;
        }
        self.bytes_bound += recipe.size_bound();
        self.recipes.insert(recipe.clone());
        self.generation = self.generation.saturating_add(1);
    }
    fn collect(&mut self) {
        if let Some(worker) = &self.worker {
            while let Ok(completion) = worker.receiver.try_recv() {
                self.in_flight = false;
                match completion.result {
                    Ok(()) => {
                        self.saved_generation = self.saved_generation.max(completion.generation);
                        self.writes += 1;
                        self.last_error = None;
                    }
                    Err(error) => { self.errors += 1; self.last_error = Some(error); }
                }
            }
        }
    }
    /// Does no file IO or serialization on the caller. At most one capture is
    /// in flight; subsequent calls retry errors and coalesce newly used recipes.
    pub(crate) fn checkpoint(&mut self) {
        self.collect();
        if self.read_only || self.in_flight || self.generation == self.saved_generation ||
            self.last_capture.is_some_and(|at| at.elapsed() < INTERVAL) { return; }
        if self.identity.is_empty() || self.identity.len() > 16384 {
            self.last_error = Some("recipe_identity_size".into()); return;
        }
        if self.worker.is_none() {
            match Worker::new(self.root.clone(), self.identity.clone()) {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => { self.last_capture = Some(Instant::now()); self.errors += 1; self.last_error = Some(error); return; }
            }
        }
        let job = Job { generation: self.generation, recipes: self.recipes.iter().cloned().collect() };
        match self.worker.as_ref().expect("worker initialized").sender.try_send(job) {
            Ok(()) => { self.in_flight = true; self.queued_generation = self.generation; self.last_capture = Some(Instant::now()); }
            Err(error) => { self.errors += 1; self.last_error = Some(error.to_string()); self.worker = None; }
        }
    }
    pub(crate) fn status(&mut self) -> serde_json::Value {
        self.collect();
        serde_json::json!({ "version": VERSION, "readOnly": self.read_only,
            "recipes": self.recipes.len(), "loaded": self.loaded, "limit": LIMIT,
            "bytesBound": self.bytes_bound, "fileLimit": FILE_LIMIT,
            "generation": self.generation, "savedGeneration": self.saved_generation,
            "queuedGeneration": self.queued_generation, "dirty": self.generation != self.saved_generation,
            "inFlight": self.in_flight, "writes": self.writes, "errors": self.errors,
            "rejected": self.rejected, "limitDrops": self.limit_drops, "lastError": self.last_error })
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        // Best effort final coalesced snapshot. Do not wait for file IO on a
        // rendering/teardown thread; the worker drains its bounded channel.
        if !self.read_only && self.generation > self.queued_generation {
            if let Some(worker) = &self.worker {
                let _ = worker.sender.try_send(Job { generation: self.generation,
                    recipes: self.recipes.iter().cloned().collect() });
            }
        }
    }
}
