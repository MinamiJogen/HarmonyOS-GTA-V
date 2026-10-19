#!/bin/zsh
set -eu

task_project_dir="${0:A:h}"
task_repo_dir="${task_project_dir:h}"
task_deveco_contents="${DEVECO_CONTENTS:-/Applications/DevEco-Studio.app/Contents}"
# Hvigor rejects non-ASCII project paths. Build a source-only mirror in /private/tmp.
task_stage_root="${HARMONY_BUILD_STAGE:-/private/tmp/harmony-runtime-probe-${UID}}"
task_stage_dir="$task_stage_root/harmony"
task_shell_only="${HARMONY_SHELL_ONLY:-0}"
task_copy_build_output="${HARMONY_COPY_BUILD_OUTPUT:-1}"
if [[ "$task_shell_only" != 0 && "$task_shell_only" != 1 ]]; then
  print -u2 'HARMONY_SHELL_ONLY must be 0 or 1.'
  exit 2
fi
if [[ "$task_copy_build_output" != 0 && "$task_copy_build_output" != 1 ]]; then
  print -u2 'HARMONY_COPY_BUILD_OUTPUT must be 0 or 1.'
  exit 2
fi
mkdir -p "$task_stage_dir"
# Keep any DevEco-generated signing material only in the private build mirror.
# Set HARMONY_RESET_STAGE_PROFILE=1 to explicitly replace the mirror profile.
if [[ ! -f "$task_stage_dir/build-profile.json5" || "${HARMONY_RESET_STAGE_PROFILE:-0}" == "1" ]]; then
  cp "$task_project_dir/build-profile.json5" "$task_stage_dir/build-profile.json5"
fi
/usr/bin/rsync -a --delete --exclude '/build-profile.json5' --exclude 'build-profile.local.json5' --exclude '.signing' --exclude '.idea' --exclude 'local.properties' --exclude '.hvigor' --exclude 'node_modules' --exclude 'oh_modules' --exclude 'entry/oh_modules' --exclude 'entry/build' --exclude 'build' --exclude 'logs' "$task_project_dir/" "$task_stage_dir/"
# Mutate only the private ASCII mirror. Explicit ON/OFF also prevents a cached
# shell mode from leaking into a later complete-game build in the same stage.
python3 - "$task_stage_dir/entry/build-profile.json5" "$task_shell_only" <<'PYSHELL'
import json
import re
import sys
from pathlib import Path
path = Path(sys.argv[1])
profile = json.loads(path.read_text())
options = profile.setdefault('buildOption', {}).setdefault('externalNativeOptions', {})
arguments = re.sub(r'(?<!\S)-DMUGUET_SHELL_ONLY(?::[^=\s]+)?=[^\s]+', '', str(options.get('arguments', ''))).strip()
options['arguments'] = (arguments + ' -DMUGUET_SHELL_ONLY=' + ('ON' if sys.argv[2] == '1' else 'OFF')).strip()
path.write_text(json.dumps(profile, indent=2) + '\n')
PYSHELL
if [[ "$task_shell_only" == 1 ]]; then
  print 'Public shell: game runtime and all embedded AOT disabled'
fi
# Native TypeScript declaration packages are local file dependencies. A fresh
# source-only mirror has no oh_modules cache, so wire them up before ArkTS.
mkdir -p "$task_stage_dir/entry/oh_modules"
for task_native_type in libharmony_probe libgraphics_frame_surface; do
  ln -sfn "$task_stage_dir/entry/src/main/cpp/types/$task_native_type" \
    "$task_stage_dir/entry/oh_modules/$task_native_type.so"
done
task_with_vvl="${HARMONY_WITH_VVL:-0}"
if [[ "$task_with_vvl" != "0" && "$task_with_vvl" != "1" ]]; then
  print -u2 'HARMONY_WITH_VVL must be 0 or 1.'
  exit 2
