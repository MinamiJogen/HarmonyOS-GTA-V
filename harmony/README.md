# HarmonyOS API 26 game shell and runtime probe

The current application version is **1.0.1** (`versionCode` **1000101**). Full
local builds include checked frame-interpolation capabilities, live processing
status, and readback-cache recovery after nonfatal graphics errors. Resource
build **8b0b5899ed** is unchanged: existing matching `data/` and shader files, or
the previously verified resource-only ZIP, remain compatible. The matching game
AOT is embedded in the local full HAP; no separate resource update is required.

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

The launcher offers New Game (Story with `-noautoload`), Load Save (a Story
save picker, or an empty-list dialog), and Demo (a separate sandbox session).
The ZIP save importer sits below the game-resource importer. Save containers,
limits, compatibility and backup behavior are described in [SAVE_FORMAT.md](SAVE_FORMAT.md).
Both the launcher and the original game menus use the same native save directory;
the guest filesystem normalizes the original engine’s Windows path separators
and syncs writes. A temporary guest read view selects a startup slot without
rewriting other saves and ends only on an engine load result. The overlay has a
menu-key toggle for arrows, Enter, Backspace and Q/E tabs. A confirmed original
Story ManualLoad publishes its exact normal slot after the original completion
branch and queue terminal; ArkUI restarts the app with validated one-shot Want
parameters and loads that slot in a fresh process. A file read is never reported
as restored progress: the new guest must emit its DeSerialize success log. Returning Home warns
about unsaved progress and restarts the application for a fresh game session.

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

The independent `GraphicsFrameProbeAbility` accepts two optional diagnostic Want
parameters, both `false` by default: `fg_swapchain_sampled=true` adds swapchain
`SAMPLED` usage only after confirming the surface and selected format support it;
an unsupported request fails explicitly. `fg_final_depth_state=true` supplies a
non-null, disabled depth/stencil pipeline state for the final fullscreen pass
when `fg_geometry=true`; it adds no depth attachment or AI depth input. Run each
control separately from the baseline. `trace.jsonl` and `result.json` record
`swapchainSampledRequested`, `swapchainSampledEnabled`, surface supported,
requested and actual usage flags, plus `finalDepthStateRequested` and
`finalDepthStateProvided`. AI configuration and image dimensions are unchanged.

`fg_inspect_only=true` also writes `fg-model-access-audit.json` for the fixed
`/sys_prod/etc/graphics_game/ai_model/aifi1.om` and `aifi2.om` capability-check
paths. It records `stat` size/result/errno, `realpath` canonical path/errno, and
read-only `open`/`close` results without reading model bytes. `pathScope` states
that these known `IsSupported` check paths are not confirmed core initialization
failure paths; accessibility does not establish model readiness.

For a precise native debugger attachment window, create `filesDir/fg-debug-wait`
before launching the surface probe. Before GPU setup, `trace.jsonl` records
`waiting-for-native-debugger` with the worker process PID. The worker checks this
fixed marker and stop state every 50 ms for at most 30 seconds. Removing the
marker or reaching the timeout resumes setup; a stop request skips setup and
uses normal cleanup. `native-debugger-wait-ended` records the exit reason.
Without the marker, execution starts immediately.

The opt-in full-game interpolation prototype is built separately from the public
shell and the system FG surface experiment:

```sh
bash game-runtime/scripts/build-ohos.sh
HARMONY_BUILD_STAGE=/private/tmp/harmony-fg-build \
HARMONY_GAME_EXPERIMENT_BUNDLE=com.example.harmonyruntimeprobe \
HARMONY_RIFE_SDK=/absolute/path/to/rife-sdk \
HARMONY_SHELL_ONLY=0 HARMONY_COPY_BUILD_OUTPUT=0 HARMONY_WITH_VVL=0 \
zsh harmony/build_signed_local.sh
```

