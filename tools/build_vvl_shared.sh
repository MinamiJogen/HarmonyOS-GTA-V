#!/usr/bin/env bash
# Build a separate OHOS validation layer using the application's shared C++ ABI.
# Existing static builds, source edits and installed dependencies stay untouched.
set -Eeuo pipefail
vvl_repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
vvl_checkout="$vvl_repo_dir/.tools/ohos-vulkan-validationlayers"
vvl_output="${VVL_SHARED_OUTPUT_ROOT:-$vvl_checkout/build-ohos-shared}"
vvl_source="$vvl_output/source"
vvl_build="$vvl_output/build/layer"
vvl_artifacts="$vvl_output/artifacts"
vvl_logs="$vvl_output/logs"
vvl_deps="$vvl_checkout/build-ohos/install"
vvl_sdk="${VVL_NATIVE_SDK:-/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native}"
vvl_cmake="$vvl_sdk/build-tools/cmake/bin/cmake"
vvl_ninja="$vvl_sdk/build-tools/cmake/bin/ninja"
vvl_jobs="${VVL_SHARED_JOBS:-4}"
vvl_old="$vvl_checkout/build-ohos/artifacts/libVkLayer_khronos_validation.so"
if [[ ! "$vvl_jobs" =~ ^[1-9][0-9]*$ ]]; then
  printf 'VVL_SHARED_JOBS must be a positive integer.\n' >&2
  exit 2
fi
for vvl_path in "$vvl_cmake" "$vvl_ninja" "$vvl_old" \
  "$vvl_deps/SPIRV-Tools/lib/libSPIRV-Tools-opt.a" \
  "$vvl_deps/Vulkan-Utility-Libraries/lib/libVulkanLayerSettings.a"; do
  if [[ ! -f "$vvl_path" ]]; then
    printf 'Required existing build input is absent: %s\n' "$vvl_path" >&2
    exit 2
  fi
done
mkdir -p "$vvl_source" "$vvl_artifacts" "$vvl_logs"
vvl_revision="$(git -C "$vvl_checkout" rev-parse HEAD)"
if [[ -f "$vvl_output/source-revision.txt" ]]; then
  if [[ "$(cat "$vvl_output/source-revision.txt")" != "$vvl_revision" ]]; then
    printf 'The frozen source revision differs; select a new VVL_SHARED_OUTPUT_ROOT.\n' >&2
    exit 2
  fi
else
  # The checkout also contains an optional local diagnostic patch. Freeze HEAD
  # independently so the runtime-linkage experiment does not silently mix it in.
  git -C "$vvl_checkout" archive --format=tar "$vvl_revision" > "$vvl_output/source.tar"
  tar -xf "$vvl_output/source.tar" -C "$vvl_source"
  printf '%s\n' "$vvl_revision" > "$vvl_output/source-revision.txt"
fi
git -C "$vvl_checkout" diff --binary > "$vvl_output/excluded-checkout.patch"
shasum -a 256 "$vvl_old" > "$vvl_output/old-artifact-before.sha256"
vvl_prefix="$vvl_deps/Vulkan-Headers;$vvl_deps/SPIRV-Headers;$vvl_deps/Vulkan-Utility-Libraries;$vvl_deps/robin-hood-hashing;$vvl_deps/SPIRV-Tools"
"$vvl_cmake" -S "$vvl_source" -B "$vvl_build" -G Ninja \
  -DCMAKE_MAKE_PROGRAM="$vvl_ninja" \
  -DCMAKE_TOOLCHAIN_FILE="$vvl_sdk/build/cmake/ohos.toolchain.cmake" \
  -DOHOS_ARCH=arm64-v8a -DOHOS_STL=c++_shared -DOHOS_PLATFORM_LEVEL=26 \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_EXPORT_COMPILE_COMMANDS=ON \
  -DCMAKE_PREFIX_PATH="$vvl_prefix" -DCMAKE_INSTALL_PREFIX="$vvl_output/install/layer" \
  -DVVL_CODEGEN=OFF -DUPDATE_DEPS=OFF -DBUILD_TESTS=OFF -DBUILD_WERROR=OFF \
  > "$vvl_logs/configure.log" 2>&1 || { tail -n 60 "$vvl_logs/configure.log" >&2; exit 1; }
printf 'Configured independent c++_shared layer; building with %s jobs.\n' "$vvl_jobs"
"$vvl_cmake" --build "$vvl_build" --parallel "$vvl_jobs" \
  > "$vvl_logs/build.log" 2>&1 || { tail -n 80 "$vvl_logs/build.log" >&2; exit 1; }
"$vvl_sdk/llvm/bin/llvm-strip" -d -s \
  -o "$vvl_artifacts/libVkLayer_khronos_validation.so" \
  "$vvl_build/layers/libVkLayer_khronos_validation.so"
