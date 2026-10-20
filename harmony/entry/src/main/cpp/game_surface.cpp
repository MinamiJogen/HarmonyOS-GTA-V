#include "game_surface.h"
#include "game_resource_import.h"
#include "game_save_import_core.h"
#include "game_save_import.h"
#include <ace/xcomponent/native_interface_xcomponent.h>
#include <hilog/log.h>
#include <native_window/external_window.h>
#include <cstdint>
#include <mutex>
#include <string>

#if MG_HAS_GAME_RUNTIME
#include "muguet_game_runtime.h"
#endif

namespace {
constexpr unsigned int kDomain = 0x4100;
constexpr const char *kTag = "HarmonyGame";
std::mutex stateMutex;
void *nativeWindow = nullptr;
uint32_t surfaceWidth = 0;
uint32_t surfaceHeight = 0;
std::string filesDirectory;
bool startRequested = false;
bool startDispatched = false;
bool gameStartedOnce = false;
bool resourceImportBlocked = false;
int32_t startResult = -1;
std::string surfaceError;
bool aspectFitApplied = false;
int32_t aspectFitResult = -1;

bool ResourceImportGate(bool begin) {
    std::lock_guard<std::mutex> lock(stateMutex);
    if (begin) {
        // Rust's process-wide guest cannot be restarted after it was stopped.
        // Keep resource replacement impossible for the rest of this process.
        if (startRequested || startDispatched || gameStartedOnce || resourceImportBlocked) return false;
        resourceImportBlocked = true;
    } else {
        resourceImportBlocked = false;
    }
    return true;
}

void SetBool(napi_env env, napi_value object, const char *key, bool value) {
    napi_value result = nullptr;
    napi_get_boolean(env, value, &result);
    napi_set_named_property(env, object, key, result);
}
void SetInt(napi_env env, napi_value object, const char *key, int32_t value) {
    napi_value result = nullptr;
    napi_create_int32(env, value, &result);
    napi_set_named_property(env, object, key, result);
}
void SetString(napi_env env, napi_value object, const char *key, const std::string &value) {
    napi_value result = nullptr;
    napi_create_string_utf8(env, value.data(), value.size(), &result);
    napi_set_named_property(env, object, key, result);
}
std::string ReadString(napi_env env, napi_value value) {
    size_t length = 0;
    if (napi_get_value_string_utf8(env, value, nullptr, 0, &length) != napi_ok) return {};
    std::string result(length + 1, '\0');
    if (napi_get_value_string_utf8(env, value, result.data(), result.size(), &length) != napi_ok) return {};
    result.resize(length);
    return result;
}
napi_value Undefined(napi_env env) {
    napi_value value = nullptr;
    napi_get_undefined(env, &value);
    return value;
}

// The runtime owns its worker thread. Surface callbacks never execute the guest
// on the ArkUI thread, and destruction stops it before the window disappears.
void TryStartLocked() {
    if (resourceImportBlocked || !startRequested || startDispatched || !nativeWindow || filesDirectory.empty() ||
        surfaceWidth == 0 || surfaceHeight == 0) return;
    // The game ability requests landscape asynchronously. Never freeze guest
    // render dimensions from a transitional portrait XComponent surface.
    if (surfaceWidth <= surfaceHeight) return;
    if (!aspectFitApplied) {
        // This window-wide policy survives size changes and applies to future
        // buffers. Set it before handing the window to the GPU thread: the
        // public NativeWindow API is not thread-safe, so OnChanged must not
        // reapply it concurrently with the runtime's native window operations.
        aspectFitResult = OH_NativeWindow_NativeWindowSetScalingModeV2(
            static_cast<OHNativeWindow *>(nativeWindow), OH_SCALING_MODE_SCALE_FIT_V2);
        aspectFitApplied = aspectFitResult == 0;
        OH_LOG_Print(LOG_APP, aspectFitApplied ? LOG_INFO : LOG_ERROR, kDomain, kTag,
            "SURFACE_ASPECT_FIT mode=%{public}d result=%{public}d window=%{public}p",
            static_cast<int32_t>(OH_SCALING_MODE_SCALE_FIT_V2), aspectFitResult, nativeWindow);
        if (!aspectFitApplied) {
            surfaceError = "Cannot apply aspect-preserving native surface scaling: " +
                std::to_string(aspectFitResult);
            return;
        }
    }
#if MG_HAS_GAME_RUNTIME
    startResult = mg_game_start(nativeWindow, surfaceWidth, surfaceHeight, filesDirectory.c_str());
    startDispatched = startResult == 0;
    gameStartedOnce = gameStartedOnce || startDispatched;
    surfaceError = startResult == 0 ? "" : "Game runtime rejected the start request.";
#else
    startResult = -2;
    surfaceError = "This build does not contain the game runtime.";
#endif
    OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag,
        "GAME_START dispatch=%{public}d result=%{public}d window=%{public}p width=%{public}u height=%{public}u files=%{public}s",
        startDispatched, startResult, nativeWindow, surfaceWidth, surfaceHeight, filesDirectory.c_str());
}
void OnCreated(OH_NativeXComponent *component, void *window) {
    uint64_t width = 0, height = 0;
    int32_t result = OH_NativeXComponent_GetXComponentSize(component, window, &width, &height);
    std::lock_guard<std::mutex> lock(stateMutex);
    nativeWindow = window;
    aspectFitApplied = false;
    aspectFitResult = -1;
    surfaceWidth = static_cast<uint32_t>(width);
    surfaceHeight = static_cast<uint32_t>(height);
    surfaceError = result == OH_NATIVEXCOMPONENT_RESULT_SUCCESS ? "" : "Cannot read native surface dimensions.";
    OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag, "SURFACE_CREATED window=%{public}p width=%{public}u height=%{public}u", window, surfaceWidth, surfaceHeight);
    TryStartLocked();
}
void OnChanged(OH_NativeXComponent *component, void *window) {
    uint64_t width = 0, height = 0;
    OH_NativeXComponent_GetXComponentSize(component, window, &width, &height);
    std::lock_guard<std::mutex> lock(stateMutex);
    if (nativeWindow != window) {
        aspectFitApplied = false;
        aspectFitResult = -1;
    }
    nativeWindow = window;
    surfaceWidth = static_cast<uint32_t>(width);
    surfaceHeight = static_cast<uint32_t>(height);
#if MG_HAS_GAME_RUNTIME
    if (startDispatched) mg_game_resize(surfaceWidth, surfaceHeight);
#endif
    TryStartLocked();
}
void OnDestroyed(OH_NativeXComponent *, void *window) {
    std::lock_guard<std::mutex> lock(stateMutex);
#if MG_HAS_GAME_RUNTIME
    if (startDispatched) mg_game_stop();
#endif
    nativeWindow = nullptr;
    aspectFitApplied = false;
    aspectFitResult = -1;
    surfaceWidth = surfaceHeight = 0;
    startDispatched = false;
    OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag, "SURFACE_DESTROYED window=%{public}p", window);
}
void OnTouch(OH_NativeXComponent *component, void *window) {
#if MG_HAS_GAME_RUNTIME
    OH_NativeXComponent_TouchEvent event{};
    if (OH_NativeXComponent_GetTouchEvent(component, window, &event) == OH_NATIVEXCOMPONENT_RESULT_SUCCESS) {
        OH_NativeXComponent_EventSourceType source = OH_NATIVEXCOMPONENT_SOURCE_TYPE_UNKNOWN;
        if (OH_NativeXComponent_GetTouchEventSourceType(component, event.id, &source) == OH_NATIVEXCOMPONENT_RESULT_SUCCESS &&
            (source == OH_NATIVEXCOMPONENT_SOURCE_TYPE_MOUSE || source == OH_NATIVEXCOMPONENT_SOURCE_TYPE_TOUCHPAD)) {
            // These devices have their own raw pointer/axis path; do not also
            // translate compatibility touch events into a second camera delta.
            return;
        }
        mg_game_touch(static_cast<uint32_t>(event.type), event.x, event.y, static_cast<uint32_t>(event.id));
    }
#endif
}
OH_NativeXComponent_Callback callbacks{OnCreated, OnChanged, OnDestroyed, OnTouch};