The private mirror retains its existing signing profile. The experiment bundle
must differ from the working game; it keeps `EntryAbility` and omits system FG
loader metadata. It reuses the local embedded AOT and requires separately
installed game resources. Normal local OHOS runtime builds include
`frame-interpolation` by default. `MUGUET_RUNTIME_FEATURES=` explicitly builds a
feature-off native diagnostic; full-game HAP packaging rejects it. A full game
requires `HARMONY_RIFE_SDK` and the generic interpolation bridge. The build records
the exact runtime archive hash and checks the constant-return capability ABI plus
real implementation markers before linking and again inside the HAP. A stale
archive manifest or a feature-off packaged library fails the build. Public shells
and the independent FG surface probe remain runtime-free.

If the optional RIFE SDK is unavailable, set `HARMONY_FG_NPU_ONLY=1` for an
explicit NPU-only full-game build and leave `HARMONY_RIFE_SDK` unset. This still
includes NPU interpolation and spatial upscaling; an explicit RIFE request fails
with an unavailable-backend error and retains original presentation. Device
support and initialization of the stock model remain required. The default
full-game build continues to require the RIFE SDK.

Inside this experiment's files directory, `frame-interpolation-npu.request`
selects the public stock-model NNRT/CANN path with GPU reconstruction;
`frame-interpolation-rife.request` selects RIFE v4.6 (NPU marker takes precedence).
Restart the process after changing these explicit local markers or configuration.
`frame_interpolation_enabled=false` disables FG. Existing NPU/RIFE markers retain
their selection; with neither marker, explicit configuration `true` selects NPU.
A missing field retains the legacy marker-only opt-in. The RIFE path verifies the pinned model
hashes in `rife-v4.6`; the NPU path reads stock model bytes only in memory.

The NPU path preserves owned copies of the actual guest final texture. Only
448×208 RGBA images are read back to a bounded worker; raw FP16 optical flow is
uploaded and reconstructed on the GPU at the original game resolution. Both
source and AI textures then use the public XEngine spatial upscaler before the
game's own wgpu surface presents them. Independent output SR slots are protected
by submission completion; output encoding does not end the guest's open pass.
RIFE remains a separate 320×192 diagnostic backend. Output uses a 200ms buffer
and configurable output deadlines (30Hz by default), with source boundaries
capped at half the output target. Pauses and resize clear
history, missing/late outputs are counted as repeats, and errors restore ordinary
presentation. NPU classifier rejection skips that midpoint; its unknown semantics
are not labeled a scene cut. `frame-interpolation-status.json` records actual
source/AI/repeat/late/skip counters and recent present timing. Validate world-scene
content and RenderService timestamps before claiming game 30fps.

Status serialization and file IO run on a bounded single worker. The GPU submits
an owned snapshot with nonblocking `try_send` to a capacity-one queue; full/drop,
serialization and write errors are explicit. The worker serializes to one byte
buffer and atomically replaces the status file using a PID-specific 0600 temporary
file. Snapshot generation, worker write start and the previous completed write
are separate monotonic observations; native present-row timestamps and rendering
counters retain their original meaning. Slow tracing covers selected expensive
stages rather than timing every guest opcode.

Initial build evidence is in `logs/build-baseline.log`. It was compiled with
`RP_HAS_RUST=0`; `artifacts/cpp-baseline-unsigned.hap` preserves that package.