# Include the exact matching SDK runtime as a comparison/reference artifact.
# The app's native build normally packages this dependency automatically.
cp "$vvl_sdk/llvm/lib/aarch64-linux-ohos/c++/libc++_shared.so" "$vvl_artifacts/libc++_shared.so"
cp "$vvl_source/LICENSE.txt" "$vvl_artifacts/LICENSE.txt"
cp "$vvl_source/scripts/ohos-patches/VkLayer_khronos_validation.json" "$vvl_artifacts/VkLayer_khronos_validation.json"
python3 - "$vvl_output" "$vvl_checkout" "$vvl_sdk" <<'PY'
from pathlib import Path
import datetime
import hashlib
import json
import re
import subprocess
import sys

root, checkout, sdk = map(Path, sys.argv[1:])
artifacts = root / 'artifacts'
llvm = sdk / 'llvm/bin'
old = checkout / 'build-ohos/artifacts/libVkLayer_khronos_validation.so'
new = artifacts / 'libVkLayer_khronos_validation.so'
unstripped = root / 'build/layer/layers/libVkLayer_khronos_validation.so'
runtime = artifacts / 'libc++_shared.so'

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def run(tool, *args):
    return subprocess.check_output([str(llvm / tool), *map(str, args)], text=True)

def write_report(name, tool, *args):
    result = run(tool, *args)
    (artifacts / name).write_text(result)
    return result

old_dynamic = write_report('old-static-dynamic.txt', 'llvm-readelf', '-d', old)
new_dynamic = write_report('shared-dynamic.txt', 'llvm-readelf', '-h', '-d', new)
new_symbols = write_report('shared-symbols.txt', 'llvm-nm', '-C', unstripped)
runtime_symbols = write_report('runtime-dynamic-symbols.txt', 'llvm-readelf', '--dyn-symbols', runtime)
new_needed = re.findall(r'Shared library: \[(.*?)\]', new_dynamic)
old_needed = re.findall(r'Shared library: \[(.*?)\]', old_dynamic)
assert 'libc++_shared.so' in new_needed, new_needed
assert 'libc++_shared.so' not in old_needed, old_needed
assert not re.search(r'\b[btTwW]\s+__cxa_get_globals(?:_fast)?$', new_symbols, re.M)
assert not re.search(r'\b[btTwW]\s+__cxa_thread_atexit$', new_symbols, re.M)
assert re.search(r'\b__cxa_get_globals$', runtime_symbols, re.M)
assert sha(old) == (root / 'old-artifact-before.sha256').read_text().split()[0]
callback_markers = [line for line in new_symbols.splitlines() if re.search(
    r'__cxa_get_globals|__cxa_thread_atexit|pthread_key_create|emutls.*destruct|emutls_pthread_key', line)]
report = {
    'builtUTC': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'sourceRevision': (root / 'source-revision.txt').read_text().strip(),
    'sourcePolicy': 'Clean git archive of HEAD; existing checkout modifications excluded and saved separately.',
    'excludedCheckoutPatchSHA256': sha(root / 'excluded-checkout.patch'),
    'configuration': {'OHOS_ARCH': 'arm64-v8a', 'OHOS_STL': 'c++_shared', 'requestedOHOS_PLATFORM_LEVEL': 26,
                      'compilerTarget': 'aarch64-linux-ohos',
                      'sdkPackage': json.loads((sdk / 'oh-uni-package.json').read_text()),
                      'platformFlagNote': 'This SDK toolchain does not consume OHOS_PLATFORM_LEVEL for compiler target selection. The SDK package and actual compiler target are recorded instead of claiming an API availability check.',
                      'dependenciesReusedReadOnly': str(checkout / 'build-ohos/install')},
    'oldArtifact': {'path': str(old), 'sha256': sha(old), 'bytes': old.stat().st_size,
                    'needed': old_needed, 'unchangedFromBeforeBuild': True},
    'sharedArtifact': {'path': str(new), 'sha256': sha(new), 'bytes': new.stat().st_size,
                       'needed': new_needed, 'unstripped': str(unstripped)},
    'matchingSharedRuntime': {'path': str(runtime), 'sha256': sha(runtime), 'bytes': runtime.stat().st_size,
                              'sdkPath': str(sdk / 'llvm/lib/aarch64-linux-ohos/c++/libc++_shared.so')},
    'callbackAudit': {'localCxaGetGlobalsDefinitionPresent': False,
                       'localCxaThreadAtexitDefinitionPresent': False,
                       'matchingRuntimeExportsCxaGetGlobals': True,
                       'sharedLayerRelevantSymbols': callback_markers,
                       'interpretation': 'The exception-runtime TSD implementation is no longer embedded in the unloadable layer. Keep libc++_shared mapped through application/worker lifetime; a DT_NEEDED dependency alone is not a universal unload-safety proof.'},
    'packageSelection': 'HARMONY_WITH_VVL=1 HARMONY_VVL_SO=' + str(new) + ' bash tools/build_fg_probe.sh <independent.bundle>',
    'acceptanceBoundary': 'Cross-build and ELF audit only; not installed, loaded or tested on a device by this build script.',
}
(artifacts / 'build-evidence.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps({'artifact': str(new), 'sha256': sha(new), 'needed': new_needed,
                  'oldArtifactUnchanged': True, 'callbackMarkers': callback_markers}, indent=2))
PY
printf 'Independent shared-runtime layer ready: %s\n' "$vvl_artifacts/libVkLayer_khronos_validation.so"
