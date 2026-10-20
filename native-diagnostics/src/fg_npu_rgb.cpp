#include "fg_npu_rgb.h"
#include "fg_npu_flow.h"

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <limits>
#include <memory>
#include <new>
#include <vector>
#if defined(__aarch64__) && defined(__ARM_NEON) && !defined(MUGUET_NPU_RGB_DISABLE_NEON)
#include <arm_neon.h>
#define MUGUET_NPU_RGB_NEON 1
#else
#define MUGUET_NPU_RGB_NEON 0
#endif

namespace {
using Clock = std::chrono::steady_clock;
constexpr int kModelWidth = MUGUET_FG_NPU_FLOW_WIDTH;
constexpr int kModelHeight = MUGUET_FG_NPU_FLOW_HEIGHT;
constexpr size_t kModelPixels = size_t(kModelWidth) * kModelHeight;
constexpr size_t kFlowElements = MUGUET_FG_NPU_FLOW_HALF_ELEMENTS;
constexpr double kQualityThreshold = 0.99;
thread_local char threadError[512]{};

template<size_t N> void CopyText(char (&destination)[N], const char* text) noexcept
{
    std::snprintf(destination, N, "%s", text ? text : "");
}
uint64_t Elapsed(Clock::time_point start) noexcept
{
    return uint64_t(std::chrono::duration_cast<std::chrono::nanoseconds>(Clock::now() - start).count());
}
float HalfToFloat(uint16_t bits) noexcept
{
    uint32_t sign = uint32_t(bits & 0x8000) << 16;
    uint32_t exponent = (bits >> 10) & 31;
    uint32_t fraction = bits & 1023;
    uint32_t result;
    if (exponent == 0) {
        if (!fraction) result = sign;
        else {
            int power = -14;
            while (!(fraction & 1024)) { fraction <<= 1; --power; }
            result = sign | (uint32_t(power + 127) << 23) | ((fraction & 1023) << 13);
        }
    } else if (exponent == 31) {
        result = sign | 0x7f800000u | (fraction << 13);
    } else {
        result = sign | ((exponent + 112) << 23) | (fraction << 13);
    }
    float value;
    std::memcpy(&value, &result, sizeof(value));
    return value;
}
uint64_t CountNonfiniteHalf(const uint16_t* values, size_t count) noexcept
{
    size_t i = 0;
    uint64_t result = 0;
#if MUGUET_NPU_RGB_NEON
    const uint16x8_t exponent = vdupq_n_u16(0x7c00);
    const auto flags = [&](const uint16_t* at) {
        return vshrq_n_u16(vceqq_u16(vandq_u16(vld1q_u16(at), exponent), exponent), 15);
    };
    // Reduce bounded groups: at most32 per sum, so no lane or horizontal
    // overflow even when count exceeds32-bit range. No FPCR/FZ16 dependence.
    while (count - i >= 32) {
        const auto sum = vaddq_u16(vaddq_u16(flags(values + i), flags(values + i + 8)),
            vaddq_u16(flags(values + i + 16), flags(values + i + 24)));
        result += vaddvq_u16(sum);
        i += 32;
    }
    while (count - i >= 8) {
        result += vaddvq_u16(flags(values + i));
        i += 8;
    }
#endif
    for (; i < count; ++i) result += (values[i] & 0x7c00) == 0x7c00;
    return result;
}
float Sigmoid(float value) noexcept
{
    if (value >= 0) return 1.0f / (1.0f + std::exp(-value));
    const float exponential = std::exp(value);
    return exponential / (1.0f + exponential);
}
struct Bilinear {
    int x0, x1, y0, y1;
    float wx, wy;
};
Bilinear Coordinates(float x, float y, int width, int height) noexcept
{
    x = std::clamp(x, 0.0f, float(width - 1));
    y = std::clamp(y, 0.0f, float(height - 1));
    const int x0 = int(x), y0 = int(y);
    return {x0, std::min(x0 + 1, width - 1), y0, std::min(y0 + 1, height - 1), x - x0, y - y0};
}
float Blend(float a, float b, float c, float d, const Bilinear& p) noexcept
{
    return (1 - p.wy) * ((1 - p.wx) * a + p.wx * b) +
        p.wy * ((1 - p.wx) * c + p.wx * d);
}
float SamplePlane(const float* plane, const Bilinear& p) noexcept
{
    return Blend(plane[size_t(p.y0) * kModelWidth + p.x0], plane[size_t(p.y0) * kModelWidth + p.x1],
        plane[size_t(p.y1) * kModelWidth + p.x0], plane[size_t(p.y1) * kModelWidth + p.x1], p);
}
std::array<float, 3> SampleRgb(const uint8_t* image, int width, const Bilinear& p) noexcept
{
    const size_t a = (size_t(p.y0) * width + p.x0) * 3, b = (size_t(p.y0) * width + p.x1) * 3;
    const size_t c = (size_t(p.y1) * width + p.x0) * 3, d = (size_t(p.y1) * width + p.x1) * 3;
    std::array<float, 3> value{};
    for (size_t channel = 0; channel < 3; ++channel)
        value[channel] = Blend(image[a + channel], image[b + channel], image[c + channel], image[d + channel], p);
    return value;
}
uint8_t Byte(float value) noexcept
{
    return uint8_t(std::clamp(value, 0.0f, 255.0f) + 0.5f);
}
void ResizeRgbToRgba(const uint8_t* image, int width, int height, uint8_t* destination) noexcept
{
    const float sx = float(width) / kModelWidth, sy = float(height) / kModelHeight;
    for (int y = 0; y < kModelHeight; ++y) {
        const float sourceY = (float(y) + 0.5f) * sy - 0.5f;
        for (int x = 0; x < kModelWidth; ++x) {
            const auto p = Coordinates((float(x) + 0.5f) * sx - 0.5f, sourceY, width, height);
            const auto rgb = SampleRgb(image, width, p);
            const size_t offset = (size_t(y) * kModelWidth + x) * 4;
            for (size_t c = 0; c < 3; ++c) destination[offset + c] = Byte(rgb[c]);
            destination[offset + 3] = 255;
        }
    }
}
void WarpRgb(const uint8_t* first, const uint8_t* second, int width, int height,
    const std::vector<float>& planes, uint8_t* result) noexcept
{
    const float toModelX = float(kModelWidth) / width, toModelY = float(kModelHeight) / height;
    const float toSourceX = float(width) / kModelWidth, toSourceY = float(height) / kModelHeight;
    for (int y = 0; y < height; ++y) {
        for (int x = 0; x < width; ++x) {
            const auto p = Coordinates((float(x) + 0.5f) * toModelX - 0.5f,
                (float(y) + 0.5f) * toModelY - 0.5f, kModelWidth, kModelHeight);
            std::array<float, 6> fields{};
            for (size_t channel = 0; channel < fields.size(); ++channel)
                fields[channel] = SamplePlane(planes.data() + channel * kModelPixels, p);
            const auto a = SampleRgb(first, width, Coordinates(x + fields[0] * toSourceX,
                y + fields[1] * toSourceY, width, height));
            const auto b = SampleRgb(second, width, Coordinates(x + fields[2] * toSourceX,
                y + fields[3] * toSourceY, width, height));
            const size_t offset = (size_t(y) * width + x) * 3;
            for (size_t channel = 0; channel < 3; ++channel) {
                const float mixed = fields[4] * a[channel] + (1 - fields[4]) * b[channel];
                result[offset + channel] = Byte(fields[5] * second[offset + channel] + (1 - fields[5]) * mixed);
            }
        }
    }
}
struct FlowOwner {
    MuguetFgNpuFlowContext* value = nullptr;
    ~FlowOwner() { if (value) muguet_fg_npu_flow_destroy(value); }
};
} // namespace

