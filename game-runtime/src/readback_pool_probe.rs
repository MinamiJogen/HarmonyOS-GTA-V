//! Independent, marker-only GPU qualification of the actual idle-buffer policy.
//! No guest, surface, shader or normal Gpu path is started or modified here.
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{cell::RefCell, ffi::{CStr, CString}, fs::File, io::Write,
    os::raw::c_char, path::Path, sync::{Arc, Mutex, mpsc,
        atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use wgpu::*;
#[path = "readback_pool.rs"] mod policy;

thread_local! { static RESULT: RefCell<CString> = RefCell::new(CString::new("{}").unwrap()); }
static RUNNING: AtomicBool = AtomicBool::new(false);
const USAGE: BufferUsages = BufferUsages::MAP_READ.union(BufferUsages::COPY_DST);
const GUARD: usize = 256;
struct RunGuard;
impl Drop for RunGuard { fn drop(&mut self) { RUNNING.store(false, Ordering::Release); } }
struct Trace { file: File, start: Instant, requests: u64, bytes: u64, guard_bytes: u64 }
impl Trace {
    fn new(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)?;
        Ok(Self { file: File::create(root.join("readback-pool-probe-trace.jsonl"))?,
            start: Instant::now(), requests: 0, bytes: 0, guard_bytes: 0 })
    }
    fn step(&mut self, stage: &str, detail: Value) -> Result<()> {
        serde_json::to_writer(&mut self.file, &json!({"stage":stage,"detail":detail,
            "elapsedMs":self.start.elapsed().as_millis(),"requestsCompleted":self.requests,
            "bytesCompared":self.bytes,"guardBytesChecked":self.guard_bytes}))?;
        self.file.write_all(b"\n")?; self.file.sync_all()?; Ok(())
    }
}
fn errors_seen(errors: &Mutex<Vec<String>>) -> Result<()> {
    let e = errors.lock().map_err(|_| anyhow::anyhow!("error lock poisoned"))?;
    anyhow::ensure!(e.is_empty(), "wgpu error: {}", e.join("; ")); Ok(())
}
fn stats(s: policy::Stats) -> Value {
    json!({"created":s.created,"reused":s.reused,"returned":s.returned,"rejected":s.rejected,
        "bypassed":s.bypassed,"cachedBuffers":s.cached_buffers,"cachedBytes":s.cached_bytes,
        "peakBuffers":s.peak_buffers,"peakBytes":s.peak_bytes})
}
fn limits(pool: &policy::Pool<Buffer>) -> Result<()> {
    let s = pool.stats();
    anyhow::ensure!(s.cached_buffers <= policy::BUFFER_LIMIT && s.cached_bytes <= policy::BYTE_LIMIT,
        "idle pool exceeded count/byte cap"); Ok(())
}
fn fresh(device: &Device, size: u64, label: &'static str) -> Buffer {
    device.create_buffer(&BufferDescriptor { label: Some(label), size, usage: USAGE, mapped_at_creation: false })
}
fn pattern(size: usize, serial: u32) -> Vec<u8> {
    let mut bytes = vec![0xcdu8; size];
    for (i, word) in bytes[GUARD..size - GUARD].chunks_exact_mut(4).enumerate() {
        let value = (i as u32).wrapping_mul(0x9e3779b9)
            .rotate_left(serial % 31).wrapping_add(serial.wrapping_mul(0x85ebca6b)) ^ 0xd35a9127;
        word.copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
type MapRx = mpsc::Receiver<std::result::Result<(), String>>;
fn map(buffer: &Buffer) -> MapRx {
    let (tx, rx) = mpsc::channel();
    buffer.slice(..).map_async(MapMode::Read, move |result| {
        let _ = tx.send(result.map_err(|e| e.to_string()));
    }); rx
}
fn map_ready(rx: MapRx) -> Result<()> {
    rx.recv_timeout(Duration::from_secs(5)).context("readback map callback timeout")?
        .map_err(anyhow::Error::msg)
}
fn mapped_bytes(buffer: &Buffer) -> Result<Vec<u8>> {
    let view = buffer.slice(..).get_mapped_range().context("mapped readback view")?;
    Ok(view.to_vec()) // view drops before the caller unmaps
}
fn compare(actual: &[u8], expected: &[u8], name: &str) -> Result<()> {
    anyhow::ensure!(actual.len() == expected.len(), "{name}: byte length mismatch");
    if let Some(i) = actual.iter().zip(expected).position(|(a, b)| a != b) {
        anyhow::bail!("{name}: byte {i} is {:02x}, expected {:02x}", actual[i], expected[i]);
    } Ok(())
}
struct Pending {
    pooled: Buffer, control: Buffer, source: Buffer, expected: Vec<u8>,
    pooled_map: MapRx, control_map: MapRx, was_reused: bool,
}

/// All leases remain out of the pool through submit, map readiness, checked
/// CPU copies and unmap. A fresh control receives the identical GPU copy.
fn batch(device: &Device, queue: &Queue, pool: &mut policy::Pool<Buffer>, size: u64,
    count: u32, serial: &mut u32, trace: &mut Trace, errors: &Mutex<Vec<String>>) -> Result<Value> {
    let key = policy::Key { size, usage: USAGE.bits() };
    anyhow::ensure!(policy::full_buffer_copy(size, size, GUARD as u64, size + 2 * GUARD as u64), "bad full-copy fixture");
    let initial_idle = pool.stats().cached_buffers;
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("readback-pool-batch") });
    let mut prepared = Vec::new();
    for _ in 0..count {
        *serial = serial.wrapping_add(1);
        let expected = pattern(size as usize, *serial);
        let mut source_bytes = vec![0xa5; size as usize + 2 * GUARD];
        source_bytes[GUARD..GUARD + size as usize].copy_from_slice(&expected);
        let source = device.create_buffer(&BufferDescriptor { label: Some("readback-pool-source"),
            size: source_bytes.len() as u64, usage: BufferUsages::COPY_SRC | BufferUsages::COPY_DST,
            mapped_at_creation: false });
        queue.write_buffer(&source, 0, &source_bytes);
        let (pooled, was_reused) = match pool.take(key) {
            Some(buffer) => (buffer, true),
            None => { pool.created(true); (fresh(device, size, "readback-pool-lease"), false) }
        };
        let control = fresh(device, size, "readback-pool-fresh-control");
        anyhow::ensure!(pooled.size() == size && pooled.usage() == USAGE, "wrong exact key returned");
        encoder.copy_buffer_to_buffer(&source, GUARD as u64, &pooled, 0, size);
        encoder.copy_buffer_to_buffer(&source, GUARD as u64, &control, 0, size);
        prepared.push((pooled, control, source, expected, was_reused));
    }
    let idle_while_pending = pool.stats().cached_buffers;
    // With six leases the per-key maximum of four is exhausted. No in-flight
    // allocation may be returned by an idle-cache lookup, even before polling.
    if count > policy::PER_KEY_LIMIT as u32 {
        anyhow::ensure!(pool.take(key).is_none(), "in-flight lease appeared idle");
    }
    limits(pool)?;
    trace.step("before-batch-submit", json!({"size":size,"pending":count,"idleBefore":initial_idle,
        "idleWhilePending":idle_while_pending}))?;
    let index = queue.submit([encoder.finish()]);
    let mut pending = Vec::new();
    for (pooled, control, source, expected, was_reused) in prepared {
        let pooled_map = map(&pooled); let control_map = map(&control);
        pending.push(Pending { pooled, control, source, expected, pooled_map, control_map, was_reused });
    }
    device.poll(PollType::Wait { submission_index: Some(index), timeout: Some(Duration::from_secs(30)) })?;
    errors_seen(errors)?;
    if count > policy::PER_KEY_LIMIT as u32 {
        anyhow::ensure!(pool.take(key).is_none(), "mapped lease appeared idle before CPU copy/unmap");
    }
    let mut reused = 0;
    let mut digest = Sha256::new();
    for request in pending {
        map_ready(request.pooled_map)?; map_ready(request.control_map)?;
        let actual = mapped_bytes(&request.pooled)?;
        let control = mapped_bytes(&request.control)?;
        compare(&actual, &request.expected, "pooled versus expected")?;
        compare(&control, &request.expected, "fresh versus expected")?;
        compare(&actual, &control, "pooled versus fresh")?;
        anyhow::ensure!(actual[..GUARD].iter().chain(&actual[actual.len()-GUARD..]).all(|b| *b == 0xcd),
            "copied guard bands changed");
        digest.update(&actual);
        // Mirrors the normal ownership boundary: views are dropped and CPU
        // destination is complete before unmap, then this buffer becomes idle.
        request.pooled.unmap(); request.control.unmap();
        trace.bytes += size * 2; trace.guard_bytes += (GUARD * 4) as u64;
        trace.requests += 1; reused += u32::from(request.was_reused);
        pool.return_unmapped(key, request.pooled);
        limits(pool)?;
        drop(request.control); drop(request.source);
    }
    let result = json!({"size":size,"requests":count,"reused":reused,
        "payloadSHA256":format!("{:x}",digest.finalize()),"pool":stats(pool.stats())});
    trace.step("batch-pass", result.clone())?; Ok(result)
}

fn policy_and_partial(device: &Device, queue: &Queue, trace: &mut Trace,
    errors: &Mutex<Vec<String>>) -> Result<Value> {
    let key = policy::Key { size: 8, usage: USAGE.bits() };
    let mut pool = policy::Pool::<Buffer>::new();
    let poison = fresh(device, 8, "readback-pool-poison-cache");
    queue.write_buffer(&poison, 0, &[0xe7; 8]);
    let index = queue.submit(std::iter::empty::<CommandBuffer>()); let ready = map(&poison);
    device.poll(PollType::Wait { submission_index: Some(index), timeout: Some(Duration::from_secs(30)) })?;
    map_ready(ready)?; compare(&mapped_bytes(&poison)?, &[0xe7; 8], "cache poison prime")?;
    poison.unmap(); pool.created(true); pool.return_unmapped(key, poison);
    let rejected = [
        !policy::full_buffer_copy(8, 4, 0, 4),
        !policy::full_buffer_copy(8, 8, 1, 16),
        !policy::full_buffer_copy(8, 8, 4, 8),
        !policy::full_buffer_copy(8, 8, u64::MAX-3, u64::MAX),
        !policy::full_texture_copy(256, 4, 256, 1, 1, 1),
        !policy::source_region_valid([1,0,0], [256,256,1], [256,256,1]),
        !policy::source_region_valid([0,0,1], [256,256,1], [256,256,1]),
    ];
    anyhow::ensure!(rejected.into_iter().all(|v| v), "partial/invalid metadata incorrectly eligible");
    let eligible = policy::full_buffer_copy(8, 4, 0, 4);
    let partial = if eligible { pool.take(key) } else { None }
        .unwrap_or_else(|| { pool.created(eligible); fresh(device, 8, "partial-fresh-zero-tail") });
    let source = device.create_buffer(&BufferDescriptor { label: Some("partial-copy-source"), size: 4,
        usage: BufferUsages::COPY_SRC | BufferUsages::COPY_DST, mapped_at_creation: false });
    queue.write_buffer(&source, 0, &[0x19,0x27,0x83,0x46]);
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor { label: Some("partial-copy-bypass") });
    encoder.copy_buffer_to_buffer(&source, 0, &partial, 0, 4);
    let index = queue.submit([encoder.finish()]); let ready = map(&partial);
    device.poll(PollType::Wait { submission_index: Some(index), timeout: Some(Duration::from_secs(30)) })?;
    map_ready(ready)?; errors_seen(errors)?;
    compare(&mapped_bytes(&partial)?, &[0x19,0x27,0x83,0x46,0,0,0,0], "fresh partial zero tail")?;
    partial.unmap(); drop(partial); drop(source);
    anyhow::ensure!(pool.stats().cached_buffers == 1 && pool.stats().reused == 0,
        "bypassed partial request touched poisoned idle buffer");
    let poison = pool.take(key).context("poisoned cache unexpectedly lost")?; let ready = map(&poison);
    device.poll(PollType::Wait { submission_index: None, timeout: Some(Duration::from_secs(30)) })?;
    map_ready(ready)?; compare(&mapped_bytes(&poison)?, &[0xe7; 8], "untouched idle poison")?;
    poison.unmap(); drop(poison); drop(pool);
    trace.bytes += 32; trace.step("partial-policy-pass", json!({"invalidPolicyCases":rejected.len(),
        "invalidGPUCallsSubmitted":0,"partialFreshZeroTail":true,"idlePoisonUntouched":true}))?;
    Ok(json!({"invalidPolicyCases":rejected.len(),"invalidGPUCallsSubmitted":0,
        "partialFreshZeroTail":true,"idlePoisonUntouched":true}))
}

