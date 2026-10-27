#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/env.sh"
input="${1:?usage: MG_AOT_ISA_PROFILE=generic|lse compile-aot.sh /absolute/path/game.wasm [new-output.cwasm] [new-patched.wasm]}"
profile="${MG_AOT_ISA_PROFILE:-generic}"
case "$profile" in generic|lse) ;; *) printf '%s\n' 'MG_AOT_ISA_PROFILE must be generic or lse.' >&2; exit 2 ;; esac
if [[ -n "${2:-}" ]]; then
  output="$2"
else
  mkdir -p "$MG_ROOT/artifacts/aot-candidates"
  candidate_dir="$(mktemp -d "$MG_ROOT/artifacts/aot-candidates/${profile}.XXXXXX")"
  output="$candidate_dir/game-ohos.cwasm"
fi
patched="${3:-${output%.cwasm}-patched.wasm}"
[[ "$input" = /* ]] || input="$PWD/$input"
[[ "$output" = /* ]] || output="$PWD/$output"
[[ "$patched" = /* ]] || patched="$PWD/$patched"
for candidate_path in "$output" "$patched" "${output%.*}.metadata.json"; do
  if [[ -e "$candidate_path" ]]; then printf 'Refusing to overwrite candidate output: %s\n' "$candidate_path" >&2; exit 2; fi
done
mkdir -p "$(dirname "$output")" "$(dirname "$patched")"
cd "$MG_ROOT"
task_network_args=()
case "${MG_AOT_OFFLINE:-0}" in
  0) ;;
  1) task_network_args+=(--offline) ;;
  *) printf '%s\n' 'MG_AOT_OFFLINE must be 0 or 1.' >&2; exit 2 ;;
esac
cargo build --locked ${task_network_args[@]+"${task_network_args[@]}"} --release --manifest-path aot-compiler/Cargo.toml \
  --config 'source.crates-io.replace-with="rsproxy-sparse"' \
  --config 'source.rsproxy-sparse.registry="sparse+https://rsproxy.cn/index/"'
aot-compiler/target/release/muguet-ohos-aot-compiler "$input" "$output" "$patched" --isa-profile "$profile" 2>&1 | tee "$output.compile.log"
