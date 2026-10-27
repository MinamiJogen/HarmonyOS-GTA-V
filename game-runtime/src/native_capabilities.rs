//! Process-advertised CPU features only. No instruction selection or acceleration is enabled.
use serde_json::{json, Value};
use std::sync::OnceLock;

// Names/bits are from SDK aarch64-linux-ohos/bits/hwcap.h. Keep unknown
// bits in the raw hex value rather than assigning undocumented meanings.
const HWCAP_BITS: &[(u32, &str)] = &[
    (0,"HWCAP_FP"), (1,"HWCAP_ASIMD"), (2,"HWCAP_EVTSTRM"), (3,"HWCAP_AES"),
    (4,"HWCAP_PMULL"), (5,"HWCAP_SHA1"), (6,"HWCAP_SHA2"), (7,"HWCAP_CRC32"),
    (8,"HWCAP_ATOMICS"), (9,"HWCAP_FPHP"), (10,"HWCAP_ASIMDHP"), (11,"HWCAP_CPUID"),
    (12,"HWCAP_ASIMDRDM"), (13,"HWCAP_JSCVT"), (14,"HWCAP_FCMA"), (15,"HWCAP_LRCPC"),
    (16,"HWCAP_DCPOP"), (17,"HWCAP_SHA3"), (18,"HWCAP_SM3"), (19,"HWCAP_SM4"),
    (20,"HWCAP_ASIMDDP"), (21,"HWCAP_SHA512"), (22,"HWCAP_SVE"), (23,"HWCAP_ASIMDFHM"),
    (24,"HWCAP_DIT"), (25,"HWCAP_USCAT"), (26,"HWCAP_ILRCPC"), (27,"HWCAP_FLAGM"),
    (28,"HWCAP_SSBS"), (29,"HWCAP_SB"), (30,"HWCAP_PACA"), (31,"HWCAP_PACG"),
];
const HWCAP2_BITS: &[(u32, &str)] = &[
    (0,"HWCAP2_DCPODP"), (1,"HWCAP2_SVE2"), (2,"HWCAP2_SVEAES"), (3,"HWCAP2_SVEPMULL"),
    (4,"HWCAP2_SVEBITPERM"), (5,"HWCAP2_SVESHA3"), (6,"HWCAP2_SVESM4"), (7,"HWCAP2_FLAGM2"),
    (8,"HWCAP2_FRINT"), (9,"HWCAP2_SVEI8MM"), (10,"HWCAP2_SVEF32MM"), (11,"HWCAP2_SVEF64MM"),
    (12,"HWCAP2_SVEBF16"), (13,"HWCAP2_I8MM"), (14,"HWCAP2_BF16"), (15,"HWCAP2_DGH"),
    (16,"HWCAP2_RNG"), (17,"HWCAP2_BTI"), (18,"HWCAP2_MTE"),
];
const AT_HWCAP: u64 = 16;
const AT_HWCAP2: u64 = 26;

#[derive(Clone, Copy)]
struct Auxv { value: Option<u64>, errno: Option<i32> }
impl Auxv {
    fn from_result(value: u64, errno: i32) -> Self {
        if value == 0 && errno != 0 { Self { value: None, errno: Some(errno) } }
        else { Self { value: Some(value), errno: None } }
    }
    fn report(self, key: u64, bits: &[(u32, &str)]) -> Value {
        let mut features = serde_json::Map::new();
        let mut known_mask = 0u64;
        for &(bit, name) in bits {
            let mask = 1u64 << bit;
            known_mask |= mask;
            features.insert(name.into(), self.value.map(|value| value & mask != 0).into());
        }
        json!({"auxvKey":key,"available":self.value.is_some(),"errno":self.errno,
            "rawHex":self.value.map(|value|format!("0x{value:016x}")),
            "undecodedBitsHex":self.value.map(|value|format!("0x{:016x}",value & !known_mask)),
            "features":features})
    }
}

