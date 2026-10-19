// OBSOLETE, NEVER BUILT/EXECUTED. The GENERAL/compute layout assumption below was
// superseded after reading the official graphics-renderpass sample. The actual
// diagnostic is harmony/entry/src/main/cpp/graphics_spatial_diagnostics.cpp.
// Draft for a second, explicitly selected offscreen diagnostic. This translation
// unit is intentionally not in CMake or any shipping binary until the device's
// public XEG capability result has been reviewed.
#include <xengine/xeg_vulkan_spatial_upscale.h>
#include <vulkan/vulkan.h>
#include <algorithm>
#include <chrono>
#include <cstdint>
#include <cstring>
#include <fstream>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace graphics_spatial_probe_draft {
void check(VkResult result, const char* call) {
    if (result != VK_SUCCESS) throw std::runtime_error(std::string(call) + ": " + std::to_string(result));
}
template<class T> T info(VkStructureType type) { T value{}; value.sType = type; return value; }
struct Context {
    // Borrowed from a separate diagnostic context, never the running game.
    VkPhysicalDevice physical = VK_NULL_HANDLE;
    VkDevice device = VK_NULL_HANDLE;
    VkQueue queue = VK_NULL_HANDLE;
    uint32_t family = UINT32_MAX;
    PFN_HMS_XEG_CreateSpatialUpscale create = nullptr;
    PFN_HMS_XEG_CmdRenderSpatialUpscale render = nullptr;
    PFN_HMS_XEG_DestroySpatialUpscale destroy = nullptr;
    bool extensionVerified = false;
};
struct Image {
    VkImage image = VK_NULL_HANDLE;
    VkImageView view = VK_NULL_HANDLE;
    VkDeviceMemory memory = VK_NULL_HANDLE;
};
struct Probe {
    Context ctx;
    Image input, output;
    VkBuffer readback = VK_NULL_HANDLE;
    VkDeviceMemory readbackMemory = VK_NULL_HANDLE;
    VkCommandPool pool = VK_NULL_HANDLE;
    VkCommandBuffer command = VK_NULL_HANDLE;
    VkFence fence = VK_NULL_HANDLE;
    XEG_SpatialUpscale upscale = VK_NULL_HANDLE;
    bool submitted = false, complete = false;
    VkPhysicalDeviceMemoryProperties memories{};
    static constexpr VkDeviceSize prefix = 256, suffix = 256;
    explicit Probe(Context context) : ctx(context) {}
    ~Probe() {
        // On a timeout the process should exit before destroying possibly in-use
        // resources. No retry, queue idle loop, or unsafe destruction is hidden.
        if (submitted && !complete) return;
        if (upscale) ctx.destroy(upscale);
        if (fence) vkDestroyFence(ctx.device, fence, nullptr);
        if (pool) vkDestroyCommandPool(ctx.device, pool, nullptr);
        if (readback) vkDestroyBuffer(ctx.device, readback, nullptr);
        if (readbackMemory) vkFreeMemory(ctx.device, readbackMemory, nullptr);
        for (Image* resource : {&input, &output}) {
            if (resource->view) vkDestroyImageView(ctx.device, resource->view, nullptr);
            if (resource->image) vkDestroyImage(ctx.device, resource->image, nullptr);
            if (resource->memory) vkFreeMemory(ctx.device, resource->memory, nullptr);
        }
    }
    uint32_t memoryType(uint32_t bits, VkMemoryPropertyFlags flags) {
        for (uint32_t i = 0; i < memories.memoryTypeCount; ++i)
            if ((bits & (1u << i)) && (memories.memoryTypes[i].propertyFlags & flags) == flags) return i;
        throw std::runtime_error("No required Vulkan memory type");
    }
    void makeImage(Image& out, VkExtent2D size) {
        auto imageInfo = info<VkImageCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO);
        imageInfo.imageType = VK_IMAGE_TYPE_2D; imageInfo.format = VK_FORMAT_R8G8B8A8_UNORM;
        imageInfo.extent = {size.width, size.height, 1}; imageInfo.mipLevels = 1; imageInfo.arrayLayers = 1;
        imageInfo.samples = VK_SAMPLE_COUNT_1_BIT; imageInfo.tiling = VK_IMAGE_TILING_OPTIMAL;
        imageInfo.usage = VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_STORAGE_BIT |
            VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT;
        imageInfo.sharingMode = VK_SHARING_MODE_EXCLUSIVE; imageInfo.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED;
        check(vkCreateImage(ctx.device, &imageInfo, nullptr, &out.image), "vkCreateImage");
        VkMemoryRequirements requirements{}; vkGetImageMemoryRequirements(ctx.device, out.image, &requirements);
        auto allocation = info<VkMemoryAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO);
        allocation.allocationSize = requirements.size;
        allocation.memoryTypeIndex = memoryType(requirements.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
        check(vkAllocateMemory(ctx.device, &allocation, nullptr, &out.memory), "vkAllocateMemory image");
        check(vkBindImageMemory(ctx.device, out.image, out.memory, 0), "vkBindImageMemory");
        auto viewInfo = info<VkImageViewCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO);
        viewInfo.image = out.image; viewInfo.viewType = VK_IMAGE_VIEW_TYPE_2D; viewInfo.format = imageInfo.format;
        viewInfo.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
        check(vkCreateImageView(ctx.device, &viewInfo, nullptr, &out.view), "vkCreateImageView");
    }
    void barrier(VkImage image, VkImageLayout from, VkImageLayout to, VkAccessFlags source,
                 VkAccessFlags destination, VkPipelineStageFlags sourceStage, VkPipelineStageFlags destinationStage) {
        auto value = info<VkImageMemoryBarrier>(VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER);
        value.srcAccessMask = source; value.dstAccessMask = destination; value.oldLayout = from; value.newLayout = to;
        value.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; value.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        value.image = image; value.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
        vkCmdPipelineBarrier(command, sourceStage, destinationStage, 0, 0, nullptr, 0, nullptr, 1, &value);
    }
    std::string run(const std::string& directory, VkExtent2D source, VkExtent2D destination) {
        if (!ctx.extensionVerified || !ctx.create || !ctx.render || !ctx.destroy)
            throw std::runtime_error("Verified public XEG_spatial_upscale plus all symbols are required");
        if (!source.width || !source.height || destination.width < source.width || destination.height < source.height ||
            destination.width > 4096 || destination.height > 4096) throw std::runtime_error("Invalid offscreen fixture dimensions");
        VkFormatProperties format{}; vkGetPhysicalDeviceFormatProperties(ctx.physical, VK_FORMAT_R8G8B8A8_UNORM, &format);
        const auto required = VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT | VK_FORMAT_FEATURE_STORAGE_IMAGE_BIT;
        if ((format.optimalTilingFeatures & required) != required) throw std::runtime_error("RGBA8 sampled/storage image unsupported");
        vkGetPhysicalDeviceMemoryProperties(ctx.physical, &memories);
        makeImage(input, source); makeImage(output, destination);
        const VkDeviceSize bytes = VkDeviceSize(destination.width) * destination.height * 4;
        auto buffer = info<VkBufferCreateInfo>(VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO);
        buffer.size = prefix + bytes + suffix; buffer.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT;
        buffer.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        check(vkCreateBuffer(ctx.device, &buffer, nullptr, &readback), "vkCreateBuffer");
        VkMemoryRequirements requirements{}; vkGetBufferMemoryRequirements(ctx.device, readback, &requirements);
        auto allocation = info<VkMemoryAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO);
        allocation.allocationSize = requirements.size;
        allocation.memoryTypeIndex = memoryType(requirements.memoryTypeBits,
            VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
        check(vkAllocateMemory(ctx.device, &allocation, nullptr, &readbackMemory), "vkAllocateMemory readback");
        check(vkBindBufferMemory(ctx.device, readback, readbackMemory, 0), "vkBindBufferMemory");
        void* mapped = nullptr; check(vkMapMemory(ctx.device, readbackMemory, 0, buffer.size, 0, &mapped), "vkMapMemory prefill");
        std::memset(mapped, 0xa5, size_t(buffer.size)); vkUnmapMemory(ctx.device, readbackMemory);
        XEG_SpatialUpscaleCreateInfo createInfo{};
        createInfo.inputSize = source; createInfo.inputRegion = {{0, 0}, source};
        createInfo.outputSize = destination; createInfo.outputRegion = {{0, 0}, destination};
        createInfo.format = VK_FORMAT_R8G8B8A8_UNORM; createInfo.sharpness = 0.3f;
        check(ctx.create(ctx.device, &createInfo, &upscale), "HMS_XEG_CreateSpatialUpscale");
        if (!upscale) throw std::runtime_error("Spatial create returned success with null handle");
        auto poolInfo = info<VkCommandPoolCreateInfo>(VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO);
        poolInfo.queueFamilyIndex = ctx.family;
        check(vkCreateCommandPool(ctx.device, &poolInfo, nullptr, &pool), "vkCreateCommandPool");
        auto commandInfo = info<VkCommandBufferAllocateInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO);
        commandInfo.commandPool = pool; commandInfo.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY; commandInfo.commandBufferCount = 1;
        check(vkAllocateCommandBuffers(ctx.device, &commandInfo, &command), "vkAllocateCommandBuffers");
        auto fenceInfo = info<VkFenceCreateInfo>(VK_STRUCTURE_TYPE_FENCE_CREATE_INFO);
        check(vkCreateFence(ctx.device, &fenceInfo, nullptr, &fence), "vkCreateFence");
        auto begin = info<VkCommandBufferBeginInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO);
        begin.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
        check(vkBeginCommandBuffer(command, &begin), "vkBeginCommandBuffer");
        VkImageSubresourceRange range{VK_IMAGE_ASPECT_COLOR_BIT, 0, 1, 0, 1};
        barrier(input.image, VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 0,
            VK_ACCESS_TRANSFER_WRITE_BIT, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT);
        VkClearColorValue constant{}; constant.float32[0] = 0.25f; constant.float32[1] = 0.5f;
        constant.float32[2] = 0.75f; constant.float32[3] = 1.0f;
        vkCmdClearColorImage(command, input.image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, &constant, 1, &range);
        barrier(input.image, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_IMAGE_LAYOUT_GENERAL, VK_ACCESS_TRANSFER_WRITE_BIT,
            VK_ACCESS_SHADER_READ_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT);
        barrier(output.image, VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_GENERAL, 0, VK_ACCESS_SHADER_WRITE_BIT,
            VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT);
        XEG_SpatialUpscaleDescription description{input.view, output.view};
        ctx.render(command, upscale, &description);
        barrier(output.image, VK_IMAGE_LAYOUT_GENERAL, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, VK_ACCESS_SHADER_WRITE_BIT,
            VK_ACCESS_TRANSFER_READ_BIT, VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT);
        VkBufferImageCopy copy{}; copy.bufferOffset = prefix;
        copy.imageSubresource = {VK_IMAGE_ASPECT_COLOR_BIT, 0, 0, 1}; copy.imageExtent = {destination.width, destination.height, 1};
        vkCmdCopyImageToBuffer(command, output.image, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL, readback, 1, &copy);
        auto hostBarrier = info<VkBufferMemoryBarrier>(VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER);
        hostBarrier.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT; hostBarrier.dstAccessMask = VK_ACCESS_HOST_READ_BIT;
        hostBarrier.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED; hostBarrier.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED;
        hostBarrier.buffer = readback; hostBarrier.offset = 0; hostBarrier.size = buffer.size;
        vkCmdPipelineBarrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT,
            0, 0, nullptr, 1, &hostBarrier, 0, nullptr);
        check(vkEndCommandBuffer(command), "vkEndCommandBuffer");
        auto submitInfo = info<VkSubmitInfo>(VK_STRUCTURE_TYPE_SUBMIT_INFO);
        submitInfo.commandBufferCount = 1; submitInfo.pCommandBuffers = &command;
        const auto started = std::chrono::steady_clock::now();
        check(vkQueueSubmit(ctx.queue, 1, &submitInfo, fence), "vkQueueSubmit"); submitted = true;
        check(vkWaitForFences(ctx.device, 1, &fence, VK_TRUE, 10'000'000'000ULL), "vkWaitForFences"); complete = true;
        const double milliseconds = std::chrono::duration<double, std::milli>(std::chrono::steady_clock::now() - started).count();
        check(vkMapMemory(ctx.device, readbackMemory, 0, buffer.size, 0, &mapped), "vkMapMemory result");
        auto data = static_cast<const uint8_t*>(mapped);
        bool guards = true; for (VkDeviceSize i = 0; i < prefix; ++i) guards &= data[i] == 0xa5;
        for (VkDeviceSize i = prefix + bytes; i < buffer.size; ++i) guards &= data[i] == 0xa5;
        uint64_t bad = 0; const int expected[4] = {64, 128, 191, 255};
        for (VkDeviceSize i = 0; i < bytes; ++i) if (std::abs(int(data[prefix + i]) - expected[i % 4]) > 3) ++bad;
        vkUnmapMemory(ctx.device, readbackMemory);
        std::ostringstream result;
        result << "{\"status\":\"" << (guards && !bad ? "pass" : "fail") << "\",\"fixture\":\"constant_rgba8\",\"inputSize\":["
            << source.width << ',' << source.height << "],\"outputSize\":[" << destination.width << ',' << destination.height
            << "],\"offscreen\":true,\"productionEnabled\":false,\"outputVerified\":" << (guards && !bad ? "true" : "false")
            << ",\"guardBytesVerified\":" << (guards ? "true" : "false") << ",\"badChannelValues\":" << bad
            << ",\"submitToFenceWallMs\":" << milliseconds << ",\"timingScope\":\"initial_clear_plus_upscale_plus_readback; not game FPS or isolated GPU timestamp\"}";
        return result.str();
    }
};
} // namespace graphics_spatial_probe_draft