For the screen-aspect experiment, set `spatial_upscale_mode: "native_quality"`,
`spatial_upscale_native_short_edge: 800` and
`spatial_upscale_output_short_edge: 1080`; omit fixed resolution and explicit
input/output pairs. The short edges specify resolution scale, while each long
edge follows the actual startup XComponent aspect and rounds to an even pixel.
A 3000×2000 canvas therefore selects 1200×800 → 1620×1080; a 16:9 canvas
selects approximately 1422×800 → 1920×1080. Invalid or conflicting adaptive
sizes disable SR with a reported configuration error instead of choosing a
fixed wide image. Explicit size pairs remain a separate experiment option.
The current ability starts in landscape and keeps guest/SR sizes for the session.
NativeWindow SCALE_FIT preserves the image aspect if the canvas later changes,
and pointer/touch coordinates use the same centered fit rectangle. New sessions
select dimensions from their actual startup canvas; live guest resizing and
portrait gameplay are not established by this change.
A build or requested dimensions do not prove GPU output. After entering the world,
`fg-final-output-capture.request` optionally captures six actual post-SR textures
in `fg-final-output-capture-<pid>/`; status maps each capture to output slot and
source/AI kind. Capture can affect timing, so collect steady frame pacing after
it finishes. Rows contain CLOCK_MONOTONIC present-call boundaries for matching
RenderService flush timestamps; source timestamps mark host encode boundaries,
not GPU completion or touch-to-photon delay.

`frame_interpolation_native_render_audit` is default-off. When explicitly enabled,
`native-render-audit-<pid>.jsonl` records at most 64 distinct nonempty geometry
draws with actual color/depth view extents and applied viewport/scissor. Sampling
stops at the cap; the asynchronous writer drains its bounded queue. Small reflection
probes remain separate from main-scene resolution evidence. Recorded commands do
not alone prove GPU completion; pair them with post-SR pixels and RS presents.

`experiments/game-ai-screen-aspect.json` selects native/output short edges
800/1080 from the actual startup canvas aspect. Its initial
`frame_interpolation_output_fps: 48` requests 24 actual source frames/s and
24 neural midpoints/s; measured cadence remains a separate acceptance check.
`experiments/game-ai-1080p.json` preserves the historical 640×360 → 1920×1080
profile and does not meet the later native-resolution requirement. The default remains 15→30; NPU experiments accept output targets
30, 32, 40 or 48, and RIFE accepts 30. The configuration is read once when the GPU
interpolator is created; restart the process after changing it. Requested rate
and requested dimensions still require independent device verification.


`frame_interpolation_classifier_policy` is read with the output rate. Its default
`conservative_reject_second_gt_0_99` retains the experimental rejection rule.
The explicit `observe_only` option records both logits, the second softmax score
and its source pair, while allowing each successful, finite neural-flow result
to reach GPU reconstruction. These classifier labels have no validated scene-cut
or quality contract. Both policies retain output guards and the complete FP16
and logit finite checks; repeats remain separate from newly inferred AI events.
The status retains at most 180 classifier observations for capture attribution.
An unknown policy fails configuration instead of silently enabling interpolation.


### Launcher graphics presets, 2026-10-20

The launcher now selects high / medium / low original-game graphics options and
independent SR / frame-generation switches. The initial selection is medium with
both switches on; selections persist across launcher and in-game load restarts.
Medium texture quality is now1 at the user's request. See
[`QUALITY_PRESETS.md`](QUALITY_PRESETS.md) for the settings and device evidence.
The texture2 balanced result below is historical and does not measure this new
texture1 medium preset.

### Historical balanced shadows and detail result, 2026-10-19

`experiments/game-ai-quality-balanced.json` was the applied app-private
profile on the MatePad Pro Max for the run below. It uses the signed runtime package identified
below; this settings change does not rebuild the HAP or change its embedded
defaults. Game resources and saves are preserved. The configuration is read once,
so changing profiles requires restarting the game process.

The tested combination enables engine FXAA (`-fxaa=1`), anisotropic filtering4,
shadow quality2 (`-shadowQuality=1`), `Shadow_SoftShadows=1` and world
`LodScale=0.15`; XEngine sharpness is reduced from0.2 to0.1. Actual post-ApplySettings
logs and the live SR context confirm these values. The specific inline shadow
filter was not separately instrumented. TextureQuality remains2; pedestrian and
vehicle LOD, population, reflections, MSAA and TXAA are unchanged. Native/output
short edges remain800/1080, derived from screen aspect:1200×800 →1620×1080 on
the3000×2000 device, with all three game buffers using FIT scaling mode4.

