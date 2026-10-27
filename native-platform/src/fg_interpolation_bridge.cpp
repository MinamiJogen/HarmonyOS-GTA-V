#include "fg_interpolation_bridge.h"
#include "fg_npu_rgb.h"
#ifndef MUGUET_INTERPOLATION_HAS_RIFE
#define MUGUET_INTERPOLATION_HAS_RIFE 1
#endif
#if MUGUET_INTERPOLATION_HAS_RIFE
#include <harmony_rife_c.h>
#endif

#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <limits>
#include <memory>
#include <mutex>
#include <new>

#if MUGUET_INTERPOLATION_HAS_RIFE
static_assert(sizeof(MuguetInterpolationDeviceInfo) == sizeof(HarmonyRifeDeviceInfo), "Device info ABI size");
static_assert(alignof(MuguetInterpolationDeviceInfo) == alignof(HarmonyRifeDeviceInfo), "Device info ABI alignment");
#define MUGUET_CHECK_INFO_OFFSET(field) \
    static_assert(offsetof(MuguetInterpolationDeviceInfo, field) == offsetof(HarmonyRifeDeviceInfo, field), "Device info ABI " #field)
MUGUET_CHECK_INFO_OFFSET(gpu_index);
MUGUET_CHECK_INFO_OFFSET(api_version);
MUGUET_CHECK_INFO_OFFSET(driver_version);
MUGUET_CHECK_INFO_OFFSET(vendor_id);
MUGUET_CHECK_INFO_OFFSET(device_id);
MUGUET_CHECK_INFO_OFFSET(supports_fp16_packed);
MUGUET_CHECK_INFO_OFFSET(supports_fp16_storage);
MUGUET_CHECK_INFO_OFFSET(supports_fp16_arithmetic);
MUGUET_CHECK_INFO_OFFSET(supports_int8_storage);
MUGUET_CHECK_INFO_OFFSET(device_name);
#undef MUGUET_CHECK_INFO_OFFSET
#endif

namespace {
enum class Backend { Rife, Npu };
constexpr int kInvalidArgument = -10001;
constexpr int kInternalError = -10002;
constexpr int kBufferTooSmall = -10003;
constexpr char kUninitializedRevision[] = "muguet_interpolation_bridge/1;backend=uninitialized";
thread_local char revision[1024] = "muguet_interpolation_bridge/1;backend=uninitialized";

void Error(char* output, size_t capacity, const char* message) noexcept
{
    if (output && capacity) std::snprintf(output, capacity, "%s", message ? message : "Unknown bridge error");
}
} // namespace

struct MuguetInterpolationContext {
    Backend backend;
#if MUGUET_INTERPOLATION_HAS_RIFE
    HarmonyRifeContext* rife = nullptr;
#endif
    MuguetFgNpuRgbContext* npu = nullptr;
    uint64_t npu_process_calls = 0;
    std::mutex calls;
    explicit MuguetInterpolationContext(Backend chosen) : backend(chosen) {}
    ~MuguetInterpolationContext()
    {
        if (npu) muguet_fg_npu_rgb_destroy(npu);
#if MUGUET_INTERPOLATION_HAS_RIFE
        if (rife) harmony_rife_destroy(rife);
#endif
    }
};

namespace {
void JsonNumber(double value, char (&output)[64]) noexcept
{
    if (std::isfinite(value)) std::snprintf(output, sizeof(output), "%.10g", value);
    else std::snprintf(output, sizeof(output), "null");
}

// Caller holds context.calls. Does not call inference or alter model state.
int LastTimingJson(MuguetInterpolationContext& context, char* output, size_t capacity)
{
    int written;
    if (context.backend == Backend::Rife) {
        written = std::snprintf(output, capacity,
            "{\"schemaVersion\":1,\"backend\":\"rife\",\"timingAvailable\":false}");
    } else {
        MuguetFgNpuRgbTiming timing{};
        const int code = muguet_fg_npu_rgb_get_timing(context.npu, &timing, sizeof(timing));
        if (code != MUGUET_FG_NPU_RGB_OK) return code;
        char logit0[64], logit1[64], probability[64], threshold[64];
        JsonNumber(timing.logits[0], logit0); JsonNumber(timing.logits[1], logit1);
        JsonNumber(timing.probability_second, probability); JsonNumber(timing.quality_gate_threshold, threshold);
        written = std::snprintf(output, capacity,
            "{\"schemaVersion\":1,\"backend\":\"npu\",\"timingAvailable\":true,\"durationUnit\":\"nanoseconds\","
            "\"attemptedCalls\":%llu,\"successfulCalls\":%llu,\"qualityRejectedCalls\":%llu,"
            "\"resizeNs\":%llu,\"flowCallNs\":%llu,\"runSyncNs\":%llu,\"finiteScanNs\":%llu,"
            "\"cpuWarpNs\":%llu,\"totalNs\":%llu,\"nonfiniteHalfElements\":%llu,"
            "\"lastStatus\":%d,\"lastFlowStatus\":%d,\"rgbWidth\":%u,\"rgbHeight\":%u,"
            "\"allOutputsFinite\":%u,\"qualityGateTriggered\":%u,\"logits\":[%s,%s],"
            "\"probabilitySecond\":%s,\"qualityGateThreshold\":%s,\"actualNpuPlacementProven\":false}",
            static_cast<unsigned long long>(timing.attempted_calls),
            static_cast<unsigned long long>(timing.successful_calls),
            static_cast<unsigned long long>(timing.quality_rejected_calls),
            static_cast<unsigned long long>(timing.last_resize_ns),
            static_cast<unsigned long long>(timing.last_flow_call_ns),
            static_cast<unsigned long long>(timing.last_run_sync_ns),
            static_cast<unsigned long long>(timing.last_finite_scan_ns),
            static_cast<unsigned long long>(timing.last_cpu_warp_ns),
            static_cast<unsigned long long>(timing.last_total_ns),
            static_cast<unsigned long long>(timing.nonfinite_half_elements),
            int(timing.last_status), int(timing.last_flow_status), timing.width, timing.height,
            timing.all_outputs_finite, timing.quality_gate_triggered, logit0, logit1, probability, threshold);
    }
    if (written < 0) return kInternalError;
    return size_t(written) >= capacity ? kBufferTooSmall : 0;
}
} // namespace

extern "C" MuguetInterpolationContext* muguet_interpolation_create(const char* model_directory,
    int gpu_index, char* error, size_t error_capacity)
{
    Error(error, error_capacity, "");
    std::snprintf(revision, sizeof(revision), "%s", kUninitializedRevision);
    try {
        const char* mode = std::getenv("MG_FRAME_INTERPOLATION");
        Backend chosen;
        if (mode && std::strcmp(mode, "rife") == 0) chosen = Backend::Rife;
        else if (mode && std::strcmp(mode, "npu") == 0) chosen = Backend::Npu;
        else {
            Error(error, error_capacity, "MG_FRAME_INTERPOLATION must be exactly rife or npu");
            return nullptr;
        }
        auto context = std::make_unique<MuguetInterpolationContext>(chosen);
        if (chosen == Backend::Rife) {
#if MUGUET_INTERPOLATION_HAS_RIFE
            if (!model_directory || !*model_directory) {
                Error(error, error_capacity, "RIFE requires a nonempty model directory"); return nullptr;
            }
            context->rife = harmony_rife_create(model_directory, gpu_index, error, error_capacity);
            if (error && error_capacity) error[error_capacity - 1] = '\0';
            if (!context->rife) {
                if (error && error_capacity && !*error) Error(error, error_capacity, "RIFE SDK create returned NULL");
                return nullptr;
            }
            const char* backendRevision = harmony_rife_build_revision();
            std::snprintf(revision, sizeof(revision), "muguet_interpolation_bridge/1;backend=rife;implementation=%s",
                backendRevision ? backendRevision : "unknown RIFE SDK revision");
#else
            (void)model_directory; (void)gpu_index;
            Error(error, error_capacity, "RIFE unavailable: this build contains NPU frame interpolation only");
            return nullptr;
#endif
        } else {
            const int32_t code = muguet_fg_npu_rgb_create(&context->npu);
            if (code != MUGUET_FG_NPU_RGB_OK || !context->npu) {
                if (error && error_capacity) {
                    const char* detail = muguet_fg_npu_rgb_last_error(nullptr);
                    std::snprintf(error, error_capacity, "NPU RGB create failed (code=%d): %s", int(code),
                        detail ? detail : "No backend error text");
                }
                return nullptr;
            }
            std::snprintf(revision, sizeof(revision),
                "muguet_interpolation_bridge/1;backend=npu;implementation=stock_aifi2_public_NNRT_CANN;"
                "rgb_wrapper_abi=%d;flow_grid=448x208;cpu_bilinear_warp=enabled;vulkan_backend=false",
                MUGUET_FG_NPU_RGB_ABI_VERSION);
        }
        return context.release();
    } catch (const std::exception& exception) {
        Error(error, error_capacity, exception.what()); return nullptr;
    } catch (...) {
        Error(error, error_capacity, "Unknown bridge exception during create"); return nullptr;
    }
}

extern "C" int muguet_interpolation_process_rgb8(MuguetInterpolationContext* context,
    const uint8_t* rgb0, const uint8_t* rgb1, int width, int height,
    uint8_t* output, size_t buffer_bytes, float timestep)
{
    if (!context || !rgb0 || !rgb1 || !output || width <= 0 || height <= 0 ||
        uint64_t(width) * uint64_t(height) * 3 > uint64_t(std::numeric_limits<size_t>::max()) ||
        uint64_t(width) * uint64_t(height) * 3 > uint64_t(buffer_bytes) ||
        !std::isfinite(timestep) || timestep < 0 || timestep > 1) return kInvalidArgument;
    try {
        std::lock_guard<std::mutex> lock(context->calls);
        if (context->backend == Backend::Npu) {
            const int code = muguet_fg_npu_rgb_process_rgb8(context->npu, rgb0, rgb1, width, height, output, buffer_bytes, timestep);
            ++context->npu_process_calls;
            if (context->npu_process_calls <= 3 || context->npu_process_calls % 60 == 0) {
                char snapshot[2048]{};
                if (LastTimingJson(*context, snapshot, sizeof(snapshot)) == 0)
                    std::fprintf(stderr, "[frame-interpolation-npu-timing] %s\n", snapshot);
            }
            return code;
        }
#if MUGUET_INTERPOLATION_HAS_RIFE
        return harmony_rife_process_rgb8(context->rife, rgb0, rgb1, width, height, output, buffer_bytes, timestep);
#else
        return kInternalError;
#endif
    } catch (...) {
        return kInternalError;
    }
}

extern "C" int muguet_interpolation_get_last_timing_json(MuguetInterpolationContext* context,
    char* output, size_t output_capacity)
{
    if (!context || !output || !output_capacity) return kInvalidArgument;
    output[0] = '\0';
    try {
        std::lock_guard<std::mutex> lock(context->calls);
        return LastTimingJson(*context, output, output_capacity);
    } catch (...) {
        return kInternalError;
    }
}

extern "C" int muguet_interpolation_get_device_info(MuguetInterpolationContext* context,
    MuguetInterpolationDeviceInfo* info)
{
    if (!context || !info) return kInvalidArgument;
    try {
        std::lock_guard<std::mutex> lock(context->calls);
        if (context->backend == Backend::Rife) {
#if MUGUET_INTERPOLATION_HAS_RIFE
            HarmonyRifeDeviceInfo backendInfo{};
            const int code = harmony_rife_get_device_info(context->rife, &backendInfo);
            if (code != 0) return code;
            std::memcpy(info, &backendInfo, sizeof(*info));
            info->device_name[sizeof(info->device_name) - 1] = '\0';
            return 0;
#else
            return kInternalError;
#endif
        }
        MuguetInterpolationDeviceInfo backendInfo{};
        backendInfo.gpu_index = -1;
        std::snprintf(backendInfo.device_name, sizeof(backendInfo.device_name),
            "HIAI_F stock aifi2 public NNRT; CPU warp enabled");
        *info = backendInfo;
        return 0;
    } catch (...) {
        return kInternalError;
    }
}

extern "C" void muguet_interpolation_destroy(MuguetInterpolationContext* context)
{
    delete context;
}

extern "C" const char* muguet_interpolation_build_revision(void)
{
    return revision;
}

extern "C" int muguet_interpolation_rife_available(void)
{
    return MUGUET_INTERPOLATION_HAS_RIFE;
}
