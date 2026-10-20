#!/usr/bin/env bash
# Build an independent AI SYSTEM presentation experiment without game/AOT/data.
set -Eeuo pipefail
task_repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
task_bundle="${1:-com.example.harmonyfgprobe}"
if [[ $# -gt 1 || "$task_bundle" == --* ]]; then
  printf 'Usage: bash tools/build_fg_probe.sh [separate.bundle.name]\n' >&2
  exit 2
fi
export HARMONY_BUILD_STAGE="${HARMONY_BUILD_STAGE:-$(mktemp -d /private/tmp/harmony-fg30-probe.XXXXXXXX)}"
export HARMONY_FG_PROBE_ONLY=1 HARMONY_FG_PROBE_BUNDLE="$task_bundle"
export HARMONY_SHELL_ONLY=1 HARMONY_COPY_BUILD_OUTPUT=0 HARMONY_ENABLE_SIGNING=false
export HARMONY_WITH_VVL="${HARMONY_WITH_VVL:-1}"
if [[ "$HARMONY_WITH_VVL" == 1 ]]; then
  # A layer with an embedded static C++ runtime leaves a stale thread-local
  # destructor on the tested OHOS loader. Use the separately built shared one.
  export HARMONY_VVL_SO="${HARMONY_VVL_SO:-$task_repo_dir/.tools/ohos-vulkan-validationlayers/build-ohos-shared/artifacts/libVkLayer_khronos_validation.so}"
fi
export HARMONY_BUILD_MODE=debug
"$task_repo_dir/harmony/build_unsigned.sh"
task_hap="$HARMONY_BUILD_STAGE/harmony/entry/build/default/outputs/default/entry-default-unsigned.hap"
python3 - "$task_hap" "$task_bundle" "${HARMONY_FG_PROBE_FUSION_AWARE:-1}" <<'PY'
import json
import sys
import zipfile
with zipfile.ZipFile(sys.argv[1]) as package:
    manifest = json.loads(package.read('module.json'))
    assert manifest['app']['bundleName'] == sys.argv[2]
    module = manifest['module']
    assert module['mainElement'] == 'GraphicsFrameProbeAbility'
    metadata = [m for m in module.get('metadata', []) if m['name'] == 'GraphicsAccelerateKit_FusionAware']
    assert metadata == ([{'name': 'GraphicsAccelerateKit_FusionAware', 'value': 'Vulkan'}] if sys.argv[3] == '1' else [])
    assert [a['name'] for a in module['abilities']] == ['GraphicsFrameProbeAbility']
    assert not any(n.endswith(('.wasm', '.cwasm', '.rpf')) for n in package.namelist())
print('Independent experiment package verified: ' + sys.argv[1])
PY
printf '\nUnsigned FG experiment: %s\nSign with a profile for %s before installing.\n' "$task_hap" "$task_bundle"