struct MuguetFgNpuRgbContext {
    FlowOwner flow;
    std::array<std::vector<uint8_t>, 2> rgba;
    std::vector<uint16_t> half;
    std::vector<float> planes;
    std::vector<uint8_t> rgb;
    MuguetFgNpuRgbTiming timing{};
    mutable char error[512]{};
};

extern "C" int32_t muguet_fg_npu_rgb_create(MuguetFgNpuRgbContext** output)
{
    threadError[0] = '\0';
    if (!output) { CopyText(threadError, "out_context is NULL"); return MUGUET_FG_NPU_RGB_INVALID_ARGUMENT; }
    *output = nullptr;
    try {
        auto context = std::make_unique<MuguetFgNpuRgbContext>();
        const auto code = muguet_fg_npu_flow_create(&context->flow.value);
        if (code != MUGUET_FG_NPU_FLOW_OK) {
            CopyText(threadError, muguet_fg_npu_flow_last_error(nullptr)); return MUGUET_FG_NPU_RGB_FLOW_FAILED;
        }
        for (auto& image : context->rgba) image.resize(MUGUET_FG_NPU_FLOW_INPUT_BYTES);
        context->half.resize(kFlowElements);
        context->planes.resize(kFlowElements);
        context->rgb.resize(kModelPixels * 3);
        context->timing.struct_size = sizeof(MuguetFgNpuRgbTiming);
        context->timing.abi_version = MUGUET_FG_NPU_RGB_ABI_VERSION;
        context->timing.quality_gate_threshold = kQualityThreshold;
        context->timing.probability_second = std::numeric_limits<double>::quiet_NaN();
        context->timing.logits[0] = context->timing.logits[1] = std::numeric_limits<float>::quiet_NaN();
        *output = context.release();
        return MUGUET_FG_NPU_RGB_OK;
    } catch (const std::bad_alloc&) {
        CopyText(threadError, "CPU workspace allocation failed"); return MUGUET_FG_NPU_RGB_ALLOCATION_FAILED;
    } catch (const std::exception& exception) {
        CopyText(threadError, exception.what()); return MUGUET_FG_NPU_RGB_INTERNAL_ERROR;
    } catch (...) {
        CopyText(threadError, "Unknown native exception"); return MUGUET_FG_NPU_RGB_INTERNAL_ERROR;
    }
}

