# HarmonyOS Rust / Wasmtime probe

This probe does not require `game.zip`. It uses the official Rust target
`aarch64-unknown-linux-ohos`, the DevEco OpenHarmony native SDK, and Wasmtime
**49.0.0** pinned in `Cargo.lock`. The host compiler is Rust **1.99.1**.

## Local toolchain installation

Run this block from the repository root:

```sh
cd runtime-probe
mkdir -p .toolchain
curl -fL https://sh.rustup.rs -o .toolchain/rustup-init.sh
CARGO_HOME="$PWD/.toolchain/cargo" RUSTUP_HOME="$PWD/.toolchain/rustup" \
  sh .toolchain/rustup-init.sh -y --no-modify-path --profile minimal --default-toolchain 1.99.1
```

`rust-toolchain.toml` locks the compiler; `Cargo.lock` locks registry versions.
The local toolchain lives in `.toolchain/`, which is ignored. Set
`RP_CARGO_HOME`, `RP_RUSTUP_HOME`, or `OHOS_SDK_NATIVE` to use another installation.
The configured crates.io mirror retains Cargo registry checksum verification.

## Build

Run this block from the repository root after installing the toolchain:

```sh
cd runtime-probe
bash scripts/build-ohos.sh wasmtime
bash scripts/compile-aot.sh
bash scripts/embed-all.sh
```

`build-ohos.sh` installs the `aarch64-unknown-linux-ohos` target if needed.
Use `bash scripts/build-ohos.sh baseline` for a C ABI build without Wasmtime;
this replaces the same library output, so rebuild `wasmtime` before testing AOT.
`source scripts/env.sh` prepares a bash session for manual commands.

Rust release builds use `lto="thin"` and emit native ARM64 ELF objects.
The build then removes `.llvmbc` / `.llvmcmd` metadata from the delivery archive
using the SDK's `llvm-objcopy`; this prevents its older LLVM inspection tools
from parsing incompatible Rust LLVM metadata.

## C / NAPI integration

Include `include/harmony_runtime_probe.h` and link
`target/aarch64-unknown-linux-ohos/release/libharmony_runtime_probe.a`, plus
`dl` and `m`, into the application's shared library. `artifacts/` holds a copy
of the final static library. `artifacts/libharmony_runtime_probe-native.a` also
removes `.llvmbc` / `.llvmcmd` metadata so the older SDK inspection tools do not
try to decode LLVM 23 bitcode; all archive code members are ARM64 ELF objects.
The baseline omits Wasmtime; it reports
`rp_runtime_available()==0`, and AOT calls return status 2 rather than success.

| API | Meaning |
| --- | --- |
| `rp_add(19,23)` | Rust native FFI check, expected 42 |
| `rp_version()` | Probe version and the actual compile target |
| `rp_runtime_available()` | 1 only when the Wasmtime feature was built |
| `rp_last_error()` | Thread-local error, valid until the next probe operation |
| `rp_run_aot_add(path,19,23,&result)` | Default file-backed Wasmtime deserialize |
| `rp_run_raw_aot_add(bytes,len,19,23,&result)` | In-place load of our trusted precompiled ELF in an executable mapping |

AOT return values are 0 success, 1 invalid arguments, 2 runtime omitted,
3 Wasmtime error. A successful result must also equal 42.

## Two execution paths with different evidence

The file path uses `Module::deserialize_file`, which can request executable
mappings via mmap / mprotect. Success there alone does not prove that a signed
HAP can execute code embedded in its library.

The embedded path links `artifacts/add-embedded.o` into the HAP's signed
shared library before packaging. It contains the **complete** Wasmtime cwasm
ELF in `.text.rp_aot` with AX flags and 64 KiB alignment. Symbols are
`rp_embedded_aot_start` and `rp_embedded_aot_end`; subtract their integer
addresses to get the byte length. Pass this complete span to
`rp_run_raw_aot_add`. Its `CustomCodeMemory` hooks do not change permissions:
the ELF loader must already have mapped the containing segment RX, and the
bytes must remain unchanged and alive. This follows Wasmtime's API contract;
check the final shared library's PT_LOAD flags and test from the signed HAP.
No arbitrary downloaded or user-modified serialized bytes should reach these
unsafe deserialization interfaces.

The offline compiler makes `add`, `memory64`, `shared`, and `shared-memory64`
artifacts. The memory64 function executes an i64-addressed store and load;
the shared functions execute atomic store, RMW add, and load. They do not yet
test multiple concurrent guest threads, large memory reservations, graphics,
audio, input, or game compatibility.

Host smoke tests and shell executables are preliminary evidence. Only the
actual signed HAP on the target device can establish that application-side
execution works.

## Host controls

Run these commands inside `runtime-probe/`:

```sh
source scripts/env.sh
target/host/release/probe-aot-compiler aarch64-apple-darwin artifacts/host
cargo run --locked --offline --release --target-dir target/host \
  -p harmony-runtime-probe --example host-smoke --features wasmtime-runtime
```

The host smoke runs file deserialization and a raw loader control for all four
small functions. Direct file RX mapping may be denied by the host; the example
records its OS error and attempts an anonymous RW copy followed by RX publication.
Any host raw success is explicitly dynamic executable-memory control, not signed
HAP evidence. Inspect each case's result and error in the command output.

## API sources

- Rust OHOS target setup: https://doc.rust-lang.org/rustc/platform-support/openharmony.html
- Wasmtime 49 crate source: https://docs.rs/wasmtime/49.0.0/wasmtime/
- Wasmtime Module API: https://docs.wasmtime.dev/api/wasmtime/struct.Module.html
