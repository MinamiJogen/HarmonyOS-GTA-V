#include "muguet_vulkan_buffer_probe.h"
#include <vulkan/vulkan.h>
#include <algorithm>
#include <atomic>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <mutex>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
constexpr VkDeviceSize kUpload = 3240, kStaging = 4096, kGuard = 65536, kAdjacent = 64;
constexpr uint64_t kWaitNs = 30ULL * 1000 * 1000 * 1000;
constexpr const char *kNegativeVuid = "VUID-vkSubmitDebugUtilsMessageEXT-messageSeverity-parameter";
template<class T> T typed(VkStructureType type) { T result{}; result.sType = type; return result; }
VkDeviceSize alignUp(VkDeviceSize n, VkDeviceSize alignment) { return (n + alignment - 1) / alignment * alignment; }
std::string quote(const std::string &s) {
    std::ostringstream out; out << '"';
    for (unsigned char c : s) {
        if (c == '"' || c == '\\') out << '\\' << char(c);
        else if (c < 32) out << "\\u" << std::hex << std::setw(4) << std::setfill('0') << unsigned(c) << std::dec;
        else out << char(c);
    }
    out << '"'; return out.str();
}
const char *allocationName(uint32_t n) { return n == 0 ? "suballocated" : n == 1 ? "standalone" : "dedicated"; }
const char *uploadName(uint32_t n) { return n == 0 ? "vkCmdCopyBuffer" : "vkCmdUpdateBuffer"; }
const char *poolName(uint32_t n) { return n == 0 ? "reset_reuse" : "destroy_recreate"; }
template<class T> std::string handleText(T handle) { std::ostringstream out; out << handle; return out.str(); }
void check(VkResult result, const char *call) {
    if (result != VK_SUCCESS) throw std::runtime_error(std::string(call) + " returned VkResult " + std::to_string(result));
}

struct Buffer {
    VkBuffer handle = VK_NULL_HANDLE;
    VkMemoryRequirements requirements{};
    VkDeviceMemory memory = VK_NULL_HANDLE;
    VkDeviceSize binding = 0;
    uint32_t type = UINT32_MAX;
    bool requiresDedicated = false;
    bool prefersDedicated = false;
};

struct Probe {
    MuguetVulkanBufferProbeConfig cfg{};
    std::string tracePath, phase = "created", error, deviceName;
    VkInstance instance = VK_NULL_HANDLE;
    VkDebugUtilsMessengerEXT messenger = VK_NULL_HANDLE;
    PFN_vkDestroyDebugUtilsMessengerEXT destroyMessenger = nullptr;
    bool debugUtilsAvailable = false, negativeIssued = false;
    std::string negativeError, negativeObservedMessage;
    std::atomic<bool> negativeObserved{false};
    std::atomic<uint32_t> unexpectedValidationErrors{0};
    mutable std::mutex validationMutex;
    std::vector<std::string> validationMessages;
    VkPhysicalDevice physical = VK_NULL_HANDLE;
    VkDevice device = VK_NULL_HANDLE;
    VkQueue queue = VK_NULL_HANDLE;
    uint32_t family = UINT32_MAX, apiVersion = 0;
    VkPhysicalDeviceMemoryProperties properties{};
    VkPhysicalDeviceProperties physicalProperties{};
    VkPhysicalDeviceFeatures physicalFeatures{};
    std::vector<std::string> deviceExtensions;
    bool physicalInfoAvailable = false, deviceExtensionsComplete = false;
    std::string metadataError;
    VkCommandPool pool = VK_NULL_HANDLE;
    VkCommandBuffer activeCommand = VK_NULL_HANDLE;
    VkFence fence = VK_NULL_HANDLE;
    std::vector<VkCommandBuffer> commands;
    std::vector<VkBuffer> buffers;
    std::vector<VkDeviceMemory> memories, mapped;
    Buffer target, staging, readback, prefix, suffix;
    uint8_t *stagingCpu = nullptr, *readbackCpu = nullptr;
    uint32_t iteration = 0, completed = 0, resetCount = 0, recreateCount = 0, poolGeneration = 0, submissions = 0;
    uint64_t verifiedBytes = 0, targetHash = 0;
    bool safeToDestroy = true, verified = false, targetBindingAdjusted = false;