napi_value Configure(napi_env env, napi_callback_info info) {
    size_t count = 3;
    napi_value args[3]{};
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
    std::lock_guard<std::mutex> lock(stateMutex);
    if (resourceImportBlocked) {
        napi_throw_error(env, nullptr, "请等待游戏资源导入完成后再开始游戏。");
        return nullptr;
    }
    if (count < 1 || gameStartedOnce) {
        napi_throw_error(env, nullptr, "请重新打开应用后选择游戏模式。");
        return nullptr;
    }
    const auto directory = ReadString(env, args[0]);
    const auto action = count > 1 ? ReadString(env, args[1]) : "demo";
    const auto saveId = count > 2 ? ReadString(env, args[2]) : "";
    if (directory.empty() || directory.front() != '/' || directory.find('\0') != std::string::npos ||
        (action != "new" && action != "load" && action != "demo")) {
        napi_throw_error(env, nullptr, "游戏启动参数无效。");
        return nullptr;
    }
    if (action == "load") {
        const auto saves = game_saves::List(directory);
        bool found = false;
        for (const auto& save : saves.saves) if (save.id == saveId && save.mode == 1 && !save.backup) found = true;
        if (!saves.ok || !found) {
            napi_throw_error(env, nullptr, "所选存档已不存在或不可读取，请刷新存档列表。");
            return nullptr;
        }
    }
#if MG_HAS_GAME_RUNTIME
    if (mg_game_set_launch(action.c_str(), saveId.c_str()) != 0) {
        napi_throw_error(env, nullptr, "无法使用所选游戏模式或存档，请重新打开应用后重试。");
        return nullptr;
    }
#endif
    filesDirectory = directory;
    startRequested = true;
    TryStartLocked();
    return Undefined(env);
}
napi_value Start(napi_env env, napi_callback_info) {
    std::lock_guard<std::mutex> lock(stateMutex);
    if (resourceImportBlocked) {
        napi_throw_error(env, nullptr, "请等待游戏资源导入完成后再开始游戏。");
        return nullptr;
    }
    startRequested = true;
    TryStartLocked();
    return Undefined(env);
}
napi_value Stop(napi_env env, napi_callback_info) {
    std::lock_guard<std::mutex> lock(stateMutex);
#if MG_HAS_GAME_RUNTIME
    if (startDispatched) mg_game_stop();
#endif
    startDispatched = false;
    startRequested = false;
    return Undefined(env);
}
napi_value Status(napi_env env, napi_callback_info) {
    std::lock_guard<std::mutex> lock(stateMutex);
    napi_value value = nullptr;
    napi_create_object(env, &value);
    SetBool(env, value, "runtimeLinked", MG_HAS_GAME_RUNTIME == 1);
    SetBool(env, value, "surfaceReady", nativeWindow != nullptr && surfaceWidth > 0 && surfaceHeight > 0);
    SetInt(env, value, "width", static_cast<int32_t>(surfaceWidth));
    SetInt(env, value, "height", static_cast<int32_t>(surfaceHeight));
    SetBool(env, value, "startRequested", startRequested);
    SetBool(env, value, "startDispatched", startDispatched);
    SetInt(env, value, "startResult", startResult);
    SetBool(env, value, "aspectFitApplied", aspectFitApplied);
    SetInt(env, value, "aspectFitResult", aspectFitResult);
    SetInt(env, value, "nativeScalingMode", static_cast<int32_t>(OH_SCALING_MODE_SCALE_FIT_V2));
    SetString(env, value, "error", surfaceError);
    SetString(env, value, "filesDirectory", filesDirectory);
#if MG_HAS_GAME_RUNTIME
    const char *json = mg_game_status_json();
    SetString(env, value, "runtimeJson", json ? json : "{}");
#else
    SetString(env, value, "runtimeJson", "{\"status\":\"not_linked\"}");
#endif
    return value;
}
napi_value Key(napi_env env, napi_callback_info info) {
    size_t count = 2;
    napi_value args[2]{};
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
#if MG_HAS_GAME_RUNTIME
    if (count == 2) {
        bool pressed = false;
        napi_get_value_bool(env, args[1], &pressed);
        std::string key = ReadString(env, args[0]);
        mg_game_key_name(key.c_str(), pressed);
    }
#endif
    return Undefined(env);
}
napi_value KeyCode(napi_env env, napi_callback_info info) {
    size_t count = 2;
    napi_value args[2]{};
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
#if MG_HAS_GAME_RUNTIME
    if (count == 2) {
        uint32_t code = 0;
        bool pressed = false;
        napi_get_value_uint32(env, args[0], &code);
        napi_get_value_bool(env, args[1], &pressed);
        mg_game_key(code, pressed);
    }
#endif
    return Undefined(env);
}
napi_value MouseButton(napi_env env, napi_callback_info info) {
    size_t count = 2;
    napi_value args[2]{};
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
#if MG_HAS_GAME_RUNTIME
    if (count == 2) {
        uint32_t button = 0;
        bool pressed = false;
        napi_get_value_uint32(env, args[0], &button);
        napi_get_value_bool(env, args[1], &pressed);
        mg_game_mouse_button(button, pressed);
    }
#endif
    return Undefined(env);
}
napi_value KeyEvent(napi_env env, napi_callback_info info) {
    size_t count = 4;
    napi_value args[4]{};
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
    bool handled = false;
#if MG_HAS_GAME_RUNTIME
    uint32_t code = 0, unicode = 0;
    bool pressed = false, suppressText = false;
    if (count == 4 && napi_get_value_uint32(env, args[0], &code) == napi_ok &&
        napi_get_value_bool(env, args[1], &pressed) == napi_ok &&
        napi_get_value_uint32(env, args[2], &unicode) == napi_ok &&
        napi_get_value_bool(env, args[3], &suppressText) == napi_ok) {
        handled = mg_game_key_event(code, pressed, unicode, suppressText);
    }
#endif
    napi_value result = nullptr;
    napi_get_boolean(env, handled, &result);
    return result;
}
napi_value InputFocus(napi_env env, napi_callback_info info) {
    size_t count = 1;
    napi_value args[1]{};
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
#if MG_HAS_GAME_RUNTIME
    bool focused = false;
    if (count == 1 && napi_get_value_bool(env, args[0], &focused) == napi_ok) mg_game_focus(focused);
#endif
    return Undefined(env);
}
napi_value Pointer(napi_env env, napi_callback_info info) {
    size_t count = 5;
    napi_value args[5]{};
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
#if MG_HAS_GAME_RUNTIME
    double x = 0, y = 0, dx = 0, dy = 0;
    bool look = false;
    if (count == 5 && napi_get_value_double(env, args[0], &x) == napi_ok &&
        napi_get_value_double(env, args[1], &y) == napi_ok &&
        napi_get_value_double(env, args[2], &dx) == napi_ok &&
        napi_get_value_double(env, args[3], &dy) == napi_ok &&
        napi_get_value_bool(env, args[4], &look) == napi_ok) {
        mg_game_pointer(x, y, dx, dy, look);
    }
#endif
    return Undefined(env);
}
napi_value Wheel(napi_env env, napi_callback_info info) {
    size_t count = 1;
    napi_value args[1]{};
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
#if MG_HAS_GAME_RUNTIME
    double notches = 0;
    if (count == 1 && napi_get_value_double(env, args[0], &notches) == napi_ok) mg_game_wheel(notches);
#endif
    return Undefined(env);
}
}

