#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/env.sh"
cd "$RP_ROOT"
cargo run --locked --release --target-dir target/host -p probe-aot-compiler -- aarch64-unknown-linux-ohos artifacts 2>&1 | tee logs/precompile-aot-ohos.log