After a visible3D scene was confirmed, a predeclared90-second movement/camera
warmup was followed by a complete91-sample,180.209337969-second measurement in
the same PID16854/epoch1, with17 acknowledged input commands. The effective mean
was **34.8261642306fps**:3378 source plus2898 neural-AI presentations, excluding
1419 repeats. All7696 RS records inside the inclusive collection boundary were
uniquely associated with recorded present intervals; an independent analysis
agrees. Continuous1/5/10-second minima were22.0/28.6/31.6fps, so this meets the
user-confirmed **mean≥30fps** criterion and does not promise every-second30fps.
All samples and the separate warmup are retained.

FG errors were null throughout; the complete current-run log has35 GPU metric
records, all with zero skipped draws and validation errors, and no targeted
pipeline/shader error messages. The asynchronous status writer had no new drops
or write errors during measurement. The game remains foreground and focused,
screen keep-on is enabled, and capture/profiling/slow tracing/native audit are
off. These observations cover this warmed run, rather than all future scenes or
thermal conditions. Fine wires, foliage and dense grids still show some aliasing;
different trial scenes do not establish a quantitative AA improvement or isolate
the cost of individual settings. No new geometry or neural pixel captures were
performed for this settings-only change; the same runtime's bounded proofs remain
in the baseline evidence below.

The complete result and exact profile/package identities are in
`evidence/device/matepad-pro-max/quality-aa-20261019/quality-balanced-acceptance.json`.
The original `experiments/game-ai-screen-aspect.json` and backed-up
`quality-aa-20261019/baseline-settings.xml` preserve the previous configuration
pair for rollback. Preliminary trials are retained separately; the first FXAA
trial included an unverified loading-to-world interval and is not treated as
full-world performance acceptance.

### Screen-aspect baseline device result, 2026-10-19

The baseline run used `dist/HarmonyOS-GTA-V-screen-aspect-AI-signed.hap`,
SHA-256 `0bbc5930a99ccc3adb35c05b9799837be84879dbaf4dd1e5279934e8cb0f6f32`,
with `experiments/game-ai-screen-aspect.json` and the app-private NPU request
marker. It is installed and enabled on the MatePad Pro Max GEM-W00 / Maleoon935 /
API26; it requires the existing separately imported game resources. This local
signed debug package is separate from the public shell and historical packages.

Dimensions follow the startup canvas rather than a fixed 16:9 shape. On the
3000×2000 (3:2) device, 61 sampled main-scene geometry draws used actual
1200×800 color/depth targets and viewport/scissor; three 32×32 auxiliary probes
were identified separately. Six actual post-SR images were 1620×1080, including
two neural AI frames. One complete source/AI/source triple differs from its
endpoints and simple byte averages; this is not ground-truth image-quality
validation. RenderService independently showed three 1620×1080 game buffers,
all scaling mode4, mapped to the 3000×2000 layer with equal x/y scale. The public
FIT policy is applied on the GPU thread after swapchain configuration.

The user confirmed **mean ≥30fps with occasional loading stalls allowed**.
After a predeclared complete 90-second movement/camera warmup, the same PID59795
ran a complete 91-sample, approximately three-minute acceptance phase with
17 acknowledged forward/camera commands. All7523 observed RS records inside its
inclusive boundary were uniquely associated with explicit monotonic present
intervals. The (start,end] throughput window is179.417785156s:4277 source plus
3224 neural AI events yielded **41.8074495429fps**, excluding21 repeated events.
Continuous1/5/10-second minima were **7.0/31.4/36.6fps**. A649.774ms source-capture
gap remains, so this meets the confirmed mean-rate criterion and does not establish
continuous one-second30fps stability. All data, including the poor interval and
separate warmup, are retained. No favorable subsection was selected.

