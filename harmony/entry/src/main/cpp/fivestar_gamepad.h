#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>

namespace fivestar_gamepad {
// GameControllerKit monitors are native callbacks. Initialization registers the device
// monitor before the button/axis monitors, and also enumerates already-connected pads.
bool initialize();
bool connected();
void set_focus(bool focused);
bool update_touch(uint32_t buttons, float lx, float ly, float rx, float ry,
                  float lt, float rt, bool active);
// Releases only the touch source; a held physical controller keeps its own input.
void release_touch();

// GameControllerKit observes rather than consumes compatibility KeyEvents. The host
// must avoid sending these same keys into its PC keyboard path. D-pad codes are also
// real keyboard arrows: include_dpad is safe only for a confirmed joystick source.
bool is_gamepad_key_code(int32_t code, bool include_dpad = false);

namespace detail {
struct Sample {
    uint32_t buttons = 0;
    float lx = 0, ly = 0, rx = 0, ry = 0, lt = 0, rt = 0;
    bool active = false;
};
inline bool finite(const Sample &s) {
    return std::isfinite(s.lx) && std::isfinite(s.ly) && std::isfinite(s.rx) &&
        std::isfinite(s.ry) && std::isfinite(s.lt) && std::isfinite(s.rt);
}
inline Sample normalized(Sample s) {
    if (!s.active || !finite(s)) return {};
    s.buttons &= 0xffffu;
    s.lx = std::clamp(s.lx, -1.f, 1.f); s.ly = std::clamp(s.ly, -1.f, 1.f);
    s.rx = std::clamp(s.rx, -1.f, 1.f); s.ry = std::clamp(s.ry, -1.f, 1.f);
    s.lt = std::clamp(s.lt, 0.f, 1.f); s.rt = std::clamp(s.rt, 0.f, 1.f);
    return s;
}
// Same order as upstream PGPadState.merge: the physical pad is the receiver and
// touch is the added source. Equal-magnitude axes preserve the physical value.
inline Sample merge(const Sample &physical, const Sample &touch) {
    const Sample a = normalized(physical), b = normalized(touch);
    const auto pick = [](float x, float y) { return std::fabs(y) > std::fabs(x) ? y : x; };
    return {a.buttons | b.buttons, pick(a.lx, b.lx), pick(a.ly, b.ly), pick(a.rx, b.rx), pick(a.ry, b.ry),
        std::max(a.lt, b.lt), std::max(a.rt, b.rt), a.active || b.active};
}
} // namespace detail
} // namespace fivestar_gamepad