fn capacity(device: &Device, trace: &mut Trace) -> Result<Value> {
    let mut count = policy::Pool::<Buffer>::new();
    for i in 0..17 { let size = 64 + i * 8; count.return_unmapped(policy::Key { size, usage: USAGE.bits() }, fresh(device, size, "count-cap")); }
    anyhow::ensure!(count.stats().cached_buffers == policy::BUFFER_LIMIT && count.stats().rejected == 1,
        "global buffer cap failed");
    let count_stats = stats(count.stats()); drop(count);
    let mut bytes = policy::Pool::<Buffer>::new();
    bytes.return_unmapped(policy::Key { size: policy::BYTE_LIMIT, usage: USAGE.bits() }, fresh(device, policy::BYTE_LIMIT, "byte-cap"));
    bytes.return_unmapped(policy::Key { size: 8, usage: USAGE.bits() }, fresh(device, 8, "byte-cap-reject"));
    bytes.return_unmapped(policy::Key { size: policy::BYTE_LIMIT + 8, usage: USAGE.bits() }, fresh(device, policy::BYTE_LIMIT + 8, "oversize-reject"));
    anyhow::ensure!(bytes.stats().cached_buffers == 1 && bytes.stats().cached_bytes == policy::BYTE_LIMIT
        && bytes.stats().rejected == 2, "byte cap failed");
    let byte_stats = stats(bytes.stats()); drop(bytes);
    let result = json!({"countCap":count_stats,"byteCap":byte_stats,"allCapacityBuffersDropped":true});
    trace.step("capacity-pass", result.clone())?; Ok(result)
}

