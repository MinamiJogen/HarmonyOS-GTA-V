#include <napi/native_api.h>
#include <hilog/log.h>
#include <cstdint>
#include <string>
#include <fstream>
#include <algorithm>
#include <vector>
#include <sstream>
#include "game_surface.h"
#include "game_resource_import.h"
#include "game_save_import.h"

#if RP_HAS_VULKAN_PROBE
#include "muguet_vulkan_probe.h"
#endif

#if RP_HAS_RUST
#include "harmony_runtime_probe.h"
#endif
#if RP_HAS_EMBEDDED_AOT
extern "C" {
extern const uint8_t rp_embedded_aot_start[];
extern const uint8_t rp_embedded_aot_end[];
}
#endif

#if RP_HAS_MEMORY64_AOT
extern "C" {
extern const uint8_t rp_memory64_aot_start[];
extern const uint8_t rp_memory64_aot_end[];
}
#endif
#if RP_HAS_SHARED_AOT
extern "C" {
extern const uint8_t rp_shared_aot_start[];
extern const uint8_t rp_shared_aot_end[];
}
#endif
#if RP_HAS_SHARED_MEMORY64_AOT
extern "C" {
extern const uint8_t rp_shared_memory64_aot_start[];
extern const uint8_t rp_shared_memory64_aot_end[];
}
#endif

namespace {
constexpr unsigned int kDomain = 0x4100;
constexpr const char *kTag = "HarmonyRuntimeProbe";

void SetBoolean(napi_env env, napi_value object, const char *name, bool value) {
    napi_value result = nullptr;
    napi_get_boolean(env, value, &result);
    napi_set_named_property(env, object, name, result);
}
void SetInt(napi_env env, napi_value object, const char *name, int32_t value) {
    napi_value result = nullptr;
    napi_create_int32(env, value, &result);
    napi_set_named_property(env, object, name, result);
}
void SetString(napi_env env, napi_value object, const char *name, const std::string &value) {
    napi_value result = nullptr;
    napi_create_string_utf8(env, value.c_str(), value.size(), &result);
    napi_set_named_property(env, object, name, result);
}

struct EmbeddedCase {
    std::string name;
    bool available = false;
    bool attempted = false;
    bool passed = false;
    int32_t status = -1;
    int32_t result = 0;
    std::string error;
    std::string address;
    size_t length = 0;
};
EmbeddedCase RunEmbedded(const char *name, const uint8_t *first, const uint8_t *last, bool runtimeAvailable) {
    EmbeddedCase result;
    result.name = name;
    result.available = first != nullptr && last != nullptr;
    if (!result.available) {
        result.error = "Trusted AOT object is not linked in this build.";
        return result;
    }
    const uintptr_t start = reinterpret_cast<uintptr_t>(first);
    const uintptr_t end = reinterpret_cast<uintptr_t>(last);
    std::ostringstream address;
    address << "0x" << std::hex << start;
    result.address = address.str();
    if (end <= start) {
        result.status = 1;
        result.error = "Embedded AOT symbol range is invalid.";
        return result;
    }
    result.length = static_cast<size_t>(end - start);
    OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag,
        "AOT_MAPPING case=%{public}s start=%{public}p end=%{public}p length=%{public}zu",
        name, static_cast<const void *>(first), static_cast<const void *>(last), result.length);
    if (!runtimeAvailable) {
        result.error = "Wasmtime support is not compiled; embedded bytes were not executed.";
        return result;
    }
#if RP_HAS_RUST
    result.attempted = true;
    result.status = rp_run_raw_aot_add(first, result.length, 20, 22, &result.result);
    const char *lastError = rp_last_error();
    if (lastError != nullptr) {
        result.error = lastError;
    }
    result.passed = result.status == 0 && result.result == 42;
#endif
    OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag,
        "AOT_CASE name=%{public}s available=%{public}d attempted=%{public}d passed=%{public}d status=%{public}d result=%{public}d error=%{public}s",
        name, result.available, result.attempted, result.passed, result.status, result.result, result.error.c_str());
    return result;
}