#[cfg(all(target_env = "ohos", target_arch = "aarch64"))]
fn read_auxv(key: u64) -> Auxv {
    // Signatures and key values match SDK sys/auxv.h, errno.h and elf.h.
    // Preserve this caller thread's errno even when getauxval reports ENOENT.
    unsafe extern "C" {
        fn getauxval(key: libc::c_ulong) -> libc::c_ulong;
        fn __errno_location() -> *mut libc::c_int;
    }
    unsafe {
        let errno = __errno_location();
        let previous = *errno;
        *errno = 0;
        let value = getauxval(key as libc::c_ulong) as u64;
        let error = *errno;
        *errno = previous;
        Auxv::from_result(value, error)
    }
}
fn collect() -> Value {
    #[cfg(all(target_env = "ohos", target_arch = "aarch64"))]
    let (observed, hwcap, hwcap2) = (true, read_auxv(AT_HWCAP), read_auxv(AT_HWCAP2));
    #[cfg(not(all(target_env = "ohos", target_arch = "aarch64")))]
    let (observed, hwcap, hwcap2) = (false, Auxv { value:None, errno:None }, Auxv { value:None, errno:None });
    json!({"observedInApplicationProcess":observed,"architecture":std::env::consts::ARCH,
        "source":if observed {"application getauxval(AT_HWCAP/AT_HWCAP2)"} else {"not collected on this host target"},
        "collection":"process-once cache; no shell or procfs CPU information",
        "hwcap":hwcap.report(AT_HWCAP,HWCAP_BITS),"hwcap2":hwcap2.report(AT_HWCAP2,HWCAP2_BITS),
        "decoderSource":"HarmonyOS SDK aarch64-linux-ohos/bits/hwcap.h",
        "specializedAotEnabledByProbe":false,"linxHotspotStartedByProbe":false,
        "scope":"CPU feature flags advertised to this application process only; no CPU model, clock rate, specialized AOT correctness or performance conclusion"})
}
pub(crate) fn snapshot() -> Value {
    static CAPABILITIES: OnceLock<Value> = OnceLock::new();
    CAPABILITIES.get_or_init(collect).clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_present_is_distinct_from_missing_auxv_and_unknown_is_not_false() {
        let zero = Auxv::from_result(0,0).report(AT_HWCAP2,HWCAP2_BITS);
        assert_eq!(zero["available"],true); assert_eq!(zero["rawHex"],"0x0000000000000000");
        assert_eq!(zero["features"]["HWCAP2_SVE2"],false);
        let absent = Auxv::from_result(0,2).report(AT_HWCAP2,HWCAP2_BITS);
        assert_eq!(absent["available"],false); assert_eq!(absent["errno"],2);
        assert!(absent["features"]["HWCAP2_SVE2"].is_null()); assert!(absent["rawHex"].is_null());
    }
    #[test]
    fn arm_feature_registers_are_decoded_independently_and_hex_preserves_high_bits() {
        let first = Auxv::from_result((1u64<<8)|(1u64<<22)|(1u64<<63),0).report(AT_HWCAP,HWCAP_BITS);
        let second = Auxv::from_result(1u64<<1,0).report(AT_HWCAP2,HWCAP2_BITS);
        assert_eq!(first["features"]["HWCAP_ATOMICS"],true); assert_eq!(first["features"]["HWCAP_SVE"],true);
        assert_eq!(first["features"]["HWCAP_AES"],false); assert_eq!(first["rawHex"],"0x8000000000400100");
        assert_eq!(first["undecodedBitsHex"],"0x8000000000000000");
        assert_eq!(second["features"]["HWCAP2_SVE2"],true); assert_eq!(second["features"]["HWCAP2_I8MM"],false);
    }
    #[test]
    fn host_snapshot_does_not_substitute_host_cpu_features_or_claim_acceleration() {
        let first = snapshot(); let second = snapshot(); assert_eq!(first,second);
        assert_eq!(first["specializedAotEnabledByProbe"],false); assert_eq!(first["linxHotspotStartedByProbe"],false);
        #[cfg(not(all(target_env = "ohos", target_arch = "aarch64")))]
        { assert_eq!(first["observedInApplicationProcess"],false); assert!(first["hwcap"]["rawHex"].is_null()); }
    }
}
