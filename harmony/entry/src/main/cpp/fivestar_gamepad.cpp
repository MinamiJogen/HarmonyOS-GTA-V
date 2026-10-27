#include "fivestar_gamepad.h"

#include <GameControllerKit/game_device.h>
#include <GameControllerKit/game_pad.h>
#include <multimodalinput/oh_key_code.h>
#include <cstdlib>
#include <chrono>
#include <map>
#include <mutex>
#include <set>
#include <string>
#include <vector>

#if MG_HAS_GAME_RUNTIME
#include "muguet_game_runtime.h"
#endif

namespace fivestar_gamepad {
namespace {
using detail::Sample;
constexpr uint32_t L2 = 1, R2 = 2, L1 = 4, R1 = 8, Y = 16, B = 32, A = 64, X = 128;
constexpr uint32_t SELECT = 256, L3 = 512, R3 = 1024, START = 2048;
constexpr uint32_t UP_BIT = 4096, RIGHT_BIT = 8192, DOWN_BIT = 16384, LEFT_BIT = 32768;
struct Device {
    Sample sample{};
    uint32_t key_buttons = 0;
    uint32_t hat_buttons = 0;
    bool left_digital = false, right_digital = false;
    bool left_axis_seen = false, right_axis_seen = false;
};
std::mutex state_mutex;
std::once_flag initialize_once;
std::map<std::string, Device> devices;
std::set<std::string> offline_devices;
std::vector<std::string> device_order;
uint64_t device_revision = 0;
Sample touch_sample{};
bool focused = false;
bool initialized = false;
int64_t focus_started_at = 0;

bool publish_locked() {
    Sample physical{};
    // Like GCController.controllers().first(extendedGamepad != nil), one controller
    // owns the physical sample; another pad cannot unexpectedly combine its sticks.
    for (const auto &id : device_order) {
        const auto it = devices.find(id);
        if (it == devices.end()) continue;
        const Device &d = it->second;
        physical = d.sample;
        physical.active = true;
        physical.buttons = d.key_buttons | d.hat_buttons;
        physical.lt = d.left_axis_seen ? d.sample.lt : d.left_digital ? 1.f : 0.f;
        physical.rt = d.right_axis_seen ? d.sample.rt : d.right_digital ? 1.f : 0.f;
        break;
    }
    const Sample merged = focused ? detail::merge(physical, touch_sample) : Sample{};
#if MG_HAS_GAME_RUNTIME
    // Keeping publication within the mutex preserves callback order. This FFI only
    // publishes atomics; no guest execution or callbacks run on the monitor thread.
    return mg_game_virtual_pad(merged.buttons, merged.lx, merged.ly, merged.rx, merged.ry,
                               merged.lt, merged.rt, merged.active);
#else
    (void)merged;
    return false;
#endif
}

Device &ensure_device_locked(const std::string &id) {
    auto found = devices.find(id);
    if (found != devices.end()) return found->second;
    device_order.push_back(id);
    ++device_revision;
    return devices.emplace(id, Device{}).first->second;
}
std::string take_string(char *value) {
    std::string result = value ? value : "";
    std::free(value); // SDK output char** allocates; documented sample uses free().
    return result;
}
std::string button_device(const GamePad_ButtonEvent *event) {
    char *id = nullptr;
    if (OH_GamePad_ButtonEvent_GetDeviceId(event, &id) != GAME_CONTROLLER_SUCCESS) {
        std::free(id); return {};
    }
    return take_string(id);
}
std::string axis_device(const GamePad_AxisEvent *event) {
    char *id = nullptr;
    if (OH_GamePad_AxisEvent_GetDeviceId(event, &id) != GAME_CONTROLLER_SUCCESS) {
        std::free(id); return {};
    }
    return take_string(id);
}
uint32_t button_bit(int32_t code) {
    switch (code) {
        case KEYCODE_BUTTON_A: return A; case KEYCODE_BUTTON_B: return B;
        case KEYCODE_BUTTON_X: return X; case KEYCODE_BUTTON_Y: return Y;
        case KEYCODE_BUTTON_L1: return L1; case KEYCODE_BUTTON_R1: return R1;
        case KEYCODE_BUTTON_L2: return L2; case KEYCODE_BUTTON_R2: return R2;
        case KEYCODE_BUTTON_SELECT: return SELECT; case KEYCODE_BUTTON_START: return START;
        case KEYCODE_BUTTON_THUMBL: return L3; case KEYCODE_BUTTON_THUMBR: return R3;
        case KEYCODE_DPAD_UP: return UP_BIT; case KEYCODE_DPAD_RIGHT: return RIGHT_BIT;
        case KEYCODE_DPAD_DOWN: return DOWN_BIT; case KEYCODE_DPAD_LEFT: return LEFT_BIT;
        default: return 0;
    }
}
void on_button(const GamePad_ButtonEvent *event, uint32_t fallback) {
    if (!event) return;
    GamePad_Button_ActionType action{};
    if (OH_GamePad_ButtonEvent_GetButtonAction(event, &action) != GAME_CONTROLLER_SUCCESS ||
        (action != DOWN && action != UP)) return;
    const std::string id = button_device(event);
    if (id.empty()) return;
    int32_t code = 0;
    const bool code_ok = OH_GamePad_ButtonEvent_GetButtonCode(event, &code) == GAME_CONTROLLER_SUCCESS;
    const uint32_t from_code = code_ok ? button_bit(code) : 0;
    const uint32_t bit = from_code ? from_code : fallback;
    if (!bit) return; // System Home/Mode is not a gameplay command.
    const bool down = action == DOWN;
    int64_t action_time = 0;
    const bool has_time = OH_GamePad_ButtonEvent_GetActionTime(event, &action_time) == GAME_CONTROLLER_SUCCESS && action_time > 0;
    std::lock_guard<std::mutex> lock(state_mutex);
    if (offline_devices.count(id)) return;
    Device &d = ensure_device_locked(id);
    if (!focused || !initialized || (has_time && action_time < focus_started_at)) return;
    if (bit == L2) d.left_digital = down;
    else if (bit == R2) d.right_digital = down;
    else if (down) d.key_buttons |= bit;
    else d.key_buttons &= ~bit;
    publish_locked();
}
#define BUTTON_CALLBACK(name, bit) \
    void name(const GamePad_ButtonEvent *event) { on_button(event, bit); }
BUTTON_CALLBACK(left_shoulder, L1)
BUTTON_CALLBACK(right_shoulder, R1)
BUTTON_CALLBACK(left_trigger, L2)
BUTTON_CALLBACK(right_trigger, R2)
BUTTON_CALLBACK(menu_button, START)
BUTTON_CALLBACK(home_button, 0)
BUTTON_CALLBACK(a_button, A)
BUTTON_CALLBACK(b_button, B)
BUTTON_CALLBACK(x_button, X)
BUTTON_CALLBACK(y_button, Y)
// The public kit has C but no distinct Select registration. Prefer the event's
// KEYCODE_BUTTON_SELECT when available; C is the additional app-facing button.
BUTTON_CALLBACK(c_button, SELECT)
BUTTON_CALLBACK(dpad_left, LEFT_BIT)
BUTTON_CALLBACK(dpad_right, RIGHT_BIT)
BUTTON_CALLBACK(dpad_up, UP_BIT)
BUTTON_CALLBACK(dpad_down, DOWN_BIT)
BUTTON_CALLBACK(left_thumb, L3)
BUTTON_CALLBACK(right_thumb, R3)
#undef BUTTON_CALLBACK

using AxisGetter = GameController_ErrorCode (*)(const GamePad_AxisEvent *, double *);
bool read_axis(const GamePad_AxisEvent *event, AxisGetter getter, float &value, bool invert = false) {
    double raw = 0;
    if (getter(event, &raw) != GAME_CONTROLLER_SUCCESS || !std::isfinite(raw)) return false;
    value = static_cast<float>(std::clamp(invert ? -raw : raw, -1.0, 1.0));
    return true;
}
void on_axis(const GamePad_AxisEvent *event) {
    if (!event) return;
    GamePad_AxisSourceType source{};
    if (OH_GamePad_AxisEvent_GetAxisSourceType(event, &source) != GAME_CONTROLLER_SUCCESS) return;
    const std::string id = axis_device(event);
    if (id.empty()) return;
    float x = 0, y = 0;
    switch (source) {
        case LEFT_THUMBSTICK:
            if (!read_axis(event, OH_GamePad_AxisEvent_GetXAxisValue, x) ||
                !read_axis(event, OH_GamePad_AxisEvent_GetYAxisValue, y, true)) return;
            break;
        case RIGHT_THUMBSTICK:
            if (!read_axis(event, OH_GamePad_AxisEvent_GetZAxisValue, x) ||
                !read_axis(event, OH_GamePad_AxisEvent_GetRZAxisValue, y, true)) return;
            break;
        case DPAD:
            if (!read_axis(event, OH_GamePad_AxisEvent_GetHatXAxisValue, x) ||
                !read_axis(event, OH_GamePad_AxisEvent_GetHatYAxisValue, y, true)) return;
            break;
        case LEFT_TRIGGER:
            if (!read_axis(event, OH_GamePad_AxisEvent_GetBrakeAxisValue, x)) return;
            break;
        case RIGHT_TRIGGER:
            if (!read_axis(event, OH_GamePad_AxisEvent_GetGasAxisValue, x)) return;
            break;
        default: return;
    }
    int64_t action_time = 0;
    const bool has_time = OH_GamePad_AxisEvent_GetActionTime(event, &action_time) == GAME_CONTROLLER_SUCCESS && action_time > 0;
    std::lock_guard<std::mutex> lock(state_mutex);
    if (offline_devices.count(id)) return;
    Device &d = ensure_device_locked(id);
    if (!focused || !initialized || (has_time && action_time < focus_started_at)) return;
    // MMI's physical Y/RZ/HatY use positive down. The upstream pad contract uses
    // positive up; the sole sign conversion is at this native boundary.
    switch (source) {
        case LEFT_THUMBSTICK: d.sample.lx = x; d.sample.ly = y; break;
        case RIGHT_THUMBSTICK: d.sample.rx = x; d.sample.ry = y; break;
        case DPAD:
            d.hat_buttons = (x < -.5f ? LEFT_BIT : 0) | (x > .5f ? RIGHT_BIT : 0) |
                (y < -.5f ? DOWN_BIT : 0) | (y > .5f ? UP_BIT : 0);
            break;
        // Trigger getters expose a signed range but the pressure portion is 0..1.
        // Never turn a negative resting value into a half-pressed trigger.
        case LEFT_TRIGGER: d.sample.lt = std::max(0.f, x); d.left_axis_seen = true; break;
        case RIGHT_TRIGGER: d.sample.rt = std::max(0.f, x); d.right_axis_seen = true; break;
        default: return;
    }
    publish_locked();
}

std::string device_info_id(GameDevice_DeviceInfo *info) {
    char *id = nullptr;
    if (OH_GameDevice_DeviceInfo_GetDeviceId(info, &id) != GAME_CONTROLLER_SUCCESS) {
        std::free(id); return {};
    }
    return take_string(id);
}
void on_device(const GameDevice_DeviceEvent *event) {
    if (!event) return;
    GameDevice_StatusChangedType change{};
    GameDevice_DeviceInfo *info = nullptr;
    if (OH_GameDevice_DeviceEvent_GetChangedType(event, &change) != GAME_CONTROLLER_SUCCESS ||
        OH_GameDevice_DeviceEvent_GetDeviceInfo(event, &info) != GAME_CONTROLLER_SUCCESS || !info) return;
    GameDevice_DeviceType type = UNKNOWN;
    const bool gamepad = OH_GameDevice_DeviceInfo_GetDeviceType(info, &type) == GAME_CONTROLLER_SUCCESS && type == GAME_PAD;
    const std::string id = gamepad ? device_info_id(info) : std::string{};
    OH_GameDevice_DestroyDeviceInfo(&info);
    if (id.empty()) return;
    std::lock_guard<std::mutex> lock(state_mutex);
    if (change == ONLINE) { offline_devices.erase(id); ensure_device_locked(id); }
    else if (change == OFFLINE) {
        offline_devices.insert(id);
        devices.erase(id);
        device_order.erase(std::remove(device_order.begin(), device_order.end(), id), device_order.end());
        ++device_revision;
    }
    // Disconnect drops that controller's complete state immediately. A surviving
    // first pad or the still-owned touch source is published in the same callback.
    publish_locked();
}
void enumerate_devices() {
    uint64_t revision = 0;
    { std::lock_guard<std::mutex> lock(state_mutex); revision = device_revision; }
    GameDevice_AllDeviceInfos *all = nullptr;
    if (OH_GameDevice_GetAllDeviceInfos(&all) != GAME_CONTROLLER_SUCCESS || !all) return;
    std::vector<std::string> online;
    int32_t count = 0;
    if (OH_GameDevice_AllDeviceInfos_GetCount(all, &count) == GAME_CONTROLLER_SUCCESS && count >= 0) {
        for (int32_t index = 0; index < std::min(count, 128); ++index) {
            GameDevice_DeviceInfo *info = nullptr;
            if (OH_GameDevice_AllDeviceInfos_GetDeviceInfo(all, index, &info) != GAME_CONTROLLER_SUCCESS || !info) continue;
            GameDevice_DeviceType type = UNKNOWN;
            if (OH_GameDevice_DeviceInfo_GetDeviceType(info, &type) == GAME_CONTROLLER_SUCCESS && type == GAME_PAD) {
                const std::string id = device_info_id(info);
                if (!id.empty()) online.push_back(id);
            }
            OH_GameDevice_DestroyDeviceInfo(&info);
        }
    }
    OH_GameDevice_DestroyAllDeviceInfos(&all);
    std::lock_guard<std::mutex> lock(state_mutex);
    // A simultaneous connect/disconnect callback is newer than this enumeration.
    if (revision == device_revision) for (const auto &id : online) ensure_device_locked(id);
    publish_locked();
}

using RegisterButton = GameController_ErrorCode (*)(GamePad_ButtonInputMonitorCallback);
using RegisterAxis = GameController_ErrorCode (*)(GamePad_AxisInputMonitorCallback);
using Unregister = GameController_ErrorCode (*)();
struct ButtonRegistration { RegisterButton add; Unregister remove; GamePad_ButtonInputMonitorCallback callback; };
struct AxisRegistration { RegisterAxis add; Unregister remove; };
#define BUTTON_REG(name, callback) {OH_GamePad_##name##_RegisterButtonInputMonitor, OH_GamePad_##name##_UnregisterButtonInputMonitor, callback}
const ButtonRegistration button_registrations[] = {
    BUTTON_REG(LeftShoulder, left_shoulder), BUTTON_REG(RightShoulder, right_shoulder),
    BUTTON_REG(LeftTrigger, left_trigger), BUTTON_REG(RightTrigger, right_trigger),
    BUTTON_REG(ButtonMenu, menu_button), BUTTON_REG(ButtonHome, home_button),
    BUTTON_REG(ButtonA, a_button), BUTTON_REG(ButtonB, b_button), BUTTON_REG(ButtonX, x_button),
    BUTTON_REG(ButtonY, y_button), BUTTON_REG(ButtonC, c_button),
    BUTTON_REG(Dpad_LeftButton, dpad_left), BUTTON_REG(Dpad_RightButton, dpad_right),
    BUTTON_REG(Dpad_UpButton, dpad_up), BUTTON_REG(Dpad_DownButton, dpad_down),
    BUTTON_REG(LeftThumbstick, left_thumb), BUTTON_REG(RightThumbstick, right_thumb)
};
#undef BUTTON_REG
#define AXIS_REG(name) {OH_GamePad_##name##_RegisterAxisInputMonitor, OH_GamePad_##name##_UnregisterAxisInputMonitor}
const AxisRegistration axis_registrations[] = {
    AXIS_REG(LeftThumbstick), AXIS_REG(RightThumbstick), AXIS_REG(Dpad), AXIS_REG(LeftTrigger), AXIS_REG(RightTrigger)
};
#undef AXIS_REG
} // namespace

bool initialize() {
    std::call_once(initialize_once, [] {
        if (OH_GameDevice_RegisterDeviceMonitor(on_device) != GAME_CONTROLLER_SUCCESS) return;
        size_t buttons_added = 0, axes_added = 0;
        for (const auto &registration : button_registrations) {
            if (registration.add(registration.callback) != GAME_CONTROLLER_SUCCESS) break;
            ++buttons_added;
        }
        if (buttons_added == std::size(button_registrations)) {
            for (const auto &registration : axis_registrations) {
                if (registration.add(on_axis) != GAME_CONTROLLER_SUCCESS) break;
                ++axes_added;
            }
        }
        if (buttons_added != std::size(button_registrations) || axes_added != std::size(axis_registrations)) {
            for (size_t i = 0; i < axes_added; ++i) axis_registrations[i].remove();
            for (size_t i = 0; i < buttons_added; ++i) button_registrations[i].remove();
            OH_GameDevice_UnregisterDeviceMonitor();
            return;
        }
        { std::lock_guard<std::mutex> lock(state_mutex); initialized = true; }
        enumerate_devices();
    });
    std::lock_guard<std::mutex> lock(state_mutex);
    return initialized;
}
bool connected() {
    std::lock_guard<std::mutex> lock(state_mutex);
    return initialized && !devices.empty();
}
void set_focus(bool value) {
    std::lock_guard<std::mutex> lock(state_mutex);
    if (focused == value) return;
    focused = value;
    focus_started_at = value ? std::chrono::duration_cast<std::chrono::milliseconds>(
        std::chrono::system_clock::now().time_since_epoch()).count() : 0;
    touch_sample = {};
    for (auto &entry : devices) {
        // Keep the connection inventory, clear every held axis/button. Focus return
        // requires fresh controller events; it cannot revive an old accelerator.
        entry.second = Device{};
    }
    publish_locked();
}
bool update_touch(uint32_t buttons, float lx, float ly, float rx, float ry, float lt, float rt, bool active) {
    const Sample supplied{buttons, lx, ly, rx, ry, lt, rt, active};
    std::lock_guard<std::mutex> lock(state_mutex);
    const bool valid = !active || detail::finite(supplied);
    touch_sample = focused && valid ? detail::normalized(supplied) : Sample{};
    const bool accepted = publish_locked();
    return valid && (!active || focused) && accepted;
}
void release_touch() {
    std::lock_guard<std::mutex> lock(state_mutex);
    touch_sample = {};
    publish_locked();
}
bool is_gamepad_key_code(int32_t code, bool include_dpad) {
    if (code >= 2301 && code <= 2315) return true;
    return include_dpad && code >= KEYCODE_DPAD_UP && code <= KEYCODE_DPAD_RIGHT;
}
} // namespace fivestar_gamepad
