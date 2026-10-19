//! Keep guest vertex slots separate from native pipeline binding numbers.

use std::collections::BTreeSet;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub(crate) struct Plan {
    pub sequence: u64,
    pub compact: bool,
    pub original: Vec<u32>,
    pub active: Vec<u32>,
    /// Entries follow the original layout order, preserving resource visits.
    pub mapping: Vec<(u32, u32)>,
}

impl Plan {
    pub fn new(original: &[u32], active: &[u32], compact: bool) -> Result<Self, &'static str> {
        let originals: BTreeSet<u32> = original.iter().copied().collect();
        let actives: BTreeSet<u32> = active.iter().copied().collect();
        if originals.len() != original.len() || actives.len() != active.len() {
            return Err("duplicate vertex buffer slot");
        }
        if originals.iter().any(|&slot| slot >= 16) || !actives.is_subset(&originals) {
            return Err("invalid vertex buffer slot set");
        }
        let ordered: Vec<u32> = actives.iter().chain(originals.difference(&actives)).copied().collect();
        let mapping = original.iter().map(|&slot| {
            let binding = if compact {
                ordered.iter().position(|&source| source == slot).unwrap() as u32
            } else {
                slot
            };
            (slot, binding)
        }).collect();
        Ok(Self { sequence: 0, compact, original: original.to_vec(), active: actives.into_iter().collect(), mapping })
    }

    pub fn binding(&self, original: u32) -> Option<u32> {
        self.mapping.iter().find_map(|&(source, binding)| (source == original).then_some(binding))
    }
}

const MAX_PIPELINES: u32 = 512;
const MAX_BINDS: u32 = 1024;
const MAX_BYTES: u64 = 1024 * 1024;

struct Trace {
    file: Option<std::fs::File>,
    pipelines: u32,
    binds: u32,
    bytes: u64,
    pipeline_truncated: bool,
    bind_truncated: bool,
    byte_truncated: bool,
}

impl Trace {
    fn new() -> Self {
        let file = std::env::var_os("MUGUET_SUPPORT").and_then(|root| {
            std::fs::OpenOptions::new().write(true).create(true).truncate(true)
                .open(std::path::Path::new(&root).join("gpu-vertex-bindings.txt")).ok()
        });
        let mut trace = Self { file, pipelines: 0, binds: 0, bytes: 0, pipeline_truncated: false, bind_truncated: false, byte_truncated: false };
        trace.write(&format!("vertex-bindings pid={} maxPipelines={MAX_PIPELINES} maxBinds={MAX_BINDS} maxBytes={MAX_BYTES} records=before-wgpu-call\n", std::process::id()));
        trace
    }

    fn write(&mut self, record: &str) {
        if let Some(file) = self.file.as_mut() {
            if file.write_all(record.as_bytes()).is_ok() {
                self.bytes += record.len() as u64;
                let _ = file.sync_all();
            }
        }
    }

    fn record(&mut self, pipeline: bool, record: String) {
        if self.file.is_none() || self.byte_truncated { return; }
        let (count, limit, truncated) = if pipeline {
            (&mut self.pipelines, MAX_PIPELINES, &mut self.pipeline_truncated)
        } else {
            (&mut self.binds, MAX_BINDS, &mut self.bind_truncated)
        };
        if *count >= limit {
            if !*truncated {
                *truncated = true;
                self.write(&format!("TRUNCATED class={} limit={limit}\n", if pipeline { "pipeline" } else { "bind" }));
            }
            return;
        }
        *count += 1;
        // Leave room for explicit truncation records.
        if self.bytes + record.len() as u64 + 256 > MAX_BYTES {
            self.byte_truncated = true;
            self.write("TRUNCATED class=bytes\n");
            return;
        }
        self.write(&record);
    }
}

fn trace() -> Option<&'static Mutex<Trace>> {
    if std::env::var("MUGUET_GPU_DIAGNOSTICS").as_deref() != Ok("1") { return None; }
    static TRACE: OnceLock<Mutex<Trace>> = OnceLock::new();
    Some(TRACE.get_or_init(|| Mutex::new(Trace::new())))
}