napi_value RunProbe(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value args[2] = {nullptr, nullptr};
    napi_get_cb_info(env, info, &argc, args, nullptr, nullptr);
    std::string path;
    if (argc >= 1) {
        size_t length = 0;
        if (napi_get_value_string_utf8(env, args[0], nullptr, 0, &length) == napi_ok) {
            std::string storage(length + 1, '\0');
            napi_get_value_string_utf8(env, args[0], storage.data(), storage.size(), &length);
            path.assign(storage.data(), length);
        }
    }
    std::string reportPath;
    if (argc >= 2) {
        size_t length = 0;
        if (napi_get_value_string_utf8(env, args[1], nullptr, 0, &length) == napi_ok) {
            std::string storage(length + 1, '\0');
            napi_get_value_string_utf8(env, args[1], storage.data(), storage.size(), &length);
            reportPath.assign(storage.data(), length);
        }
    }
    std::string vulkanJson = "{\"status\":\"not_linked\",\"physical_device_count\":0,\"physical_devices\":[],\"errors\":[]}";
    bool vulkanReportSaved = false;
#if RP_HAS_VULKAN_PROBE
    const char *rawVulkan = muguet_vulkan_probe_json();
    if (rawVulkan != nullptr) {
        vulkanJson = rawVulkan;
    }
    constexpr size_t kChunkBytes = 1800;
    const size_t chunks = (vulkanJson.size() + kChunkBytes - 1) / kChunkBytes;
    for (size_t index = 0; index < chunks; ++index) {
        const std::string chunk = vulkanJson.substr(index * kChunkBytes, kChunkBytes);
        OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag,
            "VULKAN_JSON[%{public}zu/%{public}zu] %{public}s", index + 1, chunks, chunk.c_str());
    }
    if (!reportPath.empty()) {
        std::ofstream report(reportPath, std::ios::binary | std::ios::trunc);
        if (report) {
            report.write(vulkanJson.data(), static_cast<std::streamsize>(vulkanJson.size()));
            report.close();
            vulkanReportSaved = report.good();
        }
    }
#endif
    bool rustLinked = false;
    bool runtimeAvailable = false;
    bool aotAttempted = false;
    bool aotPassed = false;
    int32_t rustResult = 0;
    int32_t aotResult = 0;
    int32_t aotStatus = -1;
    bool embeddedAvailable = false;
    bool embeddedAttempted = false;
    bool embeddedPassed = false;
    int32_t embeddedStatus = -1;
    int32_t embeddedResult = 0;
    std::string embeddedError;
    std::string rustVersion = "not linked";
    std::string error = "Rust static library is not linked in this build.";
#if RP_HAS_RUST
    rustLinked = true;
    const char *version = rp_version();
    rustVersion = version == nullptr ? "unknown" : version;
    rustResult = rp_add(20, 22);
    runtimeAvailable = rp_runtime_available() == 1;
    error.clear();
    if (!path.empty()) {
        aotAttempted = true;
        aotStatus = rp_run_aot_add(path.c_str(), 20, 22, &aotResult);
        const char *lastError = rp_last_error();
        if (lastError != nullptr) {
            error = lastError;
        }
        aotPassed = aotStatus == 0 && aotResult == 42;
    } else {
        error = runtimeAvailable ? "AOT artifact path is empty; no module was executed."
                                 : "Wasmtime support is not compiled into the Rust library.";
    }
#endif
    std::vector<EmbeddedCase> embeddedCases;
#if RP_HAS_EMBEDDED_AOT
    embeddedCases.push_back(RunEmbedded("add", rp_embedded_aot_start, rp_embedded_aot_end, runtimeAvailable));
#else
    embeddedCases.push_back(RunEmbedded("add", nullptr, nullptr, runtimeAvailable));
#endif
#if RP_HAS_MEMORY64_AOT
    embeddedCases.push_back(RunEmbedded("memory64", rp_memory64_aot_start, rp_memory64_aot_end, runtimeAvailable));
#else
    embeddedCases.push_back(RunEmbedded("memory64", nullptr, nullptr, runtimeAvailable));
#endif
#if RP_HAS_SHARED_AOT
    embeddedCases.push_back(RunEmbedded("shared", rp_shared_aot_start, rp_shared_aot_end, runtimeAvailable));
#else
    embeddedCases.push_back(RunEmbedded("shared", nullptr, nullptr, runtimeAvailable));
#endif
#if RP_HAS_SHARED_MEMORY64_AOT
    embeddedCases.push_back(RunEmbedded("shared-memory64", rp_shared_memory64_aot_start, rp_shared_memory64_aot_end, runtimeAvailable));
#else
    embeddedCases.push_back(RunEmbedded("shared-memory64", nullptr, nullptr, runtimeAvailable));