fn run(root: &Path, iterations: u32, trace: &mut Trace) -> Result<Value> {
    anyhow::ensure!((1..=64).contains(&iterations), "iterations must be1..64");
    anyhow::ensure!(crate::RT.get().is_none() && !crate::gpu::STARTED.load(Ordering::SeqCst),
        "requires independent process before game startup");
    crate::config::configure_sync_submit(false, &root.join("readback-pool-probe-submit-trace"));
    std::env::set_var("MUGUET_DEVICE_FAULT_DIR", root.join("readback-pool-probe-device-fault"));
    trace.step("begin", json!({"iterations":iterations,"sizes":[262144,524288],
        "batches":[6,2,4],"actualPolicySource":"readback_pool.rs","normalRuntimeStarted":false}))?;
    let mut descriptor = InstanceDescriptor::new_without_display_handle(); descriptor.backends = Backends::VULKAN;
    let instance = Instance::new(descriptor);
    let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
        power_preference: PowerPreference::HighPerformance, compatible_surface: None,
        force_fallback_adapter: false, apply_limit_buckets: false }))?;
    let info = adapter.get_info();
    let info = json!({"name":info.name,"vendor":info.vendor,"device":info.device,
        "driver":info.driver,"driverInfo":info.driver_info,"backend":format!("{:?}",info.backend)});
    let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
        label: Some("readback-pool-independent-probe"), required_features: Features::empty(),
        required_limits: adapter.limits(), ..Default::default() }))?;
    let errors = Arc::new(Mutex::new(Vec::<String>::new()));
    let e = errors.clone(); device.on_uncaptured_error(Arc::new(move |error: Error| {
        if let Ok(mut e) = e.lock() { if e.len() < 32 { e.push(error.to_string()); } }
    }));
    let e = errors.clone(); device.set_device_lost_callback(move |reason, message| {
        if let Ok(mut e) = e.lock() { if e.len() < 32 { e.push(format!("device lost {reason:?}: {message}")); } }
    });
    trace.step("adapter", info.clone())?;
    let partial = policy_and_partial(&device, &queue, trace, &errors)?;
    let mut pool = policy::Pool::<Buffer>::new(); let mut serial = 0; let mut batches = Vec::new();
    for _ in 0..iterations { for size in [262144,524288] { for count in [6,2,4] {
        batches.push(batch(&device, &queue, &mut pool, size, count, &mut serial, trace, &errors)?);
    } } }
    let pool_stats = stats(pool.stats());
    anyhow::ensure!(pool.stats().reused > 0 && pool.stats().rejected >= 4, "reuse/per-key pressure not exercised");
    let caps = capacity(&device, trace)?;
    drop(pool); trace.step("before-final-teardown", json!({"pool":pool_stats}))?;
    device.poll(PollType::Wait { submission_index: None, timeout: Some(Duration::from_secs(30)) })?;
    errors_seen(&errors)?; drop(queue); drop(device); drop(adapter); drop(instance);
    trace.step("after-final-teardown", json!({}))?;
    Ok(json!({"probeKind":"readback_pool","status":"pass","pass":true,
        "iterationsCompleted":iterations,"requestsCompleted":trace.requests,"expectedRequests":iterations*24,
        "bytesCompared":trace.bytes,"copiedGuardBytesChecked":trace.guard_bytes,"adapter":info,
        "pool":pool_stats,"partialAndInvalidPolicy":partial,"capacity":caps,"batches":batches,
        "finalDeviceTeardownComplete":true,"normalGameStarted":false,
        "scope":"Actual Pool<Buffer>, identical fresh/pooled GPU copies, new payload per lease, six in-flight/mapped leases excluded from idle cache, full raw bytes including copied guard bands, fresh partial-copy zero tail, invalid metadata policy without invalid GPU submission, hard cache caps and real final teardown. This is not whole normal-game or guest notification qualification and not a performance measurement."}))
}

