#ifndef MUGUET_GRAPHICS_FRAME_RATE_POLICY_H
#define MUGUET_GRAPHICS_FRAME_RATE_POLICY_H

#include <cmath>
#include <cstdint>

namespace muguet::fg_probe {
constexpr uint32_t kDefaultSourceFps = 15;
constexpr uint32_t kDefaultTargetFps = 30;

inline bool validFrameRates(double sourceFps, double targetFps, bool fgEnabled) {
    return std::isfinite(sourceFps) && std::isfinite(targetFps) &&
        std::floor(sourceFps) == sourceFps && std::floor(targetFps) == targetFps &&
        sourceFps >= 15 && sourceFps <= 60 && targetFps >= 30 && targetFps <= 144 &&
        (!fgEnabled || sourceFps < targetFps);
}

inline uint64_t sourceDeadlineNs(uint32_t frame, uint32_t sourceFps) {
    return 1'000'000'000ULL * frame / sourceFps;
}

// Match the original 30 Hz trial's 12 pixels per frame in wall-clock time.
inline uint32_t movingBarX(double elapsedSeconds, uint32_t offset, uint32_t span) {
    return static_cast<uint32_t>(std::fmod(elapsedSeconds * 360.0 + offset, span));
}

inline float geometryAngle(double elapsedSeconds) {
    return .18f * std::sin(static_cast<float>(elapsedSeconds * 6.283185307179586 / 4.0));
}
} // namespace muguet::fg_probe
#endif
