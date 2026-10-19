#include "graphics_accel_diagnostics.h"
#include <xengine/xeg_vulkan_extension.h>
#include <graphics_game_sdk/frame_generation_base.h>
#include <syscap_ndk.h>
#include <vulkan/vulkan.h>
#include <dlfcn.h>
#include <algorithm>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
std::string quote(const std::string& text) {
    std::ostringstream out; out << '"';
    for (unsigned char c : text) {
        if (c == '"' || c == '\\') out << '\\' << char(c);
        else if (c < 32) out << "\\u" << std::hex << std::setw(4) << std::setfill('0') << unsigned(c) << std::dec;
        else out << char(c);
    }
    out << '"'; return out.str();
}
const char* boolean(bool value) { return value ? "true" : "false"; }
void check(VkResult result, const char* call) {
    if (result != VK_SUCCESS) throw std::runtime_error(std::string(call) + " returned VkResult " + std::to_string(result));
}
struct Library {
    void* handle = nullptr;
    std::string error;
    explicit Library(const char* name) {
        dlerror(); handle = dlopen(name, RTLD_NOW | RTLD_LOCAL);
        if (!handle) { const char* detail = dlerror(); error = detail ? detail : "dlopen failed"; }
    }
    ~Library() { if (handle) dlclose(handle); }
    template<class T> T symbol(const char* name) const { return handle ? reinterpret_cast<T>(dlsym(handle, name)) : nullptr; }
};
struct Context {
    VkInstance instance = VK_NULL_HANDLE;
    VkPhysicalDevice physical = VK_NULL_HANDLE;
    VkPhysicalDeviceProperties properties{};
    ~Context() { if (instance) vkDestroyInstance(instance, nullptr); }
    void create() {
        VkApplicationInfo app{}; app.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO;
        app.pApplicationName = "Muguet public graphics capability probe";
        app.apiVersion = VK_API_VERSION_1_1;
        VkInstanceCreateInfo info{}; info.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO; info.pApplicationInfo = &app;
        check(vkCreateInstance(&info, nullptr, &instance), "vkCreateInstance");
        uint32_t count = 0; check(vkEnumeratePhysicalDevices(instance, &count, nullptr), "vkEnumeratePhysicalDevices count");
        if (count == 0 || count > 256) throw std::runtime_error("No reasonable Vulkan physical device list");
        std::vector<VkPhysicalDevice> devices(count);
        check(vkEnumeratePhysicalDevices(instance, &count, devices.data()), "vkEnumeratePhysicalDevices");
        physical = devices.front(); vkGetPhysicalDeviceProperties(physical, &properties);
    }
};
struct Probe {
    std::string directory, phase = "created";
    explicit Probe(const char* path) : directory(path ? path : "") {}
    void trace(const std::string& next, const std::string& extra = "") {
        phase = next;
        if (!directory.empty()) {
            std::ofstream out(directory + "/graphics-accel-probe-trace.jsonl", std::ios::app);
            out << "{\"phase\":" << quote(next);
            if (!extra.empty()) out << ',' << extra;
            out << "}\n"; out.flush();
        }
    }
    std::string run() {
        trace("syscap_query");
        const bool xengineSyscap = canIUse("SystemCapability.Graphic.XEngine");
        const bool fgSyscap = canIUse("SystemCapability.GraphicsGame.RenderAccelerate");
        trace("vulkan_instance"); Context vk; vk.create();
        trace("public_library_load"); Library xengine("libxengine.so"), fg("libframegeneration.so");
        auto enumerate = xengine.symbol<PFN_HMS_XEG_EnumerateDeviceExtensionProperties>("HMS_XEG_EnumerateDeviceExtensionProperties");
        using IsFgSupported = bool (*)(FG_FeatureType);
        auto fgSupported = fg.symbol<IsFgSupported>("HMS_FG_IsFrameGenerationSupported");
        std::vector<XEG_ExtensionProperties> extensions;
        std::string extensionError;
        bool enumerationComplete = false;
        if (xengineSyscap && enumerate) {
            trace("xengine_extensions");
            for (unsigned attempt = 0; attempt < 4; ++attempt) {
                uint32_t count = 0; VkResult result = enumerate(vk.physical, &count, nullptr);
                if (result != VK_SUCCESS && result != VK_INCOMPLETE) { extensionError = "count query VkResult " + std::to_string(result); break; }
                if (count > 4096) { extensionError = "Unreasonable XEngine extension count"; break; }
                extensions.resize(count); uint32_t capacity = count;
                result = enumerate(vk.physical, &count, extensions.empty() ? nullptr : extensions.data());
                if (count > capacity) { extensionError = "Extension count exceeded capacity"; break; }
                if (result == VK_INCOMPLETE) continue;
                if (result != VK_SUCCESS) { extensionError = "list query VkResult " + std::to_string(result); break; }
                extensions.resize(count); enumerationComplete = true; break;
            }
            if (!enumerationComplete && extensionError.empty()) extensionError = "Extension enumeration remained incomplete";
        } else extensionError = !xengineSyscap ? "SystemCapability.Graphic.XEngine is absent" : "Public query symbol unavailable";
        const auto supported = [&](const char* name) {
            return enumerationComplete && std::any_of(extensions.begin(), extensions.end(), [&](const auto& e) {
                return strncmp(e.extensionName, name, XEG_MAX_EXTENSION_NAME_SIZE) == 0;
            });
        };
        struct Feature { FG_FeatureType value; const char* name; } features[] = {
            {INTERPOLATION_GPU_GLES, "INTERPOLATION_GPU_GLES"},
            {INTERPOLATION_GPU_VULKAN, "INTERPOLATION_GPU_VULKAN"},
            {EXTRAPOLATION_GPU_GLES, "EXTRAPOLATION_GPU_GLES"},
            {EXTRAPOLATION_GPU_VULKAN, "EXTRAPOLATION_GPU_VULKAN"},
            {INTERPOLATION_AI_VULKAN, "INTERPOLATION_AI_VULKAN"}
        };
        trace("frame_generation_features");
        std::ostringstream out;
        out << "{\"status\":\"pass\",\"scope\":\"public_api_capability_query_only\",\"gameRuntimeModified\":false,\"renderFramesGenerated\":0"
            << ",\"syscap\":{\"XEngine\":" << boolean(xengineSyscap) << ",\"RenderAccelerate\":" << boolean(fgSyscap) << '}';
        out << ",\"vulkan\":{\"deviceName\":" << quote(vk.properties.deviceName)
            << ",\"apiVersion\":" << vk.properties.apiVersion << ",\"driverVersion\":" << vk.properties.driverVersion
            << ",\"vendorId\":" << vk.properties.vendorID << ",\"deviceId\":" << vk.properties.deviceID << '}';
        out << ",\"xengine\":{\"libraryLoaded\":" << boolean(xengine.handle) << ",\"libraryError\":" << quote(xengine.error)
            << ",\"enumerateSymbol\":" << boolean(enumerate) << ",\"enumerationComplete\":" << boolean(enumerationComplete)
            << ",\"enumerationError\":" << quote(extensionError) << ",\"extensions\":[";
        for (size_t i = 0; i < extensions.size(); ++i) {
            if (i) out << ',';
            const size_t len = strnlen(extensions[i].extensionName, XEG_MAX_EXTENSION_NAME_SIZE);
            out << "{\"name\":" << quote(std::string(extensions[i].extensionName, len)) << ",\"version\":" << extensions[i].version << '}';
        }
        out << "],\"spatialUpscale\":" << boolean(supported(XEG_SPATIAL_UPSCALE_EXTENSION_NAME))
            << ",\"neuralUpscale\":" << boolean(supported(XEG_NEURAL_UPSCALE_EXTENSION_NAME))
            << ",\"neuralUpscale2\":" << boolean(supported(XEG_NEURAL_UPSCALE2_EXTENSION_NAME))
            << ",\"temporalUpscale\":" << boolean(supported(XEG_TEMPORAL_UPSCALE_EXTENSION_NAME))
            << ",\"adaptiveVrs\":" << boolean(supported(XEG_ADAPTIVE_VRS_EXTENSION_NAME)) << '}';
        out << ",\"frameGeneration\":{\"libraryLoaded\":" << boolean(fg.handle) << ",\"libraryError\":" << quote(fg.error)
            << ",\"querySymbol\":" << boolean(fgSupported) << ",\"features\":[";
        for (size_t i = 0; i < std::size(features); ++i) {
            if (i) out << ',';
            out << "{\"name\":" << quote(features[i].name) << ",\"enumValue\":" << int(features[i].value) << ",\"supported\":";
            if (fgSyscap && fgSupported) out << boolean(fgSupported(features[i].value)); else out << "null";
            out << '}';
        }
        out << "]},\"offscreenSpatialUpscale\":{\"executed\":false,\"reason\":\"Capability-only first-stage probe; no upscaling or generated frames are claimed\"}"
            << ",\"productionEnabled\":false}";
        trace("complete"); return out.str();
    }
};
}

extern "C" const char* muguet_graphics_accel_probe_json(const char* filesDirectory) {
    thread_local std::string result;
    Probe probe(filesDirectory);
    try { result = probe.run(); }
    catch (const std::exception& e) {
        result = "{\"status\":\"fail\",\"phase\":" + quote(probe.phase) + ",\"error\":" + quote(e.what()) + ",\"productionEnabled\":false}";
        probe.trace("failed", "\"error\":" + quote(e.what()));
    }
    return result.c_str();
}
