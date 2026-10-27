#!/usr/bin/env bash
# Local, user-supplied game build only. Never installs a HAP or runs ZIP scripts.
set -Eeuo pipefail

task_repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
task_wasm=''
task_zip=''
task_aot_only=0
task_signed=0
task_check=0
task_offline=0
task_phase='参数检查'

usage() {
  cat <<'HELP'
鸿蒙 GTA5 v1.0.1 本地构建工具（macOS）

用法：
  bash tools/build_game.sh --check [--aot-only]
  bash tools/build_game.sh --wasm /路径/game.wasm [选项]
  bash tools/build_game.sh --zip /路径/game.zip [选项]

  --wasm PATH  使用自己的兼容 game.wasm
  --zip PATH   校验兼容 ZIP 并提取到 resources/game；不会执行 ZIP 内脚本
  --aot-only   仅生成 AOT，不编译运行时、嵌入对象或 HAP
  --signed     使用已配置的本地设备签名；默认生成 unsigned HAP
  --check      只检查环境及指定输入，不提取、不编译、不签名
  --offline    AOT 与运行时的 Cargo 构建均禁止下载依赖，需事先缓存
  --help       显示本说明

--wasm 与 --zip 二选一；--check 可不提供输入。
不提供游戏文件，不自动安装应用，不修改设备或打印签名材料。
完整流程默认使用 release 模式。详见 docs/BUILD.zh-CN.md。
环境变量：DEVECO_CONTENTS、OHOS_SDK_NATIVE、MG_CARGO_HOME、MG_RUSTUP_HOME、
          HARMONY_BUILD_STAGE（自定义 DevEco、Rust 或签名镜像位置）。
HELP
}

fail() { printf '错误：%s\n' "$*" >&2; exit 2; }
progress() { printf '\n[%s] %s\n' "$task_phase" "$*"; }
trap 'task_status=$?; printf "\n构建失败：%s（退出码 %s）。请查看上方错误。\n" "$task_phase" "$task_status" >&2' ERR

while [[ $# -gt 0 ]]; do
  case "$1" in
    --help|-h) usage; exit 0 ;;
    --wasm|--zip)
      task_option="$1"
      [[ $# -ge 2 && -n "$2" && "$2" != --* ]] || fail "$1 后需要文件路径。"
      [[ -z "$task_wasm" && -z "$task_zip" ]] || fail '--wasm 与 --zip 只能指定一次，且不能同时使用。'
      if [[ "$task_option" == --wasm ]]; then task_wasm="$2"; else task_zip="$2"; fi
      shift 2 ;;
    --aot-only) task_aot_only=1; shift ;;
    --signed) task_signed=1; shift ;;
    --check) task_check=1; shift ;;
    --offline) task_offline=1; shift ;;
    *) fail "未知参数：$1；用 --help 查看用法。" ;;
  esac
done
[[ "$task_aot_only" != 1 || "$task_signed" != 1 ]] || fail '--aot-only 不能与 --signed 同时使用。'
[[ "$task_check" == 1 || -n "$task_wasm$task_zip" ]] || fail '请指定 --wasm 或 --zip；只检查环境可用 --check。'
for task_input in "$task_wasm" "$task_zip"; do
  [[ -z "$task_input" || ( -f "$task_input" && -r "$task_input" ) ]] || fail "输入文件不存在或不可读：$task_input"
done

task_phase='环境检查'
require_command() { command -v "$1" >/dev/null 2>&1 || fail "缺少命令 $1。$2"; }
require_file() { [[ -f "$1" ]] || fail "缺少文件：$1。$2"; }
require_executable() { [[ -x "$1" ]] || fail "缺少可执行工具：$1。$2"; }
require_command python3 '请先安装 Python 3。'
[[ "$(uname -s)" == Darwin ]] || fail '此入口面向 macOS / DevEco Studio；其他系统请参考底层脚本自行适配。'

# Prefer the project's existing toolchain; a fresh checkout uses the user's
# normal installation. Explicit MG_* settings always take precedence.
if [[ -n "${MG_CARGO_HOME:-}" || -n "${MG_RUSTUP_HOME:-}" ]]; then
  export MG_CARGO_HOME="${MG_CARGO_HOME:-${CARGO_HOME:-$HOME/.cargo}}"
  export MG_RUSTUP_HOME="${MG_RUSTUP_HOME:-${RUSTUP_HOME:-$HOME/.rustup}}"
  printf 'Rust：使用指定的 MG_CARGO_HOME / MG_RUSTUP_HOME。\n'
