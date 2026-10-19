//! Opt-in CPU submission journal. Owns strings/IDs only, never GPU resources.
use alloc::{format, string::String};
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::{fmt::Write as _, fs::File, io::Write, path::PathBuf, sync::OnceLock};

const MAX_LABEL_BYTES: usize = 1024;
const LABEL_BUDGET: usize = 4 << 20;
const MAX_JOURNALS: usize = 16;
const MAX_JOURNAL_BYTES: usize = 1 << 20;
const MAX_EVENTS: usize = 8192;
const MAX_SUBMIT_BYTES: usize = 4 << 20;
const RING_SLOTS: u64 = 4;
static LABEL_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE_JOURNALS: AtomicUsize = AtomicUsize::new(0);
static EPOCH: AtomicU64 = AtomicU64::new(0);
static SUBMIT: AtomicU64 = AtomicU64::new(0);
static IO_ERRORS: AtomicU64 = AtomicU64::new(0);

struct Config { directory: Option<PathBuf>, sync: bool }
fn config() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| Config {
        directory: std::env::var_os("MUGUET_SUBMIT_TRACE_DIR").map(PathBuf::from),
        sync: std::env::var_os("MUGUET_SYNC_SUBMIT").is_some(),
    })
}
pub(super) fn sync_enabled() -> bool { config().sync }
pub(super) fn enabled() -> bool { config().directory.is_some() || config().sync }
pub(super) fn semaphore_summary(args: core::fmt::Arguments<'_>) -> String {
    struct Limited { text: String, truncated: bool }
    impl core::fmt::Write for Limited {
        fn write_str(&mut self, text: &str) -> core::fmt::Result {
            let (part, clipped) = capped(text, 16384_usize.saturating_sub(self.text.len()));
            self.text.push_str(part); self.truncated |= clipped;
            if clipped { Err(core::fmt::Error) } else { Ok(()) }
        }
    }
    let mut value = Limited { text: String::new(), truncated: false };
    let _ = core::fmt::write(&mut value, args);
    if value.truncated { value.text.push_str(" [TRUNCATED semaphore metadata]"); }
    value.text
}
fn capped(value: &str, limit: usize) -> (&str, bool) {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) { end -= 1; }
    (&value[..end], end != value.len())
}
fn write_file(name: &str, bytes: &[u8]) {
    let Some(directory) = config().directory.as_deref() else { return; };
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(directory)?;
        let mut file = File::create(directory.join(name))?;
        file.write_all(bytes)?; file.sync_all()
    })();
    if let Err(error) = result {
        if IO_ERRORS.fetch_add(1, Ordering::Relaxed) < 4 { log::error!("submit trace write {name}: {error}"); }
    }
}

#[derive(Debug)]
pub(super) struct PipelineLabel { value: String, truncated: bool, missing: bool, reserved: usize }
impl Drop for PipelineLabel {
    fn drop(&mut self) { LABEL_BYTES.fetch_sub(self.reserved, Ordering::Relaxed); }
}
pub(super) fn pipeline_label(label: Option<&str>) -> Option<PipelineLabel> {
    if !enabled() { return None; }
    let (label, truncated) = capped(label.unwrap_or("<unlabeled>"), MAX_LABEL_BYTES);
    let reserved = label.len();
    if LABEL_BYTES.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |n| n.checked_add(reserved).filter(|&n| n <= LABEL_BUDGET)).is_err() {
        return Some(PipelineLabel { value: String::new(), truncated, missing: true, reserved: 0 });
    }
    let mut value = String::new();
    if value.try_reserve_exact(reserved).is_err() {
        LABEL_BYTES.fetch_sub(reserved, Ordering::Relaxed);
        return Some(PipelineLabel { value, truncated, missing: true, reserved: 0 });
    }
    value.push_str(label);
    Some(PipelineLabel { value, truncated, missing: false, reserved })
}

#[derive(Debug)]
pub(super) struct Journal {
    pub(super) epoch: u64,
    pool: u64, generation: u64, command: u64,
    encoder_label: String, label_truncated: bool,
    events: usize, dropped: usize, missing_labels: usize, truncated_labels: usize, bytes: String, slot: bool,
}
impl Drop for Journal {
    fn drop(&mut self) { if self.slot { LIVE_JOURNALS.fetch_sub(1, Ordering::AcqRel); } }
}
impl Journal {
    pub(super) fn begin(pool: u64, generation: u64, command: u64, label: Option<&str>) -> Option<Self> {
        if !enabled() { return None; }
        let slot = LIVE_JOURNALS.fetch_update(Ordering::AcqRel, Ordering::Relaxed, |n| (n < MAX_JOURNALS).then_some(n + 1)).is_ok();
        let mut bytes = String::new();
        let slot = if slot && bytes.try_reserve_exact(MAX_JOURNAL_BYTES).is_err() {
            LIVE_JOURNALS.fetch_sub(1, Ordering::AcqRel); false
        } else { slot };
        let (label, label_truncated) = capped(label.unwrap_or("<unlabeled>"), MAX_LABEL_BYTES);
        Some(Self { epoch: EPOCH.fetch_add(1, Ordering::Relaxed) + 1,
            pool, generation, command, encoder_label: label.into(), label_truncated,
            events: 0, dropped: 0, missing_labels: 0, truncated_labels: 0, bytes, slot })
    }
    pub(super) fn event(&mut self, args: core::fmt::Arguments<'_>) {
        self.events += 1;
        if !self.slot || self.events > MAX_EVENTS || self.dropped != 0 { self.dropped += 1; return; }
        // Arguments are fixed numeric command fields or capped pipeline labels.
        let event = format!("event={} {args}\n", self.events);
        if event.len() > 8192 || self.bytes.len() + event.len() > MAX_JOURNAL_BYTES { self.dropped += 1; return; }
        self.bytes.push_str(&event);
    }
    pub(super) fn pipeline(&mut self, kind: &str, raw: u64, label: Option<&PipelineLabel>) {
        if label.is_none_or(|label| label.missing) { self.missing_labels += 1; }
        if label.is_some_and(|label| label.truncated) { self.truncated_labels += 1; }
        match label {
            Some(label) => self.event(format_args!("bindPipeline kind={kind} raw={raw:#x} label={:?} labelTruncated={} labelMissing={}", label.value, label.truncated, label.missing)),
            None => self.event(format_args!("bindPipeline kind={kind} raw={raw:#x} labelMissing=true")),
        }
    }
}

