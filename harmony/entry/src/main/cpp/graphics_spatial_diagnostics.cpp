#include "graphics_spatial_diagnostics.h"
#include <xengine/xeg_vulkan_extension.h>
#include <xengine/xeg_vulkan_spatial_upscale.h>
#include <syscap_ndk.h>
#include <vulkan/vulkan.h>
#include <dlfcn.h>
#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <mutex>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
template<class T> T info(VkStructureType type) { T value{}; value.sType = type; return value; }
std::string quote(const std::string& value) {
    std::ostringstream out; out << '"';
    for (unsigned char c : value) {
        if (c == '"' || c == '\\') out << '\\' << char(c);
        else if (c < 32) out << "\\u" << std::hex << std::setw(4) << std::setfill('0') << unsigned(c) << std::dec;
        else out << char(c);
    }
    out << '"'; return out.str();
}
const char* boolean(bool value) { return value ? "true" : "false"; }
void check(VkResult result, const char* call) {
    if (result != VK_SUCCESS) throw std::runtime_error(std::string(call) + ": VkResult " + std::to_string(result));
}
struct Image { VkImage image = VK_NULL_HANDLE; VkImageView view = VK_NULL_HANDLE; VkDeviceMemory memory = VK_NULL_HANDLE; };
struct Buffer { VkBuffer buffer = VK_NULL_HANDLE; VkDeviceMemory memory = VK_NULL_HANDLE; VkDeviceSize size = 0; };
struct Fixture {
    std::string pattern, error, outputFile;
    VkExtent2D source{}, destination{};
    uint64_t verifiedChannels = 0, badChannels = 0, alphaFailures = 0;
    unsigned maximumError = 0;
    bool guards = false, passed = false;
    double submitWallMs = 0, gpuRenderMs = -1;
    std::string json() const {
        std::ostringstream out;
        out << "{\"pattern\":" << quote(pattern) << ",\"inputSize\":[" << source.width << ',' << source.height
            << "],\"outputSize\":[" << destination.width << ',' << destination.height << "],\"status\":" << quote(passed ? "pass" : "fail")
            << ",\"verifiedChannels\":" << verifiedChannels << ",\"badChannels\":" << badChannels
            << ",\"maximumCheckedChannelError\":" << maximumError << ",\"alphaFailures\":" << alphaFailures
            << ",\"guardBytesVerified\":" << boolean(guards) << ",\"submitToFenceWallMs\":" << submitWallMs
            << ",\"gpuUpscaleTimestampMs\":" << gpuRenderMs << ",\"outputFile\":" << quote(outputFile) << ",\"error\":" << quote(error) << '}';
        return out.str();
    }
};
struct Probe {
    std::string directory, phase = "created", error, loadError;
    void* library = nullptr;
    PFN_HMS_XEG_EnumerateDeviceExtensionProperties enumerate = nullptr;
    PFN_HMS_XEG_CreateSpatialUpscale create = nullptr;
    PFN_HMS_XEG_CmdRenderSpatialUpscale render = nullptr;
    PFN_HMS_XEG_DestroySpatialUpscale destroy = nullptr;
    VkInstance instance = VK_NULL_HANDLE;
    VkPhysicalDevice physical = VK_NULL_HANDLE;
    VkDevice device = VK_NULL_HANDLE;
    VkQueue queue = VK_NULL_HANDLE;
    VkPhysicalDeviceProperties properties{};
    VkPhysicalDeviceMemoryProperties memories{};
    uint32_t family = UINT32_MAX, timestampBits = 0;
    VkDebugUtilsMessengerEXT messenger = VK_NULL_HANDLE;
    PFN_vkDestroyDebugUtilsMessengerEXT destroyMessenger = nullptr;
    bool syscap = false, extensionVerified = false, validationLayerAvailable = false, validationLayerEnabled = false;
    bool validationFeaturesAvailable = false, handleWrappingDisabled = false;
    std::atomic<bool> negativeObserved{false};
    bool safeToDestroy = true, cleanupComplete = false;
    std::atomic<uint32_t> validationErrors{0};
    std::mutex messageMutex;
    std::vector<std::string> validationMessages;
    std::vector<Fixture> fixtures;
    VkCommandPool pool = VK_NULL_HANDLE;
    VkCommandBuffer command = VK_NULL_HANDLE;
    VkFence fence = VK_NULL_HANDLE;
    VkQueryPool query = VK_NULL_HANDLE;
    XEG_SpatialUpscale upscale = VK_NULL_HANDLE;
    Image input, output;
    Buffer upload, readback;
    static constexpr VkDeviceSize prefix = 256, suffix = 256;
    explicit Probe(const char* path) : directory(path ? path : "") {}
    ~Probe() { cleanup(); }
    void trace(const std::string& next, const std::string& extra = "") {
        phase = next;
        if (!directory.empty()) {
            std::ofstream out(directory + "/graphics-spatial-probe-trace.jsonl", std::ios::app);
            out << "{\"phase\":" << quote(next); if (!extra.empty()) out << ',' << extra; out << "}\n"; out.flush();
        }
    }
    static VKAPI_ATTR VkBool32 VKAPI_CALL callback(VkDebugUtilsMessageSeverityFlagBitsEXT severity,
        VkDebugUtilsMessageTypeFlagsEXT, const VkDebugUtilsMessengerCallbackDataEXT* data, void* user) {
        auto& p = *static_cast<Probe*>(user);
        const std::string text = std::string(data && data->pMessageIdName ? data->pMessageIdName : "") + ": " +
            (data && data->pMessage ? data->pMessage : "");
        if (text.find("VUID-vkSubmitDebugUtilsMessageEXT-messageSeverity-parameter") != std::string::npos) p.negativeObserved = true;
        else if (severity & VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT) ++p.validationErrors;
        std::lock_guard<std::mutex> lock(p.messageMutex);
        if (p.validationMessages.size() < 64) p.validationMessages.push_back(text);
        return VK_FALSE;
    }
    void setup() {
        trace("syscap"); syscap = canIUse("SystemCapability.Graphic.XEngine");
        if (!syscap) throw std::runtime_error("XEngine Syscap absent");
        uint32_t count = 0; check(vkEnumerateInstanceLayerProperties(&count, nullptr), "vkEnumerateInstanceLayerProperties");
        std::vector<VkLayerProperties> layers(count); if (count) check(vkEnumerateInstanceLayerProperties(&count, layers.data()), "vkEnumerateInstanceLayerProperties list");
        for (const auto& layer : layers) if (std::strcmp(layer.layerName, "VK_LAYER_KHRONOS_validation") == 0) validationLayerAvailable = true;
        count = 0; check(vkEnumerateInstanceExtensionProperties(nullptr, &count, nullptr), "vkEnumerateInstanceExtensionProperties");
        std::vector<VkExtensionProperties> instanceExtensions(count);
        if (count) check(vkEnumerateInstanceExtensionProperties(nullptr, &count, instanceExtensions.data()), "vkEnumerateInstanceExtensionProperties list");
        bool debug = false; for (const auto& e : instanceExtensions) if (std::strcmp(e.extensionName, VK_EXT_DEBUG_UTILS_EXTENSION_NAME) == 0) debug = true;
        auto app = info<VkApplicationInfo>(VK_STRUCTURE_TYPE_APPLICATION_INFO); app.pApplicationName = "Public XEngine offscreen SR probe"; app.apiVersion = VK_API_VERSION_1_1;
        auto instanceInfo = info<VkInstanceCreateInfo>(VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO); instanceInfo.pApplicationInfo = &app;
        const char* layerName = "VK_LAYER_KHRONOS_validation";
        std::vector<const char*> enabledExtensions;
        if (debug) enabledExtensions.push_back(VK_EXT_DEBUG_UTILS_EXTENSION_NAME);
        const VkValidationFeatureDisableEXT disabledFeature = VK_VALIDATION_FEATURE_DISABLE_UNIQUE_HANDLES_EXT;
        auto validationFeatures = info<VkValidationFeaturesEXT>(VK_STRUCTURE_TYPE_VALIDATION_FEATURES_EXT);
        if (validationLayerAvailable) { instanceInfo.enabledLayerCount = 1; instanceInfo.ppEnabledLayerNames = &layerName; validationLayerEnabled = true; }
        if (validationLayerAvailable) {
            uint32_t layerExtensionCount = 0;
            check(vkEnumerateInstanceExtensionProperties(layerName, &layerExtensionCount, nullptr), "Validation layer extension count");
            std::vector<VkExtensionProperties> layerExtensions(layerExtensionCount);
            if (layerExtensionCount) check(vkEnumerateInstanceExtensionProperties(layerName, &layerExtensionCount, layerExtensions.data()), "Validation layer extension list");
            for (const auto& value : layerExtensions)
                if (std::strcmp(value.extensionName, VK_EXT_VALIDATION_FEATURES_EXTENSION_NAME) == 0) validationFeaturesAvailable = true;
            if (!validationFeaturesAvailable) throw std::runtime_error("Validation layer did not advertise public VK_EXT_validation_features");
            enabledExtensions.push_back(VK_EXT_VALIDATION_FEATURES_EXTENSION_NAME);
            // Only disable non-dispatchable handle wrapping. Core, parameter,
            // image-layout, object-lifetime, shader and thread checks retain
            // their normal validation-layer configuration. No vendor code,
            // private driver handle conversion or global environment change.
            validationFeatures.disabledValidationFeatureCount = 1;
            validationFeatures.pDisabledValidationFeatures = &disabledFeature;
            instanceInfo.pNext = &validationFeatures;
            trace("validation_settings", "\"disableOnly\":\"VK_VALIDATION_FEATURE_DISABLE_UNIQUE_HANDLES_EXT\"");
        }
        instanceInfo.enabledExtensionCount = uint32_t(enabledExtensions.size());
        instanceInfo.ppEnabledExtensionNames = enabledExtensions.empty() ? nullptr : enabledExtensions.data();
        trace("vkCreateInstance"); check(vkCreateInstance(&instanceInfo, nullptr, &instance), "vkCreateInstance");
        handleWrappingDisabled = validationLayerAvailable;
        if (debug) {
            auto makeMessenger = reinterpret_cast<PFN_vkCreateDebugUtilsMessengerEXT>(vkGetInstanceProcAddr(instance, "vkCreateDebugUtilsMessengerEXT"));
            destroyMessenger = reinterpret_cast<PFN_vkDestroyDebugUtilsMessengerEXT>(vkGetInstanceProcAddr(instance, "vkDestroyDebugUtilsMessengerEXT"));
            auto messengerInfo = info<VkDebugUtilsMessengerCreateInfoEXT>(VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT);
            messengerInfo.messageSeverity = VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT | VK_DEBUG_UTILS_MESSAGE_SEVERITY_WARNING_BIT_EXT;
            messengerInfo.messageType = VK_DEBUG_UTILS_MESSAGE_TYPE_GENERAL_BIT_EXT | VK_DEBUG_UTILS_MESSAGE_TYPE_VALIDATION_BIT_EXT | VK_DEBUG_UTILS_MESSAGE_TYPE_PERFORMANCE_BIT_EXT;
            messengerInfo.pfnUserCallback = callback; messengerInfo.pUserData = this;
            if (makeMessenger && destroyMessenger) check(makeMessenger(instance, &messengerInfo, nullptr, &messenger), "vkCreateDebugUtilsMessengerEXT");
            auto submitMessage = reinterpret_cast<PFN_vkSubmitDebugUtilsMessageEXT>(vkGetInstanceProcAddr(instance, "vkSubmitDebugUtilsMessageEXT"));
            if (messenger && submitMessage) {
                auto message = info<VkDebugUtilsMessengerCallbackDataEXT>(VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CALLBACK_DATA_EXT);
                message.pMessageIdName = "MuguetSpatialValidationNegativeControl";
                message.pMessage = "Intentional invalid severity control, no GPU resource/command involved";
                submitMessage(instance, static_cast<VkDebugUtilsMessageSeverityFlagBitsEXT>(0), VK_DEBUG_UTILS_MESSAGE_TYPE_GENERAL_BIT_EXT, &message);
            }
        }
        count = 0; check(vkEnumeratePhysicalDevices(instance, &count, nullptr), "vkEnumeratePhysicalDevices count");
        if (count == 0 || count > 256) throw std::runtime_error("No reasonable physical-device list");
        std::vector<VkPhysicalDevice> devices(count); check(vkEnumeratePhysicalDevices(instance, &count, devices.data()), "vkEnumeratePhysicalDevices");
        physical = devices.front(); vkGetPhysicalDeviceProperties(physical, &properties); vkGetPhysicalDeviceMemoryProperties(physical, &memories);
        trace("public_library"); library = dlopen("libxengine.so", RTLD_NOW | RTLD_LOCAL);
        if (!library) { const char* value = dlerror(); throw std::runtime_error(value ? value : "libxengine.so unavailable"); }
        enumerate = reinterpret_cast<PFN_HMS_XEG_EnumerateDeviceExtensionProperties>(dlsym(library, "HMS_XEG_EnumerateDeviceExtensionProperties"));
        create = reinterpret_cast<PFN_HMS_XEG_CreateSpatialUpscale>(dlsym(library, "HMS_XEG_CreateSpatialUpscale"));
        render = reinterpret_cast<PFN_HMS_XEG_CmdRenderSpatialUpscale>(dlsym(library, "HMS_XEG_CmdRenderSpatialUpscale"));
        destroy = reinterpret_cast<PFN_HMS_XEG_DestroySpatialUpscale>(dlsym(library, "HMS_XEG_DestroySpatialUpscale"));
        if (!enumerate || !create || !render || !destroy) throw std::runtime_error("One or more public spatial symbols unavailable");
        count = 0; check(enumerate(physical, &count, nullptr), "HMS_XEG_EnumerateDeviceExtensionProperties count");
        if (count > 4096) throw std::runtime_error("Unreasonable XEngine extension count");
        std::vector<XEG_ExtensionProperties> extensions(count);
        if (count) check(enumerate(physical, &count, extensions.data()), "HMS_XEG_EnumerateDeviceExtensionProperties list");
        for (const auto& e : extensions) if (std::strncmp(e.extensionName, XEG_SPATIAL_UPSCALE_EXTENSION_NAME, XEG_MAX_EXTENSION_NAME_SIZE) == 0) extensionVerified = true;
        if (!extensionVerified) throw std::runtime_error("XEG_spatial_upscale not advertised");
        count = 0; vkGetPhysicalDeviceQueueFamilyProperties(physical, &count, nullptr);
        std::vector<VkQueueFamilyProperties> families(count); vkGetPhysicalDeviceQueueFamilyProperties(physical, &count, families.data());
        for (uint32_t i = 0; i < count; ++i) if (families[i].queueCount && (families[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)) { family = i; timestampBits = families[i].timestampValidBits; break; }
        if (family == UINT32_MAX) throw std::runtime_error("Graphics queue unavailable");
        float priority = 1.0f; auto queueInfo = info<VkDeviceQueueCreateInfo>(VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO);
        queueInfo.queueFamilyIndex = family; queueInfo.queueCount = 1; queueInfo.pQueuePriorities = &priority;
        auto deviceInfo = info<VkDeviceCreateInfo>(VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO); deviceInfo.queueCreateInfoCount = 1; deviceInfo.pQueueCreateInfos = &queueInfo;
        trace("vkCreateDevice"); check(vkCreateDevice(physical, &deviceInfo, nullptr, &device), "vkCreateDevice"); vkGetDeviceQueue(device, family, 0, &queue);
        VkFormatProperties format{}; vkGetPhysicalDeviceFormatProperties(physical, VK_FORMAT_R8G8B8A8_UNORM, &format);
        const VkFormatFeatureFlags required = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT | VK_FORMAT_FEATURE_SAMPLED_IMAGE_FILTER_LINEAR_BIT | VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT;
        if ((format.optimalTilingFeatures & required) != required) throw std::runtime_error("RGBA8 sampled/linear/color attachment unsupported");
    }
    uint32_t memoryType(uint32_t bits, VkMemoryPropertyFlags required) {
        for (uint32_t i = 0; i < memories.memoryTypeCount; ++i) if ((bits & (1u << i)) && (memories.memoryTypes[i].propertyFlags & required) == required) return i;
        throw std::runtime_error("Required memory type unavailable");
    }
    void makeImage(Image& image, VkExtent2D extent) {
        auto imageInfo = info<VkImageCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO); imageInfo.imageType = VK_IMAGE_TYPE_2D;
        imageInfo.format = VK_FORMAT_R8G8B8A8_UNORM; imageInfo.extent = {extent.width, extent.height, 1}; imageInfo.mipLevels = 1;
        imageInfo.arrayLayers = 1; imageInfo.samples = VK_SAMPLE_COUNT_1_BIT; imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
        imageInfo.usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT;
        imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE; imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        check(vkCreateImage(device, &imageInfo, nullptr, &image.image), "vkCreateImage");
        VkMemoryRequirements requirements{}; vkGetImageMemoryRequirements(device, image.image, &requirements);
        auto allocation = info<VkMemoryAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO); allocation.allocationSize = requirements.size;
        allocation.memoryTypeIndex = memoryType(requirements.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
        check(vkAllocateMemory(device, &allocation, nullptr, &image.memory), "vkAllocateMemory image"); check(vkBindImageMemory(device, image.image, image.memory, 0), "vkBindImageMemory");
        auto view = info<VkImageViewCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO); view.image = image.image; view.viewType = VK_IMAGE_VIEW_TYPE_2D;
        view.format = imageInfo.format; view.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
        check(vkCreateImageView(device, &view, nullptr, &image.view), "vkCreateImageView");
    }
    void makeBuffer(Buffer& buffer, VkDeviceSize size, VkBufferUsageFlags usage) {
        buffer.size = size; auto value = info<VkBufferCreateInfo>(VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO); value.size = size; value.usage = usage; value.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        check(vkCreateBuffer(device, &value, nullptr, &buffer.buffer), "vkCreateBuffer"); VkMemoryRequirements requirements{}; vkGetBufferMemoryRequirements(device, buffer.buffer, &requirements);
        auto allocation = info<VkMemoryAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO); allocation.allocationSize = requirements.size;
        allocation.memoryTypeIndex = memoryType(requirements.memoryTypeBits, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
        check(vkAllocateMemory(device, &allocation, nullptr, &buffer.memory), "vkAllocateMemory buffer"); check(vkBindBufferMemory(device, buffer.buffer, buffer.memory, 0), "vkBindBufferMemory");
    }
    static std::vector<uint8_t> pixels(const std::string& pattern, VkExtent2D extent) {
        std::vector<uint8_t> result(size_t(extent.width) * extent.height * 4);
        const uint8_t quadrants[4][3] = {{224, 32, 48}, {32, 224, 48}, {32, 48, 224}, {224, 208, 32}};
        for (uint32_t y = 0; y < extent.height; ++y) for (uint32_t x = 0; x < extent.width; ++x) {
            auto* p = result.data() + (size_t(y) * extent.width + x) * 4;
            if (pattern == "constant") { p[0] = 64; p[1] = 128; p[2] = 191; }
            else if (pattern == "gradient") { p[0] = uint8_t(16 + 223 * x / (extent.width - 1)); p[1] = uint8_t(16 + 223 * y / (extent.height - 1)); p[2] = 48 + 128 * ((x / 64 + y / 64) % 2); }
            else { const unsigned q = (x >= extent.width / 2) + 2 * (y >= extent.height / 2); std::memcpy(p, quadrants[q], 3); }
            p[3] = 255;
        }
        return result;
    }
    void imageBarrier(VkImage image, VkImageLayout before, VkImageLayout after, VkAccessFlags source, VkAccessFlags destination,
                      VkPipelineStageFlags sourceStage, VkPipelineStageFlags destinationStage) {
        auto b = info<VkImageMemoryBarrier>(VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER); b.oldLayout = before; b.newLayout = after;
        b.srcAccessMask = source; b.dstAccessMask = destination; b.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; b.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        b.image = image; b.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1}; vkCmdPipelineBarrier(command, sourceStage, destinationStage, 0, 0, nullptr, 0, nullptr, 1, &b);
    }
    void releaseFixture() {
        if (!safeToDestroy) return;
        if (upscale) { trace("HMS_XEG_DestroySpatialUpscale"); destroy(upscale); upscale = VK_NULL_HANDLE; }
        if (query) { vkDestroyQueryPool(device, query, nullptr); query = VK_NULL_HANDLE; }
        if (fence) { vkDestroyFence(device, fence, nullptr); fence = VK_NULL_HANDLE; }
        if (pool) { vkDestroyCommandPool(device, pool, nullptr); pool = VK_NULL_HANDLE; command = VK_NULL_HANDLE; }
        for (Buffer* b : {&upload, &readback}) { if (b->buffer) vkDestroyBuffer(device, b->buffer, nullptr); if (b->memory) vkFreeMemory(device, b->memory, nullptr); *b = {}; }
        for (Image* i : {&input, &output}) { if (i->view) vkDestroyImageView(device, i->view, nullptr); if (i->image) vkDestroyImage(device, i->image, nullptr); if (i->memory) vkFreeMemory(device, i->memory, nullptr); *i = {}; }
    }
    void runFixture(Fixture& f) {
        trace("fixture_begin", "\"pattern\":" + quote(f.pattern) + ",\"sourceWidth\":" + std::to_string(f.source.width));
        const auto sourcePixels = pixels(f.pattern, f.source); const VkDeviceSize bytes = VkDeviceSize(f.destination.width) * f.destination.height * 4;
        makeImage(input, f.source); makeImage(output, f.destination); makeBuffer(upload, sourcePixels.size(), VK_BUFFER_USAGE_TRANSFER_SRC_BIT);
        makeBuffer(readback, prefix + bytes + suffix, VK_BUFFER_USAGE_TRANSFER_DST_BIT);
        void* mapped = nullptr; check(vkMapMemory(device, upload.memory, 0, upload.size, 0, &mapped), "vkMapMemory upload"); std::memcpy(mapped, sourcePixels.data(), sourcePixels.size()); vkUnmapMemory(device, upload.memory);
        check(vkMapMemory(device, readback.memory, 0, readback.size, 0, &mapped), "vkMapMemory prefill"); std::memset(mapped, 0xa5, size_t(readback.size)); vkUnmapMemory(device, readback.memory);
        XEG_SpatialUpscaleCreateInfo parameters{}; parameters.inputSize = f.source; parameters.inputRegion = {{0, 0}, f.source};
        parameters.outputSize = f.destination; parameters.outputRegion = {{0, 0}, f.destination}; parameters.format = VK_FORMAT_R8G8B8A8_UNORM; parameters.sharpness = 0.3f;
        trace("HMS_XEG_CreateSpatialUpscale"); check(create(device, &parameters, &upscale), "HMS_XEG_CreateSpatialUpscale"); if (!upscale) throw std::runtime_error("Null spatial handle after success");
        auto poolInfo = info<VkCommandPoolCreateInfo>(VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO); poolInfo.queueFamilyIndex = family;
        check(vkCreateCommandPool(device, &poolInfo, nullptr, &pool), "vkCreateCommandPool");
        auto allocation = info<VkCommandBufferAllocateInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO); allocation.commandPool = pool; allocation.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY; allocation.commandBufferCount = 1;
        check(vkAllocateCommandBuffers(device, &allocation, &command), "vkAllocateCommandBuffers"); auto fenceInfo = info<VkFenceCreateInfo>(VK_STRUCTURE_TYPE_FENCE_CREATE_INFO); check(vkCreateFence(device, &fenceInfo, nullptr, &fence), "vkCreateFence");
        if (timestampBits) { auto q = info<VkQueryPoolCreateInfo>(VK_STRUCTURE_TYPE_QUERY_POOL_CREATE_INFO); q.queryType = VK_QUERY_TYPE_TIMESTAMP; q.queryCount = 2; check(vkCreateQueryPool(device, &q, nullptr, &query), "vkCreateQueryPool"); }
        auto begin = info<VkCommandBufferBeginInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO); begin.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT; check(vkBeginCommandBuffer(command, &begin), "vkBeginCommandBuffer");
        imageBarrier(input.image, VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 0, VK_ACCESS_TRANSFER_WRITE_BIT, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT);
        VkBufferImageCopy copy{}; copy.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1}; copy.imageExtent = {f.source.width, f.source.height, 1};
        vkCmdCopyBufferToImage(command, upload.buffer, input.image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &copy);
        imageBarrier(input.image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_SHADER_READ_BIT,
            VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT);
        // Establish a real, layer-visible shader-read layout before entering
        // XEG's opaque render pass. Its initialLayout=UNDEFINED may still discard
        // prior contents legally, and its final shader-read layout matches this
        // explicit state. This is a real Vulkan transition, not a tracker edit.
        imageBarrier(output.image, VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
            0, VK_ACCESS_SHADER_READ_BIT, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT);
        if (query) { vkCmdResetQueryPool(command, query, 0, 2); vkCmdWriteTimestamp(command, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, query, 0); }
        XEG_SpatialUpscaleDescription description{input.view, output.view}; trace("HMS_XEG_CmdRenderSpatialUpscale"); render(command, upscale, &description);
        if (query) vkCmdWriteTimestamp(command, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, query, 1);
        imageBarrier(output.image, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT | VK_ACCESS_SHADER_WRITE_BIT,
            VK_ACCESS_TRANSFER_READ_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT);
        copy.bufferOffset = prefix; copy.imageExtent = {f.destination.width, f.destination.height, 1}; vkCmdCopyImageToBuffer(command, output.image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, readback.buffer, 1, &copy);
        auto host = info<VkBufferMemoryBarrier>(VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER); host.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT; host.dstAccessMask = VK_ACCESS_HOST_READ_BIT;
        host.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; host.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; host.buffer = readback.buffer; host.offset = 0; host.size = readback.size;
        vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT, 0, 0, nullptr, 1, &host, 0, nullptr);
        check(vkEndCommandBuffer(command), "vkEndCommandBuffer"); auto submit = info<VkSubmitInfo>(VK_STRUCTURE_TYPE_SUBMIT_INFO); submit.commandBufferCount = 1; submit.pCommandBuffers = &command;
        trace("vkQueueSubmit"); const auto started = std::chrono::steady_clock::now(); check(vkQueueSubmit(queue, 1, &submit, fence), "vkQueueSubmit"); safeToDestroy = false;
        trace("vkWaitForFences"); check(vkWaitForFences(device, 1, &fence, VK_TRUE, 10'000'000'000ULL), "vkWaitForFences"); safeToDestroy = true;
        f.submitWallMs = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - started).count();
        if (query) {
            uint64_t values[2]{}; check(vkGetQueryPoolResults(device, query, 0, 2, sizeof(values), values, sizeof(uint64_t), VK_QUERY_RESULT_64_BIT), "vkGetQueryPoolResults");
            const uint64_t mask = timestampBits >= 64 ? UINT64_MAX : (uint64_t(1) << timestampBits) - 1;
            f.gpuRenderMs = double((values[1] - values[0]) & mask) * properties.limits.timestampPeriod / 1e6;
        }
        trace("readback_verify"); check(vkMapMemory(device, readback.memory, 0, readback.size, 0, &mapped), "vkMapMemory readback"); const auto* data = static_cast<const uint8_t*>(mapped);
        f.guards = true; for (VkDeviceSize i = 0; i < prefix; ++i) f.guards &= data[i] == 0xa5; for (VkDeviceSize i = prefix + bytes; i < readback.size; ++i) f.guards &= data[i] == 0xa5;
        for (uint32_t y = 0; y < f.destination.height; ++y) for (uint32_t x = 0; x < f.destination.width; ++x) {
            const auto* pixel = data + prefix + (size_t(y) * f.destination.width + x) * 4;
            if (pixel[3] < 252) ++f.alphaFailures;
            const double sx = std::clamp((x + 0.5) * f.source.width / f.destination.width - 0.5, 0.0, double(f.source.width - 1));
            const double sy = std::clamp((y + 0.5) * f.source.height / f.destination.height - 0.5, 0.0, double(f.source.height - 1));
            const uint32_t ix = uint32_t(sx + 0.5), iy = uint32_t(sy + 0.5);
            const auto* expected = sourcePixels.data() + (size_t(iy) * f.source.width + ix) * 4;
            for (unsigned channel = 0; channel < 3; ++channel) {
                // Filters may legitimately sharpen/mix at discontinuities. Check
                // smooth gradients and constant interiors, excluding a four
                // source-pixel band around checker/quadrant discontinuities.
                const bool boundary = f.pattern == "quadrants" ? (std::abs(sx - f.source.width / 2.0) < 4 || std::abs(sy - f.source.height / 2.0) < 4) :
                    f.pattern == "gradient" && channel == 2 && (std::fmod(sx, 64.0) < 4 || std::fmod(sx, 64.0) > 60 || std::fmod(sy, 64.0) < 4 || std::fmod(sy, 64.0) > 60);
                if (boundary) continue;
                const unsigned difference = unsigned(std::abs(int(pixel[channel]) - int(expected[channel])));
                ++f.verifiedChannels; f.maximumError = std::max(f.maximumError, difference);
                if (difference > (f.pattern == "gradient" && channel < 2 ? 8u : 3u)) ++f.badChannels;
            }
        }
        if (f.pattern == "gradient") {
            f.outputFile = "graphics-spatial-gradient-" + std::to_string(f.destination.width) + "x" + std::to_string(f.destination.height) + ".ppm";
            std::ofstream image(directory + "/" + f.outputFile, std::ios::binary | std::ios::trunc); image << "P6\n" << f.destination.width << ' ' << f.destination.height << "\n255\n";
            for (VkDeviceSize i = 0; i < bytes; i += 4) image.write(reinterpret_cast<const char*>(data + prefix + i), 3);
            image.close(); if (!image) f.error = "Cannot save readback image";
        }
        vkUnmapMemory(device, readback.memory);
        f.passed = f.guards && f.badChannels == 0 && f.alphaFailures == 0 && f.error.empty();
        trace("fixture_complete", "\"fixture\":" + f.json()); releaseFixture();
    }
    void cleanup() {
        if (cleanupComplete || !safeToDestroy) return;
        releaseFixture();
        if (device) { trace("vkDestroyDevice"); vkDestroyDevice(device, nullptr); device = VK_NULL_HANDLE; }
        if (messenger && destroyMessenger) { destroyMessenger(instance, messenger, nullptr); messenger = VK_NULL_HANDLE; }
        if (instance) { trace("vkDestroyInstance"); vkDestroyInstance(instance, nullptr); instance = VK_NULL_HANDLE; }
        if (library) { dlclose(library); library = nullptr; } cleanupComplete = true; trace("teardown_complete");
    }
    void run() {
        setup();
        for (const auto dimensions : {0, 1}) for (const char* pattern : {"constant", "gradient", "quadrants"}) {
            Fixture fixture; fixture.pattern = pattern; fixture.source = dimensions == 0 ? VkExtent2D{802, 534} : VkExtent2D{1202, 800};
            fixture.destination = dimensions == 0 ? VkExtent2D{1202, 800} : VkExtent2D{1802, 1200};
            try { runFixture(fixture); } catch (const std::exception& failure) { fixture.error = failure.what(); fixtures.push_back(fixture); throw; }
            fixtures.push_back(fixture);
        }
    }
    std::string json() {
        bool passed = error.empty() && fixtures.size() == 6 && validationErrors.load() == 0 && cleanupComplete;
        for (const auto& f : fixtures) passed &= f.passed;
        std::ostringstream out;
        out << "{\"status\":" << quote(passed ? "pass" : "fail") << ",\"scope\":\"independent_offscreen_public_xengine_gpu_spatial_upscale\",\"deviceName\":" << quote(properties.deviceName)
            << ",\"syscap\":" << boolean(syscap) << ",\"extensionVerified\":" << boolean(extensionVerified) << ",\"validationLayerAvailable\":" << boolean(validationLayerAvailable)
            << ",\"validationLayerEnabled\":" << boolean(validationLayerEnabled) << ",\"validationNegativeControlObserved\":" << boolean(negativeObserved.load())
            << ",\"validationFeaturesExtensionAvailable\":" << boolean(validationFeaturesAvailable)
            << ",\"handleWrapping\":" << (validationLayerEnabled ? boolean(!handleWrappingDisabled) : "null")
            << ",\"validationDisableRequest\":" << (handleWrappingDisabled ? "[\"VK_VALIDATION_FEATURE_DISABLE_UNIQUE_HANDLES_EXT\"]" : "[]")
            << ",\"otherValidationFeaturesDisabledByThisProbe\":false"
            << ",\"explicitOutputShaderReadLayoutBeforeXeg\":true"
            << ",\"unexpectedValidationErrors\":" << validationErrors.load() << ",\"cleanupComplete\":" << boolean(cleanupComplete) << ",\"cleanupSkippedForPendingWork\":" << boolean(!safeToDestroy)
            << ",\"productionEnabled\":false,\"gameRuntimeModified\":false,\"generatedFrames\":0,\"phase\":" << quote(phase) << ",\"error\":" << quote(error)
            << ",\"gpuTimingScope\":\"TOP_OF_PIPE to BOTTOM_OF_PIPE around public render command; preceding upload may overlap; isolated fixture, not game FPS\",\"fixtures\":[";
        for (size_t i = 0; i < fixtures.size(); ++i) { if (i) out << ','; out << fixtures[i].json(); }
        out << "],\"validationMessages\":["; std::lock_guard<std::mutex> lock(messageMutex);
        for (size_t i = 0; i < validationMessages.size(); ++i) { if (i) out << ','; out << quote(validationMessages[i]); }
        out << "]}"; return out.str();
    }
};
}
extern "C" const char* muguet_graphics_spatial_probe_json(const char* filesDirectory) {
    thread_local std::string result; Probe probe(filesDirectory);
    try { probe.run(); } catch (const std::exception& error) { probe.error = error.what(); probe.trace("failed", "\"error\":" + quote(probe.error)); }
    probe.cleanup(); result = probe.json(); return result.c_str();
}
