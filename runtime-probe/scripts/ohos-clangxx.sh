#!/usr/bin/env bash
set -euo pipefail
sdk="${OHOS_SDK_NATIVE:-/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native}"
exec "$sdk/llvm/bin/clang++" -target aarch64-linux-ohos --sysroot="$sdk/sysroot" -D__MUSL__ "$@"
