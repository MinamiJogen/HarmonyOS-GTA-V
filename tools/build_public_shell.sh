#!/usr/bin/env bash
# Build a redistributable application shell from C++ source, without game code.
set -euo pipefail
task_repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
if [[ "${1:-}" == --help || "${1:-}" == -h ]]; then
  cat <<'HELP'
用法：bash tools/build_public_shell.sh
从全新 ASCII 临时目录重建 v1.0.1 ARM64 unsigned 应用壳。
不链接游戏运行时、游戏 AOT 或任何本地 Rust AOT 测试对象。
不读取游戏资源，不使用签名材料，不安装到设备，不覆盖完整游戏构建。
输出：dist/public-v1.0.1/HarmonyOS-GTA-V-v1.0.1-shell-arm64-unsigned.hap
需已有 DevEco Studio / API 26 与 HMS native SDK；可设置 DEVECO_CONTENTS。
HELP
  exit 0
fi
[[ $# == 0 ]] || { printf '错误：不支持参数；使用 --help 查看说明。\n' >&2; exit 2; }
command -v python3 >/dev/null || { printf '错误：缺少 Python 3。\n' >&2; exit 2; }
export DEVECO_CONTENTS="${DEVECO_CONTENTS:-/Applications/DevEco-Studio.app/Contents}"
task_reader="$DEVECO_CONTENTS/sdk/default/openharmony/native/llvm/bin/llvm-readelf"
[[ -x "$task_reader" ]] || { printf '错误：缺少 DevEco native SDK llvm-readelf。\n' >&2; exit 2; }
task_output_dir="$task_repo_dir/dist/public-v1.0.1"
mkdir -p "$task_output_dir"
# A new stage also guarantees that no cached native library can be reused.
task_stage_root="$(mktemp -d /private/tmp/harmony-gta5-public-shell.XXXXXXXX)"
export HARMONY_BUILD_STAGE="$task_stage_root"
export HARMONY_SHELL_ONLY=1
export HARMONY_COPY_BUILD_OUTPUT=0
export HARMONY_RESET_STAGE_PROFILE=1
export HARMONY_ENABLE_SIGNING=false
export HARMONY_WITH_VVL=0
export HARMONY_BUILD_MODE=release
printf '正在从源码重建无游戏应用壳；完整游戏产物和原签名目录保持不动。\n'
"$task_repo_dir/harmony/build_unsigned.sh" 2>&1 | tee "$task_output_dir/build-shell.log"
task_hap="$task_stage_root/harmony/entry/build/default/outputs/default/entry-default-unsigned.hap"
task_native_root="$task_stage_root/harmony/entry/build/default/intermediates/cmake/default/obj/arm64-v8a"
task_public_hap="$task_output_dir/HarmonyOS-GTA-V-v1.0.1-shell-arm64-unsigned.hap"
[[ -f "$task_hap" ]] || { printf '错误：构建未生成 unsigned HAP。\n' >&2; exit 1; }
python3 "$task_repo_dir/tools/verify_public_shell.py" \
  "$task_hap" "$task_native_root" "$task_reader" "$task_output_dir/verification.json"
cp "$task_hap" "$task_public_hap"
printf '\n公开应用壳已生成：%s\n' "$task_public_hap"
printf '此包未签名，支持导入游戏资源；运行游戏仍需自行编译并签名完整包。\n'