extern "C" int32_t muguet_fg_npu_rgb_process_rgb8(MuguetFgNpuRgbContext* context,
    const uint8_t* source0, const uint8_t* source1, int width, int height,
    uint8_t* output, size_t outputBytes, float timestep)
{
    if (!context) { CopyText(threadError, "context is NULL"); return MUGUET_FG_NPU_RGB_INVALID_ARGUMENT; }
    const auto begin = Clock::now();
    auto& timing = context->timing;
    const uint64_t attempted = timing.attempted_calls + 1;
    const uint64_t successful = timing.successful_calls, rejected = timing.quality_rejected_calls;
    timing = MuguetFgNpuRgbTiming{};
    timing.struct_size = sizeof(timing); timing.abi_version = MUGUET_FG_NPU_RGB_ABI_VERSION;
    timing.attempted_calls = attempted; timing.successful_calls = successful; timing.quality_rejected_calls = rejected;
    timing.quality_gate_threshold = kQualityThreshold;
    timing.probability_second = std::numeric_limits<double>::quiet_NaN();
    timing.logits[0] = timing.logits[1] = std::numeric_limits<float>::quiet_NaN();
    context->error[0] = '\0';
    const auto finish = [&](int32_t code, const char* error) {
        timing.last_status = code; timing.last_total_ns = Elapsed(begin);
        if (error) CopyText(context->error, error);
        return code;
    };
    if (!source0 || !source1 || !output || timestep != 0.5f ||
        !((width == 320 && height == 192) || (width == kModelWidth && height == kModelHeight)) ||
        outputBytes < size_t(width) * size_t(height) * 3)
        return finish(MUGUET_FG_NPU_RGB_INVALID_ARGUMENT, "Requires RGB8 320x192 or448x208, output capacity >=width*height*3 and timestep0.5");
    timing.width = uint32_t(width); timing.height = uint32_t(height);
    try {
        auto stage = Clock::now();
        ResizeRgbToRgba(source0, width, height, context->rgba[0].data());
        ResizeRgbToRgba(source1, width, height, context->rgba[1].data());
        timing.last_resize_ns = Elapsed(stage);
        stage = Clock::now();
        const auto code = muguet_fg_npu_flow_process_rgba8(context->flow.value,
            context->rgba[0].data(), context->rgba[0].size(), context->rgba[1].data(), context->rgba[1].size(),
            context->half.data(), context->half.size(), timing.logits, 2);
        timing.last_flow_call_ns = Elapsed(stage); timing.last_flow_status = code;
        if (code != MUGUET_FG_NPU_FLOW_OK)
            CopyText(context->error, muguet_fg_npu_flow_last_error(context->flow.value));
        MuguetFgNpuFlowInfo info{};
        if (muguet_fg_npu_flow_get_info(context->flow.value, &info, sizeof(info)) == MUGUET_FG_NPU_FLOW_OK)
            timing.last_run_sync_ns = info.last_run_sync_ns;
        if (code != MUGUET_FG_NPU_FLOW_OK) return finish(MUGUET_FG_NPU_RGB_FLOW_FAILED, nullptr);
        stage = Clock::now();
        timing.nonfinite_half_elements = CountNonfiniteHalf(context->half.data(), kFlowElements);
        const bool finite = timing.nonfinite_half_elements == 0 &&
            std::isfinite(timing.logits[0]) && std::isfinite(timing.logits[1]);
        timing.all_outputs_finite = finite ? 1 : 0;
        if (finite) {
            for (size_t i = 0; i < kFlowElements; ++i)
                context->planes[i] = HalfToFloat(context->half[i]);
            // Matches the transform/resolve staging: sigmoid on model-grid
            // mask texels, then bilinearly sample those masks at output pixels.
            for (size_t i = 4 * kModelPixels; i < kFlowElements; ++i)
                context->planes[i] = Sigmoid(context->planes[i]);
        }
        timing.last_finite_scan_ns = Elapsed(stage);
        if (!finite) return finish(MUGUET_FG_NPU_RGB_NONFINITE_OUTPUT, "Non-finite FP16 flow or classifier logits; RGB output untouched");
        const double first = timing.logits[0], second = timing.logits[1], maximum = std::max(first, second);
        const double a = std::exp(first - maximum), b = std::exp(second - maximum);
        timing.probability_second = b / (a + b);
        if (timing.probability_second > kQualityThreshold) {
            timing.quality_gate_triggered = 1; ++timing.quality_rejected_calls;
            return finish(MUGUET_FG_NPU_RGB_QUALITY_GATE_REJECTED, "Unknown-semantic classifier second probability >0.99; RGB output untouched");
        }
        stage = Clock::now();
        WarpRgb(source0, source1, width, height, context->planes, context->rgb.data());
        const size_t bytes = size_t(width) * size_t(height) * 3;
        std::memcpy(output, context->rgb.data(), bytes);
        timing.last_cpu_warp_ns = Elapsed(stage); ++timing.successful_calls;
        return finish(MUGUET_FG_NPU_RGB_OK, nullptr);
    } catch (const std::exception& exception) {
        return finish(MUGUET_FG_NPU_RGB_INTERNAL_ERROR, exception.what());
    } catch (...) {
        return finish(MUGUET_FG_NPU_RGB_INTERNAL_ERROR, "Unknown native exception");
    }
}

