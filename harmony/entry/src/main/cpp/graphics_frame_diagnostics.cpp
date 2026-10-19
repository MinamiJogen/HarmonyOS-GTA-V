#include "graphics_frame_diagnostics.h"
#include <graphics_game_sdk/frame_generation_vk.h>
#include <syscap_ndk.h>
#include <vulkan/vulkan.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <unistd.h>
#include <atomic>
#include <cerrno>
#include <cstring>
#include <iomanip>
#include <mutex>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
// AI generation is advertised separately from API/context availability. The
// public SDK defines system-presented frames as system-owned. Do not substitute
// FG_PRESENT_BY_GAME or fabricated depth just to obtain an offscreen output.
constexpr const char* kAiGuide = "https://developer.huawei.com/consumer/cn/doc/harmonyos-guides/graphics-accelerate-fg-ai-vulkan";
constexpr const char* kApiGuide = "https://developer.huawei.com/consumer/cn/doc/doccenter-references/api/frame__generation__vk_8h";
std::atomic_flag running = ATOMIC_FLAG_INIT;
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
struct Trace {
    std::string directory, phase = "created";
    std::string* lastPhase;
    unsigned events = 0;
    Trace(const char* root, std::string* phaseOutput) : directory(root ? root : ""), lastPhase(phaseOutput) {
        if (directory.empty()) throw std::runtime_error("filesDirectory is empty");
    }
    void step(const char* name, const std::string& fields = "") {
        phase = name;
        *lastPhase = phase;
        if (++events > 64) throw std::runtime_error("bounded frame probe trace limit exceeded");
        const std::string line = "{\"processId\":" + std::to_string(getpid()) +
            ",\"sequence\":" + std::to_string(events) + ",\"phase\":" + quote(name) +
            (fields.empty() ? "" : "," + fields) + "}\n";
        const std::string path = directory + "/graphics-frame-probe-trace.jsonl";
        int fd = open(path.c_str(), O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0600);
        if (fd < 0) throw std::runtime_error("cannot open frame probe trace");
        size_t offset = 0; bool ok = true;
        while (offset < line.size()) {
            const ssize_t n = write(fd, line.data() + offset, line.size() - offset);
            if (n < 0 && errno == EINTR) continue;
            if (n <= 0) { ok = false; break; }
            offset += static_cast<size_t>(n);
        }
        if (fsync(fd) != 0) ok = false;
        close(fd);
        if (!ok) throw std::runtime_error("cannot persist frame probe trace");
    }
};
struct Library {
    void* handle = nullptr;
    explicit Library(const char* name) {
        // Keep exactly one public-library mapping for this process. The actual
        // device run completed FG/Vulkan teardown, then hung before CABI return;
        // unloading calls vendor/plugin static destructors. No FG context or
        // Vulkan resource is retained by this loader-lifetime policy.
        static std::once_flag once;
        static void* processHandle = nullptr;
        std::call_once(once, [&] {
            processHandle = dlopen(name, RTLD_NOW | RTLD_LOCAL);
            if (!processHandle) {
                const char* error = dlerror();
                throw std::runtime_error(error ? error : "cannot open public frame generation library");
            }
        });
        handle = processHandle;
    }
    ~Library() = default; // OS reclaims the mapping at process termination.
    template<class T> T symbol(const char* name) {
        auto p = reinterpret_cast<T>(dlsym(handle, name));
        if (!p) throw std::runtime_error(std::string("public symbol unavailable: ") + name);
        return p;
    }
};
struct Api {
    decltype(&HMS_FG_IsFrameGenerationSupported) supported;
    decltype(&HMS_FG_CreateContext_VK) create;
    decltype(&HMS_FG_SetAlgorithmMode_VK) algorithm;
    decltype(&HMS_FG_SetResolution_VK) resolution;
    decltype(&HMS_FG_SetImageFormat_VK) format;
    decltype(&HMS_FG_SetIntegrationMode_VK) integration;
    decltype(&HMS_FG_Activate_VK) activate;
    decltype(&HMS_FG_IsActive_VK) isActive;
    decltype(&HMS_FG_Deactivate_VK) deactivate;
    decltype(&HMS_FG_DestroyContext_VK) destroy;
    explicit Api(Library& library)
        : supported(library.symbol<decltype(supported)>("HMS_FG_IsFrameGenerationSupported")),
          create(library.symbol<decltype(create)>("HMS_FG_CreateContext_VK")),
          algorithm(library.symbol<decltype(algorithm)>("HMS_FG_SetAlgorithmMode_VK")),
          resolution(library.symbol<decltype(resolution)>("HMS_FG_SetResolution_VK")),
          format(library.symbol<decltype(format)>("HMS_FG_SetImageFormat_VK")),
          integration(library.symbol<decltype(integration)>("HMS_FG_SetIntegrationMode_VK")),
          activate(library.symbol<decltype(activate)>("HMS_FG_Activate_VK")),
          isActive(library.symbol<decltype(isActive)>("HMS_FG_IsActive_VK")),
          deactivate(library.symbol<decltype(deactivate)>("HMS_FG_Deactivate_VK")),
          destroy(library.symbol<decltype(destroy)>("HMS_FG_DestroyContext_VK")) {}
};
void check(VkResult r, const char* name) {
    if (r != VK_SUCCESS) throw std::runtime_error(std::string(name) + " VkResult=" + std::to_string(r));
}
struct Vulkan {
    VkInstance instance = VK_NULL_HANDLE;
    VkPhysicalDevice physical = VK_NULL_HANDLE;
    VkDevice device = VK_NULL_HANDLE;
    VkPhysicalDeviceProperties properties{};
    uint32_t queueFamily = 0;
    bool shaderFloat16 = false, shaderInt16 = false, fragmentStoresAndAtomics = false;
    ~Vulkan() { if (device) vkDestroyDevice(device, nullptr); if (instance) vkDestroyInstance(instance, nullptr); }
    void create(Trace& trace) {
        uint32_t count = 0;
        check(vkEnumerateInstanceExtensionProperties(nullptr, &count, nullptr), "instance extensions count");
        if (count > 4096) throw std::runtime_error("unreasonable instance extension count");
        std::vector<VkExtensionProperties> instanceExtensions(count);
        if (count) check(vkEnumerateInstanceExtensionProperties(nullptr, &count, instanceExtensions.data()), "instance extensions list");
        const char* enabledInstanceExtensions[] = {VK_KHR_SURFACE_EXTENSION_NAME, "VK_OHOS_surface"};
        const auto hasExtension = [](const auto& list, const char* name) {
            for (const auto& ext : list) if (std::strncmp(ext.extensionName, name, VK_MAX_EXTENSION_NAME_SIZE) == 0) return true;
            return false;
        };
        for (const char* name : enabledInstanceExtensions) if (!hasExtension(instanceExtensions, name))
            throw std::runtime_error(std::string("official sample instance prerequisite absent: ") + name);
        VkApplicationInfo app{}; app.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO;
        app.pApplicationName = "Muguet public AI frame context diagnostic"; app.apiVersion = VK_API_VERSION_1_1;
        VkInstanceCreateInfo info{}; info.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO; info.pApplicationInfo = &app;
        info.enabledExtensionCount = 2; info.ppEnabledExtensionNames = enabledInstanceExtensions;
        check(vkCreateInstance(&info, nullptr, &instance), "vkCreateInstance");
        count = 0; check(vkEnumeratePhysicalDevices(instance, &count, nullptr), "physical count");
        if (count == 0 || count > 32) throw std::runtime_error("no bounded Vulkan device list");
        std::vector<VkPhysicalDevice> list(count);
        check(vkEnumeratePhysicalDevices(instance, &count, list.data()), "physical list"); physical = list.front();
        vkGetPhysicalDeviceProperties(physical, &properties);
        if (properties.apiVersion < VK_API_VERSION_1_1) throw std::runtime_error("Vulkan1.1 context prerequisite absent");
        count = 0; check(vkEnumerateDeviceExtensionProperties(physical, nullptr, &count, nullptr), "device extensions count");
        if (count > 4096) throw std::runtime_error("unreasonable device extension count");
        std::vector<VkExtensionProperties> deviceExtensions(count);
        if (count) check(vkEnumerateDeviceExtensionProperties(physical, nullptr, &count, deviceExtensions.data()), "device extensions list");
        const char* enabledDeviceExtensions[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME, VK_KHR_SHADER_FLOAT16_INT8_EXTENSION_NAME};
        for (const char* name : enabledDeviceExtensions) if (!hasExtension(deviceExtensions, name))
            throw std::runtime_error(std::string("official sample device prerequisite absent: ") + name);
        VkPhysicalDeviceShaderFloat16Int8Features float16{};
        float16.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SHADER_FLOAT16_INT8_FEATURES;
        VkPhysicalDeviceFeatures2 features{}; features.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2;
        features.pNext = &float16; vkGetPhysicalDeviceFeatures2(physical, &features);
        shaderFloat16 = float16.shaderFloat16 == VK_TRUE;
        shaderInt16 = features.features.shaderInt16 == VK_TRUE;
        fragmentStoresAndAtomics = features.features.fragmentStoresAndAtomics == VK_TRUE;
        trace.step("actual-sample-prerequisites", "\"shaderFloat16\":" + std::string(boolean(shaderFloat16)) +
            ",\"shaderInt16\":" + boolean(shaderInt16) + ",\"fragmentStoresAndAtomics\":" + boolean(fragmentStoresAndAtomics));
        if (!shaderFloat16 || !shaderInt16 || !fragmentStoresAndAtomics)
            throw std::runtime_error("one or more official sample shader feature prerequisites are false");
        count = 0; vkGetPhysicalDeviceQueueFamilyProperties(physical, &count, nullptr);
        if (count == 0 || count > 256) throw std::runtime_error("invalid queue family count");
        std::vector<VkQueueFamilyProperties> queues(count); vkGetPhysicalDeviceQueueFamilyProperties(physical, &count, queues.data());
        bool found = false;
        for (uint32_t i = 0; i < count; ++i) {
            if (queues[i].queueCount && (queues[i].queueFlags & (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT)) ==
                (VK_QUEUE_GRAPHICS_BIT | VK_QUEUE_COMPUTE_BIT)) { queueFamily = i; found = true; break; }
        }
        if (!found) throw std::runtime_error("no graphics/compute queue");
        float priority = 1.0f; VkDeviceQueueCreateInfo queue{}; queue.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO;
        queue.queueFamilyIndex = queueFamily; queue.queueCount = 1; queue.pQueuePriorities = &priority;
        // Exactly the shader-feature set in the official FG Vulkan sample,
        // after querying the true bits. The unrelated Int8 feature stays off.
        VkPhysicalDeviceFeatures enabled{};
        enabled.shaderInt16 = VK_TRUE; enabled.fragmentStoresAndAtomics = VK_TRUE;
        enabled.samplerAnisotropy = features.features.samplerAnisotropy;
        float16.pNext = nullptr; float16.shaderInt8 = VK_FALSE;
        VkDeviceCreateInfo dev{}; dev.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO;
        dev.queueCreateInfoCount = 1; dev.pQueueCreateInfos = &queue;
        dev.pEnabledFeatures = &enabled; dev.pNext = &float16;
        dev.enabledExtensionCount = 2; dev.ppEnabledExtensionNames = enabledDeviceExtensions;
        check(vkCreateDevice(physical, &dev, nullptr, &device), "vkCreateDevice");
    }
    void finish() {
        check(vkDeviceWaitIdle(device), "vkDeviceWaitIdle final");
        vkDestroyDevice(device, nullptr); device = VK_NULL_HANDLE;
        vkDestroyInstance(instance, nullptr); instance = VK_NULL_HANDLE;
    }
};
struct FrameContext {
    Api& api;
    FG_Context_VK* value = nullptr;
    bool active = false;
    explicit FrameContext(Api& functions) : api(functions) {}
    ~FrameContext() { if (value) { if (active) api.deactivate(value); api.destroy(&value); } }
    void finish(Trace& trace) {
        if (active) {
            trace.step("before-deactivate");
            const auto r = api.deactivate(value); trace.step("deactivate", "\"result\":" + std::to_string(r));
            if (r != FG_SUCCESS) throw std::runtime_error("frame context deactivation failed"); active = false;
        }
        trace.step("before-destroy-context");
        const auto r = api.destroy(&value); trace.step("destroy-context", "\"result\":" + std::to_string(r));
        if (r != FG_SUCCESS) throw std::runtime_error("frame context destruction failed");
        // Only the successful destroy return is contractual; do not require the
        // SDK to rewrite its caller's pointer beyond the documented API result.
        value = nullptr;
    }
};
std::string run(Trace& trace) {
    trace.step("public-capability");
    const bool syscap = canIUse("SystemCapability.GraphicsGame.RenderAccelerate");
    if (!syscap) return "{\"status\":\"unsupported\",\"reason\":\"RenderAccelerate syscap absent\",\"generatedFramesVerified\":0}";
    Library library("libframegeneration.so"); Api api(library);
    const bool ai = api.supported(INTERPOLATION_AI_VULKAN);
    trace.step("ai-capability", "\"supported\":" + std::string(boolean(ai)));
    if (!ai) return "{\"status\":\"unsupported\",\"reason\":\"INTERPOLATION_AI_VULKAN is false\",\"generatedFramesVerified\":0}";
    trace.step("before-vulkan-create"); Vulkan vk; vk.create(trace);
    trace.step("before-context-create"); FrameContext context(api);
    FG_ContextDescription_VK description{};
    description.vkInstance = vk.instance; description.vkPhysicalDevice = vk.physical; description.vkDevice = vk.device;
    description.framesInFlight = 1; description.fnVulkanLoaderFunction = vkGetInstanceProcAddr;
    context.value = api.create(&description);
    if (!context.value) throw std::runtime_error("public FG context creation returned null on sample-qualified Vulkan device");
    trace.step("context-created");
    const auto call = [&](const char* name, const auto& invoke) {
        const std::string before = std::string("before-") + name;
        trace.step(before.c_str());
        const auto r = invoke();
        trace.step(name, "\"result\":" + std::to_string(r));
        if (r != FG_SUCCESS) throw std::runtime_error(std::string(name) + " FG_ErrorCode=" + std::to_string(r));
    };
    FG_AlgorithmModeInfo mode{}; mode.predictionMode = FG_PREDICTION_MODE_INTERPOLATION; mode.meMode = FG_ME_MODE_ENHANCED;
    call("set-algorithm", [&] { return api.algorithm(context.value, &mode); });
    // Match the application's actual render size rather than inferring that a
    // very small synthetic image is inside the AI model's supported domain.
    FG_ResolutionInfo size{}; size.inputColorResolution = {1202, 800}; size.outputColorResolution = {1202, 800};
    call("set-resolution", [&] { return api.resolution(context.value, &size); });
    FG_ImageFormat_VK format{VK_FORMAT_R8G8B8A8_UNORM, VK_FORMAT_D24_UNORM_S8_UINT, VK_FORMAT_R8G8B8A8_UNORM};
    call("set-format", [&] { return api.format(context.value, &format); });
    FG_IntegrationInfo integration{}; integration.presentMode = FG_PRESENT_BY_SYSTEM;
    call("set-system-integration", [&] { return api.integration(context.value, &integration); });
    call("activate", [&] { return api.activate(context.value); }); context.active = true;
    bool active = false; call("is-active", [&] { return api.isActive(context.value, &active); });
    if (!active) throw std::runtime_error("successful activation did not report active");
    trace.step("offscreen-ai-output-unavailable");
    context.finish(trace);
    trace.step("before-vulkan-teardown"); vk.finish(); trace.step("after-vulkan-teardown");
    trace.step("before-library-retire", "\"policy\":\"process_lifetime_singleton\",\"willDlclose\":false");
    // Retire this stack borrow; the single process mapping intentionally stays
    // loaded. This cannot invoke vendor shutdown/static destructors.
    trace.step("after-library-retire", "\"libraryRetainedForProcessLifetime\":true,\"dlcloseCalled\":false");
    std::ostringstream result;
    result << "{\"status\":\"unsupported\",\"probe\":\"ai_vulkan_frame_generation_offscreen\",\"processId\":" << getpid()
        << ",\"scope\":\"public_context_configuration_lifecycle_only\",\"capabilitySupported\":true,\"contextCreated\":true,\"activated\":true"
        << ",\"reason\":\"No verified public caller-owned AI output path: AI guidance uses system presentation; this independent worker has no real surface or present lifecycle. No unsupported game-presentation/no-depth substitution was attempted.\""
        << ",\"deviceName\":" << quote(vk.properties.deviceName)
        << ",\"resolution\":[1202,800],\"logicalDeviceExtensionsEnabled\":[\"VK_KHR_swapchain\",\"VK_KHR_shader_float16_int8\"]"
        << ",\"sampleShaderFeaturesEnabled\":{\"shaderFloat16\":true,\"shaderInt16\":true,\"fragmentStoresAndAtomics\":true},\"dispatchCalls\":0,\"submittedCommandBuffers\":0"
        << ",\"generatedFramesVerified\":0,\"displayFramesVerified\":0,\"readbackBytesCompared\":0,\"guardBytesChecked\":0"
        << ",\"frameContextTeardownComplete\":true,\"vulkanTeardownComplete\":true,\"productionEnabled\":false"
        << ",\"libraryRetainedForProcessLifetime\":true,\"libraryUnloaded\":false,\"dlcloseCalled\":false"
        << ",\"references\":[" << quote(kAiGuide) << ',' << quote(kApiGuide) << "]}";
    trace.step("complete-unsupported"); return result.str();
}
bool saveLocalResult(const char* directory, const std::string& result) {
    if (!directory || !*directory) return false;
    const std::string path = std::string(directory) + "/graphics-frame-probe-local-result.json";
    int fd = open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    if (fd < 0) return false;
    const std::string line = result + '\n'; size_t offset = 0; bool ok = true;
    while (offset < line.size()) {
        const auto n = write(fd, line.data() + offset, line.size() - offset);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) { ok = false; break; }
        offset += static_cast<size_t>(n);
    }
    if (fsync(fd) != 0) ok = false;
    close(fd); return ok;
}
}
extern "C" const char* muguet_graphics_frame_probe_json(const char* filesDirectory) {
    thread_local std::string result;
    if (running.test_and_set(std::memory_order_acquire)) {
        result = "{\"status\":\"error\",\"reason\":\"frame probe already running\",\"generatedFramesVerified\":0}"; return result.c_str();
    }
    struct Guard { ~Guard() { running.clear(std::memory_order_release); } } guard;
    std::string phase = "entry";
    try { Trace trace(filesDirectory, &phase); result = run(trace); }
    catch (const std::exception& error) {
        result = "{\"status\":\"error\",\"probe\":\"ai_vulkan_frame_generation_offscreen\",\"processId\":" + std::to_string(getpid()) +
            ",\"phase\":" + quote(phase) + ",\"reason\":" + quote(error.what()) +
            ",\"scope\":\"public_context_configuration_lifecycle_only\",\"dispatchCalls\":0,\"generatedFramesVerified\":0,\"productionEnabled\":false}";
    }
    // Persist independently of the outer NAPI worker's completion callback.
    // This is an observation checkpoint, not a generated-frame success signal.
    if (!saveLocalResult(filesDirectory, result)) {
        result = "{\"status\":\"error\",\"phase\":\"persist-local-result\",\"reason\":\"cannot persist local FG result\",\"generatedFramesVerified\":0}";
    }
    return result.c_str();
}
