//! Emscripten JS proxy ABI for the supplied game.js build 8b0b5899ed.
//! The table and tagged argument format are taken from that archive's
//! proxiedFunctionTable and __emscripten_receive_on_main_thread_js.

use anyhow::{Context, Result};
use wasmtime::{Caller, Val};
use std::sync::atomic::{AtomicU64, Ordering};

static CALLS: AtomicU64 = AtomicU64::new(0);

const PROXY_TABLE: &[&str] = &[
    "proc_exit", "exit", "__pthread_create_js", "__syscall_accept4",
    "__syscall_bind", "__syscall_connect", "__syscall_fcntl64", "__syscall_fstat64",
    "__syscall_ftruncate64", "__syscall_getcwd", "__syscall_getdents64", "__syscall_getsockname",
    "__syscall_getsockopt", "__syscall_ioctl", "__syscall_listen", "__syscall_lstat64",
    "__syscall_mkdirat", "__syscall_newfstatat", "__syscall_openat", "__syscall_poll",
    "__syscall_recvfrom", "__syscall_renameat", "__syscall_rmdir", "__syscall_sendto",
    "__syscall_setsockopt", "__syscall_shutdown", "__syscall_socket", "__syscall_stat64",
    "__syscall_statfs64", "__syscall_unlinkat", "environ_get", "environ_sizes_get",
    "fd_close", "fd_pread", "fd_pwrite", "fd_read", "fd_seek", "fd_write",
];

fn decode_arguments(bytes: &[u8]) -> Result<Vec<i64>> {
    anyhow::ensure!(bytes.len() % 16 == 0, "proxy argument buffer is not a multiple of 16 bytes");
    bytes.chunks_exact(16).map(|pair| {
        let tag = u64::from_le_bytes(pair[..8].try_into().unwrap());
        let value = u64::from_le_bytes(pair[8..].try_into().unwrap());
        if tag != 0 {
            // A JavaScript BigInt contains the complete memory64 pointer or i64.
            Ok(value as i64)
        } else {
            // Every numeric argument in this build's proxy table is integral.
            let number = f64::from_bits(value);
            anyhow::ensure!(number.is_finite() && number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_992.0,
                "invalid numeric proxy argument {number}");
            Ok(number as i64)
        }
    }).collect()
}

pub(super) fn receive(caller: &mut Caller<'_, crate::HostState>, params: &[Val], results: &mut [Val]) -> Result<()> {
    anyhow::ensure!(params.len() == 7, "invalid Emscripten proxy signature");
    let function = super::int(params, 0);
    let em_asm = super::int(params, 1) as u64;
    let calling_thread = super::int(params, 2) as u64;
    let size = super::int(params, 3);
    let address = super::int(params, 4) as u64;
    let context = super::int(params, 5) as u64;
    let context_args = super::int(params, 6) as u64;
    anyhow::ensure!((0..=1024).contains(&size), "invalid Emscripten proxy argument size {size}");
    let mem = &crate::RT.get().context("proxy runtime not ready")?.mem;
    let end = address.checked_add(size as u64).context("proxy argument address overflow")?;
    anyhow::ensure!(end <= mem.data_size() as u64, "proxy argument buffer outside guest memory");
    let arguments = decode_arguments(&crate::read_bytes(mem, address, size as u64))?;
    let result = if em_asm != 0 {
        // ASM_CONSTS has the same addresses as the upstream EM_ASM handler;
        // use the decoded values directly, without inventing a signature buffer.
        anyhow::ensure!(matches!(em_asm, 7_980_558 | 7_980_673 | 7_980_768), "unknown proxied EM_ASM address {em_asm}");
        super::em_asm_values(mem, em_asm, &arguments)
    } else {
        let target = PROXY_TABLE.get(usize::try_from(function).context("negative proxy table index")?)
            .with_context(|| format!("unknown Emscripten proxy index {function}"))?;
        let values: Vec<_> = arguments.iter().copied().map(Val::I64).collect();
        let mut output = [Val::I64(0)];
        // Bind/listen are absent from this offline module's imported host ABI.
        // Match the explicit ENOSYS result of its other socket operations.
        if matches!(*target, "__syscall_bind" | "__syscall_listen") {
            output[0] = Val::I64(-52);
        } else {
            super::dispatch(caller, target, &values, &mut output, 1)?;
        }
        super::int(&output, 0)
    };
    if context != 0 {
        // All native handlers above are synchronous. Complete the Wasm proxy
        // context after the operation rather than leaving the waiting thread
        // asleep. This is the continuation used by game.js's Promise.resolve.
        super::export_call(caller, "_emscripten_run_js_on_main_thread_done", &[
            Val::I64(context as i64), Val::I64(context_args as i64), Val::F64((result as f64).to_bits()),
        ])?;
        super::set_f64(results, f64::NAN); // JavaScript's implicit undefined return.
    } else {
        super::set_f64(results, result as f64);
    }
    let call = CALLS.fetch_add(1, Ordering::Relaxed) + 1;
    if call <= 20 || call % 1000 == 0 {
        eprintln!("[host] proxy #{call} function {function} EM_ASM {em_asm} caller 0x{calling_thread:x} context 0x{context:x} -> {result}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tagged_arguments_preserve_memory64_pointers_and_numeric_integers() {
        let mut bytes = Vec::new();
        bytes.extend(1u64.to_le_bytes());
        bytes.extend(0xfedc_ba98_7654_3210u64.to_le_bytes());
        bytes.extend(0u64.to_le_bytes());
        bytes.extend((-100.0f64).to_le_bytes());
        assert_eq!(decode_arguments(&bytes).unwrap(), [0xfedc_ba98_7654_3210u64 as i64, -100]);
        assert!(decode_arguments(&bytes[..31]).is_err());
    }

    #[test]
    fn malformed_numeric_values_cannot_become_guest_pointers() {
        for number in [f64::NAN, f64::INFINITY, 1.5, 18_014_398_509_481_984.0] {
            let mut bytes = Vec::from(0u64.to_le_bytes());
            bytes.extend(number.to_le_bytes());
            assert!(decode_arguments(&bytes).is_err());
        }
    }
}
