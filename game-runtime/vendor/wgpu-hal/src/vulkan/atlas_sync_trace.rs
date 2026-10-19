//! Targeted continuation beyond the ordinary early-image trace budget.
//! This records API arguments and binding state; it never changes GPU work.
use alloc::{collections::{BTreeMap, BTreeSet}, format, string::String, vec::Vec};
use core::fmt;
use std::{fs::File, io::Write, sync::{Mutex, OnceLock}};

const MAX_EVENTS: u64 = 16_384;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_COMMANDS: usize = 4096;
const MAX_ATTACHMENTS: usize = 256;

#[derive(Clone, Copy)]
struct Allocation { image: u64, memory: u64, offset: u64, size: u64 }
#[derive(Default)]
struct Command { epoch: u64, generation: u64, pipeline: u64, groups: BTreeMap<u32, u64>, touched: bool }
struct Trace {
    file: File, events: u64, bytes: usize, truncated: bool,
    image: u64, memory: u64, offset: u64, size: u64,
    commands: BTreeMap<u64, Command>, command_epoch: u64,
    target_sets: BTreeSet<u64>, attachments: Vec<Allocation>, attachment_overflow: bool,
    pending_descriptors: Vec<(u64, u64, String)>, descriptor_overflow: bool,
}
static TRACE: OnceLock<Option<Mutex<Trace>>> = OnceLock::new();

fn with_trace(f: impl FnOnce(&mut Trace)) {
    let trace = TRACE.get_or_init(|| {
        let path = std::env::var_os("MUGUET_ATLAS_SYNC_TRACE")?;
        let mut file = match File::create(path) {
            Ok(file) => file,
            Err(error) => { std::eprintln!("Atlas sync trace open failed: {error}"); return None; }
        };
        let header = format!("atlasContinuation pid={} maxEvents={MAX_EVENTS} maxBytes={MAX_BYTES} selector=first-Rgba8Unorm-full-2048x1024-mip0-layer0-copy operationRecords=before-driver-call\n", std::process::id());
        if file.write_all(header.as_bytes()).and_then(|_| file.sync_all()).is_err() { return None; }
        Some(Mutex::new(Trace { file, events: 0, bytes: header.len(), truncated: false,
            image: 0, memory: 0, offset: 0, size: 0, commands: BTreeMap::new(), command_epoch: 0,
            target_sets: BTreeSet::new(), attachments: Vec::new(), attachment_overflow: false,
            pending_descriptors: Vec::new(), descriptor_overflow: false }))
    });
    if let Some(trace) = trace {
        if let Ok(mut trace) = trace.lock() { if !trace.truncated { f(&mut trace); } }
    }
}

impl Trace {
    fn write(&mut self, args: fmt::Arguments<'_>) {
        if self.truncated { return; }
        let next = self.events + 1;
        let line = format!("event={next} {args}\n");
        if next > MAX_EVENTS || self.bytes.saturating_add(line.len()) > MAX_BYTES - 256 {
            let marker = format!("TRUNCATED=true retainedEvents={} retainedBytes={} reason=atlas-event-or-byte-budget\n", self.events, self.bytes);
            let _ = self.file.write_all(marker.as_bytes()).and_then(|_| self.file.sync_all());
            self.truncated = true;
            return;
        }
        if let Err(error) = self.file.write_all(line.as_bytes()).and_then(|_| self.file.sync_all()) {
            std::eprintln!("Atlas sync trace write failed: {error}"); self.truncated = true; return;
        }
        self.events = next; self.bytes += line.len();
    }
    fn context(&mut self, cmd: u64) -> (u64, u64) {
        if let Some(command) = self.commands.get_mut(&cmd) { command.touched = true; (command.epoch, command.generation) }
        else { (0, 0) }
    }
    fn attachment(&mut self, allocation: Allocation) {
        if allocation.memory != self.memory || self.image == 0 { return; }
        let overlap = allocation.offset < self.offset.saturating_add(self.size)
            && self.offset < allocation.offset.saturating_add(allocation.size);
        self.write(format_args!("attachment-allocation image={:#x} memory={:#x} offset={} size={} overlapsAtlas={overlap}", allocation.image, allocation.memory, allocation.offset, allocation.size));
    }
}

pub(super) fn begin_command(cmd: u64, generation: u64) {
    with_trace(|trace| {
        trace.command_epoch += 1;
        let epoch = trace.command_epoch;
        if trace.commands.len() >= MAX_COMMANDS && !trace.commands.contains_key(&cmd) {
            trace.write(format_args!("TRACKING_TRUNCATED=true reason=command-registry-cap")); trace.truncated = true; return;
        }
        trace.commands.insert(cmd, Command { epoch, generation, ..Default::default() });
        if trace.image != 0 { trace.write(format_args!("command-begin cmd={cmd:#x} epoch={epoch} poolGeneration={generation} bindingsCleared=true")); }
    });
}

