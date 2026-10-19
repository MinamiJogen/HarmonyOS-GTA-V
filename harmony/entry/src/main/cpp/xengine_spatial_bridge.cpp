#include "xengine_spatial_bridge.h"
#include <xengine/xeg_vulkan_extension.h>
#include <xengine/xeg_vulkan_spatial_upscale.h>
#include <syscap_ndk.h>
#include <vulkan/vulkan.h>
#include <dlfcn.h>
#include <cmath>
#include <cstdio>
#include <cstring>
#include <memory>
#include <mutex>
#include <new>
#include <stdexcept>
#include <string>
#include <type_traits>
#include <vector>

namespace {
thread_local std::string lastError;
template<class T> T rawHandle(uint64_t value) {
    if constexpr (std::is_pointer_v<T>) return reinterpret_cast<T>(static_cast<uintptr_t>(value));
    else return static_cast<T>(value);
}
void fail(const char* message) { lastError = message; }
void failedResult(const char* call, VkResult result) {
    char value[192]{}; std::snprintf(value, sizeof(value), "%s returned VkResult %d", call, int(result)); lastError = value;
}
void* publicLibrary() {
    // The public SDK may retain device-lifetime implementation state beyond an
    // individual SpatialUpscale object. Keep its code mapping alive through all
    // borrowed VkDevice teardowns, as a directly linked SDK would be. This is
    // only a library reference: every created spatial object is still destroyed.
    // A failed dlopen throws before call_once commits, allowing a later retry.
    static std::once_flag once;
    static void* processLibrary = nullptr;
    std::call_once(once, [] {
        void* loaded = dlopen("libxengine.so", RTLD_NOW | RTLD_LOCAL);
        if (!loaded) {
            const char* error = dlerror();
            throw std::runtime_error(error ? error : "libxengine.so unavailable");
        }
        processLibrary = loaded;
    });
    return processLibrary;
}
struct Context {
    VkDevice device = VK_NULL_HANDLE;
    XEG_SpatialUpscale spatial = VK_NULL_HANDLE;
    PFN_HMS_XEG_CmdRenderSpatialUpscale render = nullptr;
    PFN_HMS_XEG_DestroySpatialUpscale destroy = nullptr;
    ~Context() {
        if (spatial && destroy) destroy(spatial);
    }
};
}

extern "C" void* muguet_xeg_spatial_create(uint64_t physicalDevice, uint64_t device,
    uint32_t inputWidth, uint32_t inputHeight, uint32_t outputWidth, uint32_t outputHeight, float sharpness) {
    lastError.clear();
    if (!physicalDevice || !device || !inputWidth || !inputHeight ||
        outputWidth < inputWidth || outputHeight < inputHeight ||
        !std::isfinite(sharpness) || sharpness < 0.0f || sharpness > 1.0f) {
        fail("Invalid spatial-upscale handles, dimensions or sharpness"); return nullptr;
    }
    if (!canIUse("SystemCapability.Graphic.XEngine")) { fail("XEngine Syscap unavailable"); return nullptr; }
    try {
        auto context = std::make_unique<Context>(); context->device = rawHandle<VkDevice>(device);
        const auto physical = rawHandle<VkPhysicalDevice>(physicalDevice);
        VkPhysicalDeviceProperties properties{}; vkGetPhysicalDeviceProperties(physical, &properties);
        const uint32_t limit = properties.limits.maxImageDimension2D;
        if (inputWidth > limit || inputHeight > limit || outputWidth > limit || outputHeight > limit) {
            fail("Spatial image dimensions exceed the actual Vulkan device limit"); return nullptr;
        }
        VkFormatProperties format{}; vkGetPhysicalDeviceFormatProperties(physical, VK_FORMAT_R8G8B8A8_UNORM, &format);
        constexpr VkFormatFeatureFlags required = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT |
            VK_FORMAT_FEATURE_SAMPLED_IMAGE_FILTER_LINEAR_BIT | VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT;
        if ((format.optimalTilingFeatures & required) != required) {
            fail("RGBA8_UNORM sampled/linear/color-attachment support unavailable"); return nullptr;
        }
        void* library = publicLibrary();
        auto enumerate = reinterpret_cast<PFN_HMS_XEG_EnumerateDeviceExtensionProperties>(dlsym(library, "HMS_XEG_EnumerateDeviceExtensionProperties"));
        auto create = reinterpret_cast<PFN_HMS_XEG_CreateSpatialUpscale>(dlsym(library, "HMS_XEG_CreateSpatialUpscale"));
        context->render = reinterpret_cast<PFN_HMS_XEG_CmdRenderSpatialUpscale>(dlsym(library, "HMS_XEG_CmdRenderSpatialUpscale"));
        context->destroy = reinterpret_cast<PFN_HMS_XEG_DestroySpatialUpscale>(dlsym(library, "HMS_XEG_DestroySpatialUpscale"));
        if (!enumerate || !create || !context->render || !context->destroy) { fail("Required public XEngine symbol unavailable"); return nullptr; }
        bool complete = false, supported = false;
        for (unsigned attempt = 0; attempt < 4; ++attempt) {
            uint32_t count = 0; auto result = enumerate(physical, &count, nullptr);
            if (result != VK_SUCCESS && result != VK_INCOMPLETE) { failedResult("HMS_XEG_EnumerateDeviceExtensionProperties count", result); return nullptr; }
            if (count > 4096) { fail("Unreasonable XEngine extension count"); return nullptr; }
            std::vector<XEG_ExtensionProperties> extensions(count); const uint32_t capacity = count;
            result = enumerate(physical, &count, extensions.empty() ? nullptr : extensions.data());
            if (count > capacity) { fail("XEngine extension count exceeded supplied capacity"); return nullptr; }
            if (result == VK_INCOMPLETE) continue;
            if (result != VK_SUCCESS) { failedResult("HMS_XEG_EnumerateDeviceExtensionProperties list", result); return nullptr; }
            for (uint32_t i = 0; i < count; ++i) {
                const auto& name = extensions[i].extensionName;
                if (std::string(name, strnlen(name, XEG_MAX_EXTENSION_NAME_SIZE)) == XEG_SPATIAL_UPSCALE_EXTENSION_NAME) supported = true;
            }
            complete = true; break;
        }
        if (!complete || !supported) { fail("Complete public extension list does not advertise XEG_spatial_upscale"); return nullptr; }
        XEG_SpatialUpscaleCreateInfo parameters{};
        parameters.inputSize = {inputWidth, inputHeight}; parameters.inputRegion = {{0, 0}, parameters.inputSize};
        parameters.outputSize = {outputWidth, outputHeight}; parameters.outputRegion = {{0, 0}, parameters.outputSize};
        parameters.format = VK_FORMAT_R8G8B8A8_UNORM; parameters.sharpness = sharpness;
        const auto result = create(context->device, &parameters, &context->spatial);
        if (result != VK_SUCCESS) { failedResult("HMS_XEG_CreateSpatialUpscale", result); return nullptr; }
        if (!context->spatial) { fail("HMS_XEG_CreateSpatialUpscale returned a null handle"); return nullptr; }
        return context.release();
    } catch (const std::exception& error) { lastError = error.what(); return nullptr; }
}

