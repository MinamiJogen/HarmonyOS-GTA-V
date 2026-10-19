#!/usr/bin/env bash
set -euo pipefail
probe_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
probe_sdk="${OHOS_SDK_NATIVE:-/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native}"
mkdir -p "$probe_root/bin"
"$probe_sdk/llvm/bin/clang++" \
  --target=aarch64-linux-ohos26 --sysroot="$probe_sdk/sysroot" -D__MUSL__ \
  -DMUGUET_VULKAN_PROBE_STANDALONE=1 \
  -std=c++17 -O2 -Wall -Wextra -Werror -fPIE -pie -static-libstdc++ \
  -fstack-protector-strong -Wl,-z,relro,-z,now \
  "$probe_root/src/vulkan_probe.cpp" -ldl -o "$probe_root/bin/vulkan-probe"
"$probe_sdk/llvm/bin/llvm-readelf" -h -l -d "$probe_root/bin/vulkan-probe"
