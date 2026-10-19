//! One pinned shader's race-free shared-memory reduction. Not a generic atomic rewrite.
use regex::{Captures, Regex};
use sha2::{Digest, Sha256};

pub const LABEL: &str = "tiled_lighting::DepthDownsampleCS_Default";
pub const SOURCE_SHA256: &str = "6abc853190922d4056100d837f304dd8f380e88c12da12dc0c72c632fbaba768";
pub const CONSTANTS: [u32; 6] = [1, 4, 8, 12, 16, 128];
pub const VARIANT: &str = "shared-u32-unrolled-v2";
pub const SHADER_FILE: &str = "23a7a3c1c0d38f46.wgsl";

pub fn requested() -> bool {
    std::env::var_os("MUGUET_DEPTH_REDUCE_OPT").is_some_and(|v| v == "1")
}

/// Only a source-verified complete writer can opt out of automatic shared zeros.
/// The target still writes all 1536 words and retains its first barrier before
/// any shared read. Other shader stages keep wgpu's default initialization.
pub fn zero_initialize_workgroup_memory(verified_full_write: bool) -> bool {
    !verified_full_write
        || !std::env::var_os("MUGUET_DEPTH_REDUCE_NO_ZERO").is_some_and(|v| v == "1")
}

pub fn sha256(source: &str) -> String { format!("{:x}", Sha256::digest(source.as_bytes())) }

pub fn rejection_reason(label: &str, source: &str, consts: &[u32]) -> Option<&'static str> {
    if label != LABEL { Some("pipeline-label-mismatch") }
    else if consts != CONSTANTS { Some("binding15-constants-mismatch-or-unreadable") }
    else if sha256(source) != SOURCE_SHA256 { Some("original-source-sha256-mismatch") }
    else { None }
}

