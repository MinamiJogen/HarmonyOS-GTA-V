//! Narrow eligibility for the trusted guest's published inline buffer uploads.
//! Allocation lifetime alone does not permit borrowing shared guest memory.
//! `wasm_gpu::EmitData/Reserve` in the audited AOT copies into a producer-locked
//! ring before release publication and cannot reuse that range until tail ack.
//! This module only recognizes that command shape; unknown shapes keep a copy.

const DATA_START: u64 = 64;
const MAX_PAYLOAD_BYTES: u64 = 4 << 20; // audited GpuUploadBuffer/Unmap chunks

#[derive(Clone, Copy, Debug)]
pub(super) struct Command {
    data_address: u64,
    inline_words: u64,
}

impl Command {
    pub(super) fn contains_exact_payload(self, address: u64, size: u64) -> bool {
        size <= MAX_PAYLOAD_BYTES && self.data_address == address
            && size.checked_add(3).map(|n| n / 4) == Some(self.inline_words)
    }
}

/// `tail` is the current unacknowledged opcode and `head` is the acquired batch
/// head. A wrapped batch's old segment ends at the ring end; opcode1 handles
/// the subsequent wrap. Never interpret bytes beyond that published segment.
pub(super) fn published_buffer_command(
    ring: u64, ring_size: u64, tail: i32, head: i32, payload_word: u64, words: u64,
) -> Option<Command> {
    let tail = u64::try_from(tail).ok()?;
    let head = u64::try_from(head).ok()?;
    if ring_size > i32::MAX as u64 || ring_size <= DATA_START
        || tail < DATA_START || head < DATA_START || tail >= ring_size || head > ring_size
        || tail == head || words < 3 || words > u32::MAX as u64
        || ring % 4 != 0 || tail % 4 != 0 || head % 4 != 0 {
        return None;
    }
    let payload = payload_word.checked_mul(4)?;
    let command = payload.checked_sub(8)?;
    if command != ring.checked_add(tail)? { return None; }
    let end = payload.checked_add(words.checked_mul(4)?)?;
    let segment_end = ring.checked_add(if head > tail { head } else { ring_size })?;
    if end > segment_end || end > ring.checked_add(ring_size)? { return None; }
    Some(Command { data_address: payload.checked_add(12)?, inline_words: words - 3 })
}

pub(super) fn borrow_allowed(
    enabled: bool, used: bool, no_overwrite: bool, command: Option<Command>, address: u64, size: u64,
) -> bool {
    enabled && (no_overwrite || !used)
        && command.is_some_and(|command| command.contains_exact_payload(address, size))
}

#[derive(Default)]
pub(super) struct Counters {
    pub calls: u64,
    pub borrowed_calls: u64,
    pub borrowed_bytes: u64,
    pub snapshot_calls: u64,
    pub snapshot_bytes: u64,
}
impl Counters {
    pub(super) fn record(&mut self, borrowed: bool, size: u64) {
        self.calls = self.calls.saturating_add(1);
        if borrowed {
            self.borrowed_calls = self.borrowed_calls.saturating_add(1);
            self.borrowed_bytes = self.borrowed_bytes.saturating_add(size);
        } else {
            self.snapshot_calls = self.snapshot_calls.saturating_add(1);
            self.snapshot_bytes = self.snapshot_bytes.saturating_add(size);
        }
    }
}