pub(crate) fn trace_pipeline(plan: &Plan, label: &str, actual_active_bindings: &[u32]) {
    let Some(trace) = trace() else { return; };
    let label: String = label.chars().take(192).collect();
    trace.lock().unwrap().record(true, format!("pipeline sequence={} compact={} label={label:?} original={:?} active={:?} originalToPhysical={:?} pipelinePhysical={actual_active_bindings:?}\n", plan.sequence, plan.compact, plan.original, plan.active, plan.mapping));
}

pub(crate) fn trace_bind(plan: &Plan, pipeline: u64, original: u32, physical: u32, buffer: u64, offset: u64, issued: bool) {
    let Some(trace) = trace() else { return; };
    trace.lock().unwrap().record(false, format!("bind sequence={} pipeline={pipeline} original={original} physical={physical} buffer={buffer} offset={offset} issued={issued}\n", plan.sequence));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_active_prefix_and_unused_reference_suffix() {
        let p = Plan::new(&[0, 1, 2, 7], &[2, 7], true).unwrap();
        assert_eq!(p.mapping, vec![(0, 2), (1, 3), (2, 0), (7, 1)]);
        assert_eq!(p.active, vec![2, 7]);
    }

    #[test]
    fn preserves_original_resource_visit_order_and_noncompact_behavior() {
        let p = Plan::new(&[7, 0, 2], &[2, 7], true).unwrap();
        assert_eq!(p.mapping, vec![(7, 1), (0, 2), (2, 0)]);
        let original = Plan::new(&[7, 0, 2], &[2, 7], false).unwrap();
        assert_eq!(original.mapping, vec![(7, 7), (0, 0), (2, 2)]);
    }

    #[test]
    fn empty_and_all_unused_layouts_still_preserve_bindings() {
        assert!(Plan::new(&[], &[], true).unwrap().mapping.is_empty());
        assert_eq!(Plan::new(&[4, 9], &[], true).unwrap().mapping, vec![(4, 0), (9, 1)]);
    }

    #[test]
    fn rejects_invalid_mapping_inputs() {
        assert!(Plan::new(&[0, 0], &[0], true).is_err());
        assert!(Plan::new(&[0], &[0, 0], true).is_err());
        assert!(Plan::new(&[0], &[1], true).is_err());
        assert!(Plan::new(&[16], &[], true).is_err());
    }

    #[test]
    fn exhaustive_slot_combinations_preserve_source_and_dense_active_bindings() {
        for mut state in 0..3_u32.pow(8) {
            let mut original = Vec::new();
            let mut active = Vec::new();
            for slot in 0..8 {
                let kind = state % 3;
                state /= 3;
                if kind != 0 { original.push(slot); }
                if kind == 2 { active.push(slot); }
            }
            let p = Plan::new(&original, &active, true).unwrap();
            let physical: BTreeSet<_> = p.mapping.iter().map(|&(_, binding)| binding).collect();
            assert_eq!(physical, (0..original.len() as u32).collect());
            assert_eq!(active.iter().map(|&slot| p.binding(slot).unwrap()).collect::<Vec<_>>(), (0..active.len() as u32).collect::<Vec<_>>());
            let source_values: Vec<_> = original.iter().map(|&slot| (slot, slot * 37 + 11)).collect();
            let mut bound = vec![0; original.len()];
            for &(slot, value) in &source_values { bound[p.binding(slot).unwrap() as usize] = value; }
            for &(slot, value) in &source_values { assert_eq!(bound[p.binding(slot).unwrap() as usize], value); }
        }
    }

    #[test]
    fn cache_is_indexed_by_physical_binding_across_pipeline_switches() {
        let a = Plan::new(&[0, 1, 2], &[0, 2], true).unwrap();
        let b = Plan::new(&[0, 1, 2], &[1, 2], true).unwrap();
        let buffers = [100, 200, 300];
        let mut cache = [None; 3];
        for plan in [&a, &b, &a] {
            for &(original, physical) in &plan.mapping {
                let buffer = buffers[original as usize];
                if cache[physical as usize] != Some(buffer) { cache[physical as usize] = Some(buffer); }
            }
            for &slot in &plan.active { assert_eq!(cache[plan.binding(slot).unwrap() as usize], Some(buffers[slot as usize])); }
        }
    }
}
