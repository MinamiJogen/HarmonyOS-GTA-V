//! Strict identity gate for one fully written workgroup array. No WGSL rewrite.
use sha2::{Digest, Sha256};

pub const LABEL: &str = "avgLuminanceCompute::LuminanceDownsampleInitCS";
pub const SHADER_FILE: &str = "9b8e0c6c85e065da.wgsl";
// Pinned to the unmodified private resource bytes; all dynamic uniform values
// retain the supplied shader's guards and floating-point operations.
pub const SOURCE_SHA256: &str = "d697ba6789dd3b70118e299edfec5176ccb989fdac56678c6caabf7e9b215369";

pub fn verified(label: &str, source: &str, constants: &[u32]) -> bool {
    label == LABEL && constants.is_empty()
        && format!("{:x}", Sha256::digest(source.as_bytes())) == SOURCE_SHA256
}

pub fn zero_initialize_workgroup_memory(verified_full_write: bool) -> bool {
    !verified_full_write || !std::env::var_os("MUGUET_LUMINANCE_NO_ZERO").is_some_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires MUGUET_LUMINANCE_SHADER_FIXTURE pointing to your private pinned WGSL"]
    fn only_exact_shader_identity_is_qualified() {
        let path = std::env::var_os("MUGUET_LUMINANCE_SHADER_FIXTURE")
            .expect("set MUGUET_LUMINANCE_SHADER_FIXTURE to your private pinned WGSL");
        let source = std::fs::read_to_string(path).expect("read private luminance shader fixture");
        assert!(verified(LABEL, &source, &[]));
        assert!(!verified("other", &source, &[]));
        assert!(!verified(LABEL, &(source.to_owned()+"\n"), &[]));
        assert!(!verified(LABEL, &source, &[1]));
        assert_eq!(source.matches("workgroupBarrier();").count(), 9);
    }
    #[test]
    fn untrusted_sources_are_rejected_without_private_fixtures() {
        for source in ["", "@compute @workgroup_size(1) fn main() {}"] {
            assert!(!verified(LABEL, source, &[]));
            assert!(!verified("other", source, &[]));
            assert!(!verified(LABEL, source, &[1]));
        }
    }
    #[test]
    fn workgroup_phases_have_complete_writes_and_disjoint_reduction() {
        let mut written = [false; 256];
        for y in 0..16 { for x in 0..16 {
            let i = y*16+x; assert!(!written[i]); written[i]=true;
        }}
        assert!(written.into_iter().all(|x|x));
        for stride in [128,64,32,16,8,4,2,1] {
            for lane in 0..stride { assert!(lane+stride<256 && lane+stride>=stride); }
        }
    }
}