    explicit Probe(const MuguetVulkanBufferProbeConfig &config, const char *path) : cfg(config), tracePath(path ? path : "") {}
    std::string additionalInfo() const {
        if (!physicalInfoAvailable) return "{\"available\":false}";
        const auto &p = physicalProperties;
        const auto &f = physicalFeatures;
        const auto &l = p.limits;
        std::ostringstream out;
        out << "{\"available\":true,\"physicalDeviceIndex\":" << cfg.physical_device
            << ",\"deviceName\":" << quote(p.deviceName) << ",\"apiVersion\":" << p.apiVersion
            << ",\"driverVersion\":" << p.driverVersion << ",\"vendorID\":" << p.vendorID
            << ",\"deviceID\":" << p.deviceID << ",\"deviceType\":" << p.deviceType
            << ",\"features\":{\"robustBufferAccess\":" << (f.robustBufferAccess ? "true" : "false");
#define FEATURE(name) out << ",\"" #name "\":" << (f.name ? "true" : "false")
        FEATURE(fullDrawIndexUint32); FEATURE(imageCubeArray); FEATURE(independentBlend); FEATURE(geometryShader);
        FEATURE(tessellationShader); FEATURE(sampleRateShading); FEATURE(dualSrcBlend); FEATURE(logicOp);
        FEATURE(multiDrawIndirect); FEATURE(drawIndirectFirstInstance); FEATURE(depthClamp); FEATURE(depthBiasClamp);
        FEATURE(fillModeNonSolid); FEATURE(depthBounds); FEATURE(wideLines); FEATURE(largePoints); FEATURE(alphaToOne);
        FEATURE(multiViewport); FEATURE(samplerAnisotropy); FEATURE(textureCompressionETC2); FEATURE(textureCompressionASTC_LDR);
        FEATURE(textureCompressionBC); FEATURE(occlusionQueryPrecise); FEATURE(pipelineStatisticsQuery);
        FEATURE(vertexPipelineStoresAndAtomics); FEATURE(fragmentStoresAndAtomics); FEATURE(shaderTessellationAndGeometryPointSize);
        FEATURE(shaderImageGatherExtended); FEATURE(shaderStorageImageExtendedFormats); FEATURE(shaderStorageImageMultisample);
        FEATURE(shaderStorageImageReadWithoutFormat); FEATURE(shaderStorageImageWriteWithoutFormat);
        FEATURE(shaderUniformBufferArrayDynamicIndexing); FEATURE(shaderSampledImageArrayDynamicIndexing);
        FEATURE(shaderStorageBufferArrayDynamicIndexing); FEATURE(shaderStorageImageArrayDynamicIndexing);
        FEATURE(shaderClipDistance); FEATURE(shaderCullDistance); FEATURE(shaderFloat64); FEATURE(shaderInt64); FEATURE(shaderInt16);
        FEATURE(shaderResourceResidency); FEATURE(shaderResourceMinLod); FEATURE(sparseBinding); FEATURE(sparseResidencyBuffer);
        FEATURE(sparseResidencyImage2D); FEATURE(sparseResidencyImage3D); FEATURE(sparseResidency2Samples); FEATURE(sparseResidency4Samples);
        FEATURE(sparseResidency8Samples); FEATURE(sparseResidency16Samples); FEATURE(sparseResidencyAliased);
        FEATURE(variableMultisampleRate); FEATURE(inheritedQueries);
#undef FEATURE
        out << "},\"limits\":{\"maxUniformBufferRange\":" << l.maxUniformBufferRange;
#define LIMIT(name) out << ",\"" #name "\":" << l.name
        LIMIT(minUniformBufferOffsetAlignment); LIMIT(maxStorageBufferRange); LIMIT(minStorageBufferOffsetAlignment);
        LIMIT(maxComputeWorkGroupInvocations); LIMIT(maxComputeSharedMemorySize); LIMIT(maxBoundDescriptorSets);
        LIMIT(maxPerStageDescriptorSamplers); LIMIT(maxPerStageDescriptorUniformBuffers); LIMIT(maxPerStageDescriptorStorageBuffers);
        LIMIT(maxPerStageDescriptorSampledImages); LIMIT(maxPerStageDescriptorStorageImages); LIMIT(maxPerStageResources);
        LIMIT(maxDescriptorSetSamplers); LIMIT(maxDescriptorSetUniformBuffers); LIMIT(maxDescriptorSetUniformBuffersDynamic);
        LIMIT(maxDescriptorSetStorageBuffers); LIMIT(maxDescriptorSetStorageBuffersDynamic); LIMIT(maxDescriptorSetSampledImages);
        LIMIT(maxDescriptorSetStorageImages); LIMIT(maxDescriptorSetInputAttachments); LIMIT(maxVertexInputAttributes);
        LIMIT(maxVertexInputBindings); LIMIT(maxVertexInputAttributeOffset); LIMIT(maxVertexInputBindingStride);
        LIMIT(maxVertexOutputComponents); LIMIT(maxFragmentInputComponents); LIMIT(maxFragmentOutputAttachments);
        LIMIT(maxColorAttachments); LIMIT(maxImageDimension2D); LIMIT(maxMemoryAllocationCount);
        LIMIT(nonCoherentAtomSize); LIMIT(bufferImageGranularity); LIMIT(maxPushConstantsSize);
#undef LIMIT
        out << ",\"maxComputeWorkGroupSize\":[" << l.maxComputeWorkGroupSize[0] << ',' << l.maxComputeWorkGroupSize[1] << ',' << l.maxComputeWorkGroupSize[2]
            << "],\"maxComputeWorkGroupCount\":[" << l.maxComputeWorkGroupCount[0] << ',' << l.maxComputeWorkGroupCount[1] << ',' << l.maxComputeWorkGroupCount[2]
            << "]},\"deviceExtensionsComplete\":" << (deviceExtensionsComplete ? "true" : "false") << ",\"deviceExtensions\":[";
        for (size_t i = 0; i < deviceExtensions.size(); ++i) { if (i) out << ','; out << quote(deviceExtensions[i]); }
        out << "],\"memoryHeaps\":[";
        for (uint32_t i = 0; i < std::min(properties.memoryHeapCount, uint32_t(VK_MAX_MEMORY_HEAPS)); ++i) {
            if (i) out << ',';
            out << "{\"index\":" << i << ",\"sizeBytes\":" << properties.memoryHeaps[i].size << ",\"flags\":" << properties.memoryHeaps[i].flags << '}';
        }
        out << "],\"memoryTypes\":[";
        for (uint32_t i = 0; i < std::min(properties.memoryTypeCount, uint32_t(VK_MAX_MEMORY_TYPES)); ++i) {
            if (i) out << ',';
            out << "{\"index\":" << i << ",\"heapIndex\":" << properties.memoryTypes[i].heapIndex << ",\"propertyFlags\":" << properties.memoryTypes[i].propertyFlags << '}';
        }
        out << "],\"metadataError\":" << quote(metadataError) << '}'; return out.str();
    }
    void queryAdditionalInfo() {
        vkGetPhysicalDeviceProperties(physical, &physicalProperties);
        vkGetPhysicalDeviceFeatures(physical, &physicalFeatures);
        // Only Vulkan 1.0 core queries. No extension structs or feature enabling.
        for (unsigned attempt = 0; attempt < 5; ++attempt) {
            uint32_t count = 0;
            auto result = vkEnumerateDeviceExtensionProperties(physical, nullptr, &count, nullptr);
            if (result != VK_SUCCESS && result != VK_INCOMPLETE) { metadataError = "Device extension count query failed: " + std::to_string(result); break; }
            if (count > (1u << 20)) { metadataError = "Unreasonable device extension count."; break; }
            std::vector<VkExtensionProperties> extensions(count); uint32_t capacity = count;
            result = vkEnumerateDeviceExtensionProperties(physical, nullptr, &count, extensions.data());
            if (count > capacity) { metadataError = "Device extension count exceeded supplied capacity."; break; }
            if (result == VK_INCOMPLETE) continue;
            if (result != VK_SUCCESS) { metadataError = "Device extension query failed: " + std::to_string(result); break; }
            for (uint32_t i = 0; i < count; ++i) deviceExtensions.emplace_back(extensions[i].extensionName);
            deviceExtensionsComplete = true; break;
        }
        if (!deviceExtensionsComplete && metadataError.empty()) metadataError = "Device extension enumeration remained incomplete.";
        physicalInfoAvailable = true;
    }
    ~Probe() {
        // A timed-out submission may still access every resource. Do not destroy
        // live resources; this standalone diagnostic process must then exit.
        if (!safeToDestroy) return;
        if (device) {
            for (auto memory : mapped) vkUnmapMemory(device, memory);
            if (pool) vkDestroyCommandPool(device, pool, nullptr);
            if (fence) vkDestroyFence(device, fence, nullptr);
            for (auto buffer : buffers) vkDestroyBuffer(device, buffer, nullptr);
            for (auto memory : memories) vkFreeMemory(device, memory, nullptr);
            vkDestroyDevice(device, nullptr);
        }
        if (messenger && destroyMessenger) destroyMessenger(instance, messenger, nullptr);
        if (instance) vkDestroyInstance(instance, nullptr);
    }
    std::string json() const {
        const bool controlSatisfied = !cfg.validation_negative_control || negativeObserved.load();
        std::ostringstream out;
        out << "{\"probe\":\"vulkan_buffer_upload_readback\",\"status\":" << quote(error.empty() && verified && controlSatisfied && unexpectedValidationErrors.load() == 0 ? "pass" : "fail")
            << ",\"scope\":\"Standalone Vulkan buffer commands; no game, shader, surface or wgpu\""
            << ",\"allocation\":" << quote(allocationName(cfg.allocation)) << ",\"upload\":" << quote(uploadName(cfg.upload))
            << ",\"poolPolicy\":" << quote(poolName(cfg.pool_policy)) << ",\"device\":" << quote(deviceName)
            << ",\"deviceApiVersion\":" << apiVersion << ",\"queueFamily\":" << family
            << ",\"additionalInfo\":" << additionalInfo()
            << ",\"phase\":" << quote(phase) << ",\"iteration\":" << iteration << ",\"completedIterations\":" << completed
            << ",\"requestedIterations\":" << cfg.iterations << ",\"commandsPerPool\":" << cfg.commands_per_pool
            << ",\"resetEvery\":" << cfg.reset_every << ",\"poolResetCount\":" << resetCount
            << ",\"poolRecreateCount\":" << recreateCount << ",\"targetBytes\":" << cfg.target_bytes
            << ",\"poolGeneration\":" << poolGeneration << ",\"completedSubmissions\":" << submissions
            << ",\"pool\":" << quote(handleText(pool)) << ",\"commandBuffer\":" << quote(handleText(activeCommand))
            << ",\"targetBuffer\":" << quote(handleText(target.handle)) << ",\"stagingBuffer\":" << quote(handleText(staging.handle))
            << ",\"targetMemory\":" << quote(handleText(target.memory)) << ",\"stagingMemory\":" << quote(handleText(staging.memory))
            << ",\"targetUsage\":" << cfg.target_usage << ",\"destinationOffset\":" << cfg.destination_offset
            << ",\"uploadBytes\":" << kUpload << ",\"stagingBytes\":" << kStaging
            << ",\"targetBindingOffset\":" << target.binding << ",\"requestedTargetBindingOffset\":" << cfg.target_binding_offset
            << ",\"targetBindingAdjusted\":" << (targetBindingAdjusted ? "true" : "false")
            << ",\"prefixBindingOffset\":" << prefix.binding << ",\"suffixBindingOffset\":" << suffix.binding
            << ",\"stagingBindingOffset\":" << staging.binding << ",\"targetMemoryType\":" << target.type
            << ",\"requestedStagingBindingOffset\":" << cfg.staging_binding_offset
            << ",\"stagingMemoryType\":" << staging.type << ",\"targetRequirementSize\":" << target.requirements.size
            << ",\"targetMemoryFlags\":" << (target.type < properties.memoryTypeCount ? properties.memoryTypes[target.type].propertyFlags : 0)
            << ",\"stagingMemoryFlags\":" << (staging.type < properties.memoryTypeCount ? properties.memoryTypes[staging.type].propertyFlags : 0)
            << ",\"targetRequirementAlignment\":" << target.requirements.alignment
            << ",\"targetRequiresDedicated\":" << (target.requiresDedicated ? "true" : "false")
            << ",\"targetPrefersDedicated\":" << (target.prefersDedicated ? "true" : "false")
            << ",\"verifiedBytes\":" << verifiedBytes << ",\"targetFnv1a64\":" << quote(std::to_string(targetHash))
            << ",\"adjacentSentinelBytesEachSide\":" << kAdjacent
            << ",\"neighborBufferSentinelBytesEach\":" << kGuard
            << ",\"fullTargetAndNeighborsVerified\":" << (verified ? "true" : "false")
            << ",\"validationNegativeControlRequested\":" << (cfg.validation_negative_control ? "true" : "false")
            << ",\"debugUtilsAvailable\":" << (debugUtilsAvailable ? "true" : "false")
            << ",\"validationNegativeControlIssued\":" << (negativeIssued ? "true" : "false")
            << ",\"validationNegativeControlObserved\":" << (negativeObserved.load() ? "true" : "false")
            << ",\"validationNegativeControlExpectedVuid\":" << quote(kNegativeVuid)
            << ",\"validationNegativeControlError\":" << quote(negativeError)
            << ",\"unexpectedValidationErrors\":" << unexpectedValidationErrors.load()
            << ",\"cleanupSkippedForPendingWork\":" << (!safeToDestroy ? "true" : "false")
            << ",\"error\":" << quote(error) << ",\"validationMessages\":[";
        {
            std::lock_guard<std::mutex> lock(validationMutex);
            for (size_t i = 0; i < validationMessages.size(); ++i) { if (i) out << ','; out << quote(validationMessages[i]); }
        }
        {
            std::lock_guard<std::mutex> lock(validationMutex);
            out << "],\"validationNegativeControlObservedMessage\":" << quote(negativeObservedMessage) << '}';
        }
        return out.str();
    }
    static VKAPI_ATTR VkBool32 VKAPI_CALL debugCallback(VkDebugUtilsMessageSeverityFlagBitsEXT severity,
        VkDebugUtilsMessageTypeFlagsEXT, const VkDebugUtilsMessengerCallbackDataEXT *data, void *user) {
        auto *probe = static_cast<Probe *>(user);
        const std::string id = data && data->pMessageIdName ? data->pMessageIdName : "";
        const std::string message = data && data->pMessage ? data->pMessage : "";
        const bool expected = id == kNegativeVuid || message.find(kNegativeVuid) != std::string::npos;
        if (expected) probe->negativeObserved.store(true);
        else if (severity & VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT) ++probe->unexpectedValidationErrors;
        {
            std::lock_guard<std::mutex> lock(probe->validationMutex);
            if (expected && probe->negativeObservedMessage.empty()) probe->negativeObservedMessage = id + ": " + message;
            if (probe->validationMessages.size() < 32) probe->validationMessages.push_back(id + ": " + message);
        }
        return VK_FALSE;
    }
    VkDebugUtilsMessengerCreateInfoEXT messengerInfo() {
        auto info = typed<VkDebugUtilsMessengerCreateInfoEXT>(VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT);
        info.messageSeverity = VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT | VK_DEBUG_UTILS_MESSAGE_SEVERITY_WARNING_BIT_EXT;
        info.messageType = VK_DEBUG_UTILS_MESSAGE_TYPE_GENERAL_BIT_EXT | VK_DEBUG_UTILS_MESSAGE_TYPE_VALIDATION_BIT_EXT | VK_DEBUG_UTILS_MESSAGE_TYPE_PERFORMANCE_BIT_EXT;
        info.pfnUserCallback = debugCallback; info.pUserData = this; return info;
    }
    void negativeControl() {
        if (!cfg.validation_negative_control) return;
        if (!debugUtilsAvailable) { negativeError = "VK_EXT_debug_utils is unavailable; no negative control was issued."; return; }
        auto create = reinterpret_cast<PFN_vkCreateDebugUtilsMessengerEXT>(vkGetInstanceProcAddr(instance, "vkCreateDebugUtilsMessengerEXT"));
        destroyMessenger = reinterpret_cast<PFN_vkDestroyDebugUtilsMessengerEXT>(vkGetInstanceProcAddr(instance, "vkDestroyDebugUtilsMessengerEXT"));
        auto submitMessage = reinterpret_cast<PFN_vkSubmitDebugUtilsMessageEXT>(vkGetInstanceProcAddr(instance, "vkSubmitDebugUtilsMessageEXT"));
        if (!create || !destroyMessenger || !submitMessage) { negativeError = "Required debug_utils entry points are unavailable."; return; }
        auto info = messengerInfo();
        const auto result = create(instance, &info, nullptr, &messenger);
        if (result != VK_SUCCESS) { negativeError = "Cannot create validation messenger: " + std::to_string(result); return; }
        auto data = typed<VkDebugUtilsMessengerCallbackDataEXT>(VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CALLBACK_DATA_EXT);
        data.pMessageIdName = "MuguetValidationNegativeControl";
        data.messageIdNumber = 0;
        data.pMessage = "Intentional debug-message severity negative control; no GPU resource or command is involved.";
        mark("before-validation-negative-control"); negativeIssued = true;
        submitMessage(instance, static_cast<VkDebugUtilsMessageSeverityFlagBitsEXT>(0), VK_DEBUG_UTILS_MESSAGE_TYPE_GENERAL_BIT_EXT, &data);
        if (!negativeObserved.load()) negativeError = "Expected VUID was not observed; validation layer activation is not established.";
        mark("after-validation-negative-control");
    }
    void mark(const char *name) {
        phase = name;
        const std::string text = json();
        if (!tracePath.empty()) {
            std::ofstream file(tracePath, std::ios::trunc | std::ios::binary);
            if (!file) throw std::runtime_error("Cannot open last-call trace: " + tracePath);
            file << text << '\n'; file.close();
            if (!file) throw std::runtime_error("Cannot save last-call trace: " + tracePath);
        }
        std::fprintf(stderr, "BUFFER_PROBE iteration=%u phase=%s\n", iteration, name); std::fflush(stderr);
    }
    uint32_t memoryType(uint32_t bits, VkMemoryPropertyFlags required, VkMemoryPropertyFlags preferred) {
        for (int pass = 0; pass < 2; ++pass) {
            const auto wanted = required | (pass == 0 ? preferred : 0);
            for (uint32_t i = 0; i < properties.memoryTypeCount; ++i)
                if ((bits & (1u << i)) && (properties.memoryTypes[i].propertyFlags & wanted) == wanted) return i;
        }
        throw std::runtime_error("No memory type satisfies the requested properties");
    }
    Buffer createBuffer(VkDeviceSize size, VkBufferUsageFlags usage) {
        Buffer result;
        auto info = typed<VkBufferCreateInfo>(VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO);
        info.size = size; info.usage = usage; info.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        check(vkCreateBuffer(device, &info, nullptr, &result.handle), "vkCreateBuffer"); buffers.push_back(result.handle);
        auto dedicated = typed<VkMemoryDedicatedRequirements>(VK_STRUCTURE_TYPE_MEMORY_DEDICATED_REQUIREMENTS);
        auto requirements = typed<VkMemoryRequirements2>(VK_STRUCTURE_TYPE_MEMORY_REQUIREMENTS_2);
        requirements.pNext = &dedicated;
        auto query = typed<VkBufferMemoryRequirementsInfo2>(VK_STRUCTURE_TYPE_BUFFER_MEMORY_REQUIREMENTS_INFO_2);
        query.buffer = result.handle;
        vkGetBufferMemoryRequirements2(device, &query, &requirements);
        result.requirements = requirements.memoryRequirements;
        result.requiresDedicated = dedicated.requiresDedicatedAllocation;
        result.prefersDedicated = dedicated.prefersDedicatedAllocation;
        return result;
    }
    VkDeviceMemory allocate(VkDeviceSize size, uint32_t type, VkBuffer dedicated = VK_NULL_HANDLE) {
        auto dedicatedInfo = typed<VkMemoryDedicatedAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO);
        dedicatedInfo.buffer = dedicated;
        auto info = typed<VkMemoryAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO);
        info.allocationSize = size; info.memoryTypeIndex = type; info.pNext = dedicated ? &dedicatedInfo : nullptr;
        VkDeviceMemory memory = VK_NULL_HANDLE;
        check(vkAllocateMemory(device, &info, nullptr, &memory), "vkAllocateMemory"); memories.push_back(memory); return memory;
    }
    void bind(Buffer &buffer, VkDeviceMemory memory, VkDeviceSize offset, uint32_t type) {
        check(vkBindBufferMemory(device, buffer.handle, memory, offset), "vkBindBufferMemory");
        buffer.memory = memory; buffer.binding = offset; buffer.type = type;
    }
    void standalone(Buffer &buffer, VkMemoryPropertyFlags required, VkMemoryPropertyFlags preferred,
                    VkDeviceSize requestedOffset = 0, bool dedicated = false) {
        if (buffer.requiresDedicated && !dedicated) throw std::runtime_error("Buffer requires dedicated allocation; non-dedicated comparison is unsupported");
        const auto offset = dedicated ? 0 : alignUp(requestedOffset, buffer.requirements.alignment);
        const auto type = memoryType(buffer.requirements.memoryTypeBits, required, preferred);
        bind(buffer, allocate(offset + buffer.requirements.size, type, dedicated ? buffer.handle : VK_NULL_HANDLE), offset, type);
    }
    uint8_t *map(Buffer &buffer) {
        void *pointer = nullptr;
        check(vkMapMemory(device, buffer.memory, 0, VK_WHOLE_SIZE, 0, &pointer), "vkMapMemory");
        mapped.push_back(buffer.memory); return static_cast<uint8_t *>(pointer) + buffer.binding;
    }
    void hostSync(const Buffer &buffer, bool flush) {
        if (properties.memoryTypes[buffer.type].propertyFlags & VK_MEMORY_PROPERTY_HOST_COHERENT_BIT) return;
        auto range = typed<VkMappedMemoryRange>(VK_STRUCTURE_TYPE_MAPPED_MEMORY_RANGE);
        range.memory = buffer.memory; range.offset = 0; range.size = VK_WHOLE_SIZE;
        check(flush ? vkFlushMappedMemoryRanges(device, 1, &range) : vkInvalidateMappedMemoryRanges(device, 1, &range),
              flush ? "vkFlushMappedMemoryRanges" : "vkInvalidateMappedMemoryRanges");
    }
    void createPool(uint32_t count) {
        auto info = typed<VkCommandPoolCreateInfo>(VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO);
        info.flags = VK_COMMAND_POOL_CREATE_TRANSIENT_BIT; info.queueFamilyIndex = family;
        check(vkCreateCommandPool(device, &info, nullptr, &pool), "vkCreateCommandPool");
        auto allocation = typed<VkCommandBufferAllocateInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO);
        allocation.commandPool = pool; allocation.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY; allocation.commandBufferCount = count;
        commands.resize(count); check(vkAllocateCommandBuffers(device, &allocation, commands.data()), "vkAllocateCommandBuffers");
        ++poolGeneration;
    }
    VkCommandBuffer begin(uint32_t slot) {
        activeCommand = commands[slot];
        auto info = typed<VkCommandBufferBeginInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO);
        info.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;
        mark("before-vkBeginCommandBuffer"); check(vkBeginCommandBuffer(commands[slot], &info), "vkBeginCommandBuffer");
        mark("after-vkBeginCommandBuffer"); return commands[slot];
    }
    void submit(VkCommandBuffer command) {
        check(vkEndCommandBuffer(command), "vkEndCommandBuffer"); check(vkResetFences(device, 1, &fence), "vkResetFences");
        auto info = typed<VkSubmitInfo>(VK_STRUCTURE_TYPE_SUBMIT_INFO);
        info.commandBufferCount = 1; info.pCommandBuffers = &command;
        mark("before-vkQueueSubmit"); safeToDestroy = false;
        check(vkQueueSubmit(queue, 1, &info, fence), "vkQueueSubmit"); mark("before-vkWaitForFences");
        check(vkWaitForFences(device, 1, &fence, VK_TRUE, kWaitNs), "vkWaitForFences");
        safeToDestroy = true; ++submissions; mark("after-vkWaitForFences");
    }
    void recycle() {
        if (cfg.pool_policy == 0) {
            mark("before-vkResetCommandPool"); check(vkResetCommandPool(device, pool, 0), "vkResetCommandPool"); ++resetCount;
            mark("after-vkResetCommandPool");
        } else {
            mark("before-vkDestroyCommandPool"); vkDestroyCommandPool(device, pool, nullptr); pool = VK_NULL_HANDLE;
            commands.clear(); mark("after-vkDestroyCommandPool"); createPool(cfg.commands_per_pool); ++recreateCount;
            mark("after-vkCreateCommandPool");
        }
    }
    static void barrier(VkCommandBuffer command, VkPipelineStageFlags from, VkPipelineStageFlags to,
                        VkAccessFlags source, VkAccessFlags destination) {
        auto info = typed<VkMemoryBarrier>(VK_STRUCTURE_TYPE_MEMORY_BARRIER);
        info.srcAccessMask = source; info.dstAccessMask = destination;
        vkCmdPipelineBarrier(command, from, to, 0, 1, &info, 0, nullptr, 0, nullptr);
    }
    void initialize() {
        if (cfg.allocation > 2 || cfg.upload > 1 || cfg.pool_policy > 1 || cfg.validation_negative_control > 1 || cfg.iterations == 0 ||
            cfg.commands_per_pool == 0 || cfg.commands_per_pool > 1024 || cfg.reset_every == 0 ||
            cfg.reset_every > cfg.commands_per_pool || cfg.target_bytes > (256u << 20) ||
            cfg.target_bytes % 4 || cfg.destination_offset % 4 || cfg.destination_offset < kAdjacent ||
            cfg.destination_offset > cfg.target_bytes || kUpload + kAdjacent > cfg.target_bytes - cfg.destination_offset ||
            cfg.target_binding_offset > (256u << 20) || cfg.staging_binding_offset > (256u << 20) ||
            (cfg.target_usage & (VK_BUFFER_USAGE_TRANSFER_SRC_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT)) !=
                (VK_BUFFER_USAGE_TRANSFER_SRC_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT)) throw std::runtime_error("Invalid probe configuration");
        auto app = typed<VkApplicationInfo>(VK_STRUCTURE_TYPE_APPLICATION_INFO);
        app.pApplicationName = "Standalone Vulkan buffer probe"; app.apiVersion = VK_API_VERSION_1_1;
        auto instanceInfo = typed<VkInstanceCreateInfo>(VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO); instanceInfo.pApplicationInfo = &app;
        const char *debugExtension = VK_EXT_DEBUG_UTILS_EXTENSION_NAME;
        auto debugInfo = messengerInfo();
        if (cfg.validation_negative_control) {
            uint32_t extensionCount = 0;
            check(vkEnumerateInstanceExtensionProperties(nullptr, &extensionCount, nullptr), "vkEnumerateInstanceExtensionProperties");
            std::vector<VkExtensionProperties> extensions(extensionCount);
            check(vkEnumerateInstanceExtensionProperties(nullptr, &extensionCount, extensions.data()), "vkEnumerateInstanceExtensionProperties");
            debugUtilsAvailable = std::any_of(extensions.begin(), extensions.begin() + extensionCount, [](const VkExtensionProperties &extension) {
                return std::strcmp(extension.extensionName, VK_EXT_DEBUG_UTILS_EXTENSION_NAME) == 0;
            });
            if (debugUtilsAvailable) { instanceInfo.enabledExtensionCount = 1; instanceInfo.ppEnabledExtensionNames = &debugExtension; instanceInfo.pNext = &debugInfo; }
        }
        mark("before-vkCreateInstance"); check(vkCreateInstance(&instanceInfo, nullptr, &instance), "vkCreateInstance");
        negativeControl();
        uint32_t count = 0; check(vkEnumeratePhysicalDevices(instance, &count, nullptr), "vkEnumeratePhysicalDevices");
        std::vector<VkPhysicalDevice> devices(count); check(vkEnumeratePhysicalDevices(instance, &count, devices.data()), "vkEnumeratePhysicalDevices");
        if (cfg.physical_device >= count) throw std::runtime_error("Selected physical device is unavailable");
        physical = devices[cfg.physical_device]; VkPhysicalDeviceProperties details{}; vkGetPhysicalDeviceProperties(physical, &details);
        deviceName = details.deviceName; apiVersion = details.apiVersion;
        if (apiVersion < VK_API_VERSION_1_1) throw std::runtime_error("This isolated comparison requires Vulkan 1.1");
        vkGetPhysicalDeviceMemoryProperties(physical, &properties);
        queryAdditionalInfo();
        uint32_t queues = 0; vkGetPhysicalDeviceQueueFamilyProperties(physical, &queues, nullptr);
        std::vector<VkQueueFamilyProperties> families(queues); vkGetPhysicalDeviceQueueFamilyProperties(physical, &queues, families.data());
        for (uint32_t i = 0; i < queues; ++i) if (families[i].queueCount && (families[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)) { family = i; break; }
        if (family == UINT32_MAX) throw std::runtime_error("No graphics queue available");
        float priority = 1.0f; auto queueInfo = typed<VkDeviceQueueCreateInfo>(VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO);
        queueInfo.queueFamilyIndex = family; queueInfo.queueCount = 1; queueInfo.pQueuePriorities = &priority;
        auto deviceInfo = typed<VkDeviceCreateInfo>(VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO); deviceInfo.queueCreateInfoCount = 1; deviceInfo.pQueueCreateInfos = &queueInfo;
        check(vkCreateDevice(physical, &deviceInfo, nullptr, &device), "vkCreateDevice"); vkGetDeviceQueue(device, family, 0, &queue);
        target = createBuffer(cfg.target_bytes, cfg.target_usage);
        prefix = createBuffer(kGuard, VK_BUFFER_USAGE_TRANSFER_SRC_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT);
        suffix = createBuffer(kGuard, VK_BUFFER_USAGE_TRANSFER_SRC_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT);
        if (cfg.allocation == 0) {
            if (target.requiresDedicated || prefix.requiresDedicated || suffix.requiresDedicated) throw std::runtime_error("Shared allocation is unsupported: a buffer requires dedicated memory");
            const auto targetOffset = alignUp(std::max(cfg.target_binding_offset, prefix.requirements.size), target.requirements.alignment);
            const auto prefixOffset = (targetOffset - prefix.requirements.size) / prefix.requirements.alignment * prefix.requirements.alignment;
            const auto suffixOffset = alignUp(targetOffset + target.requirements.size, suffix.requirements.alignment);
            const auto bits = target.requirements.memoryTypeBits & prefix.requirements.memoryTypeBits & suffix.requirements.memoryTypeBits;
            const auto type = memoryType(bits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT, 0);
            const auto memory = allocate(suffixOffset + suffix.requirements.size, type);
            bind(prefix, memory, prefixOffset, type); bind(target, memory, targetOffset, type); bind(suffix, memory, suffixOffset, type);
            targetBindingAdjusted = targetOffset != cfg.target_binding_offset;
        } else {
            standalone(target, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT, 0, 0, cfg.allocation == 2);
            standalone(prefix, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT, 0, 0, prefix.requiresDedicated);
            standalone(suffix, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT, 0, 0, suffix.requiresDedicated);
        }
        staging = createBuffer(kStaging, VK_BUFFER_USAGE_TRANSFER_SRC_BIT);
        standalone(staging, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT, VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
                   cfg.staging_binding_offset, staging.requiresDedicated);
        readback = createBuffer(cfg.target_bytes + 2 * kGuard, VK_BUFFER_USAGE_TRANSFER_DST_BIT);
        standalone(readback, VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT, VK_MEMORY_PROPERTY_HOST_COHERENT_BIT | VK_MEMORY_PROPERTY_HOST_CACHED_BIT,
                   0, readback.requiresDedicated);
        stagingCpu = map(staging); readbackCpu = map(readback);
        auto fenceInfo = typed<VkFenceCreateInfo>(VK_STRUCTURE_TYPE_FENCE_CREATE_INFO); check(vkCreateFence(device, &fenceInfo, nullptr, &fence), "vkCreateFence");
        createPool(1); auto command = begin(0);
        mark("before-sentinel-fill"); vkCmdFillBuffer(command, target.handle, 0, cfg.target_bytes, 0xA5A5A5A5);
        vkCmdFillBuffer(command, prefix.handle, 0, kGuard, 0xB1B1B1B1); vkCmdFillBuffer(command, suffix.handle, 0, kGuard, 0xC3C3C3C3);
        submit(command); vkDestroyCommandPool(device, pool, nullptr); pool = VK_NULL_HANDLE; commands.clear(); createPool(cfg.commands_per_pool);
    }
    uint8_t pattern(uint64_t n) const { return static_cast<uint8_t>((n * 37 + (n >> 3) * 11 + iteration * 17 + 0x23) & 255); }
    void verifyRange(VkDeviceSize sourceOffset, const uint8_t *bytes, VkDeviceSize size) {
        for (VkDeviceSize i = 0; i < size; ++i) {
            const auto position = sourceOffset + i;
            const auto expected = position >= cfg.destination_offset && position < cfg.destination_offset + kUpload ? pattern(position - cfg.destination_offset) : uint8_t(0xA5);
            if (bytes[i] != expected) throw std::runtime_error("Target byte mismatch at offset " + std::to_string(position) + ": expected " + std::to_string(expected) + ", actual " + std::to_string(bytes[i]));
        }
        verifiedBytes += size;
    }
    void run() {
        initialize();
        for (iteration = 0; iteration < cfg.iterations; ++iteration) {
            if (iteration && iteration % cfg.reset_every == 0) recycle();
            std::memset(stagingCpu, 0xD2, kStaging); for (VkDeviceSize i = 0; i < kUpload; ++i) stagingCpu[i] = pattern(i); hostSync(staging, true);
            const auto slot = cfg.commands_per_pool - 1 - iteration % cfg.reset_every;
            auto command = begin(slot);
            barrier(command, VK_PIPELINE_STAGE_HOST_BIT | VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT,
                    VK_ACCESS_HOST_WRITE_BIT | VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_TRANSFER_READ_BIT | VK_ACCESS_TRANSFER_WRITE_BIT);
            mark(cfg.upload == 0 ? "before-vkCmdCopyBuffer-upload" : "before-vkCmdUpdateBuffer");
            if (cfg.upload == 0) { VkBufferCopy copy{0, cfg.destination_offset, kUpload}; vkCmdCopyBuffer(command, staging.handle, target.handle, 1, &copy); }
            else vkCmdUpdateBuffer(command, target.handle, cfg.destination_offset, kUpload, stagingCpu);
            mark(cfg.upload == 0 ? "after-vkCmdCopyBuffer-upload" : "after-vkCmdUpdateBuffer");
            barrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_TRANSFER_READ_BIT);
            VkBufferCopy copy{cfg.destination_offset - kAdjacent, 0, kUpload + 2 * kAdjacent};
            mark("before-vkCmdCopyBuffer-readback-window"); vkCmdCopyBuffer(command, target.handle, readback.handle, 1, &copy);
            barrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT, VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_HOST_READ_BIT);
            submit(command); hostSync(readback, false); verifyRange(copy.srcOffset, readbackCpu, copy.size); ++completed;
        }
        --iteration; recycle(); auto command = begin(cfg.commands_per_pool - 1);
        barrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_TRANSFER_READ_BIT);
        VkBufferCopy full{0, 0, cfg.target_bytes}, before{0, cfg.target_bytes, kGuard}, after{0, cfg.target_bytes + kGuard, kGuard};
        mark("before-vkCmdCopyBuffer-final-target"); vkCmdCopyBuffer(command, target.handle, readback.handle, 1, &full);
        vkCmdCopyBuffer(command, prefix.handle, readback.handle, 1, &before); vkCmdCopyBuffer(command, suffix.handle, readback.handle, 1, &after);
        barrier(command, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT, VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_HOST_READ_BIT);
        submit(command); hostSync(readback, false); verifyRange(0, readbackCpu, cfg.target_bytes);
        for (VkDeviceSize i = 0; i < kGuard; ++i) {
            if (readbackCpu[cfg.target_bytes + i] != 0xB1 || readbackCpu[cfg.target_bytes + kGuard + i] != 0xC3)
                throw std::runtime_error("Neighbor buffer sentinel changed at byte " + std::to_string(i));
        }
        verifiedBytes += 2 * kGuard; targetHash = 14695981039346656037ULL;
        for (VkDeviceSize i = 0; i < cfg.target_bytes; ++i) { targetHash ^= readbackCpu[i]; targetHash *= 1099511628211ULL; }
        verified = true;
        if (cfg.validation_negative_control && !negativeObserved.load()) error = negativeError;
        if (unexpectedValidationErrors.load()) error += (error.empty() ? "" : "; ") + std::string("Unexpected validation errors were observed.");
        mark("all-bytes-verified");
    }
};
}

