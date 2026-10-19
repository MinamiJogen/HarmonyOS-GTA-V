#define VK_NO_PROTOTYPES
#include <vulkan/vulkan.h>
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Read-only feature query: no logical device, allocations, or GPU commands. */
int main(void) {
    void *loader = dlopen("libvulkan.so", RTLD_NOW | RTLD_LOCAL);
    if (!loader) { fprintf(stderr, "Vulkan loader unavailable\n"); return 1; }
    PFN_vkGetInstanceProcAddr get = (PFN_vkGetInstanceProcAddr)dlsym(loader, "vkGetInstanceProcAddr");
    if (!get) return 2;
    PFN_vkCreateInstance create = (PFN_vkCreateInstance)get(VK_NULL_HANDLE, "vkCreateInstance");
    VkApplicationInfo app = { .sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
        .pApplicationName = "muguet-readonly-feature-query", .apiVersion = VK_API_VERSION_1_1 };
    VkInstanceCreateInfo info = { .sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO, .pApplicationInfo = &app };
    VkInstance instance = VK_NULL_HANDLE;
    VkResult result = create(&info, NULL, &instance);
    if (result != VK_SUCCESS) { fprintf(stderr, "vkCreateInstance=%d\n", result); return 3; }
    PFN_vkEnumeratePhysicalDevices enumerate = (PFN_vkEnumeratePhysicalDevices)get(instance, "vkEnumeratePhysicalDevices");
    PFN_vkGetPhysicalDeviceFeatures2 features = (PFN_vkGetPhysicalDeviceFeatures2)get(instance, "vkGetPhysicalDeviceFeatures2");
    PFN_vkGetPhysicalDeviceProperties properties = (PFN_vkGetPhysicalDeviceProperties)get(instance, "vkGetPhysicalDeviceProperties");
    PFN_vkEnumerateDeviceExtensionProperties extensions = (PFN_vkEnumerateDeviceExtensionProperties)get(instance, "vkEnumerateDeviceExtensionProperties");
    PFN_vkDestroyInstance destroy = (PFN_vkDestroyInstance)get(instance, "vkDestroyInstance");
    if (!enumerate || !features || !properties || !extensions || !destroy) return 4;
    uint32_t count = 0;
    if (enumerate(instance, &count, NULL) != VK_SUCCESS || !count) return 5;
    VkPhysicalDevice *devices = calloc(count, sizeof(*devices));
    if (!devices || enumerate(instance, &count, devices) != VK_SUCCESS) return 6;
    puts("{\"scope\":\"physical feature query only; no logical device or GPU work\",\"devices\":[");
    for (uint32_t n = 0; n < count; ++n) {
        VkPhysicalDeviceProperties prop;
        properties(devices[n], &prop);
        uint32_t extension_count = 0;
        extensions(devices[n], NULL, &extension_count, NULL);
        VkExtensionProperties *ext = calloc(extension_count, sizeof(*ext));
        if (!ext || extensions(devices[n], NULL, &extension_count, ext) != VK_SUCCESS) return 7;
        int has_robustness2 = 0;
        for (uint32_t i = 0; i < extension_count; ++i)
            if (strcmp(ext[i].extensionName, VK_EXT_ROBUSTNESS_2_EXTENSION_NAME) == 0) has_robustness2 = 1;
        free(ext);
        VkPhysicalDeviceRobustness2FeaturesEXT robust = { .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_ROBUSTNESS_2_FEATURES_EXT };
        VkPhysicalDeviceFeatures2 f = { .sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2,
            .pNext = has_robustness2 ? &robust : NULL };
        features(devices[n], &f);
        printf("%s{\"vendorID\":%u,\"deviceID\":%u,\"apiVersion\":%u,\"driverVersion\":%u,"
            "\"robustBufferAccess\":%s,\"robustness2Extension\":%s,\"robustBufferAccess2\":%s,"
            "\"robustImageAccess2\":%s,\"nullDescriptor\":%s,"
            "\"vertexPipelineStoresAndAtomics\":%s,\"fragmentStoresAndAtomics\":%s}",
            n ? "," : "", prop.vendorID, prop.deviceID, prop.apiVersion, prop.driverVersion,
            f.features.robustBufferAccess ? "true":"false", has_robustness2 ? "true":"false",
            robust.robustBufferAccess2 ? "true":"false", robust.robustImageAccess2 ? "true":"false",
            robust.nullDescriptor ? "true":"false", f.features.vertexPipelineStoresAndAtomics ? "true":"false",
            f.features.fragmentStoresAndAtomics ? "true":"false");
    }
    puts("]}");
    free(devices);
    destroy(instance, NULL);
    dlclose(loader);
    return 0;
}