elif [[ -x "$task_repo_dir/runtime-probe/.toolchain/cargo/bin/rustup" && -d "$task_repo_dir/runtime-probe/.toolchain/rustup" ]]; then
  export MG_CARGO_HOME="$task_repo_dir/runtime-probe/.toolchain/cargo"
  export MG_RUSTUP_HOME="$task_repo_dir/runtime-probe/.toolchain/rustup"
  printf 'Rust：复用项目本地工具链。\n'
else
  export MG_CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
  export MG_RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
  printf 'Rust：未找到项目本地工具链，使用用户的 Rust 安装。\n'
fi
export DEVECO_CONTENTS="${DEVECO_CONTENTS:-/Applications/DevEco-Studio.app/Contents}"
export OHOS_SDK_NATIVE="${OHOS_SDK_NATIVE:-$DEVECO_CONTENTS/sdk/default/openharmony/native}"
source "$task_repo_dir/game-runtime/scripts/env.sh"
export RUSTUP_TOOLCHAIN=1.99.1
require_command rustup '请安装 rustup，并执行 rustup toolchain install 1.99.1。'
require_command cargo '请安装 rustup，并执行 rustup toolchain install 1.99.1。'
require_command cc '请安装 Xcode 命令行工具：xcode-select --install。'
rustup run 1.99.1 rustc --version >/dev/null 2>&1 || fail '缺少 Rust 1.99.1；请执行 rustup toolchain install 1.99.1（在上述 Rust 环境中）。'
rustup run 1.99.1 cargo --version >/dev/null 2>&1 || fail 'Rust 1.99.1 的 Cargo 不可用；请重新安装该工具链。'
require_file "$task_repo_dir/game-runtime/scripts/compile-aot.sh" '请使用完整源码仓库。'

if [[ "$task_offline" == 1 ]]; then
  export MG_AOT_OFFLINE=1
  export CARGO_NET_OFFLINE=true
  printf 'Cargo 离线模式：AOT 和运行时均禁止下载依赖。\n'
fi

if [[ "$task_aot_only" != 1 ]]; then
  case "$(rustup target list --installed --toolchain 1.99.1)" in
    *aarch64-unknown-linux-ohos*) ;;
    *) fail '缺少 OHOS 目标；请执行 rustup target add --toolchain 1.99.1 aarch64-unknown-linux-ohos。' ;;
  esac
  require_command rg '请安装 ripgrep（rg），运行时构建用于检查导出符号。'
  require_command make '请安装 Xcode 命令行工具：xcode-select --install。'
  require_executable /bin/zsh '应用构建脚本需要 macOS zsh。'
  require_executable /usr/bin/rsync '应用构建脚本需要 macOS rsync。'
  for task_tool in clang clang++ llvm-ar llvm-nm llvm-objcopy llvm-readelf; do
    require_executable "$OHOS_SDK_NATIVE/llvm/bin/$task_tool" '请安装 HarmonyOS API 26 native SDK，或设置 OHOS_SDK_NATIVE。'
  done
  require_executable "$OHOS_SDK_NATIVE/build-tools/cmake/bin/cmake" '请安装完整 native SDK。'
  require_file "$OHOS_SDK_NATIVE/build/cmake/ohos.toolchain.cmake" '请安装完整 native SDK。'
  require_executable "$DEVECO_CONTENTS/tools/node/bin/node" '请安装 DevEco Studio，或设置 DEVECO_CONTENTS。'
  require_file "$DEVECO_CONTENTS/tools/hvigor/hvigor/bin/hvigor.js" '请安装完整 DevEco Studio。'
  task_hms_root="$OHOS_SDK_NATIVE/../../hms/native/sysroot/usr"
  require_file "$task_hms_root/include/xengine/xeg_vulkan_extension.h" '请安装包含 HMS native XEngine 的 HarmonyOS SDK。'
  require_file "$task_hms_root/lib/aarch64-linux-ohos/libframegeneration.so" '请安装包含 HMS native FrameGeneration 的 HarmonyOS SDK。'
  for task_script in build-ohos.sh embed-aot-candidate.py; do
    require_file "$task_repo_dir/game-runtime/scripts/$task_script" '请使用完整源码仓库。'
  done
  if [[ "$task_signed" == 1 ]]; then
    [[ "${HARMONY_RESET_STAGE_PROFILE:-0}" != 1 ]] || fail '--signed 不能同时重置签名镜像配置，请移除 HARMONY_RESET_STAGE_PROFILE=1。'
    task_stage_root="${HARMONY_BUILD_STAGE:-/private/tmp/harmony-runtime-probe-${UID}}"
    python3 - "$task_stage_root/harmony/build-profile.json5" <<'PY_SIGNING'