The asynchronous state writer had no queue drops or write errors in all91
samples; GPU snapshot generation ranged0.858–2.027ms. CPU/GPU profiling, slow
tracing and final-output capture are off in the running final configuration;
the bounded native audit reached64 records before warmup and its sampling then
stopped. Screen keep-on remains enabled. Buffering is still200ms; touch-to-photon
latency and independent NPU hardware placement were not measured. This is one
warmed device run, not every scene or a full guest-rendering validation pass.
The standard HMS FG initialization issue remains; the working route is public
NNRT/CANN neural flow, full-source-resolution GPU reconstruction and public
XEngine spatial SR.

Evidence, full identities, exact configuration and limitations are preserved in
`evidence/device/matepad-pro-max/fg-aspect-fixed-20261019/game-screen-aspect-ai-acceptance.json`
and `game-screen-aspect-ai-hap-manifest.json`; independent geometry, pixels,
classifier provenance and aspect checks are in its `final-evidence/` directory.

### Historical output-resolution result, 2026-10-19

The earlier output-only experiment is `dist/HarmonyOS-GTA-V-1080p-AI-signed.hap`,
SHA-256 `45656943b33253c77b6397d0f7dcfc48f35e9b1801326dde69242dbe6d45d050`,
with the **separate** `experiments/game-ai-1080p.json` app-private configuration.
It uses `observe_only`, source 640×360, output 1920×1080, and targets 24→48Hz.
It does not satisfy the later native ≥800-short-edge and screen-aspect requirements;
its package, config and evidence remain historical and are not the current profile.
The original 960×540 profiles had short failures and are not the accepted profile.
The full HAP includes the local runtime/AOT and needs the existing separately
installed game resources; it is a device-bound local debug package.

On the MatePad Pro Max GEM-W00 / Maleoon935 / API26, one 180.501118438s world-scene
run included 17 acknowledged forward/camera input commands. All 7873 observed
RenderService records inside its boundary were uniquely associated with the
application's monotonic present intervals. Excluding 277 repeats, 4099 source
plus 3496 neural AI events yielded **42.0773016019Hz**. Exhaustive continuous-start
1/5/10-second windows had minima **30.0/36.4/38.9Hz**. The one-second minimum is
exactly 30 events, not rounded upward, and has no remaining margin in that window.
Six post-SR GPU readbacks and the actual native buffers were 1920×1080; two
captured images were neural AI frames. This establishes the measured output
resolution/rate, not native 1080p detail or ground-truth temporal image quality.

Evidence and exact configuration/binary identities are preserved in
`evidence/device/matepad-pro-max/fg30-20261019/game-1080p-ai-acceptance.json`
and `game-1080p-ai-hap-manifest.json`. The failed runs remain separate. This is
one device/run, with a 200ms interpolation buffer and occasional longer frame
intervals; it is not an all-scene, indefinite-run, zero-jank guarantee. Separate
underlying guest unsupported-format/usage validation messages from CPU profiling
also prevent calling this a complete guest-rendering validation pass.

The standard HMS system-FG initialization issue remains; this measured route
uses public NNRT/CANN neural flow, GPU reconstruction and public XEngine spatial
upscaling. It does not establish independent NPU hardware-placement profiling.
The optional slow-operation trace build was not installed for the accepted run.
It is default-off; `frame_interpolation_slow_trace: true` writes bounded
`gpu-slow-operations-<pid>.jsonl` (>10ms operations, at most 32768 events) through
an asynchronous queue. Profiling runs must remain separate from performance
acceptance.

For an already provisioned experiment with its resources present, send the
selected profile to `/data/storage/el2/base/haps/entry/files/config.json` using
`hdc file send -b com.minami.harmonyruntimeprobe`. Keep the app-private
`frame-interpolation-npu.request` marker present, then restart the application
process. Neither this config nor the marker is automatically enabled by a clean
installation. CPU profiling, slow tracing and final-output capture are off in
the accepted running configuration; screen keep-on remains enabled.
