#include "fg_interpolation_bridge.h"
#include "fg_npu_rgb.h"
#include <cassert>
#include <cstdlib>
#include <cstring>

struct MuguetFgNpuRgbContext {};
extern "C" int32_t muguet_fg_npu_rgb_create(MuguetFgNpuRgbContext** out) { *out = new MuguetFgNpuRgbContext; return 0; }
extern "C" void muguet_fg_npu_rgb_destroy(MuguetFgNpuRgbContext* value) { delete value; }
extern "C" int32_t muguet_fg_npu_rgb_get_timing(const MuguetFgNpuRgbContext*, MuguetFgNpuRgbTiming*, size_t) { return 0; }
extern "C" const char* muguet_fg_npu_rgb_last_error(const MuguetFgNpuRgbContext*) { return "fixture"; }
extern "C" int32_t muguet_fg_npu_rgb_process_rgb8(MuguetFgNpuRgbContext*, const uint8_t*, const uint8_t*, int, int, uint8_t*, size_t, float) { return 0; }

int main() {
    assert(muguet_interpolation_rife_available() == 0);
    char error[256]{};
    setenv("MG_FRAME_INTERPOLATION", "rife", 1);
    assert(!muguet_interpolation_create("/missing", -1, error, sizeof error));
    assert(std::strstr(error, "RIFE unavailable"));
    setenv("MG_FRAME_INTERPOLATION", "npu", 1);
    auto* npu = muguet_interpolation_create(nullptr, -1, error, sizeof error);
    assert(npu);
    MuguetInterpolationDeviceInfo info{};
    assert(muguet_interpolation_get_device_info(npu, &info) == 0 && info.gpu_index == -1);
    muguet_interpolation_destroy(npu);
}