pub(super) struct Submission { sequence: u64, metadata: String, file_truncated: bool, missing: usize }
impl Submission {
    pub(super) fn begin<'a>(queue: u64, signal: u64, fence: u64, waits: &str, signals: &str,
        commands: impl Iterator<Item = (u64, Option<&'a Journal>)>) -> Option<Self> {
        if !enabled() { return None; }
        let sequence = SUBMIT.fetch_add(1, Ordering::Relaxed) + 1;
        let mut metadata = format!("processId={}\nsubmitSequence={sequence}\nqueue={queue:#x} signalValue={signal} fence={fence:#x}\nsyncAfterSubmit={}\nwaits={waits}\nsignals={signals}\nlimits: journalPayloadBytes={} liveJournals={} eventsPerCB={} labelPayloadBytes={} submitBytes={} ringSlots={}\n", std::process::id(), config().sync, MAX_JOURNAL_BYTES, MAX_JOURNALS, MAX_EVENTS, LABEL_BUDGET, MAX_SUBMIT_BYTES, RING_SLOTS);
        let mut missing = 0; let mut count = 0; let mut file_truncated = false;
        let mut incomplete_journals = 0; let mut omitted_headers = 0;
        // First list all command buffers in actual submission order before any
        // large event payload can exhaust the file budget.
        let mut iterator = commands;
        let commands: alloc::vec::Vec<_> = iterator.by_ref().take(1024).collect();
        let omitted_commands = iterator.count();
        if omitted_commands != 0 { missing += omitted_commands; file_truncated = true; }
        for (index, (raw, journal)) in commands.iter().enumerate() {
            count += 1;
            let header = match journal {
                Some(j) => {
                    if !j.slot { missing += 1; }
                    if !j.slot || j.dropped != 0 || j.missing_labels != 0 || j.truncated_labels != 0 { incomplete_journals += 1; }
                    format!("command[{index}] raw={raw:#x} epoch={} pool={:#x} poolGeneration={} recordedRaw={:#x} encoderLabel={:?} encoderLabelTruncated={} totalEvents={} droppedEvents={} missingPipelineLabels={} truncatedPipelineLabels={} journalMissing={}\n", j.epoch, j.pool, j.generation, j.command, j.encoder_label, j.label_truncated, j.events, j.dropped, j.missing_labels, j.truncated_labels, !j.slot)
                },
                None => { missing += 1; format!("command[{index}] raw={raw:#x} journalMissing=true\n") },
            };
            if metadata.len() + header.len() < MAX_SUBMIT_BYTES - 1024 { metadata.push_str(&header); }
            else { file_truncated = true; omitted_headers += 1; }
        }
        for (index, (_, journal)) in commands.iter().enumerate() {
            if let Some(j) = journal {
                let header = format!("commandEvents index={index} epoch={} bytes={}\n", j.epoch, j.bytes.len());
                if metadata.len() + header.len() + j.bytes.len() < MAX_SUBMIT_BYTES - 1024 {
                    metadata.push_str(&header); metadata.push_str(&j.bytes);
                } else { file_truncated = true; }
            }
        }
        let _ = writeln!(metadata, "commandCount={}\nomittedCommandHeaders={}\nmissingCommandJournals={missing}\nincompleteCommandJournals={incomplete_journals}\nfileTruncated={file_truncated}\nmetadataComplete={}\ntraceIoErrors={}", count + omitted_commands, omitted_commands + omitted_headers, missing == 0 && incomplete_journals == 0 && !file_truncated, IO_ERRORS.load(Ordering::Relaxed));
        let value = Self { sequence, metadata, file_truncated, missing };
        write_file("last-submit.txt", value.metadata.as_bytes());
        write_file(&format!("submit-ring-{}.txt", sequence % RING_SLOTS), value.metadata.as_bytes());
        value.phase("before-vkQueueSubmit", "not-called");
        Some(value)
    }
    pub(super) fn phase(&self, phase: &str, result: &str) {
        write_file("last-submit-state.txt", format!("processId={}\nsubmitSequence={}\nphase={phase}\nresult={result}\nfileTruncated={}\nmissingCommandJournals={}\ntraceIoErrors={}\n", std::process::id(), self.sequence, self.file_truncated, self.missing, IO_ERRORS.load(Ordering::Relaxed)).as_bytes());
    }
    pub(super) fn completed(&self) {
        write_file("last-completed-submit.txt", self.metadata.as_bytes());
        self.phase("after-vkQueueWaitIdle-success", "Ok(())");
    }
}