extern "C" bool muguet_xeg_spatial_record(void* opaque, uint64_t commandBuffer,
    uint64_t inputView, uint64_t outputView, uint64_t outputImage) {
    lastError.clear();
    if (!opaque || !commandBuffer || !inputView || !outputView || !outputImage || inputView == outputView) {
        fail("Invalid or aliased borrowed spatial-upscale record handles"); return false;
    }
    auto& context = *static_cast<Context*>(opaque);
    XEG_SpatialUpscaleDescription description{rawHandle<VkImageView>(inputView), rawHandle<VkImageView>(outputView)};
    const auto command = rawHandle<VkCommandBuffer>(commandBuffer);
    // Establish the same real shader-read layout before and after XEG's opaque
    // pass, so a validation layer that cannot intercept XEG's internal commands
    // still sees a truthful state. The SDK may discard the previous clear (as
    // its official sample starts with a new image); no prior contents are used.
    VkImageMemoryBarrier before{}; before.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    before.srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
    before.dstAccessMask = VK_ACCESS_SHADER_READ_BIT;
    before.oldLayout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
    before.newLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
    before.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; before.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    before.image = rawHandle<VkImage>(outputImage); before.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
        0, 0, nullptr, 0, nullptr, 1, &before);
    context.render(command, context.spatial, &description);
    // Keep wgpu's existing output tracker state COLOR_TARGET truthful. The
    // external XEG render pass ended in SHADER_READ_ONLY_OPTIMAL (official
    // sample and on-device six-fixture readback contract). Its complete image
    // write is synchronized before restoring COLOR_ATTACHMENT_OPTIMAL; wgpu's
    // next normal use then emits COLOR_TARGET -> RESOURCE itself.
    VkImageMemoryBarrier barrier{}; barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
    barrier.srcAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT | VK_ACCESS_SHADER_WRITE_BIT;
    barrier.dstAccessMask = VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
    barrier.oldLayout = VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL; barrier.newLayout = VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL;
    barrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; barrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
    barrier.image = rawHandle<VkImage>(outputImage); barrier.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
    vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,
        0, 0, nullptr, 0, nullptr, 1, &barrier);
    return true;
}

extern "C" void muguet_xeg_spatial_destroy(void* context) {
    lastError.clear(); delete static_cast<Context*>(context);
}
extern "C" const char* muguet_xeg_spatial_last_error(void) { return lastError.c_str(); }
