//! Bounds for native host access to the Wasm shared-memory allocation.
use std::io::Write;
use std::ops::Range;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static VIOLATIONS: AtomicU64 = AtomicU64::new(0);
static RECORD_LOCK: Mutex<()> = Mutex::new(());
const MAX_RECORDS: u64 = 64;

fn validate_span(total: usize, address: u64, size: u64, alignment: u64) -> Result<Range<usize>, &'static str> {
    if alignment == 0 || !alignment.is_power_of_two() || address % alignment != 0 {
        return Err("invalid_alignment");
    }
    let end = address.checked_add(size).ok_or("address_size_overflow")?;
    let start = usize::try_from(address).map_err(|_| "address_exceeds_usize")?;
    let end = usize::try_from(end).map_err(|_| "end_exceeds_usize")?;
    let size = usize::try_from(size).map_err(|_| "size_exceeds_usize")?;
    if end > total || size > isize::MAX as usize {
        return Err("outside_shared_memory");
    }
    Ok(start..end)
}

fn validate_table_span(total: usize, address: u64, slots: u64) -> Result<Range<usize>, &'static str> {
    let bytes = slots.checked_mul(64).and_then(|n| n.checked_add(64)).ok_or("slot_count_mul64_add64_overflow")?;
    validate_span(total, address, bytes, 4)
}

pub fn span(total: usize, address: u64, size: u64, alignment: u64, operation: &str) -> Option<Range<usize>> {
    match validate_span(total, address, size, alignment) {
        Ok(range) => Some(range),
        Err(reason) => {
            record(total, address, size, alignment, operation, reason);
            None
        }
    }
}

pub fn table_span(total: usize, address: u64, slots: u64) -> Option<Range<usize>> {
    match validate_table_span(total, address, slots) {
        Ok(range) => Some(range),
        Err(reason) => {
            // For this operation the size field is explicitly a slot count.
            record(total, address, slots, 4, "io.table_span_slots", reason);
            None
        }
    }
}

fn json_string(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn write_record(path: &Path, line: &str) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    file.sync_all()
}

fn record(total: usize, address: u64, size: u64, alignment: u64, operation: &str, reason: &str) {
    let sequence = VIOLATIONS.fetch_add(1, Ordering::Relaxed) + 1;
    if sequence > MAX_RECORDS { return; }
    let thread = std::thread::current();
    let timestamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let line = format!("{{\"version\":1,\"sequence\":{sequence},\"timestampUnixMs\":{timestamp},\"operation\":{},\"reason\":{},\"address\":{address},\"addressHex\":\"0x{address:x}\",\"size\":{size},\"alignment\":{alignment},\"memoryBytes\":{total},\"thread\":{}}}\n", json_string(operation), json_string(reason), json_string(thread.name().unwrap_or("unnamed")));
    eprintln!("[host-memory] {}", line.trim_end());
    let _lock = RECORD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(root) = std::env::var_os("MUGUET_SUPPORT") else {
        eprintln!("[host-memory] cannot persist violation: MUGUET_SUPPORT is unset");
        return;
    };
    let path = Path::new(&root).join("host-memory-violation.jsonl");
    if let Err(error) = write_record(&path, &line) {
        eprintln!("[host-memory] cannot persist {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_exact_end_and_unaligned_byte_spans() {
        assert_eq!(validate_span(4096, 1024, 3072, 4), Ok(1024..4096));
        assert_eq!(validate_span(4096, 4096, 0, 1), Ok(4096..4096));
        assert_eq!(validate_span(4096, 7, 3, 1), Ok(7..10));
    }

    #[test]
    fn rejects_real_release_overflow_counterexample() {
        let address = u64::MAX - 7;
        assert_eq!(address.wrapping_add(16), 8);
        assert!(8 <= 3usize << 30); // The previous end-only comparison accepted it.
        assert_eq!(validate_span(3usize << 30, address, 16, 1), Err("address_size_overflow"));
        assert!(validate_span(4096, u64::MAX, 0, 1).is_err());
        assert!(validate_span(4096, 4096, 1, 1).is_err());
        assert!(validate_span(4096, 4097, 0, 1).is_err());
    }

    #[test]
    fn checks_atomic_alignment_and_whole_input_block() {
        assert_eq!(validate_span(512, 68, 444, 4), Ok(68..512));
        assert!(validate_span(512, 72, 444, 4).is_err());
        assert!(validate_span(512, 66, 444, 4).is_err());
        assert!(validate_span(512, 64, 444, 0).is_err());
        assert!(validate_span(512, 64, 444, 3).is_err());
    }

    #[test]
    fn table_count_and_header_math_cannot_wrap() {
        assert_eq!(validate_table_span(256, 64, 2), Ok(64..256));
        assert_eq!(validate_table_span(256, 64, u64::MAX), Err("slot_count_mul64_add64_overflow"));
        assert!(validate_table_span(256, u64::MAX - 3, 0).is_err());
        assert!(validate_table_span(256, 64, 3).is_err());
    }

    #[test]
    fn persists_a_complete_jsonl_record_before_returning() {
        let root = std::env::temp_dir().join(format!("muguet-host-span-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let previous = std::env::var_os("MUGUET_SUPPORT");
        std::env::set_var("MUGUET_SUPPORT", &root);
        assert!(span(4096, u64::MAX - 7, 16, 1, "quote\" slash\\ tab\t newline\n 中文\u{0001}").is_none());
        let path = root.join("host-memory-violation.jsonl");
        let line = std::fs::read_to_string(&path).unwrap();
        assert_eq!(line.lines().count(), 1);
        assert!(line.contains("\"reason\":\"address_size_overflow\""));
        assert!(line.contains("\"memoryBytes\":4096"));
        assert!(line.contains("quote\\\" slash\\\\ tab\\t newline\\n 中文\\u0001"));
        assert_eq!(json_string("\u{0001}"), "\"\\u0001\"");
        match previous { Some(value) => std::env::set_var("MUGUET_SUPPORT", value), None => std::env::remove_var("MUGUET_SUPPORT") }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
