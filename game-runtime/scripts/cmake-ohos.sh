#!/usr/bin/env bash
set -euo pipefail
sdk="${OHOS_SDK_NATIVE:-/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native}"
cmake="$sdk/build-tools/cmake/bin/cmake"
for arg in "$@"; do
  if [[ "$arg" = "--build" || "$arg" = "--install" ]]; then exec "$cmake" "$@"; fi
done
# Native dependencies such as Opus also retain __FILE__ in release builds.
# Translate the same source mappings for C/C++, preserving supplied flags.
source "$(dirname "$0")/env.sh"
task_privacy_flags=""
for task_flag in "${HARMONY_RUSTC_FLAGS[@]}"; do
  [[ "$task_flag" == --remap-path-prefix=* ]] || continue
  task_flag="-ffile-prefix-map=${task_flag#--remap-path-prefix=}"
  task_quoted="'"
  while [[ "$task_flag" == *"'"* ]]; do
    task_quoted+="${task_flag%%\'*}"
    task_quoted+="'\\''"
    task_flag="${task_flag#*\'}"
  done
  task_quoted+="$task_flag'"
  task_privacy_flags+=" $task_quoted"
done
task_arguments=()
task_has_c_flags=0
task_has_cxx_flags=0
for arg in "$@"; do
  case "$arg" in
    -DCMAKE_C_FLAGS=*|-DCMAKE_C_FLAGS:*=*|CMAKE_C_FLAGS=*|CMAKE_C_FLAGS:*=*)
      arg+="$task_privacy_flags"; task_has_c_flags=1 ;;
    -DCMAKE_CXX_FLAGS=*|-DCMAKE_CXX_FLAGS:*=*|CMAKE_CXX_FLAGS=*|CMAKE_CXX_FLAGS:*=*)
      arg+="$task_privacy_flags"; task_has_cxx_flags=1 ;;
  esac
  task_arguments+=("$arg")
done
if [[ "$task_has_c_flags" == 0 ]]; then task_arguments+=("-DCMAKE_C_FLAGS=${CFLAGS:-}$task_privacy_flags"); fi
if [[ "$task_has_cxx_flags" == 0 ]]; then task_arguments+=("-DCMAKE_CXX_FLAGS=${CXXFLAGS:-}$task_privacy_flags"); fi
exec "$cmake" "${task_arguments[@]}" -DCMAKE_TOOLCHAIN_FILE="$sdk/build/cmake/ohos.toolchain.cmake" -DOHOS_ARCH=arm64-v8a -DOHOS_PLATFORM=OHOS -DOHOS_STL=c++_static -DCMAKE_SYSTEM_NAME=OHOS