#[no_mangle]
pub unsafe extern "C" fn mg_readback_pool_probe_json(files_dir: *const c_char, iterations: u32) -> *const c_char {
    let result = (|| -> Result<Value> {
        anyhow::ensure!(!files_dir.is_null(), "null files dir");
        let root = Path::new(unsafe { CStr::from_ptr(files_dir) }.to_str()?);
        anyhow::ensure!(!RUNNING.swap(true, Ordering::AcqRel), "readback pool probe already running");
        let _guard = RunGuard; let mut trace = Trace::new(root)?;
        match run(root, iterations, &mut trace) {
            Ok(result) => Ok(result),
            Err(error) => {
                let error = format!("{error:#}"); let _ = trace.step("fail", json!({"error":error}));
                Ok(json!({"probeKind":"readback_pool","status":"fail","pass":false,
                    "iterationsCompleted":0,"requestsCompleted":trace.requests,
                    "bytesCompared":trace.bytes,"error":error}))
            }
        }
    })().unwrap_or_else(|error| json!({"probeKind":"readback_pool","status":"fail","pass":false,
        "iterationsCompleted":0,"requestsCompleted":0,"bytesCompared":0,"error":format!("{error:#}")}));
    RESULT.with(|slot| { let mut slot = slot.borrow_mut();
        *slot = CString::new(result.to_string()).expect("JSON has no raw NUL"); slot.as_ptr() })
}