pub(super) fn attachment_allocation(image: u64, memory: u64, offset: u64, size: u64) {
    with_trace(|trace| {
        let allocation = Allocation { image, memory, offset, size };
        if trace.image != 0 { trace.attachment(allocation); }
        else if trace.attachments.len() < MAX_ATTACHMENTS { trace.attachments.push(allocation); }
        else { trace.attachment_overflow = true; }
    });
}

pub(super) fn select_image(image: u64, memory: u64, offset: u64, size: u64) {
    with_trace(|trace| {
        if trace.image != 0 { return; }
        trace.image = image; trace.memory = memory; trace.offset = offset; trace.size = size;
        trace.write(format_args!("atlas-selected image={image:#x} memory={memory:#x} offset={offset} allocatedSize={size}"));
        if trace.attachment_overflow { trace.write(format_args!("ATTACHMENT_REGISTRY_TRUNCATED=true maxAttachments={MAX_ATTACHMENTS}")); }
        for allocation in core::mem::take(&mut trace.attachments) { trace.attachment(allocation); }
        if trace.descriptor_overflow { trace.write(format_args!("DESCRIPTOR_REGISTRY_TRUNCATED=true maxDescriptors={MAX_ATTACHMENTS}")); }
        for (set, bound_image, details) in core::mem::take(&mut trace.pending_descriptors) {
            if bound_image == image {
                trace.target_sets.insert(set);
                trace.write(format_args!("atlas-descriptor set={set:#x} image={image:#x} createdBeforeSelection=true {details}"));
            }
        }
    });
}

pub(super) fn image_event(image: u64, cmd: u64, args: fmt::Arguments<'_>) {
    with_trace(|trace| {
        if image != trace.image || image == 0 { return; }
        let (epoch, generation) = trace.context(cmd);
        trace.write(format_args!("atlas-image cmd={cmd:#x} epoch={epoch} poolGeneration={generation} image={image:#x} {args}"));
    });
}

pub(super) fn reset_set(set: u64) {
    with_trace(|trace| {
        trace.pending_descriptors.retain(|(old_set, _, _)| *old_set != set);
        if trace.target_sets.remove(&set) { trace.write(format_args!("descriptor-set-reallocated set={set:#x} priorAtlasBindingsRemoved=true")); }
    });
}

pub(super) fn descriptor_image(set: u64, image: u64, args: fmt::Arguments<'_>) {
    with_trace(|trace| {
        if trace.image == 0 {
            if trace.pending_descriptors.len() < MAX_ATTACHMENTS { trace.pending_descriptors.push((set, image, format!("{args}"))); }
            else { trace.descriptor_overflow = true; }
            return;
        }
        if image != trace.image || image == 0 { return; }
        trace.target_sets.insert(set);
        trace.write(format_args!("atlas-descriptor set={set:#x} image={image:#x} {args}"));
    });
}

pub(super) fn bind_group(cmd: u64, index: u32, set: u64, graphics: bool) {
    if !graphics { return; }
    with_trace(|trace| {
        let target = trace.target_sets.contains(&set);
        let Some(command) = trace.commands.get_mut(&cmd) else { return; };
        let previous = command.groups.remove(&index);
        if target { command.groups.insert(index, set); }
        if target || previous.is_some() {
            command.touched = true;
            let (epoch, generation) = (command.epoch, command.generation);
            trace.write(format_args!("atlas-group-replace cmd={cmd:#x} epoch={epoch} poolGeneration={generation} groupIndex={index} previousTargetSet={previous:x?} newSet={set:#x} newSetTargetsAtlas={target}"));
        }
    });
}

pub(super) fn pipeline(cmd: u64, pipeline: u64) {
    with_trace(|trace| { if let Some(command) = trace.commands.get_mut(&cmd) { command.pipeline = pipeline; } });
}

pub(super) fn draw(cmd: u64, args: fmt::Arguments<'_>) {
    with_trace(|trace| {
        let Some(command) = trace.commands.get_mut(&cmd) else { return; };
        if command.groups.is_empty() { return; }
        command.touched = true;
        let (epoch, generation, pipeline) = (command.epoch, command.generation, command.pipeline);
        let groups = format!("{:x?}", command.groups);
        trace.write(format_args!("atlas-bound-draw cmd={cmd:#x} epoch={epoch} poolGeneration={generation} pipeline={pipeline:#x} targetGroups={groups} {args}"));
    });
}

pub(super) fn submit(sequence: u64, signal_value: u64, commands: impl Iterator<Item = u64>) {
    with_trace(|trace| {
        if trace.image == 0 { return; }
        trace.write(format_args!("submit submit={sequence} signalValue={signal_value}"));
        for (index, cmd) in commands.enumerate() {
            let Some(command) = trace.commands.get(&cmd) else { continue; };
            if command.touched {
                let (epoch, generation) = (command.epoch, command.generation);
                trace.write(format_args!("atlas-submit-command submit={sequence} index={index} cmd={cmd:#x} epoch={epoch} poolGeneration={generation}"));
            }
        }
    });
}

pub(super) fn destroy_image(image: u64) {
    with_trace(|trace| { if image == trace.image && image != 0 { trace.write(format_args!("atlas-image-destroy image={image:#x}")); } });
}
