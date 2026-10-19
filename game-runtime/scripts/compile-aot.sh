#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/env.sh"
input="${1:?usage: compile-aot.sh /absolute/path/game.wasm [output.cwasm] [patched.wasm]}"
output="${2:-$MG_ROOT/artifacts/game-ohos.cwasm}"
patched="${3:-$MG_ROOT/artifacts/game-patched.wasm}"
[[ "$input" = /* ]] || input="$PWD/$input"
[[ "$output" = /* ]] || output="$PWD/$output"
[[ "$patched" = /* ]] || patched="$PWD/$patched"
cd "$MG_ROOT"
mkdir -p logs artifacts
# Keep the independent compiler lockfile. Fresh checkouts may download the
# checksum-verified dependencies; cached builds can opt into offline mode.
task_network_args=()
case "${MG_AOT_OFFLINE:-0}" in
  0) ;;
  1) task_network_args+=(--offline) ;;
  *) printf '%s\n' 'MG_AOT_OFFLINE must be 0 or 1.' >&2; exit 2 ;;
esac
cargo build --locked ${task_network_args[@]+"${task_network_args[@]}"} --release --manifest-path aot-compiler/Cargo.toml \
  --config 'source.crates-io.replace-with="rsproxy-sparse"' \
  --config 'source.rsproxy-sparse.registry="sparse+https://rsproxy.cn/index/"'
aot-compiler/target/release/muguet-ohos-aot-compiler "$input" "$output" "$patched" 2>&1 | tee logs/aot-compile-game.log
