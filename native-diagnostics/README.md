# Vulkan capability probe

## Independent AI frame generation experiment

Build a separate diagnostic application with the public HMS Vulkan AI flow:

```sh
bash tools/build_fg_probe.sh com.example.harmonyfgprobe
python3 native-diagnostics/test_frame_rate_policy.py
```

The build prints its ASCII project and unsigned HAP paths. It includes no game
runtime, embedded AOT or game resources. The experiment enables
`GraphicsAccelerateKit_FusionAware=Vulkan` in its generated manifest and exposes
only `GraphicsFrameProbeAbility`; the working game's manifest is not changed.
`HARMONY_FG_PROBE_FUSION_AWARE=0` omits that metadata for a loader control. Native
AI system presentation then explicitly refuses to run; inspection and FG-OFF
comparisons remain available.
The default build includes the locally built shared-C++ Vulkan validation layer
from `tools/build_vvl_shared.sh`. The prior static-C++ layer crashed at worker
exit after the loader unmapped its thread-local destructor. The shared layer
requires the app's matching `libc++_shared.so` to remain mapped. To use a
different layer, set `HARMONY_VVL_SO`; `HARMONY_WITH_VVL=0` creates an unvalidated
comparison package. Sign the generated project in DevEco using a profile for its
own bundle and the target device, then install the signed HAP.

