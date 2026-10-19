#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/env.sh"
cd "$RP_ROOT"
mkdir -p logs artifacts target/aarch64-unknown-linux-ohos/release
rustc --version | tee logs/rustc-version.txt
rustup target add aarch64-unknown-linux-ohos
if [[ "${1:-wasmtime}" == "baseline" ]]; then
  RP_TARGET=aarch64-unknown-linux-ohos rustc "${HARMONY_RUSTC_FLAGS[@]}" --edition 2021 --crate-type staticlib --crate-name harmony_runtime_probe --target aarch64-unknown-linux-ohos -C panic=abort -C opt-level=2 -C lto=thin -C embed-bitcode=yes src/lib.rs -o target/aarch64-unknown-linux-ohos/release/libharmony_runtime_probe.a 2>&1 | tee logs/build-ohos-baseline.log
else
  cargo build --locked --release --target aarch64-unknown-linux-ohos -p harmony-runtime-probe --features wasmtime-runtime 2>&1 | tee logs/build-ohos-wasmtime.log
fi
# Remove embedded LLVM metadata after Rust emits native ELF objects. This also
# lets the SDK LLVM 15 inspection tools read an archive emitted by Rust LLVM 23.
"$OHOS_SDK_NATIVE/llvm/bin/llvm-objcopy" --remove-section=.llvmbc --remove-section=.llvmcmd target/aarch64-unknown-linux-ohos/release/libharmony_runtime_probe.a artifacts/libharmony_runtime_probe.a
cp artifacts/libharmony_runtime_probe.a artifacts/libharmony_runtime_probe-native.a
"$OHOS_SDK_NATIVE/llvm/bin/llvm-nm" --defined-only artifacts/libharmony_runtime_probe.a 2>logs/archive-nm-diagnostics.txt | rg ' (rp_add|rp_version|rp_last_error|rp_runtime_available|rp_run_aot_add|rp_run_raw_aot_add)$' | tee logs/exported-symbols.txt
