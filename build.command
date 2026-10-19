#!/bin/bash
# Finder double-click entry; arguments also work when launched from Terminal.
set -u
task_repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
if [[ $# -gt 0 ]]; then
  exec /bin/bash "$task_repo_dir/tools/build_game.sh" "$@"
fi
printf '鸿蒙 GTA5 v0.8.0 本地构建\n请提供自己的兼容 game.wasm 或游戏 ZIP；工具不会提供游戏文件。\n'
printf '输入文件路径（可将文件拖入窗口），然后按回车：\n> '
IFS= read -r task_input || exit 2
command -v python3 >/dev/null 2>&1 || { printf '错误：请先安装 Python 3。\n' >&2; exit 2; }
task_input="$(python3 - "$task_input" <<'PY_INPUT'
import shlex
import sys
from pathlib import Path
text = sys.argv[1].strip()
path = Path(text).expanduser()
if not path.is_file():
    try:
        parts = shlex.split(text)
    except ValueError:
        parts = []
    if len(parts) != 1:
        sys.exit("错误：请输入或拖入一个文件的路径。")
    path = Path(parts[0]).expanduser()
if not path.is_file():
    sys.exit("错误：文件不存在或不可读。")
print(path.resolve())
PY_INPUT
)" || exit 2
case "$task_input" in
  *.[zZ][iI][pP]) task_option=--zip ;;
  *.[wW][aA][sS][mM]) task_option=--wasm ;;
  *) printf '错误：请选择 .wasm 或 .zip 文件。\n' >&2; exit 2 ;;
esac
printf '\n选择构建模式：\n  1. 完整 unsigned HAP（默认）\n  2. 仅生成 AOT\n  3. 使用已配置的本地签名生成 HAP\n输入 1 / 2 / 3，直接回车选 1：'
IFS= read -r task_mode || exit 2
task_mode_args=()
case "${task_mode:-1}" in
  1) ;;
  2) task_mode_args+=(--aot-only) ;;
  3) task_mode_args+=(--signed) ;;
  *) printf '错误：构建模式必须是 1、2 或 3。\n' >&2; exit 2 ;;
esac
/bin/bash "$task_repo_dir/tools/build_game.sh" "$task_option" "$task_input" ${task_mode_args[@]+"${task_mode_args[@]}"}
task_status=$?
if [[ -t 0 ]]; then
  printf '\n按回车关闭窗口。\n'
  IFS= read -r _ || true
fi
exit "$task_status"