Packaging the layer alone does not enable it. Following the
[OpenHarmony Vulkan loader instructions](https://github.com/openharmony/third_party_vulkan-loader/blob/master/README_OpenHarmony.md),
put its `VkLayer_khronos_validation.json` manifest in the app's files directory
(with `library_path` set to `libVkLayer_khronos_validation.so`), then set both
`debug.graphic.debug_layer=VkLayer_khronos_validation` and
`debug.graphic.debug_hap=<experiment bundle>` before starting the process.
Save and restore both previous parameter values after testing. Check the actual
`validationLayerEnabled` and `validationNegativeControlObserved` fields; a zero
error count with an inactive layer establishes no validation pass. Debug results
can be retrieved using `hdc file recv -b <bundle> <sandbox path> <local path>`.

Each process performs one bounded trial. Stop it before changing parameters:

```sh
hdc shell aa force-stop com.example.harmonyfgprobe
hdc shell aa start -b com.example.harmonyfgprobe -a GraphicsFrameProbeAbility \
  --pb fg_enabled true --pb fg_geometry true --pb fg_ui_prediction false \
  --pi fg_seconds 20 --pi fg_source_fps 15 --pi fg_target_fps 30
```

Defaults are 15 source frames/s and a requested 30 output frames/s. Source rates
must be integer 15–60 and output targets integer 30–144; enabled FG requires
source < target. An OFF comparison uses the same source rate and geometry with
`fg_enabled=false`. Explicit source 30 / target 60 reproduces the earlier
cadence. Geometry motion uses elapsed time, so its speed is unchanged by rate.

Results are saved under `filesDir/graphics-frame-surface-<pid>/`, including the
actual installed module metadata, target setter result, source submits, real
presents, validation messages and geometry readback qualification. AI receives
real color and the synthetic scene's real VP/inverse, with no depth input as
required by the [official AI Vulkan guide](https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/graphics-accelerate-fg-ai-vulkan).
These synthetic camera matrices are not used for the game's renderer.

`HMS_FG_SetTargetFps_VK` is called before activation. Its public SDK contract says
the setting takes effect after the game's first release. A successful setter or
Dispatch does not establish effective 30fps output on a sideloaded package.
`effectiveOutputTargetHz`, `generatedFramesVerified` and `displayFpsVerified`
remain null until independently measured. Compare source timestamps with system
composition timestamps and actual distinct output images; panel refresh and
source-rate multiplication are insufficient. This experiment does not enable
frame generation in the working game.

For an interface-only audit, start a fresh process with
`--pb fg_inspect_only true`. This creates no Vulkan context or XComponent surface;
`filesDir/fg-public-interface-audit.json` records the five public capability
queries and the actual system library owning the directly bound FG functions.
Global `dlsym` visibility can differ from direct ELF bindings in OHOS library
namespaces and must not be treated as a missing public API. The diagnostic
Ability also registers the existing HiAppEvent crash collector. In inspection
mode it additionally uses the deprecated public FaultLogger query to retrieve
older self C++ crash logs that predate watcher registration. Collected logs are
restricted to this diagnostic application's UID/bundle. A completed native
result and zero unexpected VUIDs do not establish that the worker/process exited
normally; inspect the corresponding crash events too.

Inspection also writes `fg-model-access-audit.json`: metadata and read-only open
checks for the two fixed stock model paths used by the inspected public
capability implementation, the current directory, and bounded names-only lists
of the core's two model directories. It does not identify the path selected by
a particular FG context. Add `--pb fg_model_metadata true` to inspection mode for
an asynchronous public NNRT build/descriptor audit in
`fg-model-metadata-audit.json`. This executes no inference and establishes no
model semantics or FG compatibility.

### Alternative neural interpolation checks

The explicit inspection flags `fg_npu_flow_benchmark=true` and
`fg_rife_benchmark=true` run offscreen synthetic pixel-pair tests. Both also
require `fg_inspect_only=true`. The former uses public NNRT/CANN and the stock
`aifi2.om`, read and transformed only in memory; it exports no model bytes and
tests packing/flow interpretation candidates. The latter requires the optional
`HARMONY_RIFE_SDK` archive/header and the pinned RIFE v4.6 model in the app's
`filesDir/rife-v4.6`. Results are `npu-flow-benchmark-result.json` and
`rife-benchmark-result.json`, plus generated PPM images. Neither runs the game
or verifies display FPS.

With `HARMONY_FG_PROBE_FUSION_AWARE=0`, `HARMONY_WITH_VVL=0` and the RIFE SDK,
start `GraphicsFrameProbeAbility --pb fg_rife_stream true --pi fg_seconds 30`
to test a live 15Hz synthetic source, neural midpoint worker and 30Hz Vulkan
output deadlines with a 200ms buffer. This path omits system FG metadata and
does not call the failing system AI dispatch. It logs each source/mid/repeat
present and bounded queue counters. Inspect its exact XComponent surface with
`hidumper -s 10 -a 'graphics-frame-surface-probeSurface fps'`; composer refresh
alone is insufficient.

On the 2026-10-19 MatePad Pro Max / Maleoon 935 trial, the live RIFE path made
451 source and 451 midpoint presents in 30 seconds, with zero repeats or skipped
slots. Independent RenderService fence timestamps covered the first 217 updates
(7.19 seconds), averaging 30.034Hz. Intervals were uneven (mostly 16.7/50ms);
this establishes the tested surface's average update cadence, not uniform
33.3ms pacing, full-resolution quality or game acceptance. Evidence is in
`evidence/device/matepad-pro-max/fg30-20261019/rife-stream-analysis.json`.

The same device's public stock-model test completed all RunSync calls with
finite outputs, approximately 7ms median inference. Packed RGBA input and XY
flow01→older/flow23→newer produced translation MAE 3.04 versus repeat 66.81 and
blend 51.32. This is one synthetic numerical test; backend options alone do not
independently prove hardware placement or production quality.

## Capability probe

Built with the installed HarmonyOS API 26 native SDK (26.0.0.105) and official
Vulkan headers. The compiler target is `aarch64-linux-ohos26`; no Linux glibc or
Android target is substituted. Vulkan is loaded with `dlopen("libvulkan.so")`,
and entry points are obtained through `vkGetInstanceProcAddr` with `dlsym`
fallbacks. No direct Vulkan loader link is needed.

```sh
bash native-diagnostics/build-vulkan-probe.sh
```

`bin/vulkan-probe` is an AArch64 PIE with `/lib/ld-musl-aarch64.so.1` interpreter
and only `libc.so` in `DT_NEEDED`. It requests a Vulkan 1.0 instance and reports:

- Loader API version and instance/device extensions.
- Each physical device name, API, driver raw version, all 55 Vulkan 1.0 feature
  booleans, memory heaps/types and selected limits.
- Sampled-image and linear-filter format bits for 16 BC and 28 core ASTC formats.
- Whether `VK_OHOS_surface` is advertised, enabled and its entry point exposed.

No logical device, native window, surface, swapchain, shader, image or presentation
is created or tested. Enumeration success does not establish game compatibility.
Heap sizes are advertised Vulkan heaps, not a free-memory or process-allocation
budget. Format bits do not establish successful allocation or rendering.

The CLI prints one JSON object to stdout. Exit 0 means the enumerations completed,
1 means a structured loader/Vulkan failure, and 2 means invalid arguments. A
device policy may reject standalone execution before `main`; that shell error
cannot be JSON produced by this probe. The same source is therefore also exposed
for execution inside a signed HAP.

## Signed HAP integration

```cmake
add_subdirectory(/absolute/path/native-diagnostics native-diagnostics-build)
target_link_libraries(your_napi_target PRIVATE muguet_vulkan_probe)
```

```cpp
#include "muguet_vulkan_probe.h"
std::string report = muguet_vulkan_probe_json();
```

The C ABI returns `const char *` owned by a thread-local string. Do not free it;
copy it on the calling thread before the next call there or before thread exit.
The static CMake target uses PIC and exposes its include path and `dl` dependency.
`main` is compiled only with `MUGUET_VULKAN_PROBE_STANDALONE=1`.

## Verification

```sh
python3 native-diagnostics/test_host_report.py
```

Four host scenarios passed: missing loader, synthetic physical-device enumeration,
synthetic zero-device failure, and the C ABI JSON export. The mock loader names
its device `SYNTHETIC HOST FIXTURE`; its output is not device or GPU evidence.
`logs/host-tests.log` records these results. `logs/build-vulkan-probe.log` records
the successful SDK compilation and ELF headers/dependencies. An additional API26
PIC object compilation produced `bin/vulkan-probe-hap.o`; its defined symbols
include `muguet_vulkan_probe_json` and no `main`.

## Isolated buffer upload and command pool probe

`src/vulkan_buffer_probe.cpp` is an independent diagnostic, linked into the signed
Harmony application through an optional request-file entry. It creates a real Vulkan 1.1 device and buffers,
without the game, Wasmtime, wgpu, shaders or presentation. Build only:

```sh
bash native-diagnostics/build-vulkan-buffer-probe.sh
```

This produces a PIC static archive with the C ABI from
`include/muguet_vulkan_buffer_probe.h`, plus an optional guarded-main executable.
The device has already rejected unbundled native executables; do not treat the
executable as a tested device launch path. The C ABI
`muguet_vulkan_buffer_probe_json(&config, tracePath)` runs on a NAPI async worker
when `filesDir/vulkan-buffer-probe-request.json` exists. In that mode the home
page creates no XComponent and never starts the game. Without that file, the
normal game path is unchanged. The request uses the config field names below
with numeric values; absent fields use defaults. Completion writes
`filesDir/vulkan-buffer-probe-result.json`; before/after critical calls write
`filesDir/vulkan-buffer-probe-last.json`. Restart the process between cases.

The same request marker selects the separate Rust/wgpu offscreen atlas diagnostic
with `{"probe_kind":"wgpu_atlas","iterations":64}`. It runs synchronously on
the existing NAPI async worker, without starting the game or creating an
XComponent. Its returned JSON is saved as
`filesDir/wgpu-atlas-probe-result.json`; Rust writes the independent stage trace
`filesDir/wgpu-atlas-probe-trace.jsonl`. Iterations must be 1 through 256, default
64. An absent `probe_kind` or `"buffer"` retains the buffer case and its existing
result paths. This routing alone does not establish a GPU or pixel-check pass.

The default target is 20 MiB plus the four bytes observed in the actual Vulkan
buffer (`20971524` bytes); `--buffer-bytes 20971520` selects exactly 20 MiB.
Staging is a 4096-byte VkBuffer. One case writes exactly 3240 bytes at a selected
destination offset, normally `9004776` or `9581136`. The source pattern changes
each iteration. No neighboring bytes are padded or rewritten.

Independent case controls are:

- `allocation=0/1/2`: shared allocation at requested target binding offset
  `5349568`, independent zero-offset allocation, or zero-offset allocation with
  `VkMemoryDedicatedAllocateInfo`. Shared allocation also binds two independent
  64 KiB sentinel buffers immediately before/after the target's required memory
  ranges. The driver-required alignment can adjust requested binding offsets;
  actual offsets, memory types and flags are recorded in JSON.
- `upload=0/1`: `vkCmdCopyBuffer` from the 4 KiB staging buffer, or
  `vkCmdUpdateBuffer` from the same CPU data. Both preserve the exact byte range.
- `pool_policy=0/1`: after a completed fence, either
  `vkResetCommandPool(flags=0)` or destroy/create the pool. Each pool allocates
  16 primary command buffers at once, matching the game allocator batch size.
  Default `reset_every=1` uses the last buffer, completes the submission, then
  recycles the pool; `reset_every=16` exercises every allocated buffer first.
- `iterations=64` by default; `destination_offset` selects one crash-observed
  offset. `staging_binding_offset=0` is fixed across allocation comparisons;
  `23150592` or `19705856` can reproduce the two observed staging offsets.
- `validation_negative_control=1` optionally enables `VK_EXT_debug_utils`, creates
  a messenger, and issues one intentionally zero-severity debug message before
  creating GPU buffers. All other callback data and message type are valid. The
  normal buffer case still runs; final success additionally requires a real
  callback for `VUID-vkSubmitDebugUtilsMessageEXT-messageSeverity-parameter` and
  no unrelated validation errors. Absence of a layer, missing extension/function
  or missing VUID is reported as failure. Default is 0; the HAP JSON accepts
  `validation_negative_control: true` (or numeric 1).

Each upload is followed by an explicit transfer dependency, readback of its full
3240-byte range and 64 bytes on each side, a fence wait, noncoherent invalidation
when required, and byte-by-byte checking. A final readback verifies the entire
target and both 64 KiB neighbor sentinels. Submissions and command-pool reuse are
strictly sequential, so an in-flight pool reset is excluded from this test.
Fence waits are bounded at 30 seconds. If completion fails, live Vulkan resources
are deliberately left allocated and the diagnostic process must be restarted.
The thread-local result and before/after trace JSON report raw handles, pool
generation, completed submissions, verified byte counts and first mismatch.

Run each allocation/method/offset/pool case in a separate process when integrated,
since a native driver crash cannot be caught as a C++ error. A passing isolated
case bounds that exact workload; it does not validate game rendering or exclude
failures that require the game's resource/descriptor history. Current validation
is local API 26 compilation with `-Wall -Wextra -Werror` and ELF/C ABI inspection;
the local build report alone claims no GPU execution or byte/readback result.

The constraints follow the Vulkan references for
[vkCmdCopyBuffer](https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdCopyBuffer.html),
[vkCmdUpdateBuffer](https://docs.vulkan.org/refpages/latest/refpages/source/vkCmdUpdateBuffer.html),
and [dedicated allocation](https://docs.vulkan.org/refpages/latest/refpages/source/VkMemoryDedicatedAllocateInfo.html).