fi
task_vvl_destination="$task_stage_dir/entry/libs/arm64-v8a/libVkLayer_khronos_validation.so"
if [[ "$task_with_vvl" == "1" ]]; then
  task_vvl_source="${HARMONY_VVL_SO:-$task_repo_dir/.tools/ohos-vulkan-validationlayers/build-ohos/artifacts/libVkLayer_khronos_validation.so}"
  if [[ ! -f "$task_vvl_source" ]]; then
    print -u2 "Validation layer does not exist: $task_vvl_source"
    exit 2
  fi
  mkdir -p "${task_vvl_destination:h}"
  cp "$task_vvl_source" "$task_vvl_destination"
  print "VVL package: ON ($task_vvl_source)"
else
  # This is only a generated file in the private ASCII build mirror.
  rm -f "$task_vvl_destination"
  print 'VVL package: OFF'
fi
if [[ -d "$task_repo_dir/native-diagnostics" ]]; then
  ln -sfn "$task_repo_dir/native-diagnostics" "$task_stage_root/native-diagnostics"
fi
if [[ -d "$task_repo_dir/runtime-probe" ]]; then
  ln -sfn "$task_repo_dir/runtime-probe" "$task_stage_root/runtime-probe"
fi
for task_native_dependency in game-runtime native-platform; do
  if [[ -d "$task_repo_dir/$task_native_dependency" ]]; then
    ln -sfn "$task_repo_dir/$task_native_dependency" "$task_stage_root/$task_native_dependency"
  fi
done
export DEVECO_SDK_HOME="$task_deveco_contents/sdk"
export NODE_HOME="$task_deveco_contents/tools/node"
export PATH="$NODE_HOME/bin:$PATH"
export NODE_PATH="$task_stage_dir/node_modules:$task_deveco_contents/tools/hvigor"
mkdir -p "$task_stage_dir/node_modules/@ohos"
ln -sfn "$task_deveco_contents/tools/hvigor/hvigor" "$task_stage_dir/node_modules/@ohos/hvigor"
ln -sfn "$task_deveco_contents/tools/hvigor/hvigor-ohos-plugin" "$task_stage_dir/node_modules/@ohos/hvigor-ohos-plugin"
cd "$task_stage_dir"
set +e
"$NODE_HOME/bin/node" "$task_deveco_contents/tools/hvigor/hvigor/bin/hvigor.js" \
  --mode module -p module=entry@default -p product=default \
  -p buildMode="${HARMONY_BUILD_MODE:-debug}" -p properties.enableSignTask="${HARMONY_ENABLE_SIGNING:-false}" assembleHap --no-daemon "$@"
task_build_status=$?
set -e
if [[ "$task_build_status" == "0" ]]; then
  set +e
  python3 - "$task_stage_dir/entry/build/default/outputs/default/entry-default-unsigned.hap" "$task_with_vvl" <<'PYVVL'
import sys
import zipfile
with zipfile.ZipFile(sys.argv[1]) as package:
    names = package.namelist()
    layers = [name for name in names if 'VkLayer' in name]
    expected = 'libs/arm64-v8a/libVkLayer_khronos_validation.so'
    if sys.argv[2] == '1':
        if expected not in names:
            raise SystemExit('VVL ON package check failed: validation library is absent.')
    elif layers:
        raise SystemExit('VVL OFF package check failed: ' + ', '.join(layers))
print('VVL package contents verified: ' + ('ON' if sys.argv[2] == '1' else 'OFF'))
PYVVL
  task_package_check_status=$?
  set -e
  if [[ "$task_package_check_status" != "0" ]]; then
    task_build_status=$task_package_check_status
  fi
fi
if [[ "$task_copy_build_output" == 1 && -d "$task_stage_dir/entry/build" ]]; then
  mkdir -p "$task_project_dir/entry/build"
  /usr/bin/rsync -a "$task_stage_dir/entry/build/" "$task_project_dir/entry/build/"
fi
print "Build stage: $task_stage_dir"
exit "$task_build_status"
