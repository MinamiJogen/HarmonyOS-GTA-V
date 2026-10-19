#!/usr/bin/env bash
# Source this file from bash, or let build-ohos.sh source it.
RP_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export CARGO_HOME="${RP_CARGO_HOME:-$RP_ROOT/.toolchain/cargo}"
export RUSTUP_HOME="${RP_RUSTUP_HOME:-$RP_ROOT/.toolchain/rustup}"
source "$RP_ROOT/../tools/rust_path_privacy.sh"
task_configure_rust_path_privacy "$(cd "$RP_ROOT/.." && pwd -P)"
export PATH="$CARGO_HOME/bin:$PATH"
export OHOS_SDK_NATIVE="${OHOS_SDK_NATIVE:-/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native}"
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_OHOS_LINKER="$RP_ROOT/scripts/ohos-clang.sh"
export CC_aarch64_unknown_linux_ohos="$RP_ROOT/scripts/ohos-clang.sh"
export AR_aarch64_unknown_linux_ohos="$OHOS_SDK_NATIVE/llvm/bin/llvm-ar"
export CXX_aarch64_unknown_linux_ohos="$RP_ROOT/scripts/ohos-clangxx.sh"
