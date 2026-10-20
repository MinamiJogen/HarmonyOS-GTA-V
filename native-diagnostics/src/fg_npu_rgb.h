#ifndef MUGUET_FG_NPU_RGB_H
#define MUGUET_FG_NPU_RGB_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum { MUGUET_FG_NPU_RGB_ABI_VERSION = 1 };
typedef enum MuguetFgNpuRgbCode {
    MUGUET_FG_NPU_RGB_OK = 0,
    MUGUET_FG_NPU_RGB_INVALID_ARGUMENT = 1,
    MUGUET_FG_NPU_RGB_ALLOCATION_FAILED = 2,
    MUGUET_FG_NPU_RGB_FLOW_FAILED = 3,
    MUGUET_FG_NPU_RGB_NONFINITE_OUTPUT = 4,
    MUGUET_FG_NPU_RGB_QUALITY_GATE_REJECTED = 5,
    MUGUET_FG_NPU_RGB_INTERNAL_ERROR = 6
} MuguetFgNpuRgbCode;

typedef struct MuguetFgNpuRgbContext MuguetFgNpuRgbContext;
typedef struct MuguetFgNpuRgbTiming {
    uint32_t struct_size;
    uint32_t abi_version;
    uint64_t attempted_calls;
    uint64_t successful_calls;
    uint64_t quality_rejected_calls;
    uint64_t last_resize_ns;
    uint64_t last_flow_call_ns;
    uint64_t last_run_sync_ns;
    uint64_t last_finite_scan_ns;
    uint64_t last_cpu_warp_ns;
    uint64_t last_total_ns;
    uint64_t nonfinite_half_elements;
    int32_t last_status;
    int32_t last_flow_status;
    uint32_t width;
    uint32_t height;
    uint32_t all_outputs_finite;
    uint32_t quality_gate_triggered;
    float logits[2];
    double probability_second;
    double quality_gate_threshold;
} MuguetFgNpuRgbTiming;

// Creates/builds the fixed public NNRT/CANN aifi2 flow model once and allocates
// reusable CPU workspaces. No private API, model export or implicit worker.
// On failure sets *out_context=NULL; inspect last_error(NULL) on this thread.
int32_t muguet_fg_npu_rgb_create(MuguetFgNpuRgbContext** out_context);

// Tightly packed RGB8 source frames, older then newer. Supports 320x192 and
// 448x208 only, timestep exactly 0.5. Each input must contain width*height*3
// readable bytes. output_bytes is capacity, must be >= width*height*3.
// Inputs are resized with pixel-center bilinear sampling to packed RGBA8
// 448x208 before public flow inference. Alpha is 255. Model flow is bilinearly
// sampled and scaled from model pixel displacement to source pixel units.
// Uses flow01->input0, flow23->input1, sigmoid(ch4) mixing and sigmoid(ch5)
// fallback to unwarped input1, as independently tested for this model/device.
// Every FP16 value and both logits must be finite. A softmax(logits)[1] > .99
// rejects the output with QUALITY_GATE_REJECTED. This classifier's semantics
// are UNKNOWN: this is a conservative quality gate, not a scene-cut detector.
// Caller must explicitly reject known scene cuts instead of labeling them AI.
// Output is untouched on any failure/rejection; no repeat or blend fallback.
// All pointers are borrowed for this synchronous call. Inputs may alias one
// another or output because output commit happens after input sampling ends.
int32_t muguet_fg_npu_rgb_process_rgb8(MuguetFgNpuRgbContext* context,
    const uint8_t* source0, const uint8_t* source1, int width, int height,
    uint8_t* output, size_t output_bytes, float timestep);

// Timing is CPU steady-clock duration. run_sync covers NNRT only; flow_call
// also includes C ABI input/output copies. No hardware-placement/FPS claim.
int32_t muguet_fg_npu_rgb_get_timing(const MuguetFgNpuRgbContext* context,
    MuguetFgNpuRgbTiming* timing, size_t timing_bytes);
const char* muguet_fg_npu_rgb_last_error(const MuguetFgNpuRgbContext* context);
// Externally serialize every operation on one context, including inspection
// and destruction. In-flight public driver calls cannot be cancelled.
void muguet_fg_npu_rgb_destroy(MuguetFgNpuRgbContext* context);

// Context-free IEEE FP16 bit validation for raw-flow GPU consumers. Counts
// every NaN/Inf by exponent bits; performs no FP conversion, model call, mask,
// warp or classifier evaluation. ARM64 uses NEON; other targets use scalar.
// values must provide count readable uint16_t elements. NULL is valid only
// when count==0; nonfinite_count must be non-NULL. Invalid arguments return
// INVALID_ARGUMENT and leave *nonfinite_count untouched; success returns OK.
// A successful scan with a nonzero count is still an invalid model output:
// the caller must reject the pair before uploading/sampling it on the GPU.
int32_t muguet_fg_npu_rgb_count_nonfinite_half(const uint16_t* values, size_t count,
    uint64_t* nonfinite_count);

#ifdef __cplusplus
}
#endif
#endif