extern "C" void muguet_vulkan_buffer_probe_defaults(MuguetVulkanBufferProbeConfig *config) {
    if (!config) return;
    *config = {};
    config->iterations = 64; config->commands_per_pool = 16; config->reset_every = 1;
    config->target_usage = VK_BUFFER_USAGE_TRANSFER_SRC_BIT | VK_BUFFER_USAGE_TRANSFER_DST_BIT | VK_BUFFER_USAGE_VERTEX_BUFFER_BIT;
    config->target_bytes = (20u << 20) + 4; config->destination_offset = 9004776; config->target_binding_offset = 5349568;
}
extern "C" const char *muguet_vulkan_buffer_probe_json(const MuguetVulkanBufferProbeConfig *config, const char *tracePath) {
    thread_local std::string result;
    MuguetVulkanBufferProbeConfig selected{}; muguet_vulkan_buffer_probe_defaults(&selected); if (config) selected = *config;
    Probe probe(selected, tracePath);
    try { probe.run(); } catch (const std::exception &error) { probe.error = error.what(); }
    result = probe.json(); return result.c_str();
}

#if MUGUET_VULKAN_BUFFER_PROBE_STANDALONE
int main(int argc, char **argv) {
    MuguetVulkanBufferProbeConfig config{}; muguet_vulkan_buffer_probe_defaults(&config);
    std::string trace, report;
    try {
        for (int i = 1; i < argc; ++i) {
            const std::string name = argv[i];
            if (name == "--help") {
                std::cout << "--allocation suballocated|standalone|dedicated --method copy|update --pool-policy reset|recreate\n"
                    "--offset 9004776|9581136 --iterations N --commands-per-pool 16 --reset-every 1\n"
                    "--binding-offset 5349568 --staging-binding-offset 0 --buffer-bytes 20971524 --usage 0x83\n"
                    "--device 0 --validation-negative-control 0|1 --trace PATH --report PATH\n"; return 0;
            }
            if (++i >= argc) throw std::runtime_error("Missing value for " + name);
            const std::string value = argv[i];
            if (name == "--allocation") { if (value == "suballocated") config.allocation = 0; else if (value == "standalone") config.allocation = 1; else if (value == "dedicated") config.allocation = 2; else throw std::runtime_error("Invalid allocation"); }
            else if (name == "--method") { if (value == "copy") config.upload = 0; else if (value == "update") config.upload = 1; else throw std::runtime_error("Invalid method"); }
            else if (name == "--pool-policy") { if (value == "reset") config.pool_policy = 0; else if (value == "recreate") config.pool_policy = 1; else throw std::runtime_error("Invalid pool policy"); }
            else if (name == "--trace") trace = value;
            else if (name == "--report") report = value;
            else {
                size_t end = 0; const uint64_t number = std::stoull(value, &end, 0); if (end != value.size()) throw std::runtime_error("Invalid numeric value");
                if (name == "--offset") config.destination_offset = number;
                else if (name == "--buffer-bytes") config.target_bytes = number;
                else if (name == "--binding-offset") config.target_binding_offset = number;
                else if (name == "--staging-binding-offset") config.staging_binding_offset = number;
                else if (number > UINT32_MAX) throw std::runtime_error("Numeric value exceeds uint32");
                else if (name == "--iterations") config.iterations = number;
                else if (name == "--commands-per-pool") config.commands_per_pool = number;
                else if (name == "--reset-every") config.reset_every = number;
                else if (name == "--usage") config.target_usage = number;
                else if (name == "--device") config.physical_device = number;
                else if (name == "--validation-negative-control") config.validation_negative_control = number;
                else throw std::runtime_error("Unknown argument " + name);
            }
        }
        const std::string json = muguet_vulkan_buffer_probe_json(&config, trace.empty() ? nullptr : trace.c_str());
        if (!report.empty()) { std::ofstream file(report); if (!file) throw std::runtime_error("Cannot open report"); file << json << '\n'; file.close(); if (!file) throw std::runtime_error("Cannot save report"); }
        std::cout << json << '\n';
        return json.find("\"status\":\"pass\"") != std::string::npos ? 0 : 1;
    } catch (const std::exception &error) { std::cerr << error.what() << '\n'; return 2; }
}
#endif
