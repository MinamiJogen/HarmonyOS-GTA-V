#!/bin/bash
set -euo pipefail
snapshot_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
snapshot_sdk="${OHOS_SDK_NATIVE:-/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native}"
mkdir -p "$snapshot_root/build"
"$snapshot_sdk/llvm/bin/clang++" --target=aarch64-linux-ohos26 \
  --sysroot="$snapshot_sdk/sysroot" -D__MUSL__ -std=c++17 -O2 \
  -Wall -Wextra -Werror -fPIC -fno-exceptions -fno-rtti -mno-outline-atomics \
  -fstack-usage -c "$snapshot_root/driver_signal_snapshot.cpp" \
  -o "$snapshot_root/build/driver_signal_snapshot-ohos.o"
"$snapshot_sdk/llvm/bin/llvm-nm" -u "$snapshot_root/build/driver_signal_snapshot-ohos.o" \
  > "$snapshot_root/build/undefined-symbols.txt"
"$snapshot_sdk/llvm/bin/llvm-objdump" -d -r "$snapshot_root/build/driver_signal_snapshot-ohos.o" \
  > "$snapshot_root/build/ohos-disassembly.txt"
/usr/bin/clang++ -std=c++17 -O2 -Wall -Wextra -Werror \
  -fno-exceptions -fno-rtti "$snapshot_root/test_snapshot_host.cpp" \
  -o "$snapshot_root/build/test_snapshot_host"
"$snapshot_root/build/test_snapshot_host"
python3 -m unittest discover -s "$snapshot_root" -p 'test_decode.py' -v
