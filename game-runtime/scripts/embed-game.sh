#!/usr/bin/env bash
set -euo pipefail
source "$(dirname "$0")/env.sh"
artifact="${1:-$MG_ROOT/artifacts/game-ohos.cwasm}"
object="${2:-$MG_ROOT/artifacts/game-embedded.o}"
[[ "$artifact" = /* ]] || artifact="$PWD/$artifact"
cat > "$MG_ROOT/artifacts/game-embedded.S" <<ASM
.section .text.mg_game_aot,"ax",@progbits
.balign 65536
.global mg_game_aot_start
.global mg_game_aot_end
.hidden mg_game_aot_start
.hidden mg_game_aot_end
mg_game_aot_start:
.incbin "$artifact"
mg_game_aot_end:
.section .note.GNU-stack,"",@progbits
ASM
"$MG_ROOT/scripts/ohos-clang.sh" -c "$MG_ROOT/artifacts/game-embedded.S" -o "$object"
"$OHOS_SDK_NATIVE/llvm/bin/llvm-readelf" -SW "$object" > "$MG_ROOT/logs/game-embedded-sections.txt"
