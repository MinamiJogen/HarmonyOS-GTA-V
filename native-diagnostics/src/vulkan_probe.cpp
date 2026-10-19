// Read-only Vulkan capabilities probe. No VkDevice, window or surface is created.
#define VK_NO_PROTOTYPES
#define VK_USE_PLATFORM_OHOS
#include <vulkan/vulkan.h>

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <dlfcn.h>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

namespace {
std::string quote(const std::string &text) {
    static const char hex[] = "0123456789abcdef";
    std::string out = "\"";
    for (unsigned char c : text) {
        if (c == '"' || c == '\\') { out += '\\'; out += char(c); }
        else if (c < 32) { out += "\\u00"; out += hex[c >> 4]; out += hex[c & 15]; }
        else { out += char(c); }
    }
    return out + '"';
}
std::string boolean(bool value) { return value ? "true" : "false"; }
template<class T> std::string number(T value) { return std::to_string(value); }
std::string version(uint32_t value) {
    return "{\"raw\":" + number(value) + ",\"variant\":" + number(VK_API_VERSION_VARIANT(value))
        + ",\"major\":" + number(VK_API_VERSION_MAJOR(value)) + ",\"minor\":"
        + number(VK_API_VERSION_MINOR(value)) + ",\"patch\":" + number(VK_API_VERSION_PATCH(value)) + "}";
}
std::string array(const std::vector<std::string> &items) {
    std::string out = "[";
    for (size_t i = 0; i < items.size(); ++i) { if (i) out += ','; out += items[i]; }
    return out + ']';
}
std::string result_name(VkResult result) {
    switch (result) {
    case VK_SUCCESS: return "VK_SUCCESS";
    case VK_NOT_READY: return "VK_NOT_READY";
    case VK_TIMEOUT: return "VK_TIMEOUT";
    case VK_INCOMPLETE: return "VK_INCOMPLETE";
    case VK_ERROR_OUT_OF_HOST_MEMORY: return "VK_ERROR_OUT_OF_HOST_MEMORY";
    case VK_ERROR_OUT_OF_DEVICE_MEMORY: return "VK_ERROR_OUT_OF_DEVICE_MEMORY";
    case VK_ERROR_INITIALIZATION_FAILED: return "VK_ERROR_INITIALIZATION_FAILED";
    case VK_ERROR_LAYER_NOT_PRESENT: return "VK_ERROR_LAYER_NOT_PRESENT";
    case VK_ERROR_EXTENSION_NOT_PRESENT: return "VK_ERROR_EXTENSION_NOT_PRESENT";
    case VK_ERROR_INCOMPATIBLE_DRIVER: return "VK_ERROR_INCOMPATIBLE_DRIVER";
    default: return "VkResult(" + number(int32_t(result)) + ")";
    }
}

struct Report {
    std::string loader = "libvulkan.so";
    bool loader_opened = false;
    bool instance_created = false;
    bool surface_extension = false;
    bool surface_enabled = false;
    bool surface_entry = false;
    bool version_entry = false;
    uint32_t loader_version = VK_API_VERSION_1_0;
    std::string loader_version_source = "Vulkan 1.0 fallback: vkEnumerateInstanceVersion unavailable";
    std::vector<std::string> instance_extensions, devices, errors, warnings;
    void error(const std::string &stage, const std::string &message) {
        errors.push_back("{\"stage\":" + quote(stage) + ",\"message\":" + quote(message) + "}");
    }
    void warning(const std::string &stage, const std::string &message) {
        warnings.push_back("{\"stage\":" + quote(stage) + ",\"message\":" + quote(message) + "}");
    }
    void vkerror(const std::string &stage, VkResult result) { error(stage, result_name(result)); }
    std::string json() const {
        std::ostringstream output;
        output << "{\"schema_version\":1,\"probe\":\"ohos_vulkan_capabilities\","
            "\"scope\":\"Enumeration only; no logical device, window, surface, swapchain, shader or presentation was tested\","
            "\"status\":" << quote(errors.empty() ? "pass" : "fail")
            << ",\"build\":{\"sdk_api\":26,\"target\":"
#if defined(__OHOS__)
            << quote("aarch64-linux-ohos26")
#else
            << quote("host-test-only")
#endif
            << ",\"vulkan_header_version\":" << VK_HEADER_VERSION << "},"
            "\"loader\":{\"library\":" << quote(loader) << ",\"opened\":" << boolean(loader_opened)
            << ",\"enumerate_instance_version_entry\":" << boolean(version_entry)
            << ",\"reported_api_version\":" << (loader_opened ? version(loader_version) : "null")
            << ",\"version_source\":" << quote(loader_version_source) << "},"
            "\"instance\":{\"created\":" << boolean(instance_created)
            << ",\"requested_api_version\":" << version(VK_API_VERSION_1_0)
            << ",\"extensions\":" << array(instance_extensions) << "},"
            "\"ohos_surface\":{\"extension_advertised\":" << boolean(surface_extension)
            << ",\"extension_enabled\":" << boolean(surface_enabled)
            << ",\"create_surface_entry_available\":" << boolean(surface_entry)
            << ",\"surface_created\":false,\"presentation_verified\":false},"
            "\"physical_device_count\":" << devices.size() << ",\"physical_devices\":" << array(devices)
            << ",\"errors\":" << array(errors) << ",\"warnings\":" << array(warnings) << "}";
        return output.str();
    }
};

template<class T> T global(void *library, PFN_vkGetInstanceProcAddr get_proc, const char *name) {
    PFN_vkVoidFunction entry = get_proc ? get_proc(VK_NULL_HANDLE, name) : nullptr;
    return entry ? reinterpret_cast<T>(entry) : reinterpret_cast<T>(dlsym(library, name));
}
template<class T> T instance_proc(void *library, PFN_vkGetInstanceProcAddr get_proc,
                                   VkInstance instance, const char *name) {
    PFN_vkVoidFunction entry = get_proc(instance, name);
    return entry ? reinterpret_cast<T>(entry) : reinterpret_cast<T>(dlsym(library, name));
}

// Vulkan counts may change between calls. Retry boundedly and never read beyond
// the allocated capacity. A persistently incomplete enumeration is an error.
template<class Item, class Enumerate>
bool enumerate(Report &report, const std::string &stage, Enumerate fn, std::vector<Item> &items) {
    for (unsigned attempt = 0; attempt < 5; ++attempt) {
        uint32_t count = 0;
        VkResult result = fn(&count, nullptr);
        if (result != VK_SUCCESS && result != VK_INCOMPLETE) { report.vkerror(stage + ":count", result); return false; }
        if (count > (1u << 20)) { report.error(stage, "unreasonable enumeration count"); return false; }
        if (count == 0) { items.clear(); return true; }
        items.resize(count);
        uint32_t capacity = count;
        result = fn(&count, items.data());
        if (count > capacity) { report.error(stage, "driver returned a count larger than supplied capacity"); return false; }
        items.resize(count);
        if (result == VK_SUCCESS) return true;
        if (result != VK_INCOMPLETE) { report.vkerror(stage + ":items", result); return false; }
    }
    report.error(stage, "enumeration remained VK_INCOMPLETE after 5 attempts");
    return false;
}

std::string extension_json(const VkExtensionProperties &item) {
    return "{\"name\":" + quote(std::string(item.extensionName, strnlen(item.extensionName, VK_MAX_EXTENSION_NAME_SIZE)))
        + ",\"spec_version\":" + number(item.specVersion) + "}";
}
bool has_extension(const std::vector<VkExtensionProperties> &items, const char *name) {
    return std::any_of(items.begin(), items.end(), [name](const VkExtensionProperties &item) {
        return strncmp(item.extensionName, name, VK_MAX_EXTENSION_NAME_SIZE) == 0;
    });
}

std::string features_json(const VkPhysicalDeviceFeatures &f) {
    std::string out = "{";
#define FEATURE(name) out += "\"" #name "\":" + boolean(f.name == VK_TRUE) + ",";
    FEATURE(robustBufferAccess) FEATURE(fullDrawIndexUint32) FEATURE(imageCubeArray)
    FEATURE(independentBlend) FEATURE(geometryShader) FEATURE(tessellationShader)
    FEATURE(sampleRateShading) FEATURE(dualSrcBlend) FEATURE(logicOp) FEATURE(multiDrawIndirect)
    FEATURE(drawIndirectFirstInstance) FEATURE(depthClamp) FEATURE(depthBiasClamp)
    FEATURE(fillModeNonSolid) FEATURE(depthBounds) FEATURE(wideLines) FEATURE(largePoints)
    FEATURE(alphaToOne) FEATURE(multiViewport) FEATURE(samplerAnisotropy)
    FEATURE(textureCompressionETC2) FEATURE(textureCompressionASTC_LDR) FEATURE(textureCompressionBC)
    FEATURE(occlusionQueryPrecise) FEATURE(pipelineStatisticsQuery)
    FEATURE(vertexPipelineStoresAndAtomics) FEATURE(fragmentStoresAndAtomics)
    FEATURE(shaderTessellationAndGeometryPointSize) FEATURE(shaderImageGatherExtended)
    FEATURE(shaderStorageImageExtendedFormats) FEATURE(shaderStorageImageMultisample)
    FEATURE(shaderStorageImageReadWithoutFormat) FEATURE(shaderStorageImageWriteWithoutFormat)
    FEATURE(shaderUniformBufferArrayDynamicIndexing) FEATURE(shaderSampledImageArrayDynamicIndexing)
    FEATURE(shaderStorageBufferArrayDynamicIndexing) FEATURE(shaderStorageImageArrayDynamicIndexing)
    FEATURE(shaderClipDistance) FEATURE(shaderCullDistance) FEATURE(shaderFloat64)
    FEATURE(shaderInt64) FEATURE(shaderInt16) FEATURE(shaderResourceResidency)
    FEATURE(shaderResourceMinLod) FEATURE(sparseBinding) FEATURE(sparseResidencyBuffer)
    FEATURE(sparseResidencyImage2D) FEATURE(sparseResidencyImage3D)
    FEATURE(sparseResidency2Samples) FEATURE(sparseResidency4Samples)
    FEATURE(sparseResidency8Samples) FEATURE(sparseResidency16Samples)
    FEATURE(sparseResidencyAliased) FEATURE(variableMultisampleRate) FEATURE(inheritedQueries)
#undef FEATURE
    out.back() = '}';
    return out;
}

struct Format { const char *name; VkFormat value; };
const Format formats[] = {
#define FORMAT(name) {#name, name}
    FORMAT(VK_FORMAT_BC1_RGB_UNORM_BLOCK), FORMAT(VK_FORMAT_BC1_RGB_SRGB_BLOCK),
    FORMAT(VK_FORMAT_BC1_RGBA_UNORM_BLOCK), FORMAT(VK_FORMAT_BC1_RGBA_SRGB_BLOCK),
    FORMAT(VK_FORMAT_BC2_UNORM_BLOCK), FORMAT(VK_FORMAT_BC2_SRGB_BLOCK),
    FORMAT(VK_FORMAT_BC3_UNORM_BLOCK), FORMAT(VK_FORMAT_BC3_SRGB_BLOCK),
    FORMAT(VK_FORMAT_BC4_UNORM_BLOCK), FORMAT(VK_FORMAT_BC4_SNORM_BLOCK),
    FORMAT(VK_FORMAT_BC5_UNORM_BLOCK), FORMAT(VK_FORMAT_BC5_SNORM_BLOCK),
    FORMAT(VK_FORMAT_BC6H_UFLOAT_BLOCK), FORMAT(VK_FORMAT_BC6H_SFLOAT_BLOCK),
    FORMAT(VK_FORMAT_BC7_UNORM_BLOCK), FORMAT(VK_FORMAT_BC7_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_4x4_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_4x4_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_5x4_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_5x4_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_5x5_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_5x5_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_6x5_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_6x5_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_6x6_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_6x6_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_8x5_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_8x5_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_8x6_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_8x6_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_8x8_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_8x8_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_10x5_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_10x5_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_10x6_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_10x6_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_10x8_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_10x8_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_10x10_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_10x10_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_12x10_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_12x10_SRGB_BLOCK),
    FORMAT(VK_FORMAT_ASTC_12x12_UNORM_BLOCK), FORMAT(VK_FORMAT_ASTC_12x12_SRGB_BLOCK),
#undef FORMAT
};

int run(Report &report) {
    void *library = dlopen(report.loader.c_str(), RTLD_NOW | RTLD_LOCAL);
    if (!library) {
        const char *error = dlerror();
        report.error("dlopen", error ? error : "unknown loader error");
        return 1;
    }
    report.loader_opened = true;
    struct LibraryGuard { void *handle; ~LibraryGuard() { dlclose(handle); } } library_guard{library};
    auto get_proc = reinterpret_cast<PFN_vkGetInstanceProcAddr>(dlsym(library, "vkGetInstanceProcAddr"));
    if (!get_proc) { report.error("dlsym", "vkGetInstanceProcAddr is missing"); return 1; }
    auto enum_version = global<PFN_vkEnumerateInstanceVersion>(library, get_proc, "vkEnumerateInstanceVersion");
    report.version_entry = enum_version != nullptr;
    if (enum_version) {
        VkResult result = enum_version(&report.loader_version);
        report.loader_version_source = "vkEnumerateInstanceVersion";
        if (result != VK_SUCCESS) { report.vkerror("vkEnumerateInstanceVersion", result); return 1; }
    }
    auto enum_extensions = global<PFN_vkEnumerateInstanceExtensionProperties>(library, get_proc, "vkEnumerateInstanceExtensionProperties");
    auto create_instance = global<PFN_vkCreateInstance>(library, get_proc, "vkCreateInstance");
    if (!enum_extensions || !create_instance) { report.error("global_functions", "required Vulkan global entry is missing"); return 1; }
    std::vector<VkExtensionProperties> extensions;
    if (!enumerate(report, "vkEnumerateInstanceExtensionProperties", [&](uint32_t *count, VkExtensionProperties *items) {
        return enum_extensions(nullptr, count, items);
    }, extensions)) return 1;
    for (const auto &entry : extensions) report.instance_extensions.push_back(extension_json(entry));
    report.surface_extension = has_extension(extensions, VK_OHOS_SURFACE_EXTENSION_NAME);
    std::vector<const char *> enabled;
    if (report.surface_extension && has_extension(extensions, VK_KHR_SURFACE_EXTENSION_NAME)) {
        enabled.push_back(VK_KHR_SURFACE_EXTENSION_NAME);
        enabled.push_back(VK_OHOS_SURFACE_EXTENSION_NAME);
    } else {
        report.warning("ohos_surface", "VK_OHOS_surface and VK_KHR_surface are not both advertised; no surface capability claimed");
    }
    VkApplicationInfo app{};
    app.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO;
    app.pApplicationName = "Harmony Vulkan capability probe";
    app.applicationVersion = 1;
    app.pEngineName = "enumeration-only";
    app.apiVersion = VK_API_VERSION_1_0;
    VkInstanceCreateInfo info{};
    info.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO;
    info.pApplicationInfo = &app;
    info.enabledExtensionCount = static_cast<uint32_t>(enabled.size());
    info.ppEnabledExtensionNames = enabled.empty() ? nullptr : enabled.data();
    VkInstance instance = VK_NULL_HANDLE;
    VkResult result = create_instance(&info, nullptr, &instance);
    if (result != VK_SUCCESS) { report.vkerror("vkCreateInstance", result); return 1; }
    report.instance_created = true;
    report.surface_enabled = !enabled.empty();
    auto destroy_instance = instance_proc<PFN_vkDestroyInstance>(library, get_proc, instance, "vkDestroyInstance");
    struct InstanceGuard { VkInstance handle; PFN_vkDestroyInstance destroy; ~InstanceGuard() { if (destroy) destroy(handle, nullptr); } } instance_guard{instance, destroy_instance};
    auto enum_devices = instance_proc<PFN_vkEnumeratePhysicalDevices>(library, get_proc, instance, "vkEnumeratePhysicalDevices");
    auto get_properties = instance_proc<PFN_vkGetPhysicalDeviceProperties>(library, get_proc, instance, "vkGetPhysicalDeviceProperties");
    auto get_features = instance_proc<PFN_vkGetPhysicalDeviceFeatures>(library, get_proc, instance, "vkGetPhysicalDeviceFeatures");
    auto get_memory = instance_proc<PFN_vkGetPhysicalDeviceMemoryProperties>(library, get_proc, instance, "vkGetPhysicalDeviceMemoryProperties");
    auto get_formats = instance_proc<PFN_vkGetPhysicalDeviceFormatProperties>(library, get_proc, instance, "vkGetPhysicalDeviceFormatProperties");
    auto enum_device_extensions = instance_proc<PFN_vkEnumerateDeviceExtensionProperties>(library, get_proc, instance, "vkEnumerateDeviceExtensionProperties");
    report.surface_entry = report.surface_enabled && get_proc(instance, "vkCreateSurfaceOHOS") != nullptr;
    if (!destroy_instance || !enum_devices || !get_properties || !get_features || !get_memory || !get_formats || !enum_device_extensions) {
        report.error("instance_functions", "one or more mandatory Vulkan 1.0 instance functions are missing");
        return 1;
    }
    std::vector<VkPhysicalDevice> devices;
    if (!enumerate(report, "vkEnumeratePhysicalDevices", [&](uint32_t *count, VkPhysicalDevice *items) {
        return enum_devices(instance, count, items);
    }, devices)) return 1;
    if (devices.empty()) { report.error("vkEnumeratePhysicalDevices", "no physical devices enumerated"); return 1; }
    for (size_t index = 0; index < devices.size(); ++index) {
        VkPhysicalDevice device = devices[index];
        VkPhysicalDeviceProperties properties{};
        VkPhysicalDeviceFeatures features{};
        VkPhysicalDeviceMemoryProperties memory{};
        get_properties(device, &properties);
        get_features(device, &features);
        get_memory(device, &memory);
        std::vector<VkExtensionProperties> device_extensions;
        bool complete = enumerate(report, "vkEnumerateDeviceExtensionProperties:" + number(index), [&](uint32_t *count, VkExtensionProperties *items) {
            return enum_device_extensions(device, nullptr, count, items);
        }, device_extensions);
        std::vector<std::string> extension_items, heaps, types, format_items;
        for (const auto &entry : device_extensions) extension_items.push_back(extension_json(entry));
        if (memory.memoryHeapCount > VK_MAX_MEMORY_HEAPS || memory.memoryTypeCount > VK_MAX_MEMORY_TYPES) {
            report.error("vkGetPhysicalDeviceMemoryProperties", "driver returned memory counts beyond struct capacity");
            memory.memoryHeapCount = std::min(memory.memoryHeapCount, uint32_t(VK_MAX_MEMORY_HEAPS));
            memory.memoryTypeCount = std::min(memory.memoryTypeCount, uint32_t(VK_MAX_MEMORY_TYPES));
        }
        for (uint32_t k = 0; k < memory.memoryHeapCount; ++k) {
            const auto &heap = memory.memoryHeaps[k];
            heaps.push_back("{\"index\":" + number(k) + ",\"size_bytes\":" + number(heap.size)
                + ",\"flags\":" + number(heap.flags) + ",\"device_local\":"
                + boolean((heap.flags & VK_MEMORY_HEAP_DEVICE_LOCAL_BIT) != 0) + "}");
        }
        for (uint32_t k = 0; k < memory.memoryTypeCount; ++k) {
            const auto &type = memory.memoryTypes[k];
            types.push_back("{\"index\":" + number(k) + ",\"heap_index\":" + number(type.heapIndex)
                + ",\"property_flags\":" + number(type.propertyFlags) + "}");
        }
        for (const auto &format : formats) {
            VkFormatProperties fp{};
            get_formats(device, format.value, &fp);
            format_items.push_back("{\"format\":" + quote(format.name) + ",\"value\":" + number(format.value)
                + ",\"linear_tiling_features\":" + number(fp.linearTilingFeatures)
                + ",\"optimal_tiling_features\":" + number(fp.optimalTilingFeatures)
                + ",\"buffer_features\":" + number(fp.bufferFeatures)
                + ",\"optimal_sampled_image\":" + boolean((fp.optimalTilingFeatures & VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT) != 0)
                + ",\"optimal_sampled_linear_filter\":" + boolean((fp.optimalTilingFeatures & VK_FORMAT_FEATURE_SAMPLED_IMAGE_FILTER_LINEAR_BIT) != 0) + "}");
        }
        const auto &limits = properties.limits;
        report.devices.push_back("{\"index\":" + number(index) + ",\"name\":"
            + quote(std::string(properties.deviceName, strnlen(properties.deviceName, VK_MAX_PHYSICAL_DEVICE_NAME_SIZE)))
            + ",\"vendor_id\":" + number(properties.vendorID) + ",\"device_id\":" + number(properties.deviceID)
            + ",\"device_type\":" + number(properties.deviceType) + ",\"api_version\":" + version(properties.apiVersion)
            + ",\"driver_version_raw\":" + number(properties.driverVersion)
            + ",\"core_features\":" + features_json(features) + ",\"device_extensions_complete\":" + boolean(complete)
            + ",\"device_extensions\":" + array(extension_items) + ",\"memory_heaps\":" + array(heaps)
            + ",\"memory_types\":" + array(types) + ",\"limits\":{\"maxImageDimension2D\":" + number(limits.maxImageDimension2D)
            + ",\"maxMemoryAllocationCount\":" + number(limits.maxMemoryAllocationCount)
            + ",\"maxStorageBufferRange\":" + number(limits.maxStorageBufferRange)
            + ",\"maxUniformBufferRange\":" + number(limits.maxUniformBufferRange)
            + ",\"maxComputeSharedMemorySize\":" + number(limits.maxComputeSharedMemorySize)
            + "},\"compressed_formats\":" + array(format_items) + "}");
    }
    return report.errors.empty() ? 0 : 1;
}
} // namespace

extern "C" __attribute__((visibility("default")))
const char *muguet_vulkan_probe_json(void) {
    // The caller must copy before another call on this thread or thread exit.
    // No free is required. A different calling thread owns a different string.
    static thread_local std::string result;
    Report report;
    run(report);
    result = report.json();
    return result.c_str();
}

#if defined(MUGUET_VULKAN_PROBE_STANDALONE)
int main(int argc, char **argv) {
    Report report;
    if (argc == 3 && std::string(argv[1]) == "--loader") report.loader = argv[2];
    else if (argc != 1) {
        report.error("arguments", "usage: vulkan-probe [--loader /path/to/libvulkan.so]");
        std::cout << report.json() << '\n';
        return 2;
    }
    int result = run(report);
    std::cout << report.json() << '\n';
    return result;
}
#endif
