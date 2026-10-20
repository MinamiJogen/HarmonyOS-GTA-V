#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/env.sh"
cd "$MG_ROOT"
mkdir -p artifacts logs
task_feature_args=()
# An unset variable is the normal full game. A deliberately empty variable is
# still useful for a feature-off native diagnostic, and is never inferred.
if [[ "${MUGUET_RUNTIME_FEATURES+x}" == x ]]; then
  task_features="$MUGUET_RUNTIME_FEATURES"
  task_feature_selection=explicit
else
  task_features=frame-interpolation
  task_feature_selection=default
fi
if [[ -n "$task_features" ]]; then
  task_feature_args=(--features "$task_features")
fi
printf 'Runtime features (%s): %s\n' "$task_feature_selection" "${task_features:-<none: explicit diagnostic>}"
# A failed build must not leave a previous archive's capability sidecar trusted.
rm -f artifacts/libmuguet_game_runtime.features.json
cargo build --locked --lib --release --target aarch64-unknown-linux-ohos -p muguet-game-runtime ${task_feature_args[@]+"${task_feature_args[@]}"} 2>&1 | tee logs/build-ohos.log
"$OHOS_SDK_NATIVE/llvm/bin/llvm-objcopy" --remove-section=.llvmbc --remove-section=.llvmcmd target/aarch64-unknown-linux-ohos/release/libmuguet_game_runtime.a artifacts/libmuguet_game_runtime.a
"$OHOS_SDK_NATIVE/llvm/bin/llvm-nm" --defined-only artifacts/libmuguet_game_runtime.a 2>logs/nm-diagnostics.txt | rg ' mg_(game_|wgpu_atlas_probe_json|depth_downsample_probe_json|luminance_probe_json|readback_pool_probe_json|wgpu_spatial_probe_json)' > logs/exported-symbols.txt
python3 "$MG_ROOT/../tools/verify_frame_generation_build.py" record \
  --archive artifacts/libmuguet_game_runtime.a \
  --manifest artifacts/libmuguet_game_runtime.features.json \
  --features "$task_features" --selection "$task_feature_selection"
