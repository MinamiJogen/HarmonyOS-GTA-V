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
task_fg_probe_only="${HARMONY_FG_PROBE_ONLY:-0}"
task_game_experiment_bundle="${HARMONY_GAME_EXPERIMENT_BUNDLE:-}"
if [[ -n "$task_game_experiment_bundle" && ( "$task_fg_probe_only" != 0 || "$task_shell_only" != 0 || "$task_copy_build_output" != 0 ) ]]; then
  print -u2 'A full-game experiment requires FG_PROBE_ONLY=0, SHELL_ONLY=0 and COPY_BUILD_OUTPUT=0.'
  exit 2
fi
if [[ "$task_fg_probe_only" != 0 && "$task_fg_probe_only" != 1 ]]; then
  print -u2 'HARMONY_FG_PROBE_ONLY must be 0 or 1.'
  exit 2
fi
if [[ "$task_fg_probe_only" == 1 && ( "$task_shell_only" != 1 || "$task_copy_build_output" != 0 ) ]]; then
  print -u2 'FG experiments require HARMONY_SHELL_ONLY=1 and HARMONY_COPY_BUILD_OUTPUT=0.'
  exit 2
fi
if [[ "$task_fg_probe_only" == 1 || -n "$task_game_experiment_bundle" ]]; then
  python3 - "$task_repo_dir" "$task_stage_dir" <<'PYFGPATH'
import sys
from pathlib import Path
source, stage = (Path(p).resolve() for p in sys.argv[1:])
if source == stage or source in stage.parents or stage in source.parents:
    raise SystemExit('FG build stage must be outside the source checkout and must not contain it.')
PYFGPATH
fi
if [[ "$task_shell_only" != 0 && "$task_shell_only" != 1 ]]; then
  print -u2 'HARMONY_SHELL_ONLY must be 0 or 1.'
  exit 2
fi
if [[ "$task_copy_build_output" != 0 && "$task_copy_build_output" != 1 ]]; then
  print -u2 'HARMONY_COPY_BUILD_OUTPUT must be 0 or 1.'
  exit 2
fi
task_runtime_archive="$task_repo_dir/game-runtime/artifacts/libmuguet_game_runtime.a"
task_runtime_manifest="$task_repo_dir/game-runtime/artifacts/libmuguet_game_runtime.features.json"
task_stage_runtime="$task_stage_root/game-runtime/artifacts/libmuguet_game_runtime.a"
task_game_aot_object="${HARMONY_GAME_AOT_OBJECT:-$task_stage_root/game-runtime/artifacts/game-embedded.o}"
task_rife_sdk="${HARMONY_RIFE_SDK:-}"
task_npu_only="${HARMONY_FG_NPU_ONLY:-0}"
if [[ "$task_npu_only" != 0 && "$task_npu_only" != 1 ]]; then
  print -u2 'HARMONY_FG_NPU_ONLY must be 0 or 1.'
  exit 2
fi
if [[ "$task_npu_only" == 1 && -n "$task_rife_sdk" ]]; then
  print -u2 'HARMONY_FG_NPU_ONLY=1 cannot be combined with HARMONY_RIFE_SDK.'
  exit 2
fi
if [[ "$task_shell_only" == 0 ]]; then
  # A configured UI switch cannot enable code absent from the linked runtime.
  # Public shell/probe builds intentionally have no runtime and bypass this.
  python3 "$task_repo_dir/tools/verify_frame_generation_build.py" check-archive \
    --archive "$task_runtime_archive" --manifest "$task_runtime_manifest"
  if [[ "$task_npu_only" == 0 && ( -z "$task_rife_sdk" || ! -f "$task_rife_sdk/include/harmony_rife_c.h" || ! -f "$task_rife_sdk/lib/libharmony_rife_full.a" ) ]]; then
    print -u2 'Full-game FG requires HARMONY_RIFE_SDK with include/harmony_rife_c.h and lib/libharmony_rife_full.a.'
    exit 2
  fi
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
python3 - "$task_stage_dir/entry/build-profile.json5" "$task_shell_only" "$task_rife_sdk" "$task_stage_runtime" "$task_game_aot_object" "$task_npu_only" <<'PYSHELL'
import json
import re
import sys
from pathlib import Path
path = Path(sys.argv[1])
rife_sdk = str(Path(sys.argv[3]).resolve()) if sys.argv[3] else ''
if not rife_sdk.isascii() or any(c.isspace() for c in rife_sdk):
    raise SystemExit('HARMONY_RIFE_SDK requires an ASCII build path without whitespace.')
