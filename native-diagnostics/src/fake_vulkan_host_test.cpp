// Synthetic loader for host error/report tests; never send this to a device.
#define VK_NO_PROTOTYPES
#include <vulkan/vulkan.h>
#include <cstdlib>
#include <cstring>

namespace {
VkResult enumerate_version(uint32_t *value) { *value = VK_API_VERSION_1_2; return VK_SUCCESS; }
VkResult enumerate_extensions(const char *, uint32_t *count, VkExtensionProperties *items) {
    if (!items) { *count = 2; return VK_SUCCESS; }
    if (*count < 2) return VK_INCOMPLETE;
    std::strcpy(items[0].extensionName, "VK_KHR_surface"); items[0].specVersion = 25;
    std::strcpy(items[1].extensionName, "VK_OHOS_surface"); items[1].specVersion = 1;
    *count = 2; return VK_SUCCESS;
}
VkResult create_instance(const VkInstanceCreateInfo *, const VkAllocationCallbacks *, VkInstance *instance) {
    *instance = reinterpret_cast<VkInstance>(uintptr_t(1)); return VK_SUCCESS;
}
void destroy_instance(VkInstance, const VkAllocationCallbacks *) {}
VkResult enumerate_devices(VkInstance, uint32_t *count, VkPhysicalDevice *items) {
    if (std::getenv("MUGUET_VULKAN_FIXTURE_EMPTY")) { *count = 0; return VK_SUCCESS; }
    if (!items) { *count = 1; return VK_SUCCESS; }
    items[0] = reinterpret_cast<VkPhysicalDevice>(uintptr_t(2)); *count = 1; return VK_SUCCESS;
}
void properties(VkPhysicalDevice, VkPhysicalDeviceProperties *value) {
    *value = {};
    std::strcpy(value->deviceName, "SYNTHETIC HOST FIXTURE \"GPU\"\nno hardware queried");
    value->apiVersion = VK_API_VERSION_1_2;
    value->deviceType = VK_PHYSICAL_DEVICE_TYPE_OTHER;
}
void features(VkPhysicalDevice, VkPhysicalDeviceFeatures *value) {
    *value = {}; value->textureCompressionASTC_LDR = VK_TRUE;
}
void memory(VkPhysicalDevice, VkPhysicalDeviceMemoryProperties *value) {
    *value = {}; value->memoryHeapCount = 1; value->memoryHeaps[0].size = 1 << 20;
    value->memoryHeaps[0].flags = VK_MEMORY_HEAP_DEVICE_LOCAL_BIT;
    value->memoryTypeCount = 1; value->memoryTypes[0].heapIndex = 0;
}
void formats(VkPhysicalDevice, VkFormat format, VkFormatProperties *value) {
    *value = {};
    if (format == VK_FORMAT_ASTC_4x4_UNORM_BLOCK) {
        value->optimalTilingFeatures = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT | VK_FORMAT_FEATURE_SAMPLED_IMAGE_FILTER_LINEAR_BIT;
    }
}
VkResult device_extensions(VkPhysicalDevice, const char *, uint32_t *count, VkExtensionProperties *) {
    *count = 0; return VK_SUCCESS;
}
} // namespace

extern "C" __attribute__((visibility("default")))
PFN_vkVoidFunction vkGetInstanceProcAddr(VkInstance, const char *name) {
#define ENTRY(api, fn) if (std::strcmp(name, #api) == 0) return reinterpret_cast<PFN_vkVoidFunction>(fn);
    ENTRY(vkEnumerateInstanceVersion, enumerate_version)
    ENTRY(vkEnumerateInstanceExtensionProperties, enumerate_extensions)
    ENTRY(vkCreateInstance, create_instance)
    ENTRY(vkDestroyInstance, destroy_instance)
    ENTRY(vkEnumeratePhysicalDevices, enumerate_devices)
    ENTRY(vkGetPhysicalDeviceProperties, properties)
    ENTRY(vkGetPhysicalDeviceFeatures, features)
    ENTRY(vkGetPhysicalDeviceMemoryProperties, memory)
    ENTRY(vkGetPhysicalDeviceFormatProperties, formats)
    ENTRY(vkEnumerateDeviceExtensionProperties, device_extensions)
#undef ENTRY
    return nullptr;
}
