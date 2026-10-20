#ifndef MUGUET_FG_NPU_FLOW_H
#define MUGUET_FG_NPU_FLOW_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum {
    MUGUET_FG_NPU_FLOW_ABI_VERSION = 1,
    MUGUET_FG_NPU_FLOW_WIDTH = 448,
    MUGUET_FG_NPU_FLOW_HEIGHT = 208,
    MUGUET_FG_NPU_FLOW_INPUT_BYTES = 448 * 208 * 4,
    MUGUET_FG_NPU_FLOW_HALF_ELEMENTS = 448 * 208 * 6,
    MUGUET_FG_NPU_FLOW_LOGIT_ELEMENTS = 2
};

typedef enum MuguetFgNpuFlowCode {
    MUGUET_FG_NPU_FLOW_OK = 0,
    MUGUET_FG_NPU_FLOW_INVALID_ARGUMENT = 1,
    MUGUET_FG_NPU_FLOW_RUNTIME_UNAVAILABLE = 2,
    MUGUET_FG_NPU_FLOW_MODEL_UNAVAILABLE = 3,
    MUGUET_FG_NPU_FLOW_MODEL_INCOMPATIBLE = 4,
    MUGUET_FG_NPU_FLOW_BACKEND_UNAVAILABLE = 5,
    MUGUET_FG_NPU_FLOW_CONFIGURATION_FAILED = 6,
    MUGUET_FG_NPU_FLOW_SCHEMA_MISMATCH = 7,
    MUGUET_FG_NPU_FLOW_ALLOCATION_FAILED = 8,
    MUGUET_FG_NPU_FLOW_RUN_FAILED = 9,
    MUGUET_FG_NPU_FLOW_INTERNAL_ERROR = 10
} MuguetFgNpuFlowCode;

typedef struct MuguetFgNpuFlowContext MuguetFgNpuFlowContext;

typedef struct MuguetFgNpuFlowInfo {
    uint32_t struct_size;
    uint32_t abi_version;
    uint32_t width;
    uint32_t height;
    uint32_t input_bytes_per_frame;
    uint32_t flow_half_elements;
    uint32_t logit_elements;
    uint32_t npu_only_fallback_disabled;
    uint64_t backend_id;
    uint64_t successful_runs;
    uint64_t last_run_sync_ns;
    int32_t last_run_sync_code;
    int32_t input_data_type;
    int32_t flow_data_type;
    int32_t logit_data_type;
    int32_t tensor_format;
    uint32_t reserved;
    char backend_name[64];
    char cann_version[128];
    char input_names[2][64];
    char output_names[2][64];
} MuguetFgNpuFlowInfo;

// Synchronous, fixed-size public NNRT/CANN wrapper. Create/build once on the
// inference worker, then reuse its compilation, executor and four tensors.
// Uses read-only stock aifi2.om, whole-file ROR1 only in memory (<8 MiB).
// Retains decoded bytes until compilation destruction, then erases them.
// Returns a code above and sets *out_context=NULL on failure.
int32_t muguet_fg_npu_flow_create(MuguetFgNpuFlowContext** out_context);

// Both inputs are tightly packed RGBA8: input[(y*448+x)*4+c], not planar,
// despite the model's NCHW descriptor. Input0 is older, input1 newer. No resize,
// padding, normalization or alpha conversion is performed here.
// flow_half_capacity is uint16_t element capacity, not bytes. The six output
// planes retain native FP16 bits in [6][208][448] order; logits are two FP32s.
// No sigmoid, finite scan, warp or classifier gate is performed. The rendering
// caller must bound/sanitize sampled values before using raw model output.
// Inputs may alias each other; the two output regions must not overlap.
// Success copies all outputs into caller-owned storage; failure leaves output
// storage untouched. Pointers are borrowed only for this synchronous call.
int32_t muguet_fg_npu_flow_process_rgba8(MuguetFgNpuFlowContext* context,
    const uint8_t* input0, size_t input0_bytes, const uint8_t* input1, size_t input1_bytes,
    uint16_t* flow_half, size_t flow_half_capacity, float* logits, size_t logit_capacity);

// Caller supplies sizeof(MuguetFgNpuFlowInfo); includes actual checked SDK
// descriptor types/names. NPU-only options are configuration evidence, not
// independent proof of hardware placement or presentation frame rate.
int32_t muguet_fg_npu_flow_get_info(const MuguetFgNpuFlowContext* context,
    MuguetFgNpuFlowInfo* info, size_t info_bytes);

// Error text is borrowed until the next operation on the same context. NULL
// returns this thread's last creation/invalid-context error. It never throws.
const char* muguet_fg_npu_flow_last_error(const MuguetFgNpuFlowContext* context);

// All operations on a context, including get_info/last_error/destroy, must be
// externally serialized. No private interfaces, implicit worker or RunAsync.
// Public runtimes remain loaded for process lifetime (driver TLS safety).
void muguet_fg_npu_flow_destroy(MuguetFgNpuFlowContext* context);

#ifdef __cplusplus
}
#endif
#endif
