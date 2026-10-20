#define VK_USE_PLATFORM_OHOS 1
#include <vulkan/vulkan.h>
#include <graphics_game_sdk/frame_generation_vk.h>
#include <ace/xcomponent/native_interface_xcomponent.h>
#include <native_window/external_window.h>
#include <napi/native_api.h>
#include <syscap_ndk.h>
#include <dlfcn.h>
#include <dirent.h>
#include <execinfo.h>
#include "graphics_frame_surface_shaders.h"
#include "graphics_frame_scene.h"
#include "graphics_frame_rate_policy.h"
#include "fg_model_metadata_audit.h"
#include "fg_model_alias_scope.h"
#include "fg_rife_benchmark.h"
#include "fg_rife_surface.h"
#include "fg_npu_flow_benchmark.h"
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>
#include <algorithm>
#include <atomic>
#include <chrono>
#include <cerrno>
#include <cmath>
#include <cstdlib>
#include <cstring>
#include <iomanip>
#include <mutex>
#include <sstream>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

namespace {
constexpr uint32_t W = 1202, H = 800;
std::string quote(const std::string& s) {
    std::ostringstream o; o << '"';
    for (unsigned char c : s) {
        if (c == '"' || c == '\\') o << '\\' << char(c);
        else if (c < 32) o << "\\u" << std::hex << std::setw(4) << std::setfill('0') << unsigned(c) << std::dec;
        else o << char(c);
    }
    o << '"'; return o.str();
}
const char* boolean(bool b) { return b ? "true" : "false"; }
bool save(const std::string& p, const std::string& line, bool append = false) {
    int fd = open(p.c_str(), O_CREAT | O_WRONLY | O_CLOEXEC | (append ? O_APPEND : O_TRUNC), 0600);
    if (fd < 0) return false;
    size_t offset = 0; bool ok = true;
    while (offset < line.size()) {
        auto n = write(fd, line.data() + offset, line.size() - offset);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) { ok = false; break; } offset += size_t(n);
    }
    if (fsync(fd) != 0) ok = false;
    close(fd); return ok;
}
template<class T> T info(VkStructureType type) { T t{}; t.sType = type; return t; }
void check(VkResult r, const char* api) {
    if (r != VK_SUCCESS) throw std::runtime_error(std::string(api) + " VkResult=" + std::to_string(r));
}
template<class T> bool has(const std::vector<T>& list, const char* name) {
    return std::any_of(list.begin(), list.end(), [&](const T& e) {
        return std::strncmp(e.extensionName, name, VK_MAX_EXTENSION_NAME_SIZE) == 0;
    });
}
struct FG {
    FG_Context_VK* context = nullptr;
    FG_Image_VK* source = nullptr;
    FG_Image_VK* output = nullptr;
    bool active = false;
    decltype(&HMS_FG_IsFrameGenerationSupported) supported = nullptr;
    decltype(&HMS_FG_CreateContext_VK) create = nullptr;
    decltype(&HMS_FG_SetAlgorithmMode_VK) algorithm = nullptr;
    decltype(&HMS_FG_SetResolution_VK) resolution = nullptr;
    decltype(&HMS_FG_SetImageFormat_VK) format = nullptr;
    decltype(&HMS_FG_SetIntegrationMode_VK) integration = nullptr;
    decltype(&HMS_FG_SetTargetFps_VK) targetFps = nullptr;
    decltype(&HMS_FG_Activate_VK) activate = nullptr;
    decltype(&HMS_FG_IsActive_VK) isActive = nullptr;
    decltype(&HMS_FG_SetUiPredictionEnabled_VK) uiPrediction = nullptr;
    decltype(&HMS_FG_Dispatch_VK) dispatch = nullptr;
    decltype(&HMS_FG_CreateImage_VK) image = nullptr;
    decltype(&HMS_FG_DestroyImage_VK) destroyImage = nullptr;
    decltype(&HMS_FG_Deactivate_VK) deactivate = nullptr;
    decltype(&HMS_FG_DestroyContext_VK) destroy = nullptr;
    void load() {
        // Public SDK linkage matches the current official guide/sample. This
        // ELF DT_NEEDED dependency is loaded before the NAPI worker is started;
        // no late dlopen/dlsym or manual unload/loader hook is performed.
        supported=&HMS_FG_IsFrameGenerationSupported;create=&HMS_FG_CreateContext_VK;
        algorithm=&HMS_FG_SetAlgorithmMode_VK;resolution=&HMS_FG_SetResolution_VK;
        format=&HMS_FG_SetImageFormat_VK;integration=&HMS_FG_SetIntegrationMode_VK;
        targetFps=&HMS_FG_SetTargetFps_VK;
        activate=&HMS_FG_Activate_VK;isActive=&HMS_FG_IsActive_VK;uiPrediction=&HMS_FG_SetUiPredictionEnabled_VK;
        dispatch=&HMS_FG_Dispatch_VK;image=&HMS_FG_CreateImage_VK;
        destroyImage=&HMS_FG_DestroyImage_VK;deactivate=&HMS_FG_Deactivate_VK;
        destroy=&HMS_FG_DestroyContext_VK;
    }
};
uint64_t nanosSince(std::chrono::steady_clock::time_point start) { return uint64_t(std::chrono::duration_cast<std::chrono::nanoseconds>(std::chrono::steady_clock::now()-start).count()); }
struct Image { VkImage image = VK_NULL_HANDLE; VkImageView view = VK_NULL_HANDLE; VkDeviceMemory memory = VK_NULL_HANDLE; VkFramebuffer framebuffer = VK_NULL_HANDLE; };
struct Trial {
    OHNativeWindow* window;
    std::string directory, phase = "created", error;
    bool enabled, uiPredictionRequested=false, finished = false, teardownComplete = false, pending = false, uiPredictionDisabled = false;
    bool uiPredictionConfigured=false;FG_ErrorCode uiPredictionResult=FG_SUCCESS;
    bool targetFpsConfigured=false;FG_ErrorCode targetFpsResult=FG_SUCCESS;
    std::string fusionAwareMetadata;
    std::string modelAliasAudit="null";
    int modelAliasRestoreErrno=0;
    bool geometryRequested=false, geometryQualificationPassed=false, retainPendingGraph=false, geometryWindowReference=false, geometrySceneDestroyed=false;
    bool swapchainSampledRequested=false, finalDepthStateRequested=false, finalDepthStateProvided=false;
    std::string geometryQualificationJson;
    std::unique_ptr<muguet::fg_scene::Scene> geometryScene;
    uint32_t seconds, sourceFps, requestedTargetFps, frames = 0, realPresents = 0, dispatches = 0, successes = 0, collecting = 0, events = 0;
    std::atomic<bool>& stop;
    VkInstance instance = VK_NULL_HANDLE; VkPhysicalDevice physical = VK_NULL_HANDLE;
    VkDevice device = VK_NULL_HANDLE; VkQueue queue = VK_NULL_HANDLE; uint32_t family = 0;
    VkSurfaceKHR surface = VK_NULL_HANDLE; VkSwapchainKHR swapchain = VK_NULL_HANDLE;
    VkExtent2D extent{}; VkFormat surfaceFormat = VK_FORMAT_UNDEFINED;
    VkImageUsageFlags swapchainSupportedUsageFlags=0, swapchainRequestedUsageFlags=0, swapchainActualUsageFlags=0;
    uint32_t loaderApiVersion=VK_API_VERSION_1_0, physicalApiVersion=0;
    VkCommandPool pool = VK_NULL_HANDLE; VkCommandBuffer command = VK_NULL_HANDLE;
    VkFence fence = VK_NULL_HANDLE, acquireFence = VK_NULL_HANDLE; VkSemaphore acquired = VK_NULL_HANDLE;
    bool acquireFenceWaitComplete=false, acquireFencePending=false;
    uint32_t acquireFenceWaits=0, acquireFenceResets=0;
    VkResult acquireFenceStatusAfterPresent=VK_NOT_READY;
    VkRenderPass renderPass = VK_NULL_HANDLE, finalRenderPass = VK_NULL_HANDLE;
    VkDescriptorSetLayout finalSetLayout = VK_NULL_HANDLE;
    VkDescriptorPool finalDescriptorPool = VK_NULL_HANDLE; VkDescriptorSet finalSet = VK_NULL_HANDLE;
    VkPipelineLayout finalPipelineLayout = VK_NULL_HANDLE; VkPipeline finalPipeline = VK_NULL_HANDLE;
    VkShaderModule finalVertex = VK_NULL_HANDLE, finalFragment = VK_NULL_HANDLE;
    VkSampler finalSampler = VK_NULL_HANDLE;
    bool finalPassCreated=false;uint32_t finalDrawsRecorded=0;
    std::vector<VkImageView> swapViews; std::vector<VkFramebuffer> swapFramebuffers;
    std::vector<VkImage> swapImages; std::vector<VkSemaphore> rendered;
    Image scene, predicted; FG fg; std::chrono::steady_clock::time_point started;
    std::atomic<uint32_t> validationErrors{0}, validationWarnings{0};
    std::atomic<bool> negativeObserved{false};
    std::atomic<uint32_t> nullFenceMessages{0}, nullFenceStackAttempts{0}, nullFenceStacksSaved{0};
    std::atomic<uintptr_t> ownFenceSnapshot{0}, ownAcquireFenceSnapshot{0};
    std::atomic<uint32_t> barrierStackAttempts{0}, renderPassStackAttempts{0}, otherCallbackStacksSaved{0};
    std::atomic<const char*> currentApi{"setup"};
    bool validationAvailable=false, validationEnabled=false, handleWrappingDisabled=false, callbackInstalled=false;
    VkDebugUtilsMessengerEXT messenger=VK_NULL_HANDLE;
    PFN_vkDestroyDebugUtilsMessengerEXT destroyMessenger=nullptr;
    std::mutex validationMutex; uint32_t validationLines=0;
    uint64_t sourceRecordNs=0, dispatchNs=0, presentNs=0, acquireNs=0, fenceNs=0, acquireFenceWaitNs=0;
    double renderElapsed=0;
    static VKAPI_ATTR VkBool32 VKAPI_CALL callback(VkDebugUtilsMessageSeverityFlagBitsEXT severity,
        VkDebugUtilsMessageTypeFlagsEXT, const VkDebugUtilsMessengerCallbackDataEXT* data, void* user) {
        auto& p=*static_cast<Trial*>(user);
        const std::string id=data&&data->pMessageIdName?std::string(data->pMessageIdName,strnlen(data->pMessageIdName,512)):std::string();
        const std::string text=data&&data->pMessage?std::string(data->pMessage,strnlen(data->pMessage,8192)):std::string();
        const bool control=id.find("VUID-vkSubmitDebugUtilsMessageEXT-messageSeverity-parameter")!=std::string::npos;
        if(control)p.negativeObserved=true;
        else if(severity&VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT)++p.validationErrors;
        else if(severity&VK_DEBUG_UTILS_MESSAGE_SEVERITY_WARNING_BIT_EXT)++p.validationWarnings;
        std::lock_guard<std::mutex> lock(p.validationMutex);
        if(p.validationLines++<64)save(p.directory+"/validation.jsonl","{\"pid\":"+std::to_string(getpid())+",\"severity\":"+std::to_string(severity)+",\"control\":"+boolean(control)+",\"duringOwnApi\":"+quote(p.currentApi.load())+",\"id\":"+quote(id)+",\"message\":"+quote(text)+"}\n",true);
        // Normal validation callback, not a signal handler. The public libc
        // unwinder records at most two stacks; no Vulkan call or SDK inspection.
        const bool nullFence=id.find("VUID-vkWaitForFences-pFences-parameter")!=std::string::npos;
        const bool barrier=id.find("VUID-vkCmdPipelineBarrier-dstAccessMask-02816")!=std::string::npos;
        const bool renderPass=id.find("InvalidRenderpass")!=std::string::npos||id.find("VUID-VkRenderPassBeginInfo-renderPass-00904")!=std::string::npos;
        if(nullFence||barrier||renderPass){
            if(nullFence)++p.nullFenceMessages;
            const bool capture=nullFence?p.nullFenceStackAttempts.fetch_add(1)<2:
                barrier?p.barrierStackAttempts.fetch_add(1)<1:p.renderPassStackAttempts.fetch_add(1)<2;
            if(capture){
                void* pcs[32]{};const int count=backtrace(pcs,32);std::ostringstream s;
                s<<"{\"pid\":"<<getpid()<<",\"id\":"<<quote(id)<<",\"duringOwnApi\":"<<quote(p.currentApi.load())
                 <<",\"ownFrameFence\":"<<quote(std::to_string(p.ownFenceSnapshot.load()))<<",\"ownAcquireFence\":"<<quote(std::to_string(p.ownAcquireFenceSnapshot.load()))<<",\"frames\":[";
                for(int i=0;i<count;++i){if(i)s<<',';Dl_info d{};const bool found=dladdr(pcs[i],&d)!=0;
                    const auto pc=reinterpret_cast<uintptr_t>(pcs[i]);const auto base=found?reinterpret_cast<uintptr_t>(d.dli_fbase):0;
                    s<<"{\"pc\":"<<quote(std::to_string(pc))<<",\"module\":"<<quote(found&&d.dli_fname?d.dli_fname:"")
                     <<",\"moduleBase\":"<<quote(std::to_string(base))<<",\"relativePc\":"<<quote(std::to_string(base&&pc>=base?pc-base:0))<<'}';}
                s<<"],\"frameLimit\":32,\"possiblyTruncated\":"<<boolean(count==32)<<"}\n";
                if(save(p.directory+(nullFence?"/null-fence-stacks.jsonl":"/renderpass-validation-stacks.jsonl"),s.str(),true)){if(nullFence)++p.nullFenceStacksSaved;else ++p.otherCallbackStacksSaved;}
            }
        }
        return VK_FALSE;
    }
    Trial(OHNativeWindow* w, std::string root, bool on, bool ui, bool geometry, uint32_t duration,
        uint32_t sourceRate, uint32_t targetRate, std::string metadata, bool sampled, bool depthState, std::atomic<bool>& stopping)
        : window(w), directory(std::move(root)), enabled(on), uiPredictionRequested(ui),
          fusionAwareMetadata(std::move(metadata)), geometryRequested(geometry), swapchainSampledRequested(sampled),
          finalDepthStateRequested(depthState), seconds(duration),
          sourceFps(sourceRate), requestedTargetFps(targetRate), stop(stopping) {}
    void trace(const std::string& s, const std::string& fields = "") {
        phase = s;
        if (++events > 256) throw std::runtime_error("bounded surface trace limit exceeded");
        if (!save(directory + "/trace.jsonl", "{\"pid\":" + std::to_string(getpid()) + ",\"phase\":" + quote(s) +
            ",\"sourceFrames\":" + std::to_string(frames) + (fields.empty() ? "" : "," + fields) + "}\n", true))
            throw std::runtime_error("cannot persist surface trace");
    }
    void fgCheck(const char* name, FG_ErrorCode code) {
        trace(name, "\"FG_ErrorCode\":" + std::to_string(code));
        if (code != FG_SUCCESS) throw std::runtime_error(std::string(name) + " FG_ErrorCode=" + std::to_string(code));
    }
    void traceActiveState(const char* event) {
        bool active=false;const auto code=fg.isActive(fg.context,&active);
        trace(event,"\"FG_ErrorCode\":"+std::to_string(code)+",\"active\":"+
            (code==FG_SUCCESS?std::string(boolean(active)):"null")+",\"modelReadyVerified\":false");
    }
    uint32_t memoryType(uint32_t bits) {
        VkPhysicalDeviceMemoryProperties m{}; vkGetPhysicalDeviceMemoryProperties(physical, &m);
        for (uint32_t i = 0; i < m.memoryTypeCount; ++i) if ((bits & (1u << i)) && (m.memoryTypes[i].propertyFlags & VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT)) return i;
        throw std::runtime_error("device-local image memory unavailable");
    }
    void makeImage(Image& image) {
        auto ci = info<VkImageCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO); ci.imageType = VK_IMAGE_TYPE_2D;
        ci.format = VK_FORMAT_R8G8B8A8_UNORM; ci.extent = {W,H,1}; ci.mipLevels = ci.arrayLayers = 1;
        ci.samples = VK_SAMPLE_COUNT_1_BIT; ci.tiling = VK_IMAGE_TILING_OPTIMAL;
        ci.usage = VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT | VK_IMAGE_USAGE_SAMPLED_BIT | VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
        ci.sharingMode = VK_SHARING_MODE_EXCLUSIVE;
        check(vkCreateImage(device,&ci,nullptr,&image.image),"vkCreateImage"); VkMemoryRequirements m{}; vkGetImageMemoryRequirements(device,image.image,&m);
        auto ai = info<VkMemoryAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO); ai.allocationSize = m.size; ai.memoryTypeIndex = memoryType(m.memoryTypeBits);
        check(vkAllocateMemory(device,&ai,nullptr,&image.memory),"vkAllocateMemory"); check(vkBindImageMemory(device,image.image,image.memory,0),"vkBindImageMemory");
        auto vi = info<VkImageViewCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO); vi.image = image.image; vi.viewType = VK_IMAGE_VIEW_TYPE_2D;
        vi.format = ci.format; vi.subresourceRange = {VK_IMAGE_ASPECT_COLOR_BIT,0,1,0,1}; check(vkCreateImageView(device,&vi,nullptr,&image.view),"vkCreateImageView");
        auto fi = info<VkFramebufferCreateInfo>(VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO); fi.renderPass = renderPass; fi.attachmentCount = 1; fi.pAttachments = &image.view;
        fi.width = W; fi.height = H; fi.layers = 1; check(vkCreateFramebuffer(device,&fi,nullptr,&image.framebuffer),"vkCreateFramebuffer");
    }
    void makeFinalPass() {
        if(!geometryRequested||!geometryQualificationPassed)throw std::runtime_error("final sampled pass requires qualified real geometry");
        // Match the official SYSTEM sample's final scene-texture draw into a
        // swapchain color renderpass. No Vulkan UI is drawn by this probe.
        VkAttachmentDescription a{};a.format=surfaceFormat;a.samples=VK_SAMPLE_COUNT_1_BIT;
        a.loadOp=VK_ATTACHMENT_LOAD_OP_DONT_CARE;a.storeOp=VK_ATTACHMENT_STORE_OP_STORE;
        a.stencilLoadOp=VK_ATTACHMENT_LOAD_OP_DONT_CARE;a.stencilStoreOp=VK_ATTACHMENT_STORE_OP_DONT_CARE;
        a.initialLayout=VK_IMAGE_LAYOUT_UNDEFINED;a.finalLayout=VK_IMAGE_LAYOUT_PRESENT_SRC_KHR;
        VkAttachmentReference color{0,VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};VkSubpassDescription sub{};
        sub.pipelineBindPoint=VK_PIPELINE_BIND_POINT_GRAPHICS;sub.colorAttachmentCount=1;sub.pColorAttachments=&color;
        // Exact dependency model in the public sample UIRenderer::CreateRenderPass:
        // one color-only subpass, no depth attachment, one incoming BY_REGION dep.
        VkSubpassDependency dependency{};
        dependency.srcSubpass=VK_SUBPASS_EXTERNAL;dependency.dstSubpass=0;
        dependency.srcStageMask=dependency.dstStageMask=VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;
        dependency.dstAccessMask=VK_ACCESS_COLOR_ATTACHMENT_READ_BIT|VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
        dependency.dependencyFlags=VK_DEPENDENCY_BY_REGION_BIT;
        auto rp=info<VkRenderPassCreateInfo>(VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO);rp.attachmentCount=1;rp.pAttachments=&a;
        rp.subpassCount=1;rp.pSubpasses=&sub;rp.dependencyCount=1;rp.pDependencies=&dependency;
        currentApi="own-create-final-renderpass";check(vkCreateRenderPass(device,&rp,nullptr,&finalRenderPass),"create final color renderpass");
        trace("own-final-renderpass-created","\"renderPass\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(finalRenderPass)))+",\"attachmentCount\":1,\"depthAttachment\":null,\"dependencyCount\":1,\"dependencyFlags\":1,\"format\":"+std::to_string(surfaceFormat));
        swapViews.resize(swapImages.size(),VK_NULL_HANDLE);swapFramebuffers.resize(swapImages.size(),VK_NULL_HANDLE);
        for(size_t i=0;i<swapImages.size();++i){
            auto vi=info<VkImageViewCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO);vi.image=swapImages[i];vi.viewType=VK_IMAGE_VIEW_TYPE_2D;
            vi.format=surfaceFormat;vi.subresourceRange={VK_IMAGE_ASPECT_COLOR_BIT,0,1,0,1};
            check(vkCreateImageView(device,&vi,nullptr,&swapViews[i]),"create swapchain color view");
            auto fb=info<VkFramebufferCreateInfo>(VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO);fb.renderPass=finalRenderPass;fb.attachmentCount=1;fb.pAttachments=&swapViews[i];
            fb.width=extent.width;fb.height=extent.height;fb.layers=1;
            currentApi="own-create-final-framebuffer";check(vkCreateFramebuffer(device,&fb,nullptr,&swapFramebuffers[i]),"create final color framebuffer");
            trace("own-final-framebuffer-created","\"index\":"+std::to_string(i)+",\"renderPass\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(finalRenderPass)))+",\"framebuffer\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(swapFramebuffers[i])))+",\"image\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(swapImages[i])))+",\"view\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(swapViews[i])))+",\"attachmentCountPassed\":1");
        }
        VkDescriptorSetLayoutBinding bindings[2]{};
        bindings[0]={0,VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE,1,VK_SHADER_STAGE_FRAGMENT_BIT,nullptr};
        bindings[1]={1,VK_DESCRIPTOR_TYPE_SAMPLER,1,VK_SHADER_STAGE_FRAGMENT_BIT,nullptr};
        auto sl=info<VkDescriptorSetLayoutCreateInfo>(VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO);sl.bindingCount=2;sl.pBindings=bindings;
        check(vkCreateDescriptorSetLayout(device,&sl,nullptr,&finalSetLayout),"create final descriptor layout");
        auto pl=info<VkPipelineLayoutCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO);pl.setLayoutCount=1;pl.pSetLayouts=&finalSetLayout;
        check(vkCreatePipelineLayout(device,&pl,nullptr,&finalPipelineLayout),"create final pipeline layout");
        VkDescriptorPoolSize sizes[2]{{VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE,1},{VK_DESCRIPTOR_TYPE_SAMPLER,1}};
        auto dp=info<VkDescriptorPoolCreateInfo>(VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO);dp.maxSets=1;dp.poolSizeCount=2;dp.pPoolSizes=sizes;
        check(vkCreateDescriptorPool(device,&dp,nullptr,&finalDescriptorPool),"create final descriptor pool");
        auto da=info<VkDescriptorSetAllocateInfo>(VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO);da.descriptorPool=finalDescriptorPool;da.descriptorSetCount=1;da.pSetLayouts=&finalSetLayout;
        check(vkAllocateDescriptorSets(device,&da,&finalSet),"allocate final descriptor set");
        auto sm=info<VkSamplerCreateInfo>(VK_STRUCTURE_TYPE_SAMPLER_CREATE_INFO);sm.magFilter=sm.minFilter=VK_FILTER_NEAREST;sm.mipmapMode=VK_SAMPLER_MIPMAP_MODE_NEAREST;
        sm.addressModeU=sm.addressModeV=sm.addressModeW=VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_EDGE;sm.maxLod=0;
        check(vkCreateSampler(device,&sm,nullptr,&finalSampler),"create final nearest sampler");
        VkDescriptorImageInfo image{VK_NULL_HANDLE,frameSourceView(),VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL};
        VkDescriptorImageInfo sampler{finalSampler,VK_NULL_HANDLE,VK_IMAGE_LAYOUT_UNDEFINED};
        VkWriteDescriptorSet writes[2]{};
        for(uint32_t i=0;i<2;++i){writes[i]=info<VkWriteDescriptorSet>(VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET);writes[i].dstSet=finalSet;writes[i].dstBinding=i;writes[i].descriptorCount=1;writes[i].descriptorType=bindings[i].descriptorType;}
        writes[0].pImageInfo=&image;writes[1].pImageInfo=&sampler;vkUpdateDescriptorSets(device,2,writes,0,nullptr);
        auto shader=info<VkShaderModuleCreateInfo>(VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO);shader.codeSize=sizeof(fg_surface_shaders::vert);shader.pCode=fg_surface_shaders::vert;
        check(vkCreateShaderModule(device,&shader,nullptr,&finalVertex),"create final vertex shader");
        shader.codeSize=sizeof(fg_surface_shaders::frag);shader.pCode=fg_surface_shaders::frag;
        check(vkCreateShaderModule(device,&shader,nullptr,&finalFragment),"create final fragment shader");
        VkPipelineShaderStageCreateInfo stages[2]{};
        for(auto& stage:stages){stage=info<VkPipelineShaderStageCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO);stage.pName="main";}
        stages[0].stage=VK_SHADER_STAGE_VERTEX_BIT;stages[0].module=finalVertex;stages[1].stage=VK_SHADER_STAGE_FRAGMENT_BIT;stages[1].module=finalFragment;
        auto vertex=info<VkPipelineVertexInputStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO);
        auto assembly=info<VkPipelineInputAssemblyStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO);assembly.topology=VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
        VkViewport viewport{0,0,float(extent.width),float(extent.height),0,1};VkRect2D scissor{{0,0},extent};
        auto vp=info<VkPipelineViewportStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO);vp.viewportCount=vp.scissorCount=1;vp.pViewports=&viewport;vp.pScissors=&scissor;
        auto raster=info<VkPipelineRasterizationStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO);raster.polygonMode=VK_POLYGON_MODE_FILL;raster.cullMode=VK_CULL_MODE_NONE;raster.frontFace=VK_FRONT_FACE_COUNTER_CLOCKWISE;raster.lineWidth=1;
        auto ms=info<VkPipelineMultisampleStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO);ms.rasterizationSamples=VK_SAMPLE_COUNT_1_BIT;
        // Optional sample-parity control: supply a disabled state while retaining
        // the same color-only renderpass and no depth/stencil attachment.
        auto depthState=info<VkPipelineDepthStencilStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO);
        depthState.depthTestEnable=VK_FALSE;depthState.depthWriteEnable=VK_FALSE;depthState.depthCompareOp=VK_COMPARE_OP_ALWAYS;
        depthState.depthBoundsTestEnable=VK_FALSE;depthState.stencilTestEnable=VK_FALSE;depthState.minDepthBounds=0;depthState.maxDepthBounds=1;
        VkPipelineColorBlendAttachmentState colorWrite{};colorWrite.colorWriteMask=VK_COLOR_COMPONENT_R_BIT|VK_COLOR_COMPONENT_G_BIT|VK_COLOR_COMPONENT_B_BIT|VK_COLOR_COMPONENT_A_BIT;
        auto blend=info<VkPipelineColorBlendStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO);blend.attachmentCount=1;blend.pAttachments=&colorWrite;
        auto pi=info<VkGraphicsPipelineCreateInfo>(VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO);pi.stageCount=2;pi.pStages=stages;pi.pVertexInputState=&vertex;pi.pInputAssemblyState=&assembly;pi.pViewportState=&vp;pi.pRasterizationState=&raster;pi.pMultisampleState=&ms;pi.pColorBlendState=&blend;pi.layout=finalPipelineLayout;pi.renderPass=finalRenderPass;
        pi.pDepthStencilState=finalDepthStateRequested?&depthState:nullptr;
        check(vkCreateGraphicsPipelines(device,VK_NULL_HANDLE,1,&pi,nullptr,&finalPipeline),"create final fullscreen pipeline");
        finalDepthStateProvided=finalDepthStateRequested;
        trace("own-final-pipeline-created","\"finalDepthStateRequested\":"+std::string(boolean(finalDepthStateRequested))+",\"finalDepthStateProvided\":"+boolean(finalDepthStateProvided)+",\"depthTestEnabled\":false,\"depthWriteEnabled\":false,\"depthBoundsTestEnabled\":false,\"stencilTestEnabled\":false,\"depthAttachment\":null");
        vkDestroyShaderModule(device,finalVertex,nullptr);finalVertex=VK_NULL_HANDLE;vkDestroyShaderModule(device,finalFragment,nullptr);finalFragment=VK_NULL_HANDLE;
        finalPassCreated=true;
    }
    void requireVulkan13Loader() {
        auto query=reinterpret_cast<PFN_vkEnumerateInstanceVersion>(vkGetInstanceProcAddr(VK_NULL_HANDLE,"vkEnumerateInstanceVersion"));
        if(query)check(query(&loaderApiVersion),"vkEnumerateInstanceVersion");
        trace("loader-api-capabilities","\"enumerateInstanceVersionAvailable\":"+std::string(boolean(query!=nullptr))+",\"loaderApiVersion\":"+std::to_string(loaderApiVersion)+",\"requestedApiVersion\":"+std::to_string(VK_API_VERSION_1_3));
        if(loaderApiVersion<VK_API_VERSION_1_3)throw std::runtime_error("actual Vulkan loader does not support requested1.3");
    }
    void requireVulkan13Physical() {
        VkPhysicalDeviceProperties properties{};vkGetPhysicalDeviceProperties(physical,&properties);physicalApiVersion=properties.apiVersion;
        trace("physical-api-capabilities","\"physicalApiVersion\":"+std::to_string(physicalApiVersion)+",\"requestedApiVersion\":"+std::to_string(VK_API_VERSION_1_3));
        if(physicalApiVersion<VK_API_VERSION_1_3)throw std::runtime_error("actual Vulkan physical device does not support requested1.3");
    }
    VkImage frameSourceImage()const{return geometryRequested?geometryScene->colorImage():scene.image;}
    VkImageView frameSourceView()const{return geometryRequested?geometryScene->colorView():scene.view;}
    void qualifyGeometryScene() {
        currentApi="geometry-scene-create";geometryScene=muguet::fg_scene::Scene::create(physical,device,W,H);
        trace("before-geometry-qualification");currentApi="geometry-scene-qualification";
        retainPendingGraph=true; // Conservative if an exception prevents a structured retirement result.
        geometryQualificationJson=geometryScene->qualify(queue,family);
        // This is a closed, pinned helper's structured result, not external JSON.
        // Fail closed unless its exact success prefix is present. Pending work
        // requires the ENTIRE outer graph/user callback/window to stay alive.
        const bool knownResult=geometryQualificationJson.rfind("{\"status\":\"pass\",\"pass\":true,",0)==0||geometryQualificationJson.rfind("{\"status\":\"fail\",\"pass\":false,",0)==0;
        const bool positivelyRetired=geometryQualificationJson.find("\"qualificationPending\":false")!=std::string::npos||geometryQualificationJson.find("\"qualificationResourcesDestroyed\":true")!=std::string::npos;
        retainPendingGraph=!knownResult||!positivelyRetired||geometryQualificationJson.find("\"qualificationPending\":true")!=std::string::npos;
        if(!save(directory+"/geometry-qualification.json",geometryQualificationJson+'\n'))throw std::runtime_error("cannot persist geometry qualification");
        if(retainPendingGraph)throw std::runtime_error("geometry qualification GPU pending; force-stop required; entire graph retained");
        geometryQualificationPassed=geometryQualificationJson.rfind("{\"status\":\"pass\",\"pass\":true,",0)==0;
        trace("after-geometry-qualification","\"gpuImagePass\":"+std::string(boolean(geometryQualificationPassed))+",\"validationErrorsObserved\":"+std::to_string(validationErrors.load()));
        if(!geometryQualificationPassed)throw std::runtime_error("geometry color/depth/guard qualification failed");
        if(validationErrors.load()!=0)throw std::runtime_error("geometry qualification reported Vulkan validation errors");
        if(validationEnabled&&(!callbackInstalled||!negativeObserved.load()))throw std::runtime_error("geometry validation callback/control unavailable");
    }
    void setup() {
        if(geometryRequested){if(OH_NativeWindow_NativeObjectReference(window)!=0)throw std::runtime_error("cannot retain geometry trial window");geometryWindowReference=true;}
        trace("begin", "\"fgEnabled\":" + std::string(boolean(enabled)) + ",\"uiPredictionRequested\":"+boolean(uiPredictionRequested)+",\"geometryRequested\":"+boolean(geometryRequested)+",\"swapchainSampledRequested\":"+boolean(swapchainSampledRequested)+",\"finalDepthStateRequested\":"+boolean(finalDepthStateRequested)+",\"publicLibraryIntegration\":\"ELF_DT_NEEDED_direct_symbols\",\"sourceResolution\":[1202,800],\"sourceTargetHz\":"+std::to_string(sourceFps)+",\"requestedOutputTargetHz\":"+std::to_string(requestedTargetFps)+",\"fusionAwareMetadata\":"+quote(fusionAwareMetadata));
        if(enabled && fusionAwareMetadata!="Vulkan") {
            trace("system-presentation-metadata-unqualified", "\"expected\":\"GraphicsAccelerateKit_FusionAware=Vulkan\",\"observed\":"+quote(fusionAwareMetadata));
            throw std::runtime_error("AI system presentation requires installed module metadata GraphicsAccelerateKit_FusionAware=Vulkan; observed "+fusionAwareMetadata+". Use the experimental metadata-enabled package.");
        }
        if (!canIUse("SystemCapability.GraphicsGame.RenderAccelerate")) throw std::runtime_error("RenderAccelerate syscap absent");
        fg.load(); if (!fg.supported(INTERPOLATION_AI_VULKAN)) throw std::runtime_error("AI Vulkan capability false");
        requireVulkan13Loader();
        auto app = info<VkApplicationInfo>(VK_STRUCTURE_TYPE_APPLICATION_INFO); app.pApplicationName = "Independent synthetic AI FG surface qualification"; app.apiVersion = VK_API_VERSION_1_3;
        uint32_t instanceCount=0;check(vkEnumerateInstanceLayerProperties(&instanceCount,nullptr),"instance layer count");
        if(instanceCount>256)throw std::runtime_error("unreasonable instance layer count");
        std::vector<VkLayerProperties> layers(instanceCount);if(instanceCount)check(vkEnumerateInstanceLayerProperties(&instanceCount,layers.data()),"instance layer list");
        const char* validationName="VK_LAYER_KHRONOS_validation";
        for(const auto& l:layers)if(std::strcmp(l.layerName,validationName)==0)validationAvailable=true;
        instanceCount=0;check(vkEnumerateInstanceExtensionProperties(nullptr,&instanceCount,nullptr),"instance extension count");
        if(instanceCount>4096)throw std::runtime_error("unreasonable instance extension count");
        std::vector<VkExtensionProperties> instanceExtensions(instanceCount);if(instanceCount)check(vkEnumerateInstanceExtensionProperties(nullptr,&instanceCount,instanceExtensions.data()),"instance extension list");
        std::vector<const char*> ie{VK_KHR_SURFACE_EXTENSION_NAME,VK_OHOS_SURFACE_EXTENSION_NAME};
        for(auto name:ie)if(!has(instanceExtensions,name))throw std::runtime_error(std::string("instance extension unavailable: ")+name);
        const bool debug=has(instanceExtensions,VK_EXT_DEBUG_UTILS_EXTENSION_NAME);
        if(debug)ie.push_back(VK_EXT_DEBUG_UTILS_EXTENSION_NAME);
        const VkValidationFeatureDisableEXT noWrap=VK_VALIDATION_FEATURE_DISABLE_UNIQUE_HANDLES_EXT;
        auto validation=info<VkValidationFeaturesEXT>(VK_STRUCTURE_TYPE_VALIDATION_FEATURES_EXT);
        auto ic = info<VkInstanceCreateInfo>(VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO); ic.pApplicationInfo = &app;
        if(validationAvailable){
            instanceCount=0;check(vkEnumerateInstanceExtensionProperties(validationName,&instanceCount,nullptr),"layer extension count");
            if(instanceCount>4096)throw std::runtime_error("unreasonable layer extension count");
            std::vector<VkExtensionProperties> ex(instanceCount);if(instanceCount)check(vkEnumerateInstanceExtensionProperties(validationName,&instanceCount,ex.data()),"layer extension list");
            if(!has(ex,VK_EXT_VALIDATION_FEATURES_EXTENSION_NAME))throw std::runtime_error("cannot safely disable opaque vendor handle wrapping with public validation features");
            ie.push_back(VK_EXT_VALIDATION_FEATURES_EXTENSION_NAME);validation.disabledValidationFeatureCount=1;validation.pDisabledValidationFeatures=&noWrap;
            ic.pNext=&validation;ic.enabledLayerCount=1;ic.ppEnabledLayerNames=&validationName;
            // Opaque public FG code must receive real non-dispatchable handles.
            // Retain validation; disable only the layer's UNIQUE_HANDLES wrapper.
        }
        ic.enabledExtensionCount=uint32_t(ie.size());ic.ppEnabledExtensionNames=ie.data();
        trace("create-instance","\"validationAvailable\":"+std::string(boolean(validationAvailable))+",\"disableOnly\":\"UNIQUE_HANDLES\"");check(vkCreateInstance(&ic,nullptr,&instance),"vkCreateInstance");
        validationEnabled=validationAvailable;handleWrappingDisabled=validationEnabled;
        if(debug){auto makeMessenger=reinterpret_cast<PFN_vkCreateDebugUtilsMessengerEXT>(vkGetInstanceProcAddr(instance,"vkCreateDebugUtilsMessengerEXT"));
            destroyMessenger=reinterpret_cast<PFN_vkDestroyDebugUtilsMessengerEXT>(vkGetInstanceProcAddr(instance,"vkDestroyDebugUtilsMessengerEXT"));
            auto mi=info<VkDebugUtilsMessengerCreateInfoEXT>(VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT);mi.messageSeverity=VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT|VK_DEBUG_UTILS_MESSAGE_SEVERITY_WARNING_BIT_EXT;
            mi.messageType=VK_DEBUG_UTILS_MESSAGE_TYPE_GENERAL_BIT_EXT|VK_DEBUG_UTILS_MESSAGE_TYPE_VALIDATION_BIT_EXT|VK_DEBUG_UTILS_MESSAGE_TYPE_PERFORMANCE_BIT_EXT;mi.pfnUserCallback=callback;mi.pUserData=this;
            if(makeMessenger&&destroyMessenger){check(makeMessenger(instance,&mi,nullptr,&messenger),"create validation messenger");callbackInstalled=true;}
            auto send=reinterpret_cast<PFN_vkSubmitDebugUtilsMessageEXT>(vkGetInstanceProcAddr(instance,"vkSubmitDebugUtilsMessageEXT"));
            if(messenger&&send&&validationEnabled){auto message=info<VkDebugUtilsMessengerCallbackDataEXT>(VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CALLBACK_DATA_EXT);message.pMessageIdName="MuguetFgSurfaceNegativeControl";message.pMessage="Intentional invalid severity control, no GPU resource involved";
                send(instance,static_cast<VkDebugUtilsMessageSeverityFlagBitsEXT>(0),VK_DEBUG_UTILS_MESSAGE_TYPE_GENERAL_BIT_EXT,&message);}
        }
        const int geometry = OH_NativeWindow_NativeWindowHandleOpt(window,SET_BUFFER_GEOMETRY,int(W),int(H));
        trace("set-buffer-geometry", "\"result\":" + std::to_string(geometry)); if (geometry != 0) throw std::runtime_error("native buffer geometry rejected");
        auto sc = info<VkSurfaceCreateInfoOHOS>(VK_STRUCTURE_TYPE_SURFACE_CREATE_INFO_OHOS); sc.window = window;
        auto createSurface = reinterpret_cast<PFN_vkCreateSurfaceOHOS>(vkGetInstanceProcAddr(instance,"vkCreateSurfaceOHOS"));
        if (!createSurface) throw std::runtime_error("public OHOS surface entry point unavailable");
        check(createSurface(instance,&sc,nullptr,&surface),"vkCreateSurfaceOHOS");
        uint32_t n = 0; check(vkEnumeratePhysicalDevices(instance,&n,nullptr),"physical count");
        if (!n || n > 32) throw std::runtime_error("invalid physical list"); std::vector<VkPhysicalDevice> devices(n); check(vkEnumeratePhysicalDevices(instance,&n,devices.data()),"physical list"); physical = devices.front();requireVulkan13Physical();
        n = 0; vkGetPhysicalDeviceQueueFamilyProperties(physical,&n,nullptr); if (!n || n > 256) throw std::runtime_error("invalid family list");
        std::vector<VkQueueFamilyProperties> families(n); vkGetPhysicalDeviceQueueFamilyProperties(physical,&n,families.data()); bool found = false;
        for (uint32_t i=0;i<n;++i) { VkBool32 present = VK_FALSE; check(vkGetPhysicalDeviceSurfaceSupportKHR(physical,i,surface,&present),"surface queue support");
            if (present && families[i].queueCount && (families[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)) { family=i;found=true;break; } }
        if (!found) throw std::runtime_error("no common graphics/present queue");
        n = 0; check(vkEnumerateDeviceExtensionProperties(physical,nullptr,&n,nullptr),"device extensions count"); if(n>4096)throw std::runtime_error("invalid extension count");
        std::vector<VkExtensionProperties> ex(n); check(vkEnumerateDeviceExtensionProperties(physical,nullptr,&n,ex.data()),"device extensions list");
        const char* de[] = {VK_KHR_SWAPCHAIN_EXTENSION_NAME,VK_KHR_SHADER_FLOAT16_INT8_EXTENSION_NAME}; for(auto name:de)if(!has(ex,name))throw std::runtime_error(std::string("sample prerequisite missing: ")+name);
        auto fp16 = info<VkPhysicalDeviceShaderFloat16Int8Features>(VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_SHADER_FLOAT16_INT8_FEATURES);
        auto f2 = info<VkPhysicalDeviceFeatures2>(VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_FEATURES_2); f2.pNext=&fp16;vkGetPhysicalDeviceFeatures2(physical,&f2);
        if(!fp16.shaderFloat16 || !f2.features.shaderInt16 || !f2.features.fragmentStoresAndAtomics)throw std::runtime_error("actual sample shader features missing");
        VkPhysicalDeviceFeatures features{};features.shaderInt16=features.fragmentStoresAndAtomics=VK_TRUE;features.samplerAnisotropy=f2.features.samplerAnisotropy;fp16.shaderInt8=VK_FALSE;
        float priority=1;auto qc=info<VkDeviceQueueCreateInfo>(VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO);qc.queueFamilyIndex=family;qc.queueCount=1;qc.pQueuePriorities=&priority;
        auto dc=info<VkDeviceCreateInfo>(VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO);dc.pNext=&fp16;dc.pEnabledFeatures=&features;dc.queueCreateInfoCount=1;dc.pQueueCreateInfos=&qc;dc.enabledExtensionCount=2;dc.ppEnabledExtensionNames=de;
        trace("create-device");check(vkCreateDevice(physical,&dc,nullptr,&device),"vkCreateDevice");vkGetDeviceQueue(device,family,0,&queue);
        VkSurfaceCapabilitiesKHR caps{};check(vkGetPhysicalDeviceSurfaceCapabilitiesKHR(physical,surface,&caps),"surface capabilities");
        const VkImageUsageFlags requiredSurfaceUsage=VK_IMAGE_USAGE_TRANSFER_DST_BIT|VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
        swapchainSupportedUsageFlags=caps.supportedUsageFlags;
        swapchainRequestedUsageFlags=requiredSurfaceUsage|(swapchainSampledRequested?VK_IMAGE_USAGE_SAMPLED_BIT:0);
        trace("swapchain-usage-policy","\"swapchainSampledRequested\":"+std::string(boolean(swapchainSampledRequested))+",\"swapchainSupportedUsageFlags\":"+std::to_string(swapchainSupportedUsageFlags)+",\"swapchainRequestedUsageFlags\":"+std::to_string(swapchainRequestedUsageFlags));
        // The public FG system-present integration creates color image views /
        // framebuffers for the swapchain. TRANSFER_DST alone is insufficient.
        if ((caps.supportedUsageFlags & requiredSurfaceUsage)!=requiredSurfaceUsage)
            throw std::runtime_error("surface TRANSFER_DST|COLOR_ATTACHMENT unsupported");
        if(swapchainSampledRequested&&!(caps.supportedUsageFlags&VK_IMAGE_USAGE_SAMPLED_BIT))
            throw std::runtime_error("requested surface SAMPLED usage unsupported");
        extent = caps.currentExtent.width != UINT32_MAX ? caps.currentExtent : VkExtent2D{std::clamp(W,caps.minImageExtent.width,caps.maxImageExtent.width),std::clamp(H,caps.minImageExtent.height,caps.maxImageExtent.height)};
        n=0;check(vkGetPhysicalDeviceSurfaceFormatsKHR(physical,surface,&n,nullptr),"surface formats count");if(!n || n>256)throw std::runtime_error("invalid surface formats");
        std::vector<VkSurfaceFormatKHR> formats(n);check(vkGetPhysicalDeviceSurfaceFormatsKHR(physical,surface,&n,formats.data()),"surface formats");
        VkSurfaceFormatKHR chosen=formats.front(); bool chosenSupported=false;
        for(auto f:formats)if(f.format==VK_FORMAT_R8G8B8A8_UNORM || f.format==VK_FORMAT_B8G8R8A8_UNORM){chosen=f;chosenSupported=true;if(f.format==VK_FORMAT_R8G8B8A8_UNORM)break;}
        if(!chosenSupported)throw std::runtime_error("no supported UNORM surface color format"); surfaceFormat=chosen.format;
        VkFormatProperties rgba{},dst{};vkGetPhysicalDeviceFormatProperties(physical,VK_FORMAT_R8G8B8A8_UNORM,&rgba);vkGetPhysicalDeviceFormatProperties(physical,surfaceFormat,&dst);
        const auto required=VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT|VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT|VK_FORMAT_FEATURE_BLIT_SRC_BIT;
        if((rgba.optimalTilingFeatures & required)!=required || (dst.optimalTilingFeatures & (VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT|VK_FORMAT_FEATURE_BLIT_DST_BIT))!=(VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT|VK_FORMAT_FEATURE_BLIT_DST_BIT))throw std::runtime_error("actual RGBA8 render/sample/blit-source and surface color/blit-destination features missing");
        if(swapchainSampledRequested&&!(dst.optimalTilingFeatures&VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT))throw std::runtime_error("requested surface format sampling unsupported");
        uint32_t imageCount=caps.minImageCount+1;if(caps.maxImageCount)imageCount=std::min(imageCount,caps.maxImageCount);if(!imageCount || imageCount>16)throw std::runtime_error("unreasonable swapchain image count");
        auto sw=info<VkSwapchainCreateInfoKHR>(VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR);sw.surface=surface;sw.minImageCount=imageCount;sw.imageFormat=chosen.format;sw.imageColorSpace=chosen.colorSpace;sw.imageExtent=extent;
        sw.imageArrayLayers=1;sw.imageUsage=swapchainRequestedUsageFlags;sw.imageSharingMode=VK_SHARING_MODE_EXCLUSIVE;sw.preTransform=caps.currentTransform;
        sw.compositeAlpha=VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR; if(!(caps.supportedCompositeAlpha & sw.compositeAlpha)) {for(auto a:{VK_COMPOSITE_ALPHA_PRE_MULTIPLIED_BIT_KHR,VK_COMPOSITE_ALPHA_POST_MULTIPLIED_BIT_KHR,VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR})if(caps.supportedCompositeAlpha&a){sw.compositeAlpha=a;break;}}
        sw.presentMode=VK_PRESENT_MODE_FIFO_KHR;sw.clipped=VK_TRUE;check(vkCreateSwapchainKHR(device,&sw,nullptr,&swapchain),"vkCreateSwapchainKHR");
        swapchainActualUsageFlags=sw.imageUsage;
        trace("swapchain-created","\"swapchainSampledRequested\":"+std::string(boolean(swapchainSampledRequested))+",\"swapchainSampledEnabled\":"+boolean((swapchainActualUsageFlags&VK_IMAGE_USAGE_SAMPLED_BIT)!=0)+",\"swapchainSupportedUsageFlags\":"+std::to_string(swapchainSupportedUsageFlags)+",\"swapchainRequestedUsageFlags\":"+std::to_string(swapchainRequestedUsageFlags)+",\"swapchainActualUsageFlags\":"+std::to_string(swapchainActualUsageFlags));
        n=0;check(vkGetSwapchainImagesKHR(device,swapchain,&n,nullptr),"swapchain images count");if(!n||n>16)throw std::runtime_error("unreasonable actual swapchain count");swapImages.resize(n);check(vkGetSwapchainImagesKHR(device,swapchain,&n,swapImages.data()),"swapchain images");
        auto sem=info<VkSemaphoreCreateInfo>(VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO);rendered.resize(n,VK_NULL_HANDLE);for(auto& s:rendered)check(vkCreateSemaphore(device,&sem,nullptr,&s),"vkCreateSemaphore renderEnd");check(vkCreateSemaphore(device,&sem,nullptr,&acquired),"vkCreateSemaphore acquire");
        auto pc=info<VkCommandPoolCreateInfo>(VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO);pc.flags=VK_COMMAND_POOL_CREATE_TRANSIENT_BIT;pc.queueFamilyIndex=family;check(vkCreateCommandPool(device,&pc,nullptr,&pool),"vkCreateCommandPool");
        auto ca=info<VkCommandBufferAllocateInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO);ca.commandPool=pool;ca.level=VK_COMMAND_BUFFER_LEVEL_PRIMARY;ca.commandBufferCount=1;check(vkAllocateCommandBuffers(device,&ca,&command),"vkAllocateCommandBuffers");
        auto fc=info<VkFenceCreateInfo>(VK_STRUCTURE_TYPE_FENCE_CREATE_INFO);fc.flags=VK_FENCE_CREATE_SIGNALED_BIT;check(vkCreateFence(device,&fc,nullptr,&fence),"vkCreateFence");if(!fence)throw std::runtime_error("created frame fence is null");ownFenceSnapshot=reinterpret_cast<uintptr_t>(fence);trace("own-frame-fence-created","\"handle\":"+quote(std::to_string(ownFenceSnapshot.load())));
        fc.flags=0;check(vkCreateFence(device,&fc,nullptr,&acquireFence),"vkCreateFence acquire");
        if(!acquireFence||acquireFence==fence)throw std::runtime_error("acquire fence must be real and independent");
        ownAcquireFenceSnapshot=reinterpret_cast<uintptr_t>(acquireFence);
        trace("own-acquire-fence-created","\"handle\":"+quote(std::to_string(ownAcquireFenceSnapshot.load()))+",\"initiallySignaled\":false,\"independentFromSubmitFence\":true");
        if(geometryRequested){qualifyGeometryScene();makeFinalPass();}else{
        VkAttachmentDescription attachment{};attachment.format=VK_FORMAT_R8G8B8A8_UNORM;attachment.samples=VK_SAMPLE_COUNT_1_BIT;attachment.loadOp=VK_ATTACHMENT_LOAD_OP_CLEAR;attachment.storeOp=VK_ATTACHMENT_STORE_OP_STORE;
        attachment.stencilLoadOp=VK_ATTACHMENT_LOAD_OP_DONT_CARE;attachment.stencilStoreOp=VK_ATTACHMENT_STORE_OP_DONT_CARE;attachment.initialLayout=VK_IMAGE_LAYOUT_UNDEFINED;attachment.finalLayout=VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
        VkAttachmentReference color{0,VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL};VkSubpassDescription sub{};sub.pipelineBindPoint=VK_PIPELINE_BIND_POINT_GRAPHICS;sub.colorAttachmentCount=1;sub.pColorAttachments=&color;
        VkSubpassDependency dependency{};dependency.srcSubpass=0;dependency.dstSubpass=VK_SUBPASS_EXTERNAL;dependency.srcStageMask=VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT;dependency.srcAccessMask=VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT;
        dependency.dstStageMask=VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;dependency.dstAccessMask=VK_ACCESS_SHADER_READ_BIT;
        auto rp=info<VkRenderPassCreateInfo>(VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO);rp.attachmentCount=1;rp.pAttachments=&attachment;rp.subpassCount=1;rp.pSubpasses=&sub;rp.dependencyCount=1;rp.pDependencies=&dependency;
        check(vkCreateRenderPass(device,&rp,nullptr,&renderPass),"vkCreateRenderPass");makeImage(scene);makeImage(predicted);
        trace("own-offscreen-objects-created","\"renderPass\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(renderPass)))+",\"attachmentCount\":1,\"depthAttachment\":null,\"dependencyCount\":1,\"sceneFramebuffer\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(scene.framebuffer)))+",\"predictedFramebuffer\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(predicted.framebuffer)))+",\"sceneImage\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(scene.image)))+",\"predictedImage\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(predicted.image))));
        }
        trace(geometryRequested?"final-path-geometry-sampled-color":"final-path-transfer-blit","\"finalColorRenderpassResourcesCreated\":"+std::string(boolean(finalPassCreated))+",\"finalDepthStateRequested\":"+boolean(finalDepthStateRequested)+",\"finalDepthStateProvided\":"+boolean(finalDepthStateProvided)+",\"acquireFenceReal\":true");
        if(enabled){
            FG_ContextDescription_VK cd{instance,physical,device,1,vkGetInstanceProcAddr};trace("before-FG-create");fg.context=fg.create(&cd);if(!fg.context)throw std::runtime_error("FG context returned null");
            FG_AlgorithmModeInfo mode{FG_PREDICTION_MODE_INTERPOLATION,FG_ME_MODE_ENHANCED};fgCheck("FG-set-algorithm",fg.algorithm(fg.context,&mode));
            FG_ResolutionInfo size{{W,H},{W,H},{W,H}};fgCheck("FG-set-resolution",fg.resolution(fg.context,&size));FG_ImageFormat_VK format{VK_FORMAT_R8G8B8A8_UNORM,geometryRequested?VK_FORMAT_D32_SFLOAT:VK_FORMAT_D24_UNORM_S8_UINT,VK_FORMAT_R8G8B8A8_UNORM};fgCheck("FG-set-format",fg.format(fg.context,&format));
            FG_IntegrationInfo integration{};integration.presentMode=FG_PRESENT_BY_SYSTEM;fgCheck("FG-set-system",fg.integration(fg.context,&integration));
            // This setter's documented effect starts after the game's first
            // release. A successful return is configuration, not 30 Hz output.
            targetFpsResult=fg.targetFps(fg.context,int(requestedTargetFps));targetFpsConfigured=true;
            trace("FG-set-target-fps", "\"FG_ErrorCode\":"+std::to_string(targetFpsResult)+",\"requestedOutputTargetHz\":"+std::to_string(requestedTargetFps)+",\"documentedEffectAfterFirstRelease\":true,\"effectiveOutputTargetHz\":null");
            if(targetFpsResult!=FG_SUCCESS)throw std::runtime_error("FG SetTargetFps error="+std::to_string(targetFpsResult));
            // Public UI prediction is an explicit independent probe option.
            // The scene still contains no fabricated Vulkan UI draw.
            uiPredictionResult=fg.uiPrediction(fg.context,uiPredictionRequested);uiPredictionConfigured=true;
            fgCheck("FG-set-ui-prediction",uiPredictionResult);uiPredictionDisabled=!uiPredictionRequested;
            fgCheck("FG-activate",fg.activate(fg.context));fg.active=true;
            traceActiveState("FG-active-state-after-activation");
            fg.source=fg.image(fg.context,frameSourceImage(),frameSourceView());if(!fg.source)throw std::runtime_error("FG source image registration returned null");
            trace("FG-system-source-only","\"sdkOutputImageRegistered\":false,\"dispatchOutputInfoZero\":true");
        }
        trace("ready", "\"swapchainSize\":["+std::to_string(extent.width)+","+std::to_string(extent.height)+"],\"surfaceFormat\":"+std::to_string(surfaceFormat));
    }
    void draw(Image& image, bool bars, double sceneSeconds = 0) {
        VkClearValue background{};background.color.float32[0]=0.025f;background.color.float32[1]=0.045f;background.color.float32[2]=0.08f;background.color.float32[3]=1;
        auto rp=info<VkRenderPassBeginInfo>(VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO);rp.renderPass=renderPass;rp.framebuffer=image.framebuffer;rp.renderArea={{0,0},{W,H}};rp.clearValueCount=1;rp.pClearValues=&background;
        currentApi=bars?"own-source-renderpass-begin":"own-predicted-init-renderpass-begin";
        if(frames==0)trace("before-own-offscreen-begin","\"role\":"+quote(bars?"scene":"predicted-init")+",\"command\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(command)))+",\"renderPass\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(rp.renderPass)))+",\"framebuffer\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(rp.framebuffer))));
        vkCmdBeginRenderPass(command,&rp,VK_SUBPASS_CONTENTS_INLINE);
        if(frames==0)trace("after-own-offscreen-begin","\"role\":"+quote(bars?"scene":"predicted-init"));
        if(bars){for(uint32_t i=0;i<3;++i){VkClearAttachment color{};color.aspectMask=VK_IMAGE_ASPECT_COLOR_BIT;color.colorAttachment=0;color.clearValue.color.float32[3]=1;color.clearValue.color.float32[i]=1;color.clearValue.color.float32[(i+1)%3]=0.4f;
            const uint32_t x=muguet::fg_probe::movingBarX(sceneSeconds,i*320,W-96);VkClearRect rect{{{int32_t(x),int32_t(120+i*190)},{96,128}},0,1};vkCmdClearAttachments(command,1,&color,1,&rect);}}
        vkCmdEndRenderPass(command);
    }
    void imageBarrier(VkImage image,VkImageLayout before,VkImageLayout after,VkAccessFlags src,VkAccessFlags dst,VkPipelineStageFlags srcStage,VkPipelineStageFlags dstStage){
        auto b=info<VkImageMemoryBarrier>(VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER);b.oldLayout=before;b.newLayout=after;b.srcAccessMask=src;b.dstAccessMask=dst;b.srcQueueFamilyIndex=b.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;b.image=image;b.subresourceRange={VK_IMAGE_ASPECT_COLOR_BIT,0,1,0,1};vkCmdPipelineBarrier(command,srcStage,dstStage,0,0,nullptr,0,nullptr,1,&b);
    }
    void drawGeometryFinal(uint32_t index) {
        if(!geometryRequested||!geometryQualificationPassed||!finalPassCreated||index>=swapFramebuffers.size())throw std::runtime_error("unprepared geometry final pass");
        currentApi="own-final-source-fragment-barrier";
        // The scene's renderpass and public FG Dispatch both leave its color in
        // SHADER_READ_ONLY. Make their writes visible to the real sampled draw.
        imageBarrier(frameSourceImage(),VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,
            VK_ACCESS_SHADER_WRITE_BIT|VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,VK_ACCESS_SHADER_READ_BIT,
            VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,VK_PIPELINE_STAGE_FRAGMENT_SHADER_BIT);
        auto rp=info<VkRenderPassBeginInfo>(VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO);rp.renderPass=finalRenderPass;rp.framebuffer=swapFramebuffers[index];rp.renderArea={{0,0},extent};
        currentApi="own-final-color-renderpass-begin";
        if(frames==0)trace("before-own-final-begin","\"command\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(command)))+",\"renderPass\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(rp.renderPass)))+",\"framebuffer\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(rp.framebuffer)))+",\"sourceImage\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(frameSourceImage())))+",\"sourceView\":"+quote(std::to_string(reinterpret_cast<uintptr_t>(frameSourceView())))+",\"attachmentCount\":1,\"depthAttachment\":null");
        vkCmdBeginRenderPass(command,&rp,VK_SUBPASS_CONTENTS_INLINE);
        if(frames==0)trace("after-own-final-begin");
        vkCmdBindPipeline(command,VK_PIPELINE_BIND_POINT_GRAPHICS,finalPipeline);
        vkCmdBindDescriptorSets(command,VK_PIPELINE_BIND_POINT_GRAPHICS,finalPipelineLayout,0,1,&finalSet,0,nullptr);
        vkCmdDraw(command,3,1,0,0);vkCmdEndRenderPass(command);++finalDrawsRecorded;
    }
    void resetAcquireFenceForNextFrame() {
        if(!acquireFence)throw std::runtime_error("acquire fence is null before acquire");
        if(acquireFenceWaitComplete){
            currentApi="own-vkResetFences-acquire-next-frame";
            check(vkResetFences(device,1,&acquireFence),"vkResetFences acquire next frame");
            acquireFenceWaitComplete=false;++acquireFenceResets;
            if(frames<2)trace("own-acquire-fence-reset-next-frame","\"afterPreviousSubmitWait\":true,\"afterPreviousPresent\":true");
        }
    }
    void waitAcquiredImageFence() {
        currentApi="own-vkWaitForFences-acquire";const auto t=std::chrono::steady_clock::now();
        check(vkWaitForFences(device,1,&acquireFence,VK_TRUE,5'000'000'000ULL),"vkWaitForFences acquire");
        acquireFenceWaitNs+=nanosSince(t);acquireFencePending=false;acquireFenceWaitComplete=true;++acquireFenceWaits;
        // Deliberately keep it signaled through submission and presentation.
        // Its next reset is at the beginning of the next completed frame only.
        if(frames<2)trace("own-acquire-fence-wait-complete","\"leaveSignaledThroughPresent\":true");
    }
    void frame() {
        currentApi="own-vkWaitForFences";if(!fence)throw std::runtime_error("frame fence is null before wait");auto t=std::chrono::steady_clock::now();check(vkWaitForFences(device,1,&fence,VK_TRUE,5'000'000'000ULL),"vkWaitForFences");fenceNs+=nanosSince(t);pending=false;
        resetAcquireFenceForNextFrame();
        currentApi="own-vkAcquireNextImageKHR";uint32_t index=0;t=std::chrono::steady_clock::now();VkResult acquiredResult=vkAcquireNextImageKHR(device,swapchain,5'000'000'000ULL,acquired,acquireFence,&index);acquireNs+=nanosSince(t);
        if(acquiredResult!=VK_SUCCESS && acquiredResult!=VK_SUBOPTIMAL_KHR)check(acquiredResult,"vkAcquireNextImageKHR");acquireFencePending=true;if(index>=swapImages.size())throw std::runtime_error("acquired index outside image list");
        waitAcquiredImageFence();
        currentApi="own-reset-and-begin";check(vkResetFences(device,1,&fence),"vkResetFences");check(vkResetCommandPool(device,pool,0),"vkResetCommandPool");auto begin=info<VkCommandBufferBeginInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO);begin.flags=VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;check(vkBeginCommandBuffer(command,&begin),"vkBeginCommandBuffer");
        currentApi="source-renderpass";t=std::chrono::steady_clock::now();
        const double sceneSeconds=std::chrono::duration<double>(std::chrono::steady_clock::now()-started).count();
        if(geometryRequested){geometryScene->record(command,muguet::fg_probe::geometryAngle(sceneSeconds));}else{draw(scene,true,sceneSeconds);if(frames==0)draw(predicted,false);}
        sourceRecordNs+=nanosSince(t);
        if(enabled){FG_DispatchDescription_VK d{};d.inputColorInfo.image=fg.source;
            FG_ImageSync_VK sync{VK_ACCESS_SHADER_READ_BIT,VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,VK_PIPELINE_STAGE_ALL_COMMANDS_BIT};d.inputColorInfo.initialSync=d.inputColorInfo.finalSync=sync;
            // This is the real static screen-space camera of this constructed
            // synthetic scene. It is not a substitute for the game's camera.
            if(geometryRequested){std::copy(geometryScene->viewProj().begin(),geometryScene->viewProj().end(),d.viewProj.data);std::copy(geometryScene->invViewProj().begin(),geometryScene->invViewProj().end(),d.invViewProj.data);}else{for(int i:{0,5,10,15})d.viewProj.data[i]=d.invViewProj.data[i]=1;}d.vkCommandBuffer=command;d.frameIdx=0;
            if(dispatches==0)trace("before-first-FG-dispatch");currentApi="HMS_FG_Dispatch_VK";t=std::chrono::steady_clock::now();const auto code=fg.dispatch(fg.context,&d);dispatchNs+=nanosSince(t);if(dispatches==0){trace("after-first-FG-dispatch","\"FG_ErrorCode\":"+std::to_string(code));traceActiveState("FG-active-state-after-first-dispatch");}++dispatches;if(code==FG_SUCCESS)++successes;else if(code==FG_COLLECTING_PREVIOUS_FRAMES)++collecting;else{trace("FG-dispatch-error","\"FG_ErrorCode\":"+std::to_string(code));throw std::runtime_error("FG Dispatch error="+std::to_string(code));}}
        if(geometryRequested){drawGeometryFinal(index);}else{
        currentApi="own-final-transfer-blit";
        imageBarrier(frameSourceImage(),VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,VK_ACCESS_SHADER_READ_BIT|VK_ACCESS_SHADER_WRITE_BIT|VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,VK_ACCESS_TRANSFER_READ_BIT,VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT);
        imageBarrier(swapImages[index],VK_IMAGE_LAYOUT_UNDEFINED,VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,0,VK_ACCESS_TRANSFER_WRITE_BIT,VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT);
        VkImageBlit blit{};blit.srcSubresource=blit.dstSubresource={VK_IMAGE_ASPECT_COLOR_BIT,0,0,1};blit.srcOffsets[1]={int32_t(W),int32_t(H),1};blit.dstOffsets[1]={int32_t(extent.width),int32_t(extent.height),1};
        vkCmdBlitImage(command,frameSourceImage(),VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,swapImages[index],VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,1,&blit,VK_FILTER_NEAREST);
        imageBarrier(swapImages[index],VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,VK_ACCESS_TRANSFER_WRITE_BIT,0,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT);
        }
        check(vkEndCommandBuffer(command),"vkEndCommandBuffer");VkPipelineStageFlags stage=geometryRequested?VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT:VK_PIPELINE_STAGE_TRANSFER_BIT;auto submit=info<VkSubmitInfo>(VK_STRUCTURE_TYPE_SUBMIT_INFO);submit.waitSemaphoreCount=1;submit.pWaitSemaphores=&acquired;submit.pWaitDstStageMask=&stage;submit.commandBufferCount=1;submit.pCommandBuffers=&command;submit.signalSemaphoreCount=1;submit.pSignalSemaphores=&rendered[index];
        currentApi="own-vkQueueSubmit";
        check(vkQueueSubmit(queue,1,&submit,fence),"vkQueueSubmit");pending=true;++frames;
        auto present=info<VkPresentInfoKHR>(VK_STRUCTURE_TYPE_PRESENT_INFO_KHR);present.waitSemaphoreCount=1;present.pWaitSemaphores=&rendered[index];present.swapchainCount=1;present.pSwapchains=&swapchain;present.pImageIndices=&index;
        currentApi="own-vkQueuePresentKHR";t=std::chrono::steady_clock::now();auto code=vkQueuePresentKHR(queue,&present);presentNs+=nanosSince(t);if(code!=VK_SUCCESS && code!=VK_SUBOPTIMAL_KHR)check(code,"vkQueuePresentKHR");++realPresents;
        if(frames<=2){acquireFenceStatusAfterPresent=vkGetFenceStatus(device,acquireFence);trace("own-acquire-fence-after-present","\"VkResult\":"+std::to_string(acquireFenceStatusAfterPresent)+",\"acquireFenceNotResetInThisFrame\":true");}
        if(frames%sourceFps==0)trace("progress","\"dispatchCalls\":"+std::to_string(dispatches)+",\"dispatchSuccess\":"+std::to_string(successes)+",\"collecting\":"+std::to_string(collecting)+",\"realPresentSuccesses\":"+std::to_string(realPresents));
    }
    void destroyImage(Image& i){if(i.framebuffer)vkDestroyFramebuffer(device,i.framebuffer,nullptr);if(i.view)vkDestroyImageView(device,i.view,nullptr);if(i.image)vkDestroyImage(device,i.image,nullptr);if(i.memory)vkFreeMemory(device,i.memory,nullptr);i={};}
    void cleanup(){
        currentApi="teardown-vkDeviceWaitIdle";trace("before-device-idle-teardown");if(device)check(vkDeviceWaitIdle(device),"vkDeviceWaitIdle teardown");pending=false;
        if(acquireFencePending){currentApi="teardown-wait-acquire-fence";check(vkWaitForFences(device,1,&acquireFence,VK_TRUE,5'000'000'000ULL),"vkWaitForFences acquire teardown");acquireFencePending=false;}
        currentApi="FG-context-teardown";if(fg.context){if(fg.active){fgCheck("FG-deactivate",fg.deactivate(fg.context));fg.active=false;}if(fg.source){fgCheck("FG-destroy-source",fg.destroyImage(fg.context,fg.source));fg.source=nullptr;}if(fg.output){fgCheck("FG-destroy-output",fg.destroyImage(fg.context,fg.output));fg.output=nullptr;}fgCheck("FG-destroy-context",fg.destroy(&fg.context));fg.context=nullptr;}
        if(device){currentApi="own-vulkan-object-teardown";if(pool)vkDestroyCommandPool(device,pool,nullptr);pool=VK_NULL_HANDLE;
            if(finalPipeline)vkDestroyPipeline(device,finalPipeline,nullptr);finalPipeline=VK_NULL_HANDLE;
            if(finalVertex)vkDestroyShaderModule(device,finalVertex,nullptr);finalVertex=VK_NULL_HANDLE;
            if(finalFragment)vkDestroyShaderModule(device,finalFragment,nullptr);finalFragment=VK_NULL_HANDLE;
            if(finalPipelineLayout)vkDestroyPipelineLayout(device,finalPipelineLayout,nullptr);finalPipelineLayout=VK_NULL_HANDLE;
            if(finalDescriptorPool)vkDestroyDescriptorPool(device,finalDescriptorPool,nullptr);finalDescriptorPool=VK_NULL_HANDLE;finalSet=VK_NULL_HANDLE;
            if(finalSetLayout)vkDestroyDescriptorSetLayout(device,finalSetLayout,nullptr);finalSetLayout=VK_NULL_HANDLE;
            if(finalSampler)vkDestroySampler(device,finalSampler,nullptr);finalSampler=VK_NULL_HANDLE;
            for(auto f:swapFramebuffers)if(f)vkDestroyFramebuffer(device,f,nullptr);swapFramebuffers.clear();
            for(auto v:swapViews)if(v)vkDestroyImageView(device,v,nullptr);swapViews.clear();
            if(finalRenderPass)vkDestroyRenderPass(device,finalRenderPass,nullptr);finalRenderPass=VK_NULL_HANDLE;
            if(geometryScene){trace("before-geometry-scene-destroy");geometryScene.reset();geometrySceneDestroyed=true;trace("after-geometry-scene-destroy");}
            destroyImage(scene);destroyImage(predicted);if(renderPass)vkDestroyRenderPass(device,renderPass,nullptr);renderPass=VK_NULL_HANDLE;if(fence)vkDestroyFence(device,fence,nullptr);fence=VK_NULL_HANDLE;ownFenceSnapshot=0;if(acquireFence)vkDestroyFence(device,acquireFence,nullptr);acquireFence=VK_NULL_HANDLE;ownAcquireFenceSnapshot=0;if(acquired)vkDestroySemaphore(device,acquired,nullptr);acquired=VK_NULL_HANDLE;for(auto s:rendered)if(s)vkDestroySemaphore(device,s,nullptr);rendered.clear();if(swapchain)vkDestroySwapchainKHR(device,swapchain,nullptr);swapchain=VK_NULL_HANDLE;vkDestroyDevice(device,nullptr);device=VK_NULL_HANDLE;}
        if(surface)vkDestroySurfaceKHR(instance,surface,nullptr);surface=VK_NULL_HANDLE;if(messenger&&destroyMessenger)destroyMessenger(instance,messenger,nullptr);messenger=VK_NULL_HANDLE;if(instance)vkDestroyInstance(instance,nullptr);instance=VK_NULL_HANDLE;if(geometryWindowReference){OH_NativeWindow_NativeObjectUnreference(window);geometryWindowReference=false;}teardownComplete=true;trace("after-complete-teardown");
    }
    std::string result()const{
        const double elapsed=started.time_since_epoch().count()?std::chrono::duration<double>(std::chrono::steady_clock::now()-started).count():0;
        std::ostringstream r;
        r << "{\"status\":" << quote(retainPendingGraph?"inflight_retained_requires_force_stop":error.empty()&&teardownComplete&&validationErrors.load()==0?"completed_unverified_display":"error")
          << ",\"probe\":\"synthetic_ai_system_surface\",\"pid\":" << getpid()
          << ",\"requestedVulkanApiVersion\":" << VK_API_VERSION_1_3 << ",\"loaderApiVersion\":" << loaderApiVersion << ",\"physicalApiVersion\":" << physicalApiVersion
          << ",\"phase\":" << quote(phase) << ",\"error\":" << quote(error)
          << ",\"modelAliasAudit\":" << modelAliasAudit << ",\"modelAliasRestoreErrno\":" << modelAliasRestoreErrno
          << ",\"geometryRequested\":" << boolean(geometryRequested) << ",\"geometryQualificationPassed\":" << boolean(geometryQualificationPassed)
          << ",\"geometrySceneDestroyed\":" << boolean(geometrySceneDestroyed) << ",\"pendingGraphRetainedForProcessLifetime\":" << boolean(retainPendingGraph)
          << ",\"geometryWindowReferenceRetained\":" << boolean(geometryWindowReference) << ",\"aiDepthInputProvided\":false"
          << ",\"uiPredictionRequested\":" << boolean(uiPredictionRequested)
          << ",\"uiPredictionConfigured\":" << boolean(uiPredictionConfigured) << ",\"uiPredictionSetResult\":" << (uiPredictionConfigured?std::to_string(uiPredictionResult):"null")
          << ",\"uiPredictionExplicitlyDisabled\":" << boolean(uiPredictionDisabled)
          << ",\"swapchainSampledRequested\":" << boolean(swapchainSampledRequested) << ",\"swapchainSampledEnabled\":" << boolean((swapchainActualUsageFlags&VK_IMAGE_USAGE_SAMPLED_BIT)!=0)
          << ",\"swapchainSupportedUsageFlags\":" << swapchainSupportedUsageFlags << ",\"swapchainRequestedUsageFlags\":" << swapchainRequestedUsageFlags << ",\"swapchainActualUsageFlags\":" << swapchainActualUsageFlags
          << ",\"swapchainUsage\":" << quote(!swapchainActualUsageFlags?"not_created":(swapchainActualUsageFlags&VK_IMAGE_USAGE_SAMPLED_BIT)?"TRANSFER_DST|COLOR_ATTACHMENT|SAMPLED":"TRANSFER_DST|COLOR_ATTACHMENT")
          << ",\"finalDepthStateRequested\":" << boolean(finalDepthStateRequested) << ",\"finalDepthStateProvided\":" << boolean(finalDepthStateProvided)
          << ",\"finalDepthTestEnabled\":false,\"finalDepthWriteEnabled\":false,\"finalDepthBoundsTestEnabled\":false,\"finalStencilTestEnabled\":false"
          << ",\"finalPresentationPath\":" << quote(finalPassCreated?"qualified_geometry_fullscreen_sampled_color":geometryRequested?"geometry_final_pass_not_prepared":"transfer_blit_with_independent_real_acquire_fence")
          << ",\"finalTriangleVertexCount\":" << (finalPassCreated?3:0) << ",\"finalColorRenderpassResourcesCreated\":" << boolean(finalPassCreated) << ",\"finalDrawsRecorded\":" << finalDrawsRecorded
          << ",\"sdkOutputImageRegistered\":false,\"dispatchOutputInfoZero\":true"
          << ",\"fgEnabled\":" << boolean(enabled) << ",\"sourceResolution\":[1202,800],\"swapchainResolution\":[" << extent.width << ',' << extent.height << ']'
          << ",\"sourceTargetHz\":" << sourceFps << ",\"requestedOutputTargetHz\":" << requestedTargetFps
          << ",\"targetFpsSetCalled\":" << boolean(targetFpsConfigured) << ",\"targetFpsSetResult\":" << (targetFpsConfigured?std::to_string(targetFpsResult):"null")
          << ",\"targetFpsDocumentedEffectAfterFirstRelease\":true,\"effectiveOutputTargetHz\":null"
          << ",\"targetFpsEffectLimitation\":\"Public SetTargetFps takes effect after the game's first release; setter success does not establish its effect for a sideloaded package.\""
          << ",\"fusionAwareMetadata\":" << quote(fusionAwareMetadata) << ",\"systemPresentationMetadataSatisfied\":" << boolean(fusionAwareMetadata=="Vulkan")
          << ",\"motionClock\":\"steady_clock\",\"barSpeedPixelsPerSecond\":360,\"geometryMotionPeriodSeconds\":4"
          << ",\"elapsedSeconds\":" << elapsed << ",\"renderElapsedSeconds\":" << renderElapsed
          << ",\"sourceSubmitRateHz\":" << (renderElapsed>0?frames/renderElapsed:0)
          << ",\"sourceRecordApiWallNs\":" << sourceRecordNs << ",\"dispatchApiWallNs\":" << dispatchNs
          << ",\"realPresentApiWallNs\":" << presentNs << ",\"acquireApiWallNs\":" << acquireNs << ",\"fenceWaitApiWallNs\":" << fenceNs
          << ",\"acquireFenceWaitApiWallNs\":" << acquireFenceWaitNs << ",\"acquireFenceWaits\":" << acquireFenceWaits << ",\"acquireFenceResets\":" << acquireFenceResets << ",\"acquireFenceStatusAfterFirstPresents\":" << (realPresents?std::to_string(acquireFenceStatusAfterPresent):"null")
          << ",\"sourceFrames\":" << frames << ",\"realPresentSuccesses\":" << realPresents << ",\"dispatchCalls\":" << dispatches
          << ",\"dispatchSuccessReturns\":" << successes << ",\"collectingReturns\":" << collecting
          << ",\"stoppedEarly\":" << boolean(stop.load()) << ",\"requestedSourceFrames\":" << seconds*sourceFps
          << ",\"generatedFramesVerified\":null,\"displayFpsVerified\":null,\"sourceTimesTwoIsNotEvidence\":true"
          << ",\"completeTeardown\":" << boolean(teardownComplete) << ",\"libraryUnloaded\":false,\"libraryRetainedForProcessLifetime\":true,\"publicLibraryIntegration\":\"ELF_DT_NEEDED_direct_symbols\",\"lateFgDlopenCalled\":false,\"normalGameStarted\":false"
          << ",\"validationErrorsObserved\":" << (callbackInstalled?std::to_string(validationErrors.load()):"null")
          << ",\"validationWarningsObserved\":" << (callbackInstalled?std::to_string(validationWarnings.load()):"null")
          << ",\"validationCallbackInstalled\":" << boolean(callbackInstalled) << ",\"validationLayerEnabled\":" << boolean(validationEnabled)
          << ",\"validationNegativeControlObserved\":" << boolean(negativeObserved.load()) << ",\"uniqueHandlesOnlyDisabled\":" << boolean(handleWrappingDisabled)
          << ",\"validationMessagesTruncated\":" << boolean(validationLines>64)
          << ",\"nullFenceVuidMessages\":" << nullFenceMessages.load() << ",\"nullFenceCallbackStacksSaved\":" << nullFenceStacksSaved.load()
          << ",\"nullFenceCallbackStackLimit\":2,\"callbackStackFrameLimit\":32"
          << ",\"renderPassCallbackStacksSaved\":" << otherCallbackStacksSaved.load() << ",\"renderPassCallbackStackLimit\":3"
          << ",\"observationScope\":\"Real synthetic color rendering, optional no-depth AI Dispatch and real Vulkan swapchain presentation. Timings are CPU API wall durations, not GPU or scanout timings. Source, Dispatch and real present API counts do not verify generated/display frames. Public library retained for process lifetime.\"}";
        return r.str();
    }
    bool waitForNativeDebugger() {
        // The marker is fixed inside this app's filesDir. No caller-supplied
        // debugger path or GPU resource is used before this bounded wait.
        const std::string marker=directory+"/../fg-debug-wait";
        if(access(marker.c_str(),F_OK)!=0)return !stop.load();
        const auto waitStart=std::chrono::steady_clock::now();
        const auto deadline=waitStart+std::chrono::seconds(30);
        trace("waiting-for-native-debugger","\"marker\":"+quote(marker)+",\"maxWaitMs\":30000,\"pollIntervalMs\":50,\"gpuResourcesCreated\":false");
        std::string reason;int markerError=0;
        while(true){
            if(stop.load()){reason="stop_requested";break;}
            errno=0;
            if(access(marker.c_str(),F_OK)!=0){markerError=errno;reason=markerError==ENOENT?"marker_removed":"marker_unavailable";break;}
            const auto now=std::chrono::steady_clock::now();
            if(now>=deadline){reason="timeout";break;}
            std::this_thread::sleep_until(std::min(deadline,now+std::chrono::milliseconds(50)));
        }
        const auto elapsedMs=std::chrono::duration_cast<std::chrono::milliseconds>(std::chrono::steady_clock::now()-waitStart).count();
        trace("native-debugger-wait-ended","\"reason\":"+quote(reason)+",\"waitedMs\":"+std::to_string(elapsedMs)+",\"markerErrno\":"+std::to_string(markerError)+",\"stopRequested\":"+boolean(stop.load())+",\"gpuResourcesCreated\":false");
        return !stop.load();
    }
    std::string run(){
        std::unique_ptr<muguet::fg_probe::ModelAliasScope> alias;
        auto restoreAlias=[&] {
            if(!alias)return;
            modelAliasRestoreErrno=alias->restore();
            if(modelAliasRestoreErrno){if(!error.empty())error+="; ";error+="cwd restoration failed errno="+std::to_string(modelAliasRestoreErrno);}
            try{trace("model-alias-cwd-restored","\"restoreErrno\":"+std::to_string(modelAliasRestoreErrno));}catch(...){}
        };
        try{
            if(waitForNativeDebugger()&&!stop.load()){
                alias=std::make_unique<muguet::fg_probe::ModelAliasScope>(directory+"/..");
                modelAliasAudit=alias->prepareIfRequested();
                trace("model-alias-preparation","\"audit\":"+modelAliasAudit);
                if(modelAliasAudit.find("\"status\":\"failed\"")!=std::string::npos)
                    throw std::runtime_error("model path adaptation failed; see modelAliasAudit");
                setup();started=std::chrono::steady_clock::now();
                for(uint32_t i=0;i<seconds*sourceFps&&!stop.load();++i){
                    std::this_thread::sleep_until(started+std::chrono::nanoseconds(muguet::fg_probe::sourceDeadlineNs(i,sourceFps)));
                    if(stop.load())break;frame();
                }
            }
            finished=true;
        }catch(const std::exception& e){error=e.what();try{trace("error","\"detail\":"+quote(error));}catch(...) {}}
        if(started.time_since_epoch().count())renderElapsed=std::chrono::duration<double>(std::chrono::steady_clock::now()-started).count();
        if(retainPendingGraph){restoreAlias();try{trace("qualification-pending-graph-retained","\"forceStopRequired\":true,\"cleanupSkipped\":true");}catch(...){}auto r=result();save(directory+"/result.json",r+'\n');return r;}
        try{cleanup();}catch(const std::exception& e){if(geometryRequested&&geometryScene)retainPendingGraph=true;if(!error.empty())error+="; ";error+=std::string("teardown: ")+e.what();}
        restoreAlias();
        try{trace("before-library-retire","\"willDlclose\":false");trace("after-library-retire","\"retainedForProcessLifetime\":true");}catch(const std::exception& e){error+=std::string("; trace: ")+e.what();}auto r=result();save(directory+"/result.json",r+'\n');return r;}
};
struct Engine {
    std::mutex mutex,shutdownMutex;std::thread worker;std::atomic<bool> stop{false};OHNativeWindow* window=nullptr;
    std::string directory,latest="{\"status\":\"not_started\"}",fusionAwareMetadata="unavailable";bool configured=false,enabled=true,uiPrediction=false,geometry=false,swapchainSampled=false,finalDepthState=false,rifeStream=false,started=false;uint32_t seconds=20,sourceFps=muguet::fg_probe::kDefaultSourceFps,targetFps=muguet::fg_probe::kDefaultTargetFps;
    ~Engine(){shutdown();}
    void startLocked(){if(!configured||!window||started)return;started=true;stop=false;auto w=window;auto path=directory;auto on=enabled;auto ui=uiPrediction;auto useGeometry=geometry;auto duration=seconds;auto source=sourceFps;auto target=targetFps;auto metadata=fusionAwareMetadata;auto sampled=swapchainSampled;auto depthState=finalDepthState;auto useRife=rifeStream;
        latest="{\"status\":\"running\",\"pid\":"+std::to_string(getpid())+",\"directory\":"+quote(path)+",\"sourceTargetHz\":"+std::to_string(source)+",\"requestedOutputTargetHz\":"+std::to_string(target)+",\"fusionAwareMetadata\":"+quote(metadata)+",\"swapchainSampledRequested\":"+boolean(sampled)+",\"finalDepthStateRequested\":"+boolean(depthState)+",\"rifeStream\":"+boolean(useRife)+"}";worker=std::thread([this,w,path,on,ui,useGeometry,duration,source,target,metadata,sampled,depthState,useRife]{std::string json;if(useRife){json=muguet::fg_probe::runRifeSurface(w,path,metadata,duration,stop);}else{auto trial=std::make_unique<Trial>(w,path,on,ui,useGeometry,duration,source,target,metadata,sampled,depthState,stop);json=trial->run();if(trial->retainPendingGraph){(void)trial.release();}/* Pending graph is intentionally never destroyed; root force-stops this process. */}std::lock_guard<std::mutex>lock(mutex);latest=std::move(json);});}
    void surface(void* w){std::lock_guard<std::mutex>lock(mutex);if(window)return;window=static_cast<OHNativeWindow*>(w);if(OH_NativeWindow_NativeObjectReference(window)!=0){window=nullptr;latest="{\"status\":\"error\",\"error\":\"native window reference failed\"}";return;}startLocked();}
    void shutdown(){std::lock_guard<std::mutex> serial(shutdownMutex);std::thread joining;{std::lock_guard<std::mutex>lock(mutex);stop=true;joining=std::move(worker);}if(joining.joinable())joining.join();std::lock_guard<std::mutex>lock(mutex);if(window)OH_NativeWindow_NativeObjectUnreference(window);window=nullptr;}
} engine;
std::string readString(napi_env env,napi_value v){size_t n=0;if(napi_get_value_string_utf8(env,v,nullptr,0,&n)!=napi_ok||n>4096)return{};std::string s(n+1,'\0');if(napi_get_value_string_utf8(env,v,s.data(),s.size(),&n)!=napi_ok)return{};s.resize(n);return s;}
struct ModelMetadataWork {
    napi_async_work work=nullptr;
    napi_deferred deferred=nullptr;
    std::string root,result;
    bool rifeAudit=false;
    bool npuAudit=false;
};
napi_value scheduleDiagnostic(napi_env env,napi_callback_info cb,bool rifeAudit,bool npuAudit=false) {
    size_t count=1;napi_value args[1]{};
    napi_get_cb_info(env,cb,&count,args,nullptr,nullptr);
    auto task=std::make_unique<ModelMetadataWork>();
    task->rifeAudit=rifeAudit;
    task->npuAudit=npuAudit;
    task->root=count?readString(env,args[0]):"";
    if(task->root.empty()){napi_throw_error(env,nullptr,"Expected own files directory");return nullptr;}
    napi_value promise=nullptr,name=nullptr;
    if(napi_create_promise(env,&task->deferred,&promise)!=napi_ok||
        napi_create_string_utf8(env,npuAudit?"NpuFlowInterpolationAudit":rifeAudit?"RifeInterpolationAudit":"FgModelMetadataAudit",NAPI_AUTO_LENGTH,&name)!=napi_ok){
        napi_throw_error(env,nullptr,"Cannot create model metadata work");return nullptr;
    }
    auto execute=[](napi_env,void* data){
        auto& work=*static_cast<ModelMetadataWork*>(data);
        try {work.result=work.npuAudit?muguet::fg_probe::inspectNpuFlowInterpolation(work.root):work.rifeAudit?muguet::fg_probe::inspectRifeInterpolation(work.root):muguet::fg_probe::inspectSystemModelMetadata();}
        catch(const std::exception& e){work.result="{\"status\":\"error\",\"message\":"+quote(e.what())+"}";}
        catch(...){work.result="{\"status\":\"error\",\"message\":\"Unknown metadata audit failure\"}";}
        if(!save(work.root+(work.npuAudit?"/npu-flow-benchmark-result.json":work.rifeAudit?"/rife-benchmark-result.json":"/fg-model-metadata-audit.json"),work.result+'\n'))
            work.result="{\"status\":\"error\",\"message\":\"Cannot save model metadata audit\"}";
    };
    auto complete=[](napi_env environment,napi_status status,void* data){
        std::unique_ptr<ModelMetadataWork> work(static_cast<ModelMetadataWork*>(data));
        napi_value result=nullptr;
        const std::string output=status==napi_ok?work->result:"{\"status\":\"cancelled\"}";
        napi_create_string_utf8(environment,output.c_str(),output.size(),&result);
        napi_resolve_deferred(environment,work->deferred,result);
        napi_delete_async_work(environment,work->work);
    };
    if(napi_create_async_work(env,nullptr,name,execute,complete,task.get(),&task->work)!=napi_ok){
        napi_throw_error(env,nullptr,"Cannot create model metadata work");return nullptr;
    }
    if(napi_queue_async_work(env,task->work)!=napi_ok){
        napi_delete_async_work(env,task->work);napi_throw_error(env,nullptr,"Cannot queue model metadata work");return nullptr;
    }
    task.release();return promise;
}
napi_value inspectModelMetadata(napi_env env,napi_callback_info cb){return scheduleDiagnostic(env,cb,false);}
napi_value inspectRife(napi_env env,napi_callback_info cb){return scheduleDiagnostic(env,cb,true);}
napi_value inspectNpuFlow(napi_env env,napi_callback_info cb){return scheduleDiagnostic(env,cb,false,true);}
constexpr char modelPathScope[]="known_IsSupported_check_paths_not_confirmed_core_initialization_failure_paths";
std::string inspectCapabilityModelPaths() {
    // These fixed filenames come from the inspected public NDK capability
    // check. A successful metadata/open check does not load or validate a model,
    // and does not identify the filename in the core's canonicalPath failure.
    constexpr const char* paths[]={
        "/sys_prod/etc/graphics_game/ai_model/aifi1.om",
        "/sys_prod/etc/graphics_game/ai_model/aifi2.om"
    };
    constexpr const char* directories[]={
        "/sys_prod/etc/graphics_game/ai_model",
        "/system/etc/graphics_game/AIModel"
    };
    char cwd[4096]{};errno=0;
    const bool cwdSucceeded=getcwd(cwd,sizeof(cwd))!=nullptr;
    const int cwdError=cwdSucceeded?0:errno;
    std::ostringstream out;
    out<<"{\"pid\":"<<getpid()<<",\"pathScope\":"<<quote(modelPathScope)
       <<",\"operations\":\"stat_realpath_open_readonly_cloexec_close_fixed_directory_names_getcwd_only\",\"modelBytesRead\":0,\"modelReadyVerified\":false"
       <<",\"getcwdBufferBytes\":"<<sizeof(cwd)<<",\"getcwdSucceeded\":"<<boolean(cwdSucceeded)<<",\"getcwdReturn\":";
    if(cwdSucceeded)out<<quote(cwd);else out<<"null";
    out<<",\"getcwdErrno\":"<<cwdError<<",\"paths\":[";
    bool first=true;
    for(const char* path:paths){
        struct stat metadata{};errno=0;
        const int statResult=stat(path,&metadata);
        const int statError=statResult<0?errno:0;
        errno=0;char* canonical=realpath(path,nullptr);
        const bool canonicalSucceeded=canonical!=nullptr;
        const int canonicalError=canonicalSucceeded?0:errno;
        const std::string canonicalPath=canonicalSucceeded?canonical:"";
        std::free(canonical);
        errno=0;const int fd=open(path,O_RDONLY|O_CLOEXEC);
        const int openError=fd<0?errno:0;
        int closeResult=0,closeError=0;
        if(fd>=0){errno=0;closeResult=close(fd);closeError=closeResult<0?errno:0;}
        if(!first)out<<',';first=false;
        out<<"{\"path\":"<<quote(path)<<",\"statReturn\":"<<statResult<<",\"statErrno\":"<<statError<<",\"sizeBytes\":";
        if(statResult==0)out<<metadata.st_size;else out<<"null";
        out<<",\"realpathSucceeded\":"<<boolean(canonicalSucceeded)<<",\"canonicalPath\":";
        if(canonicalSucceeded)out<<quote(canonicalPath);else out<<"null";
        out<<",\"realpathErrno\":"<<canonicalError<<",\"openFlags\":\"O_RDONLY|O_CLOEXEC\",\"openReturn\":"<<fd<<",\"openErrno\":"<<openError
           <<",\"closeReturn\":";
        if(fd>=0)out<<closeResult;else out<<"null";
        out<<",\"closeErrno\":";
        if(fd>=0)out<<closeError;else out<<"null";
        out<<'}';
    }
    out<<"],\"directoryScope\":\"fixed_Core_whitelist_directories_names_only_no_recursion\",\"directoryEntryLimit\":64,\"directories\":[";
    first=true;
    for(const char* path:directories){
        errno=0;DIR* directoryHandle=opendir(path);
        const bool opened=directoryHandle!=nullptr;
        const int openError=opened?0:errno;
        if(!first)out<<',';first=false;
        out<<"{\"path\":"<<quote(path)<<",\"opendirSucceeded\":"<<boolean(opened)<<",\"opendirErrno\":"<<openError<<",\"entries\":[";
        uint32_t count=0;int readError=0,closeResult=0,closeError=0;
        bool reachedEnd=false;
        if(opened){
            while(count<64){
                errno=0;dirent* entry=readdir(directoryHandle);
                if(!entry){readError=errno;reachedEnd=readError==0;break;}
                if(std::strcmp(entry->d_name,".")==0||std::strcmp(entry->d_name,"..")==0)continue;
                if(count++)out<<',';
                out<<"{\"name\":"<<quote(entry->d_name)<<",\"dtype\":"<<unsigned(entry->d_type)<<'}';
            }
            errno=0;closeResult=closedir(directoryHandle);closeError=closeResult<0?errno:0;
        }
        out<<"],\"entryCount\":"<<count<<",\"entryLimitReached\":"<<boolean(count==64)<<",\"readdirReachedEnd\":"<<boolean(reachedEnd)<<",\"readdirErrno\":";
        if(opened)out<<readError;else out<<"null";
        out<<",\"closedirReturn\":";
        if(opened)out<<closeResult;else out<<"null";
        out<<",\"closedirErrno\":";
        if(opened)out<<closeError;else out<<"null";
        out<<'}';
    }
    out<<"]}";return out.str();
}
napi_value inspectPublicInterfaces(napi_env env,napi_callback_info cb) {
    size_t count=1; napi_value args[1]{};
    napi_get_cb_info(env,cb,&count,args,nullptr,nullptr);
    const std::string root=count?readString(env,args[0]):"";
    const std::string modelAudit=inspectCapabilityModelPaths();
    const bool modelAuditPersisted=!root.empty()&&save(root+"/fg-model-access-audit.json",modelAudit+'\n');
    const bool syscap=canIUse("SystemCapability.GraphicsGame.RenderAccelerate");
    struct Feature { FG_FeatureType value; const char* name; } features[] = {
        {INTERPOLATION_GPU_GLES,"INTERPOLATION_GPU_GLES"},
        {INTERPOLATION_GPU_VULKAN,"INTERPOLATION_GPU_VULKAN"},
        {EXTRAPOLATION_GPU_GLES,"EXTRAPOLATION_GPU_GLES"},
        {EXTRAPOLATION_GPU_VULKAN,"EXTRAPOLATION_GPU_VULKAN"},
        {INTERPOLATION_AI_VULKAN,"INTERPOLATION_AI_VULKAN"}
    };
    FG api;api.load();
    struct Symbol { const char* name; void* address; };
    const Symbol symbols[]={
        {"HMS_FG_IsFrameGenerationSupported",reinterpret_cast<void*>(api.supported)},
        {"HMS_FG_CreateContext_VK",reinterpret_cast<void*>(api.create)},
        {"HMS_FG_SetAlgorithmMode_VK",reinterpret_cast<void*>(api.algorithm)},
        {"HMS_FG_SetResolution_VK",reinterpret_cast<void*>(api.resolution)},
        {"HMS_FG_SetImageFormat_VK",reinterpret_cast<void*>(api.format)},
        {"HMS_FG_SetIntegrationMode_VK",reinterpret_cast<void*>(api.integration)},
        {"HMS_FG_SetTargetFps_VK",reinterpret_cast<void*>(api.targetFps)},
        {"HMS_FG_SetUiPredictionEnabled_VK",reinterpret_cast<void*>(api.uiPrediction)},
        {"HMS_FG_Activate_VK",reinterpret_cast<void*>(api.activate)},
        {"HMS_FG_IsActive_VK",reinterpret_cast<void*>(api.isActive)},
        {"HMS_FG_CreateImage_VK",reinterpret_cast<void*>(api.image)},
        {"HMS_FG_Dispatch_VK",reinterpret_cast<void*>(api.dispatch)},
        {"HMS_FG_DestroyImage_VK",reinterpret_cast<void*>(api.destroyImage)},
        {"HMS_FG_Deactivate_VK",reinterpret_cast<void*>(api.deactivate)},
        {"HMS_FG_DestroyContext_VK",reinterpret_cast<void*>(api.destroy)}
    };
    std::ostringstream out;
    out<<"{\"pid\":"<<getpid()<<",\"scope\":\"public_interfaces_only_no_Vulkan_context_or_surface_created\",\"pathScope\":"<<quote(modelPathScope)
       <<",\"modelAccessAuditFile\":\"fg-model-access-audit.json\",\"modelAccessAuditPersisted\":"<<boolean(modelAuditPersisted)
       <<",\"renderAccelerateSyscap\":"<<boolean(syscap)<<",\"features\":[";
    bool first=true;
    for(const auto& feature:features){
        if(!first)out<<',';first=false;
        out<<"{\"name\":"<<quote(feature.name)<<",\"enumValue\":"<<int(feature.value)<<",\"supported\":";
        if(syscap)out<<boolean(HMS_FG_IsFrameGenerationSupported(feature.value));else out<<"null";
        out<<'}';
    }
    out<<"],\"publicSymbols\":[";first=true;
    for(const auto& symbol:symbols){
        const char* name=symbol.name;
        if(!first)out<<',';first=false;
        void* address=symbol.address;Dl_info owner{};
        const bool resolved=address&&dladdr(address,&owner)!=0;
        out<<"{\"name\":"<<quote(name)<<",\"available\":"<<boolean(address!=nullptr)
           <<",\"binding\":\"direct_ELF_dependency\",\"globalLookupVisible\":"<<boolean(dlsym(RTLD_DEFAULT,name)!=nullptr)
           <<",\"library\":"<<quote(resolved&&owner.dli_fname?owner.dli_fname:"")
           <<",\"relativeAddress\":";
        if(resolved&&owner.dli_fbase)out<<reinterpret_cast<uintptr_t>(address)-reinterpret_cast<uintptr_t>(owner.dli_fbase);else out<<"null";
        out<<'}';
    }
    out<<"],\"modelReadyVerified\":false,\"generatedFramesVerified\":null}";
    std::string result=out.str();
    if(root.empty()||!save(root+"/fg-public-interface-audit.json",result+'\n'))
        result="{\"status\":\"error\",\"reason\":\"Cannot persist public interface audit\"}";
    napi_value value;napi_create_string_utf8(env,result.c_str(),result.size(),&value);return value;
}
napi_value configure(napi_env env,napi_callback_info cb) {
    size_t n=12; napi_value a[12]{};
    const bool read=napi_get_cb_info(env,cb,&n,a,nullptr,nullptr)==napi_ok;
    bool on=true,ui=false,geometry=false,sampled=false,depthState=false,rife=false;
    double duration=20,source=muguet::fg_probe::kDefaultSourceFps,target=muguet::fg_probe::kDefaultTargetFps;
    std::string root=n?readString(env,a[0]):"",metadata="unavailable";
    bool ok=read&&n>=3&&n<=11&&!root.empty()&&
        napi_get_value_bool(env,a[1],&on)==napi_ok&&
        napi_get_value_double(env,a[2],&duration)==napi_ok&&std::isfinite(duration)&&duration>=3&&duration<=60&&std::floor(duration)==duration&&
        (n<4||napi_get_value_bool(env,a[3],&ui)==napi_ok)&&
        (n<5||napi_get_value_bool(env,a[4],&geometry)==napi_ok)&&
        (n<6||napi_get_value_double(env,a[5],&source)==napi_ok)&&
        (n<7||napi_get_value_double(env,a[6],&target)==napi_ok)&&
        (n<9||napi_get_value_bool(env,a[8],&sampled)==napi_ok)&&
        (n<10||napi_get_value_bool(env,a[9],&depthState)==napi_ok)&&
        (n<11||napi_get_value_bool(env,a[10],&rife)==napi_ok)&&
        muguet::fg_probe::validFrameRates(source,target,on)&&
        (!rife||(!on&&!ui&&!geometry&&source==15&&target==30));
    if(ok&&n>=8){metadata=readString(env,a[7]);ok=!metadata.empty();}
    {
        std::lock_guard<std::mutex>lock(engine.mutex);
        if(engine.started)ok=false;
        else if(!ok)engine.latest="{\"status\":\"configuration_error\",\"error\":\"Expected files directory, boolean FG, integer seconds 3..60, optional boolean UI/geometry, integer source FPS 15..60 and output target FPS 30..144, metadata string, boolean swapchain sampling and boolean final depth state; enabled FG requires source < target.\"}";
        else {
            engine.directory=root+"/graphics-frame-surface-"+std::to_string(getpid());
            ok=mkdir(engine.directory.c_str(),0700)==0||errno==EEXIST;
            if(ok){
                engine.enabled=on;engine.uiPrediction=ui;engine.geometry=geometry;engine.seconds=uint32_t(duration);
                engine.swapchainSampled=sampled;engine.finalDepthState=depthState;engine.rifeStream=rife;
                engine.sourceFps=uint32_t(source);engine.targetFps=uint32_t(target);engine.fusionAwareMetadata=metadata;engine.configured=true;
                engine.latest="{\"status\":\"configured\",\"sourceTargetHz\":"+std::to_string(engine.sourceFps)+",\"requestedOutputTargetHz\":"+std::to_string(engine.targetFps)+",\"fusionAwareMetadata\":"+quote(metadata)+",\"swapchainSampledRequested\":"+(sampled?"true":"false")+",\"finalDepthStateRequested\":"+(depthState?"true":"false")+"}";
                engine.startLocked();
            }else engine.latest="{\"status\":\"configuration_error\",\"error\":\"Cannot create independent probe result directory.\"}";
        }
    }
    napi_value r;napi_get_boolean(env,ok,&r);return r;
}
napi_value status(napi_env env,napi_callback_info){std::lock_guard<std::mutex>lock(engine.mutex);napi_value r;napi_create_string_utf8(env,engine.latest.c_str(),engine.latest.size(),&r);return r;}
napi_value shutdown(napi_env env,napi_callback_info){engine.shutdown();napi_value r;napi_get_undefined(env,&r);return r;}
void created(OH_NativeXComponent*,void* w){engine.surface(w);}void changed(OH_NativeXComponent*,void*){}void destroyed(OH_NativeXComponent*,void*){engine.shutdown();}void touch(OH_NativeXComponent*,void*){}
napi_value init(napi_env env,napi_value exports){napi_property_descriptor p[]={{"configure",nullptr,configure,nullptr,nullptr,nullptr,napi_default,nullptr},{"status",nullptr,status,nullptr,nullptr,nullptr,napi_default,nullptr},{"stop",nullptr,shutdown,nullptr,nullptr,nullptr,napi_default,nullptr},{"inspectPublicInterfaces",nullptr,inspectPublicInterfaces,nullptr,nullptr,nullptr,napi_default,nullptr},{"inspectModelMetadata",nullptr,inspectModelMetadata,nullptr,nullptr,nullptr,napi_default,nullptr},{"inspectRife",nullptr,inspectRife,nullptr,nullptr,nullptr,napi_default,nullptr},{"inspectNpuFlow",nullptr,inspectNpuFlow,nullptr,nullptr,nullptr,napi_default,nullptr}};napi_define_properties(env,exports,7,p);
    napi_value object=nullptr;OH_NativeXComponent* xc=nullptr;if(napi_get_named_property(env,exports,OH_NATIVE_XCOMPONENT_OBJ,&object)==napi_ok&&napi_unwrap(env,object,reinterpret_cast<void**>(&xc))==napi_ok&&xc){static OH_NativeXComponent_Callback callbacks{created,changed,destroyed,touch};OH_NativeXComponent_RegisterCallback(xc,&callbacks);}return exports;}
napi_module module={1,0,nullptr,init,"graphics_frame_surface",nullptr,{0}};
__attribute__((constructor)) void registerModule(){napi_module_register(&module);}
}
