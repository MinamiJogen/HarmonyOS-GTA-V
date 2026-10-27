# Render submission pipeline: offline stop/go prototype

This experiment measures whether collecting **verified HAL operations** once,
then emitting resource transitions and rendering into the same command buffer,
can justify proceeding beyond an offline prototype. It does not change the
runtime, install a package, or claim a Maleoon performance improvement.

The real current `wgpu-core` and `wgpu-hal` sources are copied to a disposable
`/private/tmp` directory. The real render-pass validator, bind/vertex/index
state machines, initialization handling, attachment scope merge, trackers and
queue checks execute there. Only the temporary copy changes. Existing dirty
vendor sources are fingerprinted before and after execution.

`integrate.py` changes the temporary render-pass caller to select common direct
and indexed draws before any validation. Supported state includes pipelines,
bind groups and dynamic offsets, vertex/index ranges, blend/stencil state,
viewport/scissor, immediates and debug markers. Indirect/mesh/bundle/query
commands, timestamp/occlusion descriptors, multiview, resolves and discarding
attachment descriptors retain the existing recording path. A later eligible
pass can still consume an earlier discarded surface: the original discard
initialization fixup runs before its transitions and verified rendering.

The existing validation walk writes resolved operations into the separate
`VerifiedOp` type. Attachment validation and scope merging still finish before
barriers are extracted. The candidate uses the original
`insert_barriers_from_scope` implementation, followed by tape replay and one
command-buffer close. Replay never revisits `ArcRenderCommand`, the binder, or
draw validation helpers. Both branches count original validation-loop visits;
the differential harness asserts equality and actual candidate selection.

The recorder is deliberately **NOOP-specific**. Its commands own cloned NOOP
handles and parameter slices. The temporary backend gives resources unique,
clone-stable identities so the audit can compare handles as well as arguments.
It uses no reference lifetime erasure, raw handle pointers or `transmute`.
Porting this to Vulkan would require a separately reviewed ownership design;
this experiment is not that implementation.

Audited correctness comparisons flatten the **finished command-buffer list in
submission order**, after the legacy path has inserted its prepass. They
compare transition descriptors, HAL render/draw arguments, tracker start/end
states, initialization/discard actions and resource owner sets. Cases include
direct/indexed drawing, state changes, zero-count drawing, cross-pass texture
hazards, discard fixups, indirect/query/mixed fallback and matching invalid
argument errors. Lifecycle checks use the real core APIs for client handle
drops, destruction, mapping, replaced unused bind groups and successful
submission/polling. These are host core checks; NOOP cannot establish Vulkan
synchronization validity or identical pixels on a GPU. Complex subresource,
MSAA, multiview and bundle coverage is not established.

Release benchmarks use randomized paired A/B ordering, 8 warmup pairs,
41 sample pairs and 8 complete cycles per sample. A cycle includes encoder
creation, API recording, pass ending, original validation, scope/init tracking,
eligibility scanning, tape allocation and copying, barrier preparation, replay,
finish, and encoder/command-buffer/tape disposal. Fixture resource and pipeline
construction is excluded symmetrically. Audit formatting and trace collection
are disabled during timing. Normal queue submission, GPU execution, driver
recording and frame scheduling are outside these host timings.

`gates.json` was declared before measurement and is hashed with the frozen
experiment inputs. `decide.py` verifies that hash against the measured run and
requires all six primary direct/indexed/state-change cases at 32/256 draws to
improve median complete host cost by at least 3%, with p95 regression at most
2%. Mixed fallback median regression must be at most 2%. A failed gate means
**STOP_THIS_PROTOTYPE**, even if command-buffer counts fall. The report retains
each scenario's result instead of hiding losses in a weighted average.

Fixture eligibility is not real-game coverage. Representative coverage must
reach at least 50% of passes and 50% of direct draws before promotion; unknown
coverage prevents promotion. Even after an offline pass, only controlled
paired device trials with improved native total frame time could establish
success. A reduction in driver calls alone never qualifies.

Reproduce from the repository root using the cached Rust environment:

```sh
python3 game-runtime/experiments/render-submit/prepare.py \
  --output evidence/render-submit-reproduction
python3 game-runtime/experiments/render-submit/decide.py \
  evidence/render-submit-reproduction
```

The runner is offline and requires the repository's cached Rust dependencies.
It fails on source drift, compile/test failures, zero tests, or changed original
vendor input. `--debug` is for diagnosis; `decide.py` requires a release run.
The experiment writes `prepared-source.json`, a reviewable
`host-source-changes.patch`, resolved Cargo lockfile, test log and `host-run.json`.
The decision step writes `measurements.json` and `decision.json`.

The Khronos sample supports investigating CPU
command-buffer management costs, but its reported gains are specific to its
implementation and hardware, and are not used as estimated savings here.
[Khronos command buffer usage sample](https://docs.vulkan.org/samples/latest/samples/performance/command_buffer_usage/README.html).