void RegisterGameSurface(napi_env env, napi_value exports) {
    bool hasComponent = false;
    if (napi_has_named_property(env, exports, OH_NATIVE_XCOMPONENT_OBJ, &hasComponent) != napi_ok || !hasComponent) return;
    napi_value value = nullptr;
    napi_get_named_property(env, exports, OH_NATIVE_XCOMPONENT_OBJ, &value);
    OH_NativeXComponent *component = nullptr;
    if (napi_unwrap(env, value, reinterpret_cast<void **>(&component)) == napi_ok && component) {
        const int32_t result = OH_NativeXComponent_RegisterCallback(component, &callbacks);
        OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag, "XCOMPONENT_REGISTER result=%{public}d", result);
    }
}
void ExportGameMethods(napi_env env, napi_value exports) {
    SetGameResourceImportGate(ResourceImportGate);
    SetGameSaveImportGate(ResourceImportGate);
    napi_property_descriptor methods[] = {
        {"configureGame", nullptr, Configure, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"startGame", nullptr, Start, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"stopGame", nullptr, Stop, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"gameStatus", nullptr, Status, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"sendKey", nullptr, Key, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"sendKeyCode", nullptr, KeyCode, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"sendMouseButton", nullptr, MouseButton, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"sendKeyEvent", nullptr, KeyEvent, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"setInputFocus", nullptr, InputFocus, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"sendPointer", nullptr, Pointer, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"sendWheel", nullptr, Wheel, nullptr, nullptr, nullptr, napi_default, nullptr},
    };
    napi_define_properties(env, exports, sizeof(methods) / sizeof(methods[0]), methods);
}
