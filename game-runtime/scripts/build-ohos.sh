#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/env.sh"
cd "$MG_ROOT"
mkdir -p artifacts logs
cargo build --locked --lib --release --target aarch64-unknown-linux-ohos -p muguet-game-runtime 2>&1 | tee logs/build-ohos.log
"$OHOS_SDK_NATIVE/llvm/bin/llvm-objcopy" --remove-section=.llvmbc --remove-section=.llvmcmd target/aarch64-unknown-linux-ohos/release/libmuguet_game_runtime.a artifacts/libmuguet_game_runtime.a
"$OHOS_SDK_NATIVE/llvm/bin/llvm-nm" --defined-only artifacts/libmuguet_game_runtime.a 2>logs/nm-diagnostics.txt | rg ' mg_(game_|wgpu_atlas_probe_json|depth_downsample_probe_json|luminance_probe_json|readback_pool_probe_json|wgpu_spatial_probe_json)' > logs/exported-symbols.txt
