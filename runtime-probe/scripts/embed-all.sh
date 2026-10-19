#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/env.sh"
cd "$RP_ROOT"
bash scripts/embed-aot.sh artifacts/add.cwasm artifacts/add-embedded.o rp_embedded_aot
bash scripts/embed-aot.sh artifacts/memory64.cwasm artifacts/memory64-embedded.o rp_memory64_aot
bash scripts/embed-aot.sh artifacts/shared.cwasm artifacts/shared-embedded.o rp_shared_aot
bash scripts/embed-aot.sh artifacts/shared-memory64.cwasm artifacts/shared-memory64-embedded.o rp_shared_memory64_aot