/// Return None unless the complete source, label and immutable binding15 values match.
/// Keep dispatch, floating-point expressions, image operations and guards.
/// Each lane writes its own six words; at step s only lanes i<s update their
/// record, reading only records i+s in the non-writing half. Workgroup barriers
/// separate all phases. Consequently these particular atomics have no competing
/// readers/writers during an access and ordinary u32 memory operations suffice.
pub fn rewrite(label: &str, source: &str, consts: &[u32]) -> Result<Option<String>, String> {
    if rejection_reason(label, source, consts).is_some() {
        return Ok(None);
    }
    let declaration = "array<atomic<u32>, 1536u>";
    if source.matches(declaration).count() != 1 { return Err("pinned Workgroup declaration mismatch".into()); }
    let mut out = source.replace(declaration, "array<u32, 1536u>");
    // The SHA pin bounds these patterns to known side-effect-free index/value
    // expressions. No arbitrary shader or pointer is accepted by this transform.
    for (pattern, expected, op) in [
        (r"atomicLoad\(&\(g0\[(.*?)\]\)\)", 12, "load"),
        (r"atomicStore\(&\(g0\[(.*?)\]\), (.*?)\);", 7, "store"),
        (r"_ = atomicMin\(&\(g0\[(.*?)\]\), (.*?)\);", 2, "min"),
        (r"_ = atomicMax\(&\(g0\[(.*?)\]\), (.*?)\);", 3, "max"),
    ] {
        let re = Regex::new(pattern).map_err(|e| e.to_string())?;
        if re.captures_iter(&out).count() != expected { return Err(format!("pinned {op} count mismatch")); }
        out = re.replace_all(&out, |c: &Captures<'_>| match op {
            "load" => format!("(g0[{}])", &c[1]),
            "store" => format!("g0[{}] = ({});", &c[1], &c[2]),
            _ => format!("g0[{}] = {op}(g0[{}], {});", &c[1], &c[1], &c[2]),
        }).into_owned();
    }
    if out.contains("atomic") || out.matches("workgroupBarrier();").count() != 2
        || out.matches("textureLoad(").count() != 2 || out.matches("textureStore(").count() != 1 {
        return Err("pinned transformation postcondition failed".into());
    }
    // The only six values in this immutable uniform are known above. The first
    // is a bit-pattern count seed; the remaining values are byte offsets and
    // the initial reduction stride, not floating-point arithmetic constants.
    let seed = "bitcast<f32>(v.tint_symbol_2[0i].x)";
    if out.matches(seed).count() != 1 { return Err("pinned count seed mismatch".into()); }
    out = out.replace(seed, "bitcast<f32>(1u)");
    let start = out.find("  let v_34 =").ok_or("pinned reduction start missing")?;
    let end = out.find("  if ((bitcast<u32>(v_7) != 0u)) {").ok_or("pinned reduction end missing")?;
    if start >= end { return Err("pinned reduction window order mismatch".into()); }
    let mut reduction = String::from("  // Exact fixed-constant reduction: each barrier remains unconditional.\n");
    for stride in [128u32, 64, 32, 16, 8, 4, 2, 1] {
        reduction.push_str(&format!("  if (reduce_lane < {stride}u) {{\n    let reduce_src = (reduce_lane + {stride}u) * 6u;\n"));
        for (field, op) in [(0, "min"), (1, "max"), (2, "max"), (3, "max"), (4, "min")] {
            reduction.push_str(&format!("    g0[reduce_base + {field}u] = {op}(g0[reduce_base + {field}u], g0[reduce_src + {field}u]);\n"));
        }
        // Preserve the original signed-bitcast sum, including wrapping bits.
        reduction.push_str("    g0[reduce_base + 5u] = bitcast<u32>((bitcast<i32>(g0[reduce_base + 5u]) + bitcast<i32>(g0[reduce_src + 5u])));\n  }\n  workgroupBarrier();\n");
    }
    // The unchanged floating-point post-processing tests this lane carrier
    // before overwriting it. Other old offset/stride carriers are overwritten
    // before any observable use in that tail.
    reduction.push_str("  v_7 = bitcast<f32>(reduce_lane);\n");
    out.replace_range(start..end, &reduction);
    let lane_anchor = "  let v_2 = vec3<i32>(vThreadGroupID);";
    if out.matches(lane_anchor).count() != 1 { return Err("pinned lane anchor mismatch".into()); }
    out = out.replace(lane_anchor, &format!("{lane_anchor}\n  let reduce_lane = vThreadIDInGroup.y * 16u + vThreadIDInGroup.x;\n  let reduce_base = reduce_lane * 6u;"));
    // The initial six stores already have the same one-lane ownership proof.
    // Express their indices directly rather than through an f32 bit carrier.
    let old_base = "bitcast<u32>((bitcast<i32>(v_3) * 6i))";
    if out.matches(old_base).count() != 2 { return Err("pinned initial base count mismatch".into()); }
    out = out.replace(old_base, "reduce_base");
    if out.matches("workgroupBarrier();").count() != 9 || out.contains("loop {")
        || out.contains("v.tint_symbol_2[") || out.contains("atomic") {
        return Err("unrolled reduction postcondition failed".into());
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires MUGUET_DEPTH_SHADER_FIXTURE pointing to your private pinned WGSL"]
    fn only_the_exact_supplied_source_and_constants_are_accepted() {
        let path = std::env::var_os("MUGUET_DEPTH_SHADER_FIXTURE")
            .expect("set MUGUET_DEPTH_SHADER_FIXTURE to your private pinned WGSL");
        let original = std::fs::read_to_string(path).expect("read private depth shader fixture");
        let original_source = original.as_str();
        let optimized = rewrite(LABEL, original_source, &CONSTANTS).unwrap().unwrap();
        assert!(!optimized.contains("atomic"));
        assert!(optimized.contains("g0[reduce_base + 0u] = min(g0[reduce_base + 0u], g0[reduce_src + 0u]);"));
        assert_eq!(optimized.matches("bitcast<i32>(g0[reduce_base + 5u]) + bitcast<i32>(g0[reduce_src + 5u])").count(),8);
        assert_eq!(optimized.matches("workgroupBarrier();").count(),9);
        assert!(!optimized.contains("loop {"));
        assert!(!optimized.contains("v.tint_symbol_2["));
        assert!(optimized.contains("@binding(15u) var<uniform> v : tint_symbol_3;"));
        for name in ["textureLoad(", "textureStore(", "@workgroup_size(16u, 16u, 1u)"] {
            assert_eq!(optimized.matches(name).count(), original_source.matches(name).count());
        }
        assert!(rewrite("tiled_lightingMS::DepthDownsampleCS_Default", original_source, &CONSTANTS).unwrap().is_none());
        assert!(rewrite(LABEL, &(original_source.to_owned()+" "), &CONSTANTS).unwrap().is_none());
        assert!(rewrite(LABEL, original_source, &[1,4,8,12,16,64]).unwrap().is_none());
        assert!(rewrite(LABEL, &optimized, &CONSTANTS).unwrap().is_none());
    }
    #[test]
    fn untrusted_sources_are_rejected_without_private_fixtures() {
        for source in ["", "@compute @workgroup_size(1) fn main() {}"] {
            assert_eq!(rejection_reason(LABEL, source, &CONSTANTS), Some("original-source-sha256-mismatch"));
            assert!(rewrite(LABEL, source, &CONSTANTS).unwrap().is_none());
            assert_eq!(rejection_reason("other", source, &CONSTANTS), Some("pipeline-label-mismatch"));
            assert_eq!(rejection_reason(LABEL, source, &[]), Some("binding15-constants-mismatch-or-unreadable"));
        }
    }
    #[test]
    fn workgroup_phases_have_no_aliasing_or_concurrent_ownership() {
        let mut init = std::collections::BTreeSet::new();
        for i in 0..256 { for c in 0..6 { assert!(init.insert(6*i+c)); } }
        assert_eq!(init.len(),1536); assert_eq!(init.last(),Some(&1535));
        let mut steps=0;
        let mut s=128;
        while s>0 {
            let mut reads=std::collections::BTreeSet::new();
            let mut writes=std::collections::BTreeSet::new();
            for i in 0..s {for c in 0..6 {
                assert!(writes.insert(6*i+c)); reads.insert(6*(i+s)+c);
                assert!(6*(i+s)+c<1536);
            }}
            assert!(reads.is_disjoint(&writes));steps+=1;s>>=1;
        }
        assert_eq!(steps,8); // plus the initialization barrier = nine dynamic barriers.
    }
}