runtime = str(Path(sys.argv[4]).absolute())
aot_object = str(Path(sys.argv[5]).absolute())
if sys.argv[2] == '0' and (not runtime.isascii() or any(c.isspace() for c in runtime)):
    raise SystemExit('Full-game runtime requires an ASCII build stage without whitespace.')
if sys.argv[2] == '0' and (not aot_object.isascii() or any(c.isspace() for c in aot_object)):
    raise SystemExit('Full-game AOT object requires an ASCII build path without whitespace.')
profile = json.loads(path.read_text())
options = profile.setdefault('buildOption', {}).setdefault('externalNativeOptions', {})
arguments = re.sub(r'(?<!\S)-DMUGUET_SHELL_ONLY(?::[^=\s]+)?=[^\s]+', '', str(options.get('arguments', ''))).strip()
arguments = re.sub(r'(?<!\S)-DMUGUET_RIFE_SDK(?::[^=\s]+)?=[^\s]*', '', arguments).strip()
arguments = re.sub(r'(?<!\S)-DMUGUET_FG_NPU_ONLY(?::[^=\s]+)?=[^\s]*', '', arguments).strip()
arguments = re.sub(r'(?<!\S)-DGAME_RUNTIME_LIBRARY(?::[^=\s]+)?=[^\s]*', '', arguments).strip()
arguments = re.sub(r'(?<!\S)-DGAME_AOT_OBJECT(?::[^=\s]+)?=[^\s]*', '', arguments).strip()
options['arguments'] = (arguments + ' -DMUGUET_SHELL_ONLY=' + ('ON' if sys.argv[2] == '1' else 'OFF') +
                        ' -DMUGUET_FG_NPU_ONLY=' + ('ON' if sys.argv[6] == '1' else 'OFF') +
                        ' -DMUGUET_RIFE_SDK=' + rife_sdk +
                        (' -DGAME_RUNTIME_LIBRARY=' + runtime + ' -DGAME_AOT_OBJECT=' + aot_object if sys.argv[2] == '0' else '')).strip()
path.write_text(json.dumps(profile, indent=2) + '\n')
PYSHELL
if [[ "$task_fg_probe_only" == 1 ]]; then
  # Apply the FG loader metadata only to a separate, runtime-free experiment.
  # The working game manifest and its output directory remain independent.
  python3 - "$task_stage_dir" "${HARMONY_FG_PROBE_BUNDLE:-com.example.harmonyfgprobe}" "${HARMONY_FG_PROBE_FUSION_AWARE:-1}" <<'PYFG'
import json
import re
import sys
from pathlib import Path
root, bundle, fusion_aware = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
if fusion_aware not in ('0', '1'):
    raise SystemExit('HARMONY_FG_PROBE_FUSION_AWARE must be 0 or 1.')
if not re.fullmatch(r'[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+', bundle):
    raise SystemExit('Invalid FG experiment bundle name.')
app_path = root / 'AppScope/app.json5'
app = json.loads(app_path.read_text())
if bundle == app['app']['bundleName']:
    raise SystemExit('FG experiment must use a different bundle from the working game.')
app['app']['bundleName'] = bundle
app_path.write_text(json.dumps(app, indent=2) + '\n')
module_path = root / 'entry/src/main/module.json5'
module = json.loads(module_path.read_text())
data = module['module']
data['mainElement'] = 'GraphicsFrameProbeAbility'
probe = next(a for a in data['abilities'] if a['name'] == 'GraphicsFrameProbeAbility')
probe['skills'] = [{'entities': ['entity.system.home'], 'actions': ['ohos.want.action.home']}]
data['abilities'] = [probe]
data['metadata'] = [m for m in data.get('metadata', []) if m['name'] != 'GraphicsAccelerateKit_FusionAware']
if fusion_aware == '1':
    data['metadata'].append({'name': 'GraphicsAccelerateKit_FusionAware', 'value': 'Vulkan'})
