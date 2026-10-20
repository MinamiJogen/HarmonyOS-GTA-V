#define VK_USE_PLATFORM_OHOS 1
#include "fg_rife_surface.h"
#include "fg_rife_stream.h"
#include <vulkan/vulkan.h>
#include <native_window/external_window.h>
#include <algorithm>
#include <chrono>
#include <cstring>
#include <fcntl.h>
#include <sstream>
#include <stdexcept>
#include <thread>
#include <unistd.h>
#include <vector>

namespace muguet::fg_probe {
namespace {
template<class T>T Init(VkStructureType type){T value{};value.sType=type;return value;}
std::string Quote(const std::string& s){std::string out="\"";for(unsigned char c:s){if(c=='"'||c=='\\'){out+='\\';out+=char(c);}else if(c<32)out+=' ';else out+=char(c);}return out+'"';}
void Check(VkResult result,const char* operation){if(result!=VK_SUCCESS)throw std::runtime_error(std::string(operation)+" result="+std::to_string(result));}
bool Save(const std::string& path,const std::string& data){int fd=open(path.c_str(),O_CREAT|O_TRUNC|O_WRONLY|O_CLOEXEC,0600);if(fd<0)return false;size_t n=0;while(n<data.size()){ssize_t count=write(fd,data.data()+n,data.size()-n);if(count<0&&errno==EINTR)continue;if(count<=0){close(fd);return false;}n+=size_t(count);}const bool ok=fsync(fd)==0;const int closed=close(fd);return ok&&closed==0;}
uint64_t Hash(const std::vector<uint8_t>& bytes){uint64_t h=14695981039346656037ULL;for(uint8_t b:bytes){h^=b;h*=1099511628211ULL;}return h;}
constexpr uint32_t Width=320,Height=192;
struct Display {
    VkInstance instance=VK_NULL_HANDLE;VkPhysicalDevice physical=VK_NULL_HANDLE;
    VkDevice device=VK_NULL_HANDLE;VkQueue queue=VK_NULL_HANDLE;uint32_t family=0;
    VkSurfaceKHR surface=VK_NULL_HANDLE;VkSwapchainKHR swapchain=VK_NULL_HANDLE;VkExtent2D extent{};
    VkFormat format=VK_FORMAT_UNDEFINED;VkCommandPool pool=VK_NULL_HANDLE;VkCommandBuffer command=VK_NULL_HANDLE;
    VkFence fence=VK_NULL_HANDLE;VkSemaphore acquired=VK_NULL_HANDLE;
    std::vector<VkImage> swapImages;std::vector<VkSemaphore> rendered;
    VkBuffer staging=VK_NULL_HANDLE;VkDeviceMemory stagingMemory=VK_NULL_HANDLE;void* mapped=nullptr;bool coherent=false;
    VkImage image=VK_NULL_HANDLE;VkDeviceMemory imageMemory=VK_NULL_HANDLE;
    bool retired=false;uint64_t presents=0;std::string gpu;
    uint32_t MemoryType(uint32_t bits,VkMemoryPropertyFlags required,VkMemoryPropertyFlags preferred=0){
        VkPhysicalDeviceMemoryProperties p{};vkGetPhysicalDeviceMemoryProperties(physical,&p);uint32_t fallback=UINT32_MAX;
        for(uint32_t i=0;i<p.memoryTypeCount;++i)if((bits&(1u<<i))&&(p.memoryTypes[i].propertyFlags&required)==required){
            if((p.memoryTypes[i].propertyFlags&preferred)==preferred)return i;fallback=i;}
        if(fallback==UINT32_MAX)throw std::runtime_error("display memory type missing");return fallback;
    }
    void Create(OHNativeWindow* window){
        auto app=Init<VkApplicationInfo>(VK_STRUCTURE_TYPE_APPLICATION_INFO);app.pApplicationName="Independent online RIFE display";app.apiVersion=VK_API_VERSION_1_1;
        const char* extensions[]={VK_KHR_SURFACE_EXTENSION_NAME,VK_OHOS_SURFACE_EXTENSION_NAME};
        auto ic=Init<VkInstanceCreateInfo>(VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO);ic.pApplicationInfo=&app;ic.enabledExtensionCount=2;ic.ppEnabledExtensionNames=extensions;
        Check(vkCreateInstance(&ic,nullptr,&instance),"vkCreateInstance");
        if(OH_NativeWindow_NativeWindowHandleOpt(window,SET_BUFFER_GEOMETRY,int(Width),int(Height))!=0)throw std::runtime_error("display native geometry rejected");
        auto makeSurface=reinterpret_cast<PFN_vkCreateSurfaceOHOS>(vkGetInstanceProcAddr(instance,"vkCreateSurfaceOHOS"));
        if(!makeSurface)throw std::runtime_error("vkCreateSurfaceOHOS missing");
        auto si=Init<VkSurfaceCreateInfoOHOS>(VK_STRUCTURE_TYPE_SURFACE_CREATE_INFO_OHOS);si.window=window;
        Check(makeSurface(instance,&si,nullptr,&surface),"vkCreateSurfaceOHOS");
        uint32_t n=0;Check(vkEnumeratePhysicalDevices(instance,&n,nullptr),"physical device count");if(!n||n>32)throw std::runtime_error("invalid display devices");
        std::vector<VkPhysicalDevice> devices(n);Check(vkEnumeratePhysicalDevices(instance,&n,devices.data()),"physical devices");physical=devices.front();
        VkPhysicalDeviceProperties properties{};vkGetPhysicalDeviceProperties(physical,&properties);gpu=properties.deviceName;
        vkGetPhysicalDeviceQueueFamilyProperties(physical,&n,nullptr);if(!n||n>256)throw std::runtime_error("invalid display queues");
        std::vector<VkQueueFamilyProperties> families(n);vkGetPhysicalDeviceQueueFamilyProperties(physical,&n,families.data());bool found=false;
        for(uint32_t i=0;i<n;++i){VkBool32 supported=VK_FALSE;Check(vkGetPhysicalDeviceSurfaceSupportKHR(physical,i,surface,&supported),"present support");if(supported&&families[i].queueCount&&(families[i].queueFlags&VK_QUEUE_GRAPHICS_BIT)){family=i;found=true;break;}}
        if(!found)throw std::runtime_error("no common display queue");float priority=1;
        auto qi=Init<VkDeviceQueueCreateInfo>(VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO);qi.queueFamilyIndex=family;qi.queueCount=1;qi.pQueuePriorities=&priority;
        const char* deviceExtension=VK_KHR_SWAPCHAIN_EXTENSION_NAME;
        auto di=Init<VkDeviceCreateInfo>(VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO);di.queueCreateInfoCount=1;di.pQueueCreateInfos=&qi;di.enabledExtensionCount=1;di.ppEnabledExtensionNames=&deviceExtension;
        Check(vkCreateDevice(physical,&di,nullptr,&device),"vkCreateDevice");vkGetDeviceQueue(device,family,0,&queue);
        VkSurfaceCapabilitiesKHR caps{};Check(vkGetPhysicalDeviceSurfaceCapabilitiesKHR(physical,surface,&caps),"surface capabilities");
        const auto usage=VK_IMAGE_USAGE_TRANSFER_DST_BIT|VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT;
        if((caps.supportedUsageFlags&usage)!=usage)throw std::runtime_error("display surface usage unsupported");
        extent=caps.currentExtent.width!=UINT32_MAX?caps.currentExtent:VkExtent2D{std::clamp(Width,caps.minImageExtent.width,caps.maxImageExtent.width),std::clamp(Height,caps.minImageExtent.height,caps.maxImageExtent.height)};
        Check(vkGetPhysicalDeviceSurfaceFormatsKHR(physical,surface,&n,nullptr),"surface format count");if(!n||n>256)throw std::runtime_error("invalid display formats");
        std::vector<VkSurfaceFormatKHR> formats(n);Check(vkGetPhysicalDeviceSurfaceFormatsKHR(physical,surface,&n,formats.data()),"surface formats");VkSurfaceFormatKHR chosen{};
        for(auto f:formats)if(f.format==VK_FORMAT_R8G8B8A8_UNORM||f.format==VK_FORMAT_B8G8R8A8_UNORM){chosen=f;if(f.format==VK_FORMAT_R8G8B8A8_UNORM)break;}
        if(chosen.format==VK_FORMAT_UNDEFINED)throw std::runtime_error("no display UNORM format");format=chosen.format;
        VkFormatProperties src{},dst{};vkGetPhysicalDeviceFormatProperties(physical,VK_FORMAT_R8G8B8A8_UNORM,&src);vkGetPhysicalDeviceFormatProperties(physical,format,&dst);
        if(!(src.optimalTilingFeatures&VK_FORMAT_FEATURE_BLIT_SRC_BIT)||!(dst.optimalTilingFeatures&VK_FORMAT_FEATURE_BLIT_DST_BIT))throw std::runtime_error("display blit formats unsupported");
        uint32_t count=caps.minImageCount+1;if(caps.maxImageCount)count=std::min(count,caps.maxImageCount);
        auto sw=Init<VkSwapchainCreateInfoKHR>(VK_STRUCTURE_TYPE_SWAPCHAIN_CREATE_INFO_KHR);sw.surface=surface;sw.minImageCount=count;sw.imageFormat=format;sw.imageColorSpace=chosen.colorSpace;sw.imageExtent=extent;sw.imageArrayLayers=1;sw.imageUsage=usage;sw.imageSharingMode=VK_SHARING_MODE_EXCLUSIVE;sw.preTransform=caps.currentTransform;sw.compositeAlpha=VK_COMPOSITE_ALPHA_OPAQUE_BIT_KHR;
        if(!(caps.supportedCompositeAlpha&sw.compositeAlpha)){for(auto a:{VK_COMPOSITE_ALPHA_PRE_MULTIPLIED_BIT_KHR,VK_COMPOSITE_ALPHA_POST_MULTIPLIED_BIT_KHR,VK_COMPOSITE_ALPHA_INHERIT_BIT_KHR})if(caps.supportedCompositeAlpha&a){sw.compositeAlpha=a;break;}}
        sw.presentMode=VK_PRESENT_MODE_FIFO_KHR;sw.clipped=VK_TRUE;Check(vkCreateSwapchainKHR(device,&sw,nullptr,&swapchain),"vkCreateSwapchainKHR");
        n=0;Check(vkGetSwapchainImagesKHR(device,swapchain,&n,nullptr),"swapchain count");if(!n||n>16)throw std::runtime_error("invalid swapchain count");swapImages.resize(n);Check(vkGetSwapchainImagesKHR(device,swapchain,&n,swapImages.data()),"swapchain images");
        auto sem=Init<VkSemaphoreCreateInfo>(VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO);Check(vkCreateSemaphore(device,&sem,nullptr,&acquired),"acquire semaphore");rendered.resize(n,VK_NULL_HANDLE);for(auto& s:rendered)Check(vkCreateSemaphore(device,&sem,nullptr,&s),"render semaphore");
        auto fc=Init<VkFenceCreateInfo>(VK_STRUCTURE_TYPE_FENCE_CREATE_INFO);fc.flags=VK_FENCE_CREATE_SIGNALED_BIT;Check(vkCreateFence(device,&fc,nullptr,&fence),"display fence");
        auto pc=Init<VkCommandPoolCreateInfo>(VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO);pc.queueFamilyIndex=family;pc.flags=VK_COMMAND_POOL_CREATE_TRANSIENT_BIT;Check(vkCreateCommandPool(device,&pc,nullptr,&pool),"display pool");
        auto ca=Init<VkCommandBufferAllocateInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO);ca.commandPool=pool;ca.level=VK_COMMAND_BUFFER_LEVEL_PRIMARY;ca.commandBufferCount=1;Check(vkAllocateCommandBuffers(device,&ca,&command),"display command");
        auto buffer=Init<VkBufferCreateInfo>(VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO);buffer.size=Width*Height*4;buffer.usage=VK_BUFFER_USAGE_TRANSFER_SRC_BIT;buffer.sharingMode=VK_SHARING_MODE_EXCLUSIVE;Check(vkCreateBuffer(device,&buffer,nullptr,&staging),"display staging");
        VkMemoryRequirements requirements{};vkGetBufferMemoryRequirements(device,staging,&requirements);
        auto allocation=Init<VkMemoryAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO);allocation.allocationSize=requirements.size;allocation.memoryTypeIndex=MemoryType(requirements.memoryTypeBits,VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT,VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
        VkPhysicalDeviceMemoryProperties memory{};vkGetPhysicalDeviceMemoryProperties(physical,&memory);coherent=bool(memory.memoryTypes[allocation.memoryTypeIndex].propertyFlags&VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
        Check(vkAllocateMemory(device,&allocation,nullptr,&stagingMemory),"display staging memory");Check(vkBindBufferMemory(device,staging,stagingMemory,0),"bind staging");Check(vkMapMemory(device,stagingMemory,0,VK_WHOLE_SIZE,0,&mapped),"map staging");
        auto imageInfo=Init<VkImageCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO);imageInfo.imageType=VK_IMAGE_TYPE_2D;imageInfo.format=VK_FORMAT_R8G8B8A8_UNORM;imageInfo.extent={Width,Height,1};imageInfo.mipLevels=imageInfo.arrayLayers=1;imageInfo.samples=VK_SAMPLE_COUNT_1_BIT;imageInfo.tiling=VK_IMAGE_TILING_OPTIMAL;imageInfo.usage=VK_IMAGE_USAGE_TRANSFER_DST_BIT|VK_IMAGE_USAGE_TRANSFER_SRC_BIT;imageInfo.sharingMode=VK_SHARING_MODE_EXCLUSIVE;
        Check(vkCreateImage(device,&imageInfo,nullptr,&image),"display source image");vkGetImageMemoryRequirements(device,image,&requirements);allocation.allocationSize=requirements.size;allocation.memoryTypeIndex=MemoryType(requirements.memoryTypeBits,VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);Check(vkAllocateMemory(device,&allocation,nullptr,&imageMemory),"display image memory");Check(vkBindImageMemory(device,image,imageMemory,0),"bind display image");
    }
    void Barrier(VkImage target,VkImageLayout before,VkImageLayout after,VkAccessFlags src,VkAccessFlags dst,VkPipelineStageFlags srcStage,VkPipelineStageFlags dstStage){
        auto b=Init<VkImageMemoryBarrier>(VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER);b.image=target;b.oldLayout=before;b.newLayout=after;b.srcAccessMask=src;b.dstAccessMask=dst;b.srcQueueFamilyIndex=b.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;b.subresourceRange={VK_IMAGE_ASPECT_COLOR_BIT,0,1,0,1};vkCmdPipelineBarrier(command,srcStage,dstStage,0,0,nullptr,0,nullptr,1,&b);
    }
    void Present(const RifeStreamFrame& frame){
        if(frame.width!=int(Width)||frame.height!=int(Height)||frame.rgb8.size()!=size_t(Width)*Height*3)throw std::runtime_error("invalid RIFE display frame");
        Check(vkWaitForFences(device,1,&fence,VK_TRUE,5'000'000'000ULL),"wait display fence");
        auto* pixels=static_cast<uint8_t*>(mapped);for(size_t i=0;i<frame.rgb8.size()/3;++i){pixels[4*i]=frame.rgb8[3*i];pixels[4*i+1]=frame.rgb8[3*i+1];pixels[4*i+2]=frame.rgb8[3*i+2];pixels[4*i+3]=255;}
        // A small diagnostic marker labels slot/kind. Content hashes are taken
        // before this overlay; marker changes never count as AI image changes.
        for(uint32_t y=Height-8;y<Height;++y)for(uint32_t x=0;x<128;++x){auto* p=pixels+(y*Width+x)*4;const bool bit=(frame.presentationIndex>>(x/8))&1;p[0]=frame.kind==RifeStreamKind::Repeat?255:bit?255:0;p[1]=frame.kind==RifeStreamKind::Interpolated?255:bit?255:0;p[2]=frame.kind==RifeStreamKind::Source?255:bit?255:0;}
        if(!coherent){auto range=Init<VkMappedMemoryRange>(VK_STRUCTURE_TYPE_MAPPED_MEMORY_RANGE);range.memory=stagingMemory;range.offset=0;range.size=VK_WHOLE_SIZE;Check(vkFlushMappedMemoryRanges(device,1,&range),"flush staging");}
        uint32_t index=0;auto acquiredResult=vkAcquireNextImageKHR(device,swapchain,5'000'000'000ULL,acquired,VK_NULL_HANDLE,&index);if(acquiredResult!=VK_SUCCESS&&acquiredResult!=VK_SUBOPTIMAL_KHR)Check(acquiredResult,"acquire display image");if(index>=swapImages.size())throw std::runtime_error("display image index invalid");
        Check(vkResetFences(device,1,&fence),"reset display fence");Check(vkResetCommandPool(device,pool,0),"reset display pool");auto begin=Init<VkCommandBufferBeginInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO);begin.flags=VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;Check(vkBeginCommandBuffer(command,&begin),"begin display command");
        Barrier(image,VK_IMAGE_LAYOUT_UNDEFINED,VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,0,VK_ACCESS_TRANSFER_WRITE_BIT,VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT);
        VkBufferImageCopy copy{};copy.imageSubresource={VK_IMAGE_ASPECT_COLOR_BIT,0,0,1};copy.imageExtent={Width,Height,1};vkCmdCopyBufferToImage(command,staging,image,VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,1,&copy);
        Barrier(image,VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,VK_ACCESS_TRANSFER_WRITE_BIT,VK_ACCESS_TRANSFER_READ_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT);
        Barrier(swapImages[index],VK_IMAGE_LAYOUT_UNDEFINED,VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,0,VK_ACCESS_TRANSFER_WRITE_BIT,VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT);
        VkImageBlit blit{};blit.srcSubresource=blit.dstSubresource={VK_IMAGE_ASPECT_COLOR_BIT,0,0,1};blit.srcOffsets[1]={int32_t(Width),int32_t(Height),1};blit.dstOffsets[1]={int32_t(extent.width),int32_t(extent.height),1};vkCmdBlitImage(command,image,VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,swapImages[index],VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,1,&blit,VK_FILTER_NEAREST);
        Barrier(swapImages[index],VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,VK_IMAGE_LAYOUT_PRESENT_SRC_KHR,VK_ACCESS_TRANSFER_WRITE_BIT,0,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT);Check(vkEndCommandBuffer(command),"end display command");
        VkPipelineStageFlags waitStage=VK_PIPELINE_STAGE_TRANSFER_BIT;auto submit=Init<VkSubmitInfo>(VK_STRUCTURE_TYPE_SUBMIT_INFO);submit.waitSemaphoreCount=1;submit.pWaitSemaphores=&acquired;submit.pWaitDstStageMask=&waitStage;submit.commandBufferCount=1;submit.pCommandBuffers=&command;submit.signalSemaphoreCount=1;submit.pSignalSemaphores=&rendered[index];Check(vkQueueSubmit(queue,1,&submit,fence),"submit display");
        auto present=Init<VkPresentInfoKHR>(VK_STRUCTURE_TYPE_PRESENT_INFO_KHR);present.waitSemaphoreCount=1;present.pWaitSemaphores=&rendered[index];present.swapchainCount=1;present.pSwapchains=&swapchain;present.pImageIndices=&index;auto result=vkQueuePresentKHR(queue,&present);if(result!=VK_SUCCESS&&result!=VK_SUBOPTIMAL_KHR)Check(result,"present display");++presents;
    }
    void Retire(){
        if(device){Check(vkDeviceWaitIdle(device),"display teardown idle");if(mapped)vkUnmapMemory(device,stagingMemory);mapped=nullptr;if(staging)vkDestroyBuffer(device,staging,nullptr);staging=VK_NULL_HANDLE;if(stagingMemory)vkFreeMemory(device,stagingMemory,nullptr);stagingMemory=VK_NULL_HANDLE;if(image)vkDestroyImage(device,image,nullptr);image=VK_NULL_HANDLE;if(imageMemory)vkFreeMemory(device,imageMemory,nullptr);imageMemory=VK_NULL_HANDLE;if(pool)vkDestroyCommandPool(device,pool,nullptr);pool=VK_NULL_HANDLE;if(fence)vkDestroyFence(device,fence,nullptr);fence=VK_NULL_HANDLE;if(acquired)vkDestroySemaphore(device,acquired,nullptr);acquired=VK_NULL_HANDLE;for(auto s:rendered)if(s)vkDestroySemaphore(device,s,nullptr);rendered.clear();if(swapchain)vkDestroySwapchainKHR(device,swapchain,nullptr);swapchain=VK_NULL_HANDLE;vkDestroyDevice(device,nullptr);device=VK_NULL_HANDLE;}
        if(instance){if(surface)vkDestroySurfaceKHR(instance,surface,nullptr);surface=VK_NULL_HANDLE;vkDestroyInstance(instance,nullptr);instance=VK_NULL_HANDLE;}retired=true;
    }
    ~Display(){try{Retire();}catch(...){}}
};
}
std::string runRifeSurface(OHNativeWindow* window,const std::string& directory,const std::string& metadata,uint32_t seconds,std::atomic<bool>& stop){
    Display display;RifeStream stream;std::string error,rows;uint64_t sources=0,generated=0,repeats=0;int64_t firstPresent=0,lastPresent=0;
    try{
        if(metadata!="absent")throw std::runtime_error("independent RIFE display requires FusionAware metadata absent");
        display.Create(window);
        if(!stream.start(directory+"/../rife-v4.6",15,30,Width,Height))throw std::runtime_error("RIFE workers could not start");
        const int64_t initializationDeadline=RifeStream::steadyNowNs()+15'000'000'000LL;int64_t end=0;
        while(!stop.load()){
            const int64_t now=RifeStream::steadyNowNs();if(end&&now>=end)break;
            RifeStreamFrame frame;
            if(!stream.waitNextFrame(frame,250)){const std::string streamError=stream.errorMessage();if(!streamError.empty())throw std::runtime_error(streamError);if(!firstPresent&&RifeStream::steadyNowNs()>initializationDeadline)throw std::runtime_error("RIFE display initialization timeout");continue;}
            const uint64_t contentHash=Hash(frame.rgb8);const int64_t begin=RifeStream::steadyNowNs();display.Present(frame);const int64_t completed=RifeStream::steadyNowNs();
            if(!firstPresent){firstPresent=completed;end=firstPresent+int64_t(seconds)*1'000'000'000LL;}lastPresent=completed;
            if(frame.kind==RifeStreamKind::Source)++sources;else if(frame.kind==RifeStreamKind::Interpolated)++generated;else ++repeats;
            std::ostringstream row;row<<"{\"present\":"<<display.presents<<",\"slot\":"<<frame.presentationIndex<<",\"kind\":"<<Quote(rifeStreamKindName(frame.kind))<<",\"originalKind\":"<<Quote(rifeStreamKindName(frame.originalKind))<<",\"source0Id\":"<<frame.source0Id<<",\"source1Id\":"<<frame.source1Id<<",\"source0Ns\":"<<frame.source0Ns<<",\"source1Ns\":"<<frame.source1Ns<<",\"contentTimeNs\":"<<frame.contentTimeNs<<",\"deadlineNs\":"<<frame.deadlineNs<<",\"producedAtNs\":"<<frame.producedAtNs<<",\"presentBeginNs\":"<<begin<<",\"presentCompleteNs\":"<<completed<<",\"rgbContentFnv1a64\":"<<Quote(std::to_string(contentHash))<<",\"underrun\":"<<(frame.underrun?"true":"false")<<",\"sourceGap\":"<<(frame.sourceGap?"true":"false")<<"}\n";rows+=row.str();
            if(display.presents>uint64_t(seconds+2)*30)throw std::runtime_error("bounded RIFE display frame limit exceeded");
        }
    }catch(const std::exception& e){error=e.what();}
    stream.stop();if(error.empty())error=stream.errorMessage();try{display.Retire();}catch(const std::exception& e){if(!error.empty())error+="; ";error+=e.what();}
    const bool rowsSaved=Save(directory+"/rife-display-frames.jsonl",rows);
    const double cadence=lastPresent>firstPresent?double(display.presents-1)*1e9/double(lastPresent-firstPresent):0;
    std::ostringstream result;result<<"{\"status\":"<<Quote(error.empty()?"completed_unverified_compositor":"error")<<",\"probe\":\"online_rife_surface\",\"pid\":"<<getpid()<<",\"error\":"<<Quote(error)<<",\"fusionAwareMetadata\":"<<Quote(metadata)<<",\"gpu\":"<<Quote(display.gpu)<<",\"sourceResolution\":[320,192],\"swapchainResolution\":["<<display.extent.width<<','<<display.extent.height<<"],\"sourceTargetHz\":15,\"requestedOutputTargetHz\":30,\"presentSuccesses\":"<<display.presents<<",\"sourcePresents\":"<<sources<<",\"interpolatedPresents\":"<<generated<<",\"repeatPresents\":"<<repeats<<",\"firstPresentCompleteNs\":"<<firstPresent<<",\"lastPresentCompleteNs\":"<<lastPresent<<",\"presentApiCadenceHz\":"<<cadence<<",\"displayFpsVerified\":null,\"diagnosticSlotKindMarkerOverlaid\":true,\"contentHashesExcludeMarker\":true,\"frameRowsSaved\":"<<(rowsSaved?"true":"false")<<",\"displayGpuRetired\":"<<(display.retired?"true":"false")<<",\"stream\":"<<stream.statsJson()<<",\"observationScope\":\"Live synthetic source, neural interpolation and real Vulkan presents. API cadence and frame provenance require independent compositor/pixel observation. No game qualification.\"}";
    const std::string output=result.str();Save(directory+"/result.json",output+'\n');return output;
}
}
