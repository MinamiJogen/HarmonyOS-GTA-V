# HarmonyOS API 26 game shell and runtime probe

The home page creates a native XComponent surface and starts the actual OHOS
game runtime when its static library and signed game AOT object are linked. It
uses the application's real `filesDir/game` resource directory, preserves saves
in `filesDir/saves`, and writes `game-status.json` for device evidence. Runtime
initialization is asynchronous. A surface or accepted start request is never
reported as evidence that game rendering succeeded; the page reports presents
only after the renderer increments its actual presentation count.

The ability requests landscape, full-screen layout, and
`setWindowKeepScreenOn(true)`. `GameRuntime` hilog prints `filesDir`, `cacheDir`,
and the expected resource root. `HarmonyGame` logs XComponent creation and the
start result. The overlay supplies held WASD, Enter, Escape, F, Space, Q/E/R,
Shift, aiming and firing; direct surface drags feed camera movement through the
runtime's native touch input. A keyboard can also supply native OHOS key codes.

The status panel opens a separate diagnostic page for the original
ArkTS → C++ Node-API → Rust FFI and embedded Wasmtime tests.

For an isolated Vulkan upload/pool experiment, place a numeric configuration in
`filesDir/vulkan-buffer-probe-request.json` before starting a fresh process. The
home page then skips the game and XComponent, runs the independent native probe
on a NAPI async worker, and displays completion without blocking ArkUI. The
worker writes `vulkan-buffer-probe-result.json` and a before/after
`vulkan-buffer-probe-last.json` trace in `filesDir`. Remove the request file and
restart the process to resume normal game startup. Configuration fields and
the byte/sentinel checks are documented in `native-diagnostics/README.md`.

The diagnostic page automatically runs the probe when opened. It separately reports the native
bridge, Rust addition, whether the library includes Wasmtime support, and whether
an AOT module was actually executed. An absent library, disabled runtime, or empty
module path is never reported as an AOT pass.

## Build

Run `./harmony/build_unsigned.sh` from the repository root. The script uses the
bundled DevEco Studio tools at `/Applications/DevEco-Studio.app` by default.
Set `DEVECO_CONTENTS` if the installation lives elsewhere. Builds default to
`debug`; set `HARMONY_BUILD_MODE=release` to use release code generation. The
local signing wrapper still uses the existing device provisioning profile;
release code generation does not make that a distribution signature.

Hvigor rejects non-ASCII project paths. The script mirrors project sources to
`/private/tmp/harmony-runtime-probe-$UID/harmony`, builds there, then copies
`entry/build` back. The runtime-probe directory is referenced through a symlink;
other projects are neither edited nor built. An existing mirror build profile is
preserved so that DevEco-generated signing material stays local to that mirror.
To intentionally replace it with the unsigned source profile, set
`HARMONY_RESET_STAGE_PROFILE=1`.

The unsigned package is `entry/build/default/outputs/default/entry-default-unsigned.hap`.
It needs a valid device-bound HarmonyOS signature before installation.
No signing passwords or profiles are included in this project. Once DevEco
has configured automatic signing in the private mirror, run
`./harmony/build_signed_local.sh` to build using that existing profile. The signed
package is copied back to `entry/build/default/outputs/default/` alongside the
unsigned package. The wrapper checks that a product actually references local
signing material and does not create or print credentials.

`HARMONY_WITH_VVL=1` optionally adds the official OHOS validation-layer library
to `entry/libs/arm64-v8a` in the private ASCII mirror, after source synchronization.
`HARMONY_VVL_SO` can override its source path; otherwise the artifact under
`.tools/ohos-vulkan-validationlayers/build-ohos/artifacts/` is used. The default
is OFF. Every successful build checks that the HAP includes the layer exactly
when requested; OFF also removes the generated layer from the mirror. This does
not change device parameters or copy a layer manifest into the sandbox.

The optional Vulkan buffer request accepts `validation_negative_control: true`.
It enables `VK_EXT_debug_utils`, registers a messenger, and submits one debug
message with zero severity and otherwise valid parameters before the normal
buffer case. Success requires actually observing
`VUID-vkSubmitDebugUtilsMessageEXT-messageSeverity-parameter` through that
messenger, plus successful buffer/sentinel checks and no unexpected validation
errors. Missing layer evidence cannot produce a pass. The default is false.

## Rust linkage

The game path additionally links
`game-runtime/artifacts/libmuguet_game_runtime.a` (the native archive with Rust
LLVM metadata removed for the SDK linker),
`game-runtime/artifacts/game-embedded.o`, and the native OHAudio adapter.
The game's trusted AOT bytes are mapped into the signed executable image using
`mg_game_aot_start/end`. A build with the runtime archive but no AOT object fails
at configuration. A build without the runtime archive remains installable for
surface/path diagnosis and clearly reports that the game runtime is missing.

XComponent surface destruction requests a runtime stop before returning. The
current runtime has process-level globals; restart the application process to
initialize a second game session or a replacement surface.

CMake checks for
`../runtime-probe/target/aarch64-unknown-linux-ohos/release/libharmony_runtime_probe.a`
relative to this project. When it exists, the C++ bridge links the static library.
Otherwise it builds only the C++ diagnostic and reports Rust as not linked.
The Rust library must match the C ABI declared in `harmony_probe.cpp`.

C ABI: `rp_add`, `rp_version`, `rp_last_error`, `rp_runtime_available`,
`rp_run_aot_add`, `rp_run_raw_aot_add`. The AOT input must be a trusted, architecture-compatible
Wasmtime serialized artifact. The prototype accepts an absolute application-readable
path. It does not deserialize any game data.

If `runtime-probe/artifacts/{add,memory64,shared,shared-memory64}-embedded.o`
exist alongside the Rust library, CMake links the trusted serialized AOT bytes
into the native library's executable mapping. The prototype automatically runs
each available case and records its independent status and result. All four
small modules should return 42. The memory64 cases use an i64 linear-memory
address; shared cases execute atomic store/add/load on one calling thread.
These cases do not establish concurrent wait/notify or a complete game's runtime
compatibility. The result JSON and hilog include each embedded address and length
for repeated-launch address-layout checks.
The final native ELF program headers must show that the embedded bytes are in
an executable load segment. This is distinct from the optional file-path probe,
which exercises the operating system's mapping and protection behavior.

## Device verification

Bundle: `com.shuyuan.harmonyosgtav`; ability: `EntryAbility`.
Launch and inspect hilog with tag `HarmonyRuntimeProbe`. A UI result or hilog
entry is required to establish device success; a successful package build alone
only establishes compilation and packaging.

The Vulkan probe is linked from `../native-diagnostics` when available. It creates
an instance and enumerates capabilities without creating a logical device,
surface, or swapchain. The UI shows device/version and BC/ASTC support. The
complete JSON is written to the app's `filesDir/vulkan-report.json` and
logged as ordered `VULKAN_JSON[index/total]` chunks below the hilog message limit.
The complete probe result with an ISO timestamp is also saved to
`filesDir/probe-result.json`. The UI displays both evidence paths.
Enumeration success does not establish rendering or presentation support.

Initial build evidence is in `logs/build-baseline.log`. It was compiled with
`RP_HAS_RUST=0`; `artifacts/cpp-baseline-unsigned.hap` preserves that package.
