#!/usr/bin/env bash
# Generate an object with a complete trusted cwasm ELF nested in an RX segment.
# Link this object into the SIGNED application's shared library before packaging.
set -euo pipefail
source "$(dirname "$0")/env.sh"
cd "$RP_ROOT"
artifact="${1:-$RP_ROOT/artifacts/add.cwasm}"
object="${2:-$RP_ROOT/artifacts/add-embedded.o}"
prefix="${3:-rp_embedded_aot}"
[[ "$prefix" =~ ^[a-zA-Z_][a-zA-Z0-9_]*$ ]] || { echo "invalid symbol prefix" >&2; exit 1; }
[[ "$artifact" = /* ]] || artifact="$RP_ROOT/$artifact"
cat > "$RP_ROOT/artifacts/embed-${prefix}.S" <<ASM
.section .text.rp_aot,"ax",@progbits
.balign 65536
.global ${prefix}_start
.global ${prefix}_end
.hidden ${prefix}_start
.hidden ${prefix}_end
${prefix}_start:
.incbin "$artifact"
${prefix}_end:
.section .note.GNU-stack,"",@progbits
ASM
"$RP_ROOT/scripts/ohos-clang.sh" -c "$RP_ROOT/artifacts/embed-${prefix}.S" -o "$object"
"$OHOS_SDK_NATIVE/llvm/bin/llvm-readelf" -SW "$object" | tee "$RP_ROOT/logs/embedded-sections-${prefix}.txt"
