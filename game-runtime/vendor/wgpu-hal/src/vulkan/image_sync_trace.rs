//! Bounded, durable record of early image synchronization operations.
//! Diagnostic only: none of these records changes a Vulkan argument.

use alloc::format;
use core::fmt;
use std::{fs::File, io::Write, sync::{Mutex, OnceLock}};

const MAX_EVENTS: u64 = 1024;
const MAX_BYTES: usize = 1024 * 1024;

struct Trace {
    file: File,
    events: u64,
    bytes: usize,
    truncated: bool,
}

static TRACE: OnceLock<Option<Mutex<Trace>>> = OnceLock::new();

pub(super) fn record(args: fmt::Arguments<'_>) {
    let trace = TRACE.get_or_init(|| {
        let path = std::env::var_os("MUGUET_IMAGE_SYNC_TRACE")?;
        let mut file = match File::create(&path) {
            Ok(file) => file,
            Err(error) => {
                std::eprintln!("Vulkan image sync trace open failed: {error}");
                return None;
            }
        };
        let header = format!("boundedEarlyImageSync maxEvents={MAX_EVENTS} maxBytes={MAX_BYTES} pid={} operationRecords=before-driver-call\n", std::process::id());
        if file.write_all(header.as_bytes()).and_then(|_| file.sync_all()).is_err() {
            std::eprintln!("Vulkan image sync trace header write failed");
            return None;
        }
        Some(Mutex::new(Trace { file, events: 0, bytes: header.len(), truncated: false }))
    });
    let Some(trace) = trace else { return };
    let Ok(mut trace) = trace.lock() else { return };
    if trace.truncated { return }
    let next = trace.events + 1;
    let line = format!("event={next} {args}\n");
    if next > MAX_EVENTS || trace.bytes.saturating_add(line.len()) > MAX_BYTES - 256 {
        let marker = format!("TRUNCATED=true retainedEvents={} retainedBytes={} reason=early-event-or-byte-budget\n", trace.events, trace.bytes);
        let _ = trace.file.write_all(marker.as_bytes()).and_then(|_| trace.file.sync_all());
        trace.truncated = true;
        return;
    }
    if let Err(error) = trace.file.write_all(line.as_bytes()).and_then(|_| trace.file.sync_all()) {
        std::eprintln!("Vulkan image sync trace write failed: {error}");
        trace.truncated = true;
        return;
    }
    trace.events = next;
    trace.bytes += line.len();
}