import json
import sys
from pathlib import Path
try:
    app = json.loads(Path(sys.argv[1]).read_text()).get("app", {})
    configs = {item.get("name") for item in app.get("signingConfigs", []) if item.get("material")}
    if not any(item.get("signingConfig") in configs for item in app.get("products", [])):
        raise ValueError("unconfigured")
except Exception:
    sys.exit("错误：尚未配置本地签名。请先构建 unsigned 包，再用 DevEco 打开 ASCII 镜像配置自动签名；签名材料不会打印。")
PY_SIGNING
    require_executable "$task_repo_dir/harmony/build_signed_local.sh" '请使用完整源码仓库。'
  else
    require_executable "$task_repo_dir/harmony/build_unsigned.sh" '请使用完整源码仓库。'
  fi
fi
printf '环境检查通过%s。\n' "$( [[ "$task_aot_only" == 1 ]] && printf '（仅 AOT，不要求 OHOS SDK 或目标）' || true )"

task_phase='输入校验'
if [[ -n "$task_wasm" ]]; then
  task_wasm="$(python3 - "$task_wasm" <<'PY_WASM'
import sys
from pathlib import Path
path = Path(sys.argv[1]).resolve()
with path.open("rb") as source:
    if source.read(8) != b"\0asm\x01\0\0\0":
        sys.exit("错误：输入不是有效版本的 WebAssembly 文件（需要 Wasm v1 头）。")
print(path)
PY_WASM
)"
elif [[ -n "$task_zip" ]]; then
  task_zip="$(python3 - "$task_zip" <<'PY_PATH'
import sys
from pathlib import Path
print(Path(sys.argv[1]).resolve())
PY_PATH
)"
  # Limited read-only preflight before the much more expensive extraction.
  # Import only our own validator. Nothing from the ZIP is imported or executed.
  python3 - "$task_repo_dir/tools" "$task_zip" "$task_repo_dir/resources/game" <<'PY_ZIP'
import os
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from validate_game_package import Audit
report = Audit(Path(sys.argv[2]), hash_wasm=False).run()
if report["status"] != "pass":
    errors = "; ".join(item["code"] + ": " + item["message"] for item in report["errors"][:5])
    sys.exit("错误：ZIP 预检未通过：" + errors)
if any(item["code"] == "case_collision" for item in report["warnings"]):
    sys.exit("错误：ZIP 含大小写冲突的文件路径，无法在 macOS 上可靠提取。")
builds = [item["build"] for item in report["builds"] if item.get("wasm")]
if len(builds) != 1:
    sys.exit("错误：ZIP 必须恰好包含一个兼容 b/<build>/game.wasm，当前发现 " + str(len(builds)) + " 个。")
root = Path(sys.argv[3])
for candidate in (root.parent, root):
    if candidate.is_symlink():
        sys.exit("错误：资源目标路径包含符号链接，请先移开该链接；工具不会沿链接写入资源。")
if root.exists():
    for directory, dirs, files in os.walk(root, followlinks=False):
        if any((Path(directory) / name).is_symlink() for name in dirs + files):
            sys.exit("错误：resources/game 内已有符号链接，请先自行移开；工具不会沿链接提取或覆盖文件。")
existing = [p for p in root.glob("b/*/game.wasm") if p.is_file() and p.parent.name != builds[0]]
if existing:
    sys.exit("错误：resources/game 已有其他 build 的 game.wasm；请先自行移开旧资源。工具不会删除现有资源。")
