// Host regression of the actual adapter with SDK-shaped fake events/functions.
// Compile with the SDK GameControllerKit/info/multimodalinput header directories
// (symlink only those directories into an include root; do not replace host libc).
// This does not establish Bluetooth/device axis or controller compatibility.
#define MG_HAS_GAME_RUNTIME 1
#include "../harmony/entry/src/main/cpp/fivestar_gamepad.cpp"
#include <cassert>
#include <cstring>
#include <iostream>
#include <limits>

struct GamePad_ButtonEvent { std::string id; GamePad_Button_ActionType action; int32_t code; int64_t time = 0; };
struct GamePad_AxisEvent {
    std::string id; GamePad_AxisSourceType source; double x = 0, y = 0, z = 0, rz = 0;
    double hatx = 0, haty = 0, brake = 0, gas = 0; int64_t time = 0;
};
struct GameDevice_DeviceInfo { std::string id; GameDevice_DeviceType type = GAME_PAD; };
struct GameDevice_DeviceEvent { GameDevice_StatusChangedType change; GameDevice_DeviceInfo info; };
struct GameDevice_AllDeviceInfos { std::vector<GameDevice_DeviceInfo> infos; };

using fivestar_gamepad::detail::Sample;
static Sample delivered{};
static unsigned publications = 0, registrations = 0;
static GameDevice_DeviceMonitorCallback device_monitor = nullptr;
static std::map<std::string, GamePad_ButtonInputMonitorCallback> button_monitors;
static std::map<std::string, GamePad_AxisInputMonitorCallback> axis_monitors;
static char *copy_string(const std::string &s) {
    auto *out = static_cast<char *>(std::malloc(s.size() + 1));
    std::memcpy(out, s.c_str(), s.size() + 1); return out;
}
extern "C" bool mg_game_virtual_pad(uint32_t buttons, float lx, float ly, float rx, float ry, float lt, float rt, bool active) {
    delivered = {buttons, lx, ly, rx, ry, lt, rt, active}; ++publications; return true;
}
#define MOCK_BUTTON(name) \
    extern "C" GameController_ErrorCode OH_GamePad_##name##_RegisterButtonInputMonitor(GamePad_ButtonInputMonitorCallback cb) { \
        assert(device_monitor); button_monitors[#name] = cb; ++registrations; return GAME_CONTROLLER_SUCCESS; } \
    extern "C" GameController_ErrorCode OH_GamePad_##name##_UnregisterButtonInputMonitor() { button_monitors.erase(#name); return GAME_CONTROLLER_SUCCESS; }
MOCK_BUTTON(LeftShoulder) MOCK_BUTTON(RightShoulder) MOCK_BUTTON(LeftTrigger) MOCK_BUTTON(RightTrigger)
MOCK_BUTTON(ButtonMenu) MOCK_BUTTON(ButtonHome) MOCK_BUTTON(ButtonA) MOCK_BUTTON(ButtonB) MOCK_BUTTON(ButtonX)
MOCK_BUTTON(ButtonY) MOCK_BUTTON(ButtonC) MOCK_BUTTON(Dpad_LeftButton) MOCK_BUTTON(Dpad_RightButton)
MOCK_BUTTON(Dpad_UpButton) MOCK_BUTTON(Dpad_DownButton) MOCK_BUTTON(LeftThumbstick) MOCK_BUTTON(RightThumbstick)
#undef MOCK_BUTTON
#define MOCK_AXIS(name) \
    extern "C" GameController_ErrorCode OH_GamePad_##name##_RegisterAxisInputMonitor(GamePad_AxisInputMonitorCallback cb) { \
        assert(device_monitor); axis_monitors[#name] = cb; ++registrations; return GAME_CONTROLLER_SUCCESS; } \
    extern "C" GameController_ErrorCode OH_GamePad_##name##_UnregisterAxisInputMonitor() { axis_monitors.erase(#name); return GAME_CONTROLLER_SUCCESS; }
MOCK_AXIS(LeftThumbstick) MOCK_AXIS(RightThumbstick) MOCK_AXIS(Dpad) MOCK_AXIS(LeftTrigger) MOCK_AXIS(RightTrigger)
#undef MOCK_AXIS
extern "C" GameController_ErrorCode OH_GameDevice_RegisterDeviceMonitor(GameDevice_DeviceMonitorCallback cb) { device_monitor = cb; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GameDevice_UnregisterDeviceMonitor() { device_monitor = nullptr; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GameDevice_GetAllDeviceInfos(GameDevice_AllDeviceInfos **all) {
    *all = new GameDevice_AllDeviceInfos{{{"pad-one", GAME_PAD}}}; return GAME_CONTROLLER_SUCCESS;
}
extern "C" GameController_ErrorCode OH_GameDevice_AllDeviceInfos_GetCount(const GameDevice_AllDeviceInfos *all, int32_t *count) { *count = all->infos.size(); return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GameDevice_AllDeviceInfos_GetDeviceInfo(const GameDevice_AllDeviceInfos *all, int32_t index, GameDevice_DeviceInfo **info) { *info = new GameDevice_DeviceInfo(all->infos.at(index)); return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GameDevice_DestroyAllDeviceInfos(GameDevice_AllDeviceInfos **all) { delete *all; *all = nullptr; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GameDevice_DestroyDeviceInfo(GameDevice_DeviceInfo **info) { delete *info; *info = nullptr; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GameDevice_DeviceInfo_GetDeviceType(const GameDevice_DeviceInfo *info, GameDevice_DeviceType *type) { *type = info->type; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GameDevice_DeviceInfo_GetDeviceId(const GameDevice_DeviceInfo *info, char **id) { *id = copy_string(info->id); return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GameDevice_DeviceEvent_GetChangedType(const GameDevice_DeviceEvent *e, GameDevice_StatusChangedType *type) { *type = e->change; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GameDevice_DeviceEvent_GetDeviceInfo(const GameDevice_DeviceEvent *e, GameDevice_DeviceInfo **info) { *info = new GameDevice_DeviceInfo(e->info); return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GamePad_ButtonEvent_GetDeviceId(const GamePad_ButtonEvent *e, char **id) { *id = copy_string(e->id); return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GamePad_ButtonEvent_GetButtonAction(const GamePad_ButtonEvent *e, GamePad_Button_ActionType *action) { *action = e->action; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GamePad_ButtonEvent_GetButtonCode(const GamePad_ButtonEvent *e, int32_t *code) { *code = e->code; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GamePad_ButtonEvent_GetActionTime(const GamePad_ButtonEvent *e, int64_t *time) { *time = e->time; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GamePad_AxisEvent_GetDeviceId(const GamePad_AxisEvent *e, char **id) { *id = copy_string(e->id); return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GamePad_AxisEvent_GetAxisSourceType(const GamePad_AxisEvent *e, GamePad_AxisSourceType *source) { *source = e->source; return GAME_CONTROLLER_SUCCESS; }
extern "C" GameController_ErrorCode OH_GamePad_AxisEvent_GetActionTime(const GamePad_AxisEvent *e, int64_t *time) { *time = e->time; return GAME_CONTROLLER_SUCCESS; }
#define MOCK_GETTER(name, member) \
    extern "C" GameController_ErrorCode OH_GamePad_AxisEvent_Get##name##Value(const GamePad_AxisEvent *e, double *value) { *value = e->member; return GAME_CONTROLLER_SUCCESS; }
MOCK_GETTER(XAxis, x) MOCK_GETTER(YAxis, y) MOCK_GETTER(ZAxis, z) MOCK_GETTER(RZAxis, rz)
MOCK_GETTER(HatXAxis, hatx) MOCK_GETTER(HatYAxis, haty) MOCK_GETTER(BrakeAxis, brake) MOCK_GETTER(GasAxis, gas)
#undef MOCK_GETTER

int main() {
    using namespace fivestar_gamepad;
    // Equal-magnitude axes preserve the physical source; buttons OR, triggers max.
    Sample physical{64, .5f, -.8f, .2f, -.2f, .3f, .7f, true};
    Sample touch{128, -.5f, .2f, -.9f, .6f, .8f, .1f, true};
    auto merged = detail::merge(physical, touch);
    assert(merged.buttons == 192 && merged.lx == .5f && merged.ly == -.8f && merged.rx == -.9f && merged.ry == .6f);
    assert(merged.lt == .8f && merged.rt == .7f && merged.active);
    touch.active = false; assert(detail::merge(physical, touch).buttons == 64);
    touch.active = true; touch.lx = std::numeric_limits<float>::infinity();
    assert(detail::merge(physical, touch).buttons == 64);
    assert(!detail::merge({}, {}).active);
    assert(initialize() && connected()); assert(registrations == 22);
    assert(initialize() && registrations == 22); // idempotent, no duplicate monitors
    set_focus(true);
    GamePad_ButtonEvent a{"pad-one", DOWN, KEYCODE_BUTTON_A};
    button_monitors.at("ButtonA")(&a);
    assert(delivered.buttons == 64 && delivered.active); // no touch timer ran
    assert(update_touch(128, -.7f, .2f, 0, 0, 0, .8f, true));
    assert(delivered.buttons == 192 && delivered.lx == -.7f && delivered.rt == .8f);
    release_touch(); assert(delivered.buttons == 64 && delivered.lx == 0 && delivered.rt == 0);
    a.action = UP; button_monitors.at("ButtonA")(&a); assert(delivered.buttons == 0);
    GamePad_AxisEvent left{"pad-one", LEFT_THUMBSTICK}; left.x = .4; left.y = -.75;
    axis_monitors.at("LeftThumbstick")(&left); assert(delivered.lx == .4f && delivered.ly == .75f);
    GamePad_AxisEvent right{"pad-one", RIGHT_THUMBSTICK}; right.z = -.6; right.rz = .8;
    axis_monitors.at("RightThumbstick")(&right); assert(delivered.rx == -.6f && delivered.ry == -.8f);
    GamePad_ButtonEvent lt{"pad-one", DOWN, KEYCODE_BUTTON_L2}; button_monitors.at("LeftTrigger")(&lt);
    assert(delivered.lt == 1);
    GamePad_AxisEvent trigger{"pad-one", LEFT_TRIGGER}; trigger.brake = .2;
    axis_monitors.at("LeftTrigger")(&trigger); assert(delivered.lt == .2f);
    lt.action = UP; button_monitors.at("LeftTrigger")(&lt); assert(delivered.lt == .2f); // analog wins
    trigger.brake = -1; axis_monitors.at("LeftTrigger")(&trigger); assert(delivered.lt == 0);
    GamePad_AxisEvent hat{"pad-one", DPAD}; hat.hatx = 1; hat.haty = -1;
    axis_monitors.at("Dpad")(&hat); assert((delivered.buttons & (4096 | 8192)) == (4096 | 8192));
    GamePad_ButtonEvent dp{"pad-one", DOWN, KEYCODE_DPAD_RIGHT}; button_monitors.at("Dpad_RightButton")(&dp);
    dp.action = UP; button_monitors.at("Dpad_RightButton")(&dp); assert(delivered.buttons & 8192); // hat owns it too
    hat.hatx = 0; hat.haty = 0; axis_monitors.at("Dpad")(&hat); assert((delivered.buttons & 0xf000) == 0);
    GameDevice_DeviceEvent second{ONLINE, {"pad-two", GAME_PAD}}; device_monitor(&second);
    GamePad_ButtonEvent second_x{"pad-two", DOWN, KEYCODE_BUTTON_X}; button_monitors.at("ButtonX")(&second_x);
    assert((delivered.buttons & 128) == 0); // first controller matches upstream selection
    GameDevice_DeviceEvent first_offline{OFFLINE, {"pad-one", GAME_PAD}}; device_monitor(&first_offline);
    assert(connected() && delivered.buttons == 128); // next controller takes over
    left.x = 1; axis_monitors.at("LeftThumbstick")(&left); assert(delivered.lx == 0); // stale disconnect callback ignored
    assert(update_touch(16, 0, 0, 0, 0, .5f, 0, true));
    second.change = OFFLINE; device_monitor(&second); assert(!connected() && delivered.buttons == 16 && delivered.lt == .5f);
    release_touch(); assert(!delivered.active && delivered.buttons == 0);
    second.change = ONLINE; device_monitor(&second);
    button_monitors.at("ButtonX")(&second_x); assert(delivered.buttons == 128);
    set_focus(false); assert(!delivered.active && delivered.buttons == 0);
    button_monitors.at("ButtonX")(&second_x); assert(!delivered.active);
    set_focus(true); assert(delivered.active && delivered.buttons == 0);
    second_x.time = 1; button_monitors.at("ButtonX")(&second_x); assert(delivered.buttons == 0); // queued before focus
    second_x.time = 0; button_monitors.at("ButtonX")(&second_x); assert(delivered.buttons == 128);
    assert(!update_touch(16, std::numeric_limits<float>::quiet_NaN(), 0, 0, 0, 0, 0, true));
    assert(delivered.buttons == 128); // malformed touch cannot poison the physical sample
    assert(is_gamepad_key_code(KEYCODE_BUTTON_A)); assert(!is_gamepad_key_code(KEYCODE_DPAD_UP));
    assert(is_gamepad_key_code(KEYCODE_DPAD_UP, true)); assert(!is_gamepad_key_code(2017));
    assert(publications > 25);
    std::cout << "Five Star native gamepad callback/merge checks passed; publications=" << publications << '\n';
}
