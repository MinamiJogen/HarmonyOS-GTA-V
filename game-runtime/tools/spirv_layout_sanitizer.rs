//! Conservative SPIR-V ArrayStride legalization, prepared offline.
//!
//! This file is deliberately not part of the production runtime module tree.
//! Layout-bearing buffer types are preserved. Types shared with explicit or
//! unknown storage classes are reported and left unchanged, never guessed.
use std::collections::{BTreeMap, BTreeSet};

const MAGIC: u32 = 0x0723_0203;
const OP_TYPE_ARRAY: u16 = 28;
const OP_TYPE_RUNTIME_ARRAY: u16 = 29;
const OP_TYPE_STRUCT: u16 = 30;
const OP_TYPE_POINTER: u16 = 32;
const OP_DECORATE: u16 = 71;
const ARRAY_STRIDE: u32 = 6;
const BLOCK: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayStrideRecord {
    pub type_id: u32,
    pub stride: u32,
    pub storage_classes: Vec<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub removed: Vec<ArrayStrideRecord>,
    pub retained_mixed: Vec<ArrayStrideRecord>,
    pub retained_other: Vec<ArrayStrideRecord>,
}

/// Remove only ArrayStride decorations on array types used exclusively by
/// non-Block Workgroup, or (SPIR-V > 1.4) Private/Function pointer roots.
/// All instructions other than those complete four-word decorations remain
/// byte-for-byte identical; no IDs, interfaces, arithmetic or bindings change.
pub fn sanitize_array_stride(words: &[u32]) -> Result<(Vec<u32>, Report), String> {
    if words.len() < 5 || words[0] != MAGIC {
        return Err("missing little-endian SPIR-V header".into());
    }
    let version = words[1];
    if version >> 16 != 1 || version & 0xff != 0 || words[4] != 0 {
        return Err("unsupported SPIR-V header version/schema".into());
    }
    let mut instructions = Vec::new();
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut arrays = BTreeSet::new();
    let mut pointers = Vec::new();
    let mut blocks = BTreeSet::new();
    let mut decorations = Vec::new();
    let mut offset = 5;
    while offset < words.len() {
        let count = (words[offset] >> 16) as usize;
        let opcode = words[offset] as u16;
        let end = offset.checked_add(count).ok_or("instruction range overflow")?;
        if count == 0 || end > words.len() {
            return Err(format!("invalid instruction length at word {offset}"));
        }
        let w = &words[offset..end];
        let minimum = match opcode {
            OP_TYPE_ARRAY => 4,
            OP_TYPE_RUNTIME_ARRAY | OP_TYPE_STRUCT => 3,
            OP_TYPE_POINTER => 4,
            OP_DECORATE => 3,
            _ => 1,
        };
        // An empty OpTypeStruct is legal (two words).
        if count < minimum && !(opcode == OP_TYPE_STRUCT && count == 2) {
            return Err(format!("truncated opcode {opcode} at word {offset}"));
        }
        match opcode {
            OP_TYPE_ARRAY | OP_TYPE_RUNTIME_ARRAY => {
                arrays.insert(w[1]);
                children.insert(w[1], vec![w[2]]);
            }
            OP_TYPE_STRUCT => { children.insert(w[1], w[2..].to_vec()); }
            OP_TYPE_POINTER => { pointers.push((w[2], w[3])); }
            OP_DECORATE if w[2] == BLOCK => { blocks.insert(w[1]); }
            OP_DECORATE if w[2] == ARRAY_STRIDE => {
                // Never drop a malformed instruction with extra operands.
                if count != 4 { return Err(format!("malformed ArrayStride at word {offset}")); }
                decorations.push((w[1], w[3]));
            }
            _ => {}
        }
        instructions.push((offset, end, opcode));
        offset = end;
    }
    let mut storage: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
    let mut explicit_workgroup = BTreeSet::new();
    for (class, root) in pointers {
        let mut reached = BTreeSet::new();
        let mut pending = vec![root];
        while let Some(id) = pending.pop() {
            if !reached.insert(id) { continue; }
            if let Some(members) = children.get(&id) { pending.extend(members); }
        }
        // Conservatively preserve even unusual nested Block types.
        if class == 4 && reached.iter().any(|id| blocks.contains(id)) {
            explicit_workgroup.extend(reached.iter().copied());
        }
        for id in reached { storage.entry(id).or_default().insert(class); }
    }
    let is_non_explicit = |class: &u32| *class == 4 || (version > 0x0001_0400 && matches!(*class, 6 | 7));
    let mut removable = BTreeSet::new();
    let mut report = Report::default();
    for (id, stride) in decorations {
        let uses = storage.get(&id).cloned().unwrap_or_default();
        let record = ArrayStrideRecord { type_id: id, stride, storage_classes: uses.iter().copied().collect() };
        let has_non_explicit = uses.iter().any(is_non_explicit);
        let exclusively_non_explicit = !uses.is_empty() && uses.iter().all(is_non_explicit);
        if arrays.contains(&id) && exclusively_non_explicit && !explicit_workgroup.contains(&id) {
            removable.insert(id);
            report.removed.push(record);
        } else if has_non_explicit && (!exclusively_non_explicit || explicit_workgroup.contains(&id)) {
            report.retained_mixed.push(record);
        } else {
            report.retained_other.push(record);
        }
    }
    let mut output = Vec::with_capacity(words.len());
    output.extend_from_slice(&words[..5]);
    for (start, end, opcode) in instructions {
        let w = &words[start..end];
        if opcode == OP_DECORATE && w[2] == ARRAY_STRIDE && removable.contains(&w[1]) { continue; }
        output.extend_from_slice(w);
    }
    Ok((output, report))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn module(version: u32, classes: &[u32], block: bool) -> Vec<u32> {
        let mut w=vec![MAGIC,version,0,100,0];
        w.extend_from_slice(&[(4<<16)|71,3,6,16]);
        if block { w.extend_from_slice(&[(3<<16)|71,4,2]); }
        w.extend_from_slice(&[(4<<16)|28,3,1,2,(3<<16)|30,4,3]);
        for (i,class) in classes.iter().enumerate() { w.extend_from_slice(&[(4<<16)|32,10+i as u32,*class,4]); }
        w
    }
    #[test] fn removes_only_complete_decoration() {
        for class in [4,6,7] {
            let input=module(0x0001_0600,&[class],false);
            let (out, report)=sanitize_array_stride(&input).unwrap();
            assert_eq!(report.removed.len(),1);
            let mut expected=input[..5].to_vec();expected.extend_from_slice(&input[9..]);
            assert_eq!(out,expected);
        }
    }
    #[test] fn explicit_nested_buffer_layout_is_identical() {
        for class in [2,9,12,5343,5349] {
            let input=module(0x0001_0600,&[class],false);
            let (out,report)=sanitize_array_stride(&input).unwrap();
            assert_eq!(out,input);assert!(report.removed.is_empty());
        }
    }
    #[test] fn mixed_storage_preserves_layout_and_reports() {
        let input=module(0x0001_0600,&[6,2],false);
        let (out,report)=sanitize_array_stride(&input).unwrap();
        assert_eq!(out,input);assert!(report.removed.is_empty());assert_eq!(report.retained_mixed.len(),1);
        assert_eq!(report.retained_mixed[0].storage_classes,vec![2,6]);
    }
    #[test] fn block_workgroup_is_preserved() {
        let input=module(0x0001_0600,&[4],true);
        let (out,report)=sanitize_array_stride(&input).unwrap();
        assert_eq!(out,input);assert!(report.removed.is_empty());
    }
    #[test] fn pre_1_5_legal_private_and_function_are_preserved() {
        for class in [6,7] {
            let input=module(0x0001_0400,&[class],false);
            let (out,report)=sanitize_array_stride(&input).unwrap();
            assert_eq!(out,input);assert!(report.removed.is_empty());
        }
    }
    #[test] fn malformed_input_is_rejected_without_panic() {
        assert!(sanitize_array_stride(&[]).is_err());
        let mut input=module(0x0001_0600,&[6],false);input.push(0);
        assert!(sanitize_array_stride(&input).is_err());
        input.pop();input.truncate(input.len()-1);assert!(sanitize_array_stride(&input).is_err());
        let bad=vec![MAGIC,0x0001_0600,0,100,0,(3<<16)|71,3,6];
        assert!(sanitize_array_stride(&bad).is_err());
    }
}