print("ZIP 预检通过；将提取到 resources/game，不执行归档内脚本。")
PY_ZIP
fi

if [[ "$task_check" == 1 ]]; then
  printf '\n检查完成；未提取资源、编译或签名。\n'
  exit 0
fi

cd "$task_repo_dir"
if [[ -n "$task_zip" ]]; then
  task_phase='1/5 资源准备'
  progress '校验、提取用户资源并生成本地校验记录。'
  python3 "$task_repo_dir/tools/prepare_game_resources.py" "$task_zip" \
    "$task_repo_dir/resources/game" "$task_repo_dir/evidence/resources/prepare-report.json"
  task_wasm="$(python3 - "$task_repo_dir/resources/game" <<'PY_SELECT'
import sys
from pathlib import Path
paths = sorted(p.resolve() for p in Path(sys.argv[1]).glob("b/*/game.wasm") if p.is_file())
if len(paths) != 1:
    sys.exit("错误：提取后没有找到唯一的 b/<build>/game.wasm，请检查 resources/game。")
print(paths[0])
PY_SELECT
)"
fi

task_phase='2/5 AOT 编译'
progress '从用户提供的 WebAssembly 生成可信 ARM64 AOT；此步骤可能需要较长时间。'
mkdir -p "$task_repo_dir/game-runtime/artifacts/aot-candidates"
task_aot_dir="$(mktemp -d "$task_repo_dir/game-runtime/artifacts/aot-candidates/build-game.XXXXXX")"
task_aot="$task_aot_dir/game-ohos.cwasm"
task_aot_metadata="$task_aot_dir/game-ohos.metadata.json"
bash "$task_repo_dir/game-runtime/scripts/compile-aot.sh" "$task_wasm" "$task_aot" "$task_aot_dir/game-patched.wasm"
require_file "$task_aot" 'AOT 工具未生成预期输出，请检查上方日志。'
require_file "$task_aot_metadata" 'AOT 工具未生成元数据，请检查上方日志。'
if [[ "$task_aot_only" == 1 ]]; then
  printf '\nAOT 编译完成：%s\n' "$task_aot"
  exit 0
fi

task_phase='3/5 运行时编译'
progress '构建 OHOS ARM64 Rust 运行时。'
bash "$task_repo_dir/game-runtime/scripts/build-ohos.sh"
task_phase='4/5 嵌入 AOT'
progress '生成待链接到签名原生库的 AOT 对象。'
task_object_dir="$(mktemp -d /private/tmp/harmony-game-aot.XXXXXX)"
export HARMONY_GAME_AOT_OBJECT="$task_object_dir/game-embedded.o"
python3 "$task_repo_dir/game-runtime/scripts/embed-aot-candidate.py" "$task_aot" "$task_aot_metadata" "$HARMONY_GAME_AOT_OBJECT"
task_phase='5/5 HAP 构建'
export HARMONY_BUILD_MODE="${HARMONY_BUILD_MODE:-release}"
if [[ "$task_signed" == 1 ]]; then
  progress '使用已配置的本地签名构建 HAP。'
  "$task_repo_dir/harmony/build_signed_local.sh"
  task_output='entry-default-signed.hap'
else
  export HARMONY_ENABLE_SIGNING=false
  progress '构建未签名 HAP；安装前需要配置自己的签名。'
  "$task_repo_dir/harmony/build_unsigned.sh"
  task_output='entry-default-unsigned.hap'
fi
require_file "$task_repo_dir/harmony/entry/build/default/outputs/default/$task_output" 'HAP 构建未生成预期输出，请检查上方日志。'
if [[ "$task_signed" != 1 ]]; then
  python3 "$task_repo_dir/harmony/verify_game_hap.py" --unsigned \
    --hap "$task_repo_dir/harmony/entry/build/default/outputs/default/$task_output" \
    --aot "$task_aot" --report "$task_aot_dir/game-hap-verification.json"
fi
printf '\n构建完成：%s/harmony/entry/build/default/outputs/default/%s\n' "$task_repo_dir" "$task_output"
printf '本地完整包含用户游戏 AOT；请勿作为不含游戏文件的公开安装包上传。\n'
