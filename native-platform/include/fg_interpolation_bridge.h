#ifndef MUGUET_FG_INTERPOLATION_BRIDGE_H
#define MUGUET_FG_INTERPOLATION_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct MuguetInterpolationContext MuguetInterpolationContext;

// Layout deliberately matches HarmonyRifeDeviceInfo and the existing Rust
// #[repr(C)] worker. The four feature fields describe the RIFE Vulkan backend;
// NPU leaves Vulkan metadata/features zero and reports gpu_index=-1.
typedef struct MuguetInterpolationDeviceInfo {
    int32_t gpu_index;
    uint32_t api_version, driver_version, vendor_id, device_id;
    int32_t supports_fp16_packed, supports_fp16_storage;
    int32_t supports_fp16_arithmetic, supports_int8_storage;
    char device_name[256];
} MuguetInterpolationDeviceInfo;

// MG_FRAME_INTERPOLATION must be exactly "rife" or "npu" at creation.
// The choice is retained in the context even if the environment changes.
// RIFE forwards model_directory/gpu_index to the SDK. NPU ignores both: it
// uses the read-only stock aifi2 model and public NNRT, followed by CPU warp.
// NULL on failure with a bounded, NUL-terminated error when capacity>0.
MuguetInterpolationContext* muguet_interpolation_create(const char* model_directory,
    int gpu_index, char* error, size_t error_capacity);

// Synchronous packed RGB8 pair -> RGB8. Pointers are borrowed until return.
// NPU currently supports 320x192/448x208 and timestep==0.5 only. Its nonzero
// status (including quality rejection) is forwarded, with no silent fallback.
// RIFE retains its SDK behavior. Zero is backend API success, not FPS proof.
int muguet_interpolation_process_rgb8(MuguetInterpolationContext* context,
    const uint8_t* rgb0, const uint8_t* rgb1, int width, int height,
    uint8_t* output, size_t buffer_bytes, float timestep);
int muguet_interpolation_get_device_info(MuguetInterpolationContext* context,
    MuguetInterpolationDeviceInfo* info);

// Read-only snapshot, durations in nanoseconds. NPU reports the RGB wrapper's
// resize/flow-call/RunSync/finite-scan/CPU-warp/total stages and statuses. RIFE
// reports timingAvailable=false. Supply >=2048 bytes; -10003 means truncated
// (NUL-terminated, not valid JSON), other nonzero values indicate API failure.
int muguet_interpolation_get_last_timing_json(MuguetInterpolationContext* context,
    char* output, size_t output_capacity);

// Calls on a context are serialized; destruction must not run concurrently.
// Backend driver calls are synchronous and cannot be cancelled by destroy.
void muguet_interpolation_destroy(MuguetInterpolationContext* context);

// Thread-local revision of the most recently created context on this thread.
// Includes the chosen backend and RIFE's real build revision when applicable.
// Before creation (or after failed creation), identifies an uninitialized bridge.
const char* muguet_interpolation_build_revision(void);

#ifdef __cplusplus
}
#endif
#endif