extern "C" int32_t muguet_fg_npu_rgb_get_timing(const MuguetFgNpuRgbContext* context,
    MuguetFgNpuRgbTiming* timing, size_t bytes)
{
    if (!context || !timing || bytes != sizeof(*timing)) {
        if (context) CopyText(context->error, "get_timing requires exact struct byte size and non-NULL output");
        else CopyText(threadError, "get_timing context is NULL");
        return MUGUET_FG_NPU_RGB_INVALID_ARGUMENT;
    }
    std::memcpy(timing, &context->timing, sizeof(*timing));
    return MUGUET_FG_NPU_RGB_OK;
}
extern "C" const char* muguet_fg_npu_rgb_last_error(const MuguetFgNpuRgbContext* context)
{
    return context ? context->error : threadError;
}
extern "C" void muguet_fg_npu_rgb_destroy(MuguetFgNpuRgbContext* context)
{
    delete context;
}
extern "C" int32_t muguet_fg_npu_rgb_count_nonfinite_half(const uint16_t* values, size_t count,
    uint64_t* output)
{
    if (!output || (!values && count) || count > std::numeric_limits<size_t>::max() / sizeof(uint16_t))
        return MUGUET_FG_NPU_RGB_INVALID_ARGUMENT;
    *output = CountNonfiniteHalf(values, count);
    return MUGUET_FG_NPU_RGB_OK;
}