#endif
    embeddedAvailable = embeddedCases[0].available;
    embeddedAttempted = embeddedCases[0].attempted;
    embeddedPassed = embeddedCases[0].passed;
    embeddedStatus = embeddedCases[0].status;
    embeddedResult = embeddedCases[0].result;
    embeddedError = embeddedCases[0].error;
    const char *abi =
#if defined(__aarch64__)
        "arm64-v8a / aarch64-linux-ohos";
#else
        "non-aarch64";
#endif
    napi_value result = nullptr;
    napi_create_object(env, &result);
    SetString(env, result, "vulkanJson", vulkanJson);
    SetString(env, result, "vulkanReportPath", vulkanReportSaved ? reportPath : "");
    SetBoolean(env, result, "vulkanReportSaved", vulkanReportSaved);
    SetBoolean(env, result, "cppReady", true);
    SetString(env, result, "abi", abi);
    SetBoolean(env, result, "rustLinked", rustLinked);
    SetString(env, result, "rustVersion", rustVersion);
    SetInt(env, result, "rustResult", rustResult);
    SetBoolean(env, result, "runtimeAvailable", runtimeAvailable);
    SetBoolean(env, result, "aotAttempted", aotAttempted);
    SetBoolean(env, result, "aotPassed", aotPassed);
    SetInt(env, result, "aotResult", aotResult);
    SetInt(env, result, "aotStatus", aotStatus);
    SetString(env, result, "error", error);
    SetBoolean(env, result, "embeddedAvailable", embeddedAvailable);
    SetBoolean(env, result, "embeddedAttempted", embeddedAttempted);
    SetBoolean(env, result, "embeddedPassed", embeddedPassed);
    SetInt(env, result, "embeddedStatus", embeddedStatus);
    SetInt(env, result, "embeddedResult", embeddedResult);
    SetString(env, result, "embeddedError", embeddedError);
    napi_value cases = nullptr;
    napi_create_array_with_length(env, embeddedCases.size(), &cases);
    for (size_t index = 0; index < embeddedCases.size(); ++index) {
        const EmbeddedCase &item = embeddedCases[index];
        napi_value object = nullptr;
        napi_create_object(env, &object);
        SetString(env, object, "name", item.name);
        SetBoolean(env, object, "available", item.available);
        SetBoolean(env, object, "attempted", item.attempted);
        SetBoolean(env, object, "passed", item.passed);
        SetInt(env, object, "status", item.status);
        SetInt(env, object, "result", item.result);
        SetString(env, object, "error", item.error);
        SetString(env, object, "address", item.address);
        SetInt(env, object, "length", static_cast<int32_t>(item.length));
        napi_set_element(env, cases, static_cast<uint32_t>(index), object);
    }
    napi_set_named_property(env, result, "embeddedCases", cases);
    OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag,
        "embeddedAvailable=%{public}d embeddedAttempted=%{public}d embeddedPassed=%{public}d embeddedStatus=%{public}d embeddedResult=%{public}d error=%{public}s",
        embeddedAvailable, embeddedAttempted, embeddedPassed, embeddedStatus, embeddedResult, embeddedError.c_str());
    OH_LOG_Print(LOG_APP, LOG_INFO, kDomain, kTag,
        "cpp=1 abi=%{public}s rustLinked=%{public}d rustResult=%{public}d runtimeAvailable=%{public}d aotAttempted=%{public}d aotPassed=%{public}d aotStatus=%{public}d aotResult=%{public}d error=%{public}s",
        abi, rustLinked, rustResult, runtimeAvailable, aotAttempted, aotPassed, aotStatus, aotResult, error.c_str());
    return result;
}

napi_value Init(napi_env env, napi_value exports) {
    napi_property_descriptor descriptors[] = {
        {"runProbe", nullptr, RunProbe, nullptr, nullptr, nullptr, napi_default, nullptr}
    };
    napi_define_properties(env, exports, 1, descriptors);
    ExportGameMethods(env, exports);
    ExportGameResourceImport(env, exports);
    ExportGameSaveImport(env, exports);
    ExportVulkanBufferDiagnostics(env, exports);
    RegisterGameSurface(env, exports);
    return exports;
}
}

static napi_module module = {1, 0, nullptr, Init, "harmony_probe", nullptr, {0}};
extern "C" __attribute__((constructor)) void RegisterHarmonyProbe(void) {
    napi_module_register(&module);
}