module_path.write_text(json.dumps(module, indent=2) + '\n')
string_paths = list((root / 'AppScope/resources').glob('*/element/string.json'))
string_paths += list((root / 'entry/src/main/resources').glob('*/element/string.json'))
for path in string_paths:
    values = json.loads(path.read_text())
    for item in values.get('string', []):
        if item['name'] == 'app_name':
            item['value'] = 'AI FG 30 FPS Probe'
    path.write_text(json.dumps(values, ensure_ascii=False, indent=2) + '\n')
print('Independent FG experiment: ' + bundle + '; public SYSTEM presentation metadata ' + ('enabled' if fusion_aware == '1' else 'disabled for control'))
PYFG
fi
if [[ -n "$task_game_experiment_bundle" ]]; then
  python3 - "$task_stage_dir" "$task_game_experiment_bundle" <<'PYGAME'
import json
import re
import sys
from pathlib import Path
root, bundle = Path(sys.argv[1]), sys.argv[2]
if not re.fullmatch(r'[A-Za-z][A-Za-z0-9_]*(?:\.[A-Za-z][A-Za-z0-9_]*)+', bundle):
    raise SystemExit('Invalid full-game experiment bundle name.')
app_path = root / 'AppScope/app.json5'
app = json.loads(app_path.read_text())
if bundle == app['app']['bundleName']:
    raise SystemExit('Full-game experiment must use a different bundle from the working game.')
app['app']['bundleName'] = bundle
app_path.write_text(json.dumps(app, indent=2) + '\n')
module_path = root / 'entry/src/main/module.json5'
module = json.loads(module_path.read_text())
module['module']['metadata'] = [m for m in module['module'].get('metadata', [])
                              if m['name'] != 'GraphicsAccelerateKit_FusionAware']
module_path.write_text(json.dumps(module, indent=2) + '\n')
print('Private full-game experiment: ' + bundle + '; system FG loader metadata absent')
PYGAME
fi
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
if [[ "$task_shell_only" == 0 ]]; then
  python3 - "$task_runtime_archive" "$task_stage_runtime" <<'PYRUNTIMEPATH'
import sys
from pathlib import Path
if Path(sys.argv[1]).resolve(strict=True) != Path(sys.argv[2]).resolve(strict=True):
    raise SystemExit('Build stage runtime is not the capability-verified archive.')
PYRUNTIMEPATH
fi
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
if [[ "$task_build_status" == 0 && "$task_shell_only" == 0 ]]; then
  set +e
  for task_hap_name in entry-default-unsigned.hap entry-default-signed.hap; do
    task_checked_hap="$task_stage_dir/entry/build/default/outputs/default/$task_hap_name"
    if [[ "$task_hap_name" == entry-default-signed.hap && "${HARMONY_ENABLE_SIGNING:-false}" != true ]]; then
      continue
    fi
    python3 "$task_repo_dir/tools/verify_frame_generation_build.py" check-hap \
      --archive "$task_runtime_archive" --manifest "$task_runtime_manifest" \
      --hap "$task_checked_hap" --report "$task_checked_hap.runtime-capabilities.json"
    task_capability_status=$?
    if [[ "$task_capability_status" != 0 ]]; then
      task_build_status=$task_capability_status
      break
    fi
  done
  set -e
fi
if [[ "$task_build_status" == 0 && "$task_copy_build_output" == 1 && -d "$task_stage_dir/entry/build" ]]; then
  mkdir -p "$task_project_dir/entry/build"
  /usr/bin/rsync -a "$task_stage_dir/entry/build/" "$task_project_dir/entry/build/"
fi
print "Build stage: $task_stage_dir"
exit "$task_build_status"
