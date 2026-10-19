# Vulkan capability probe

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
