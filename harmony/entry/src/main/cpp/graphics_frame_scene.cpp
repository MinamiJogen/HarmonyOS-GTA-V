#include "graphics_frame_scene.h"
#include "graphics_frame_scene_shaders.h"
#include "graphics_frame_scene_depth_readback_shaders.h"
#include <algorithm>
#include <cmath>
#include <cstring>
#include <sstream>
#include <stdexcept>
#include <vector>

namespace muguet::fg_scene {
namespace {
template<class T> T info(VkStructureType type) { T value{}; value.sType=type; return value; }
void check(VkResult result,const char* call) {
    if(result!=VK_SUCCESS) throw std::runtime_error(std::string(call)+" VkResult="+std::to_string(result));
}
struct Vertex { float position[3],color[4]; };
const Vertex vertices[] = {
    {{-.55f,-.35f,.5f},{.9f,.1f,.15f,1}}, {{.55f,-.35f,.5f},{.9f,.1f,.15f,1}}, {{0,.6f,.5f},{.9f,.1f,.15f,1}},
    {{-.85f,-.7f,1.5f},{.1f,.25f,.9f,1}}, {{.85f,-.7f,1.5f},{.1f,.25f,.9f,1}}, {{0,.9f,1.5f},{.1f,.25f,.9f,1}},
};
struct Push { std::array<float,16> matrix; float translation[4]; };
static_assert(sizeof(Push)==80 && sizeof(Vertex)==28 && sizeof(float)==4);
struct Image { VkImage image{}; VkImageView view{}; VkDeviceMemory memory{}; };
struct Buffer { VkBuffer buffer{}; VkDeviceMemory memory{}; VkDeviceSize bytes{},allocation{}; bool coherent=false; };
constexpr VkDeviceSize GUARD=256;
constexpr unsigned char SENTINEL=0xa5;
}
struct Scene::Impl {
    VkPhysicalDevice physical{}; VkDevice device{}; uint32_t width{},height{},pitch{};
    Image color,depth; Buffer vertex,colorRead,depthRead;
    VkRenderPass pass{}; VkFramebuffer framebuffer{}; VkPipelineLayout layout{}; VkPipeline pipeline{};
    VkShaderModule vert{},frag{}; VkCommandPool pool{}; VkCommandBuffer command{}; VkFence fence{};
    bool qualificationPending=false;
    VkPipeline depthReadPipeline{}; VkPipelineLayout depthReadLayout{}; VkShaderModule depthReadShader{};
    VkDescriptorSetLayout depthReadSetLayout{}; VkDescriptorPool depthReadPool{}; VkDescriptorSet depthReadSet{};
    std::array<float,16> vp{1,0,0,0, 0,-1,0,0, 0,0,.5f,0, 0,0,0,1};
    std::array<float,16> inverse{1,0,0,0, 0,-1,0,0, 0,0,2,0, 0,0,0,1};
    ~Impl() { cleanup(); }
    uint32_t memoryType(uint32_t bits,VkMemoryPropertyFlags required,VkMemoryPropertyFlags preferred=0) {
        VkPhysicalDeviceMemoryProperties properties{};vkGetPhysicalDeviceMemoryProperties(physical,&properties);
        for(unsigned attempt=0;attempt<2;++attempt) for(uint32_t i=0;i<properties.memoryTypeCount;++i) {
            const auto flags=properties.memoryTypes[i].propertyFlags;
            if((bits&(1u<<i)) && (flags&required)==required && (attempt || (flags&preferred)==preferred))return i;
        }
        throw std::runtime_error("required image/buffer memory type unavailable");
    }
    void gate(VkFormat format,VkImageUsageFlags usage,VkFormatFeatureFlags required) {
        VkFormatProperties properties{};vkGetPhysicalDeviceFormatProperties(physical,format,&properties);
        if((properties.optimalTilingFeatures&required)!=required)throw std::runtime_error("required optimal format features unavailable");
        VkImageFormatProperties combination{};
        check(vkGetPhysicalDeviceImageFormatProperties(physical,format,VK_IMAGE_TYPE_2D,VK_IMAGE_TILING_OPTIMAL,usage,0,&combination),"image format/usage gate");
        if(width>combination.maxExtent.width || height>combination.maxExtent.height || !(combination.sampleCounts&VK_SAMPLE_COUNT_1_BIT))
            throw std::runtime_error("image extent/sample count unsupported");
    }
    void makeImage(Image& image,VkFormat format,VkImageUsageFlags usage,VkImageAspectFlags aspect) {
        auto ci=info<VkImageCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO);ci.imageType=VK_IMAGE_TYPE_2D;
        ci.format=format;ci.extent={width,height,1};ci.mipLevels=ci.arrayLayers=1;ci.samples=VK_SAMPLE_COUNT_1_BIT;
        ci.tiling=VK_IMAGE_TILING_OPTIMAL;ci.usage=usage;ci.sharingMode=VK_SHARING_MODE_EXCLUSIVE;
        check(vkCreateImage(device,&ci,nullptr,&image.image),"create scene image");VkMemoryRequirements requirements{};vkGetImageMemoryRequirements(device,image.image,&requirements);
        auto ai=info<VkMemoryAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO);ai.allocationSize=requirements.size;ai.memoryTypeIndex=memoryType(requirements.memoryTypeBits,0,VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT);
        check(vkAllocateMemory(device,&ai,nullptr,&image.memory),"allocate scene image");check(vkBindImageMemory(device,image.image,image.memory,0),"bind scene image");
        auto vi=info<VkImageViewCreateInfo>(VK_STRUCTURE_TYPE_IMAGE_VIEW_CREATE_INFO);vi.image=image.image;vi.viewType=VK_IMAGE_VIEW_TYPE_2D;vi.format=format;vi.subresourceRange={aspect,0,1,0,1};
        check(vkCreateImageView(device,&vi,nullptr,&image.view),"create scene view");
    }
    void makeBuffer(Buffer& buffer,VkDeviceSize bytes,VkBufferUsageFlags usage) {
        buffer.bytes=bytes;auto ci=info<VkBufferCreateInfo>(VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO);ci.size=bytes;ci.usage=usage;ci.sharingMode=VK_SHARING_MODE_EXCLUSIVE;
        check(vkCreateBuffer(device,&ci,nullptr,&buffer.buffer),"create scene buffer");VkMemoryRequirements requirements{};vkGetBufferMemoryRequirements(device,buffer.buffer,&requirements);
        auto ai=info<VkMemoryAllocateInfo>(VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO);ai.allocationSize=requirements.size;ai.memoryTypeIndex=memoryType(requirements.memoryTypeBits,VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT,VK_MEMORY_PROPERTY_HOST_COHERENT_BIT);
        VkPhysicalDeviceMemoryProperties properties{};vkGetPhysicalDeviceMemoryProperties(physical,&properties);
        buffer.coherent=(properties.memoryTypes[ai.memoryTypeIndex].propertyFlags&VK_MEMORY_PROPERTY_HOST_COHERENT_BIT)!=0;buffer.allocation=ai.allocationSize;
        check(vkAllocateMemory(device,&ai,nullptr,&buffer.memory),"allocate scene buffer");check(vkBindBufferMemory(device,buffer.buffer,buffer.memory,0),"bind scene buffer");
    }
    void writeBuffer(Buffer& buffer,const void* data) {
        void* mapped=nullptr;check(vkMapMemory(device,buffer.memory,0,VK_WHOLE_SIZE,0,&mapped),"map scene upload");
        if(data)std::memcpy(mapped,data,size_t(buffer.bytes));else std::memset(mapped,SENTINEL,size_t(buffer.allocation));
        VkResult flushed=VK_SUCCESS;if(!buffer.coherent){auto range=info<VkMappedMemoryRange>(VK_STRUCTURE_TYPE_MAPPED_MEMORY_RANGE);range.memory=buffer.memory;range.size=VK_WHOLE_SIZE;flushed=vkFlushMappedMemoryRanges(device,1,&range);}
        vkUnmapMemory(device,buffer.memory);check(flushed,"flush scene upload");
    }
    void init() {
        if(!physical||!device||width<64||height<64||width>4096||height>4096)throw std::runtime_error("invalid scene device/extent");
        VkPhysicalDeviceProperties properties{};vkGetPhysicalDeviceProperties(physical,&properties);
        if(properties.limits.maxPushConstantsSize<sizeof(Push)||width>properties.limits.maxFramebufferWidth||height>properties.limits.maxFramebufferHeight)throw std::runtime_error("scene push constants/framebuffer limit");
        constexpr auto colorUsage=VK_IMAGE_USAGE_COLOR_ATTACHMENT_BIT|VK_IMAGE_USAGE_SAMPLED_BIT|VK_IMAGE_USAGE_TRANSFER_SRC_BIT;
        constexpr auto depthUsage=VK_IMAGE_USAGE_DEPTH_STENCIL_ATTACHMENT_BIT|VK_IMAGE_USAGE_TRANSFER_SRC_BIT|VK_IMAGE_USAGE_SAMPLED_BIT;
        gate(VK_FORMAT_R8G8B8A8_UNORM,colorUsage,VK_FORMAT_FEATURE_COLOR_ATTACHMENT_BIT|VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT|VK_FORMAT_FEATURE_TRANSFER_SRC_BIT);
        gate(VK_FORMAT_D32_SFLOAT,depthUsage,VK_FORMAT_FEATURE_DEPTH_STENCIL_ATTACHMENT_BIT|VK_FORMAT_FEATURE_TRANSFER_SRC_BIT|VK_FORMAT_FEATURE_SAMPLED_IMAGE_BIT);
        makeImage(color,VK_FORMAT_R8G8B8A8_UNORM,colorUsage,VK_IMAGE_ASPECT_COLOR_BIT);makeImage(depth,VK_FORMAT_D32_SFLOAT,depthUsage,VK_IMAGE_ASPECT_DEPTH_BIT);
        VkAttachmentDescription attachments[2]{};
        for(auto& a:attachments){a.samples=VK_SAMPLE_COUNT_1_BIT;a.loadOp=VK_ATTACHMENT_LOAD_OP_CLEAR;a.storeOp=VK_ATTACHMENT_STORE_OP_STORE;a.stencilLoadOp=VK_ATTACHMENT_LOAD_OP_DONT_CARE;a.stencilStoreOp=VK_ATTACHMENT_STORE_OP_DONT_CARE;a.initialLayout=VK_IMAGE_LAYOUT_UNDEFINED;}
        attachments[0].format=VK_FORMAT_R8G8B8A8_UNORM;attachments[0].finalLayout=VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL;
        attachments[1].format=VK_FORMAT_D32_SFLOAT;attachments[1].finalLayout=VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
        VkAttachmentReference colorRef{0,VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL},depthRef{1,VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL};
        VkSubpassDescription sub{};sub.pipelineBindPoint=VK_PIPELINE_BIND_POINT_GRAPHICS;sub.colorAttachmentCount=1;sub.pColorAttachments=&colorRef;sub.pDepthStencilAttachment=&depthRef;
        constexpr auto writers=VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT|VK_PIPELINE_STAGE_EARLY_FRAGMENT_TESTS_BIT|VK_PIPELINE_STAGE_LATE_FRAGMENT_TESTS_BIT;
        constexpr auto writes=VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT|VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_WRITE_BIT;
        VkSubpassDependency dependencies[2]{};dependencies[0]={VK_SUBPASS_EXTERNAL,0,VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,writers,VK_ACCESS_MEMORY_READ_BIT|VK_ACCESS_MEMORY_WRITE_BIT,writes|VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_READ_BIT,0};
        dependencies[1]={0,VK_SUBPASS_EXTERNAL,writers,VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,writes,VK_ACCESS_SHADER_READ_BIT|VK_ACCESS_TRANSFER_READ_BIT,0};
        auto rp=info<VkRenderPassCreateInfo>(VK_STRUCTURE_TYPE_RENDER_PASS_CREATE_INFO);rp.attachmentCount=2;rp.pAttachments=attachments;rp.subpassCount=1;rp.pSubpasses=&sub;rp.dependencyCount=2;rp.pDependencies=dependencies;
        check(vkCreateRenderPass(device,&rp,nullptr,&pass),"create scene renderpass");VkImageView views[]{color.view,depth.view};
        auto fb=info<VkFramebufferCreateInfo>(VK_STRUCTURE_TYPE_FRAMEBUFFER_CREATE_INFO);fb.renderPass=pass;fb.attachmentCount=2;fb.pAttachments=views;fb.width=width;fb.height=height;fb.layers=1;check(vkCreateFramebuffer(device,&fb,nullptr,&framebuffer),"create scene framebuffer");
        VkPushConstantRange range{VK_SHADER_STAGE_VERTEX_BIT,0,sizeof(Push)};auto pl=info<VkPipelineLayoutCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO);pl.pushConstantRangeCount=1;pl.pPushConstantRanges=&range;check(vkCreatePipelineLayout(device,&pl,nullptr,&layout),"create scene pipeline layout");
        auto sm=info<VkShaderModuleCreateInfo>(VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO);sm.codeSize=sizeof(fg_scene_shaders::vert);sm.pCode=fg_scene_shaders::vert;check(vkCreateShaderModule(device,&sm,nullptr,&vert),"scene vertex shader");sm.codeSize=sizeof(fg_scene_shaders::frag);sm.pCode=fg_scene_shaders::frag;check(vkCreateShaderModule(device,&sm,nullptr,&frag),"scene fragment shader");
        VkPipelineShaderStageCreateInfo stages[2]{};for(auto& s:stages){s=info<VkPipelineShaderStageCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO);s.pName="main";}stages[0].stage=VK_SHADER_STAGE_VERTEX_BIT;stages[0].module=vert;stages[1].stage=VK_SHADER_STAGE_FRAGMENT_BIT;stages[1].module=frag;
        VkVertexInputBindingDescription binding{0,sizeof(Vertex),VK_VERTEX_INPUT_RATE_VERTEX};VkVertexInputAttributeDescription attributes[]{{0,0,VK_FORMAT_R32G32B32_SFLOAT,0},{1,0,VK_FORMAT_R32G32B32A32_SFLOAT,12}};
        auto vi=info<VkPipelineVertexInputStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_VERTEX_INPUT_STATE_CREATE_INFO);vi.vertexBindingDescriptionCount=1;vi.pVertexBindingDescriptions=&binding;vi.vertexAttributeDescriptionCount=2;vi.pVertexAttributeDescriptions=attributes;
        auto assembly=info<VkPipelineInputAssemblyStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_INPUT_ASSEMBLY_STATE_CREATE_INFO);assembly.topology=VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST;
        VkViewport viewport{0,0,float(width),float(height),0,1};VkRect2D scissor{{0,0},{width,height}};auto viewportState=info<VkPipelineViewportStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_VIEWPORT_STATE_CREATE_INFO);viewportState.viewportCount=viewportState.scissorCount=1;viewportState.pViewports=&viewport;viewportState.pScissors=&scissor;
        auto raster=info<VkPipelineRasterizationStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_RASTERIZATION_STATE_CREATE_INFO);raster.polygonMode=VK_POLYGON_MODE_FILL;raster.cullMode=VK_CULL_MODE_NONE;raster.lineWidth=1;
        auto ms=info<VkPipelineMultisampleStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_MULTISAMPLE_STATE_CREATE_INFO);ms.rasterizationSamples=VK_SAMPLE_COUNT_1_BIT;
        auto ds=info<VkPipelineDepthStencilStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_DEPTH_STENCIL_STATE_CREATE_INFO);ds.depthTestEnable=ds.depthWriteEnable=VK_TRUE;ds.depthCompareOp=VK_COMPARE_OP_LESS;
        VkPipelineColorBlendAttachmentState colorWrite{};colorWrite.colorWriteMask=15;auto blend=info<VkPipelineColorBlendStateCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_COLOR_BLEND_STATE_CREATE_INFO);blend.attachmentCount=1;blend.pAttachments=&colorWrite;
        auto pi=info<VkGraphicsPipelineCreateInfo>(VK_STRUCTURE_TYPE_GRAPHICS_PIPELINE_CREATE_INFO);pi.stageCount=2;pi.pStages=stages;pi.pVertexInputState=&vi;pi.pInputAssemblyState=&assembly;pi.pViewportState=&viewportState;pi.pRasterizationState=&raster;pi.pMultisampleState=&ms;pi.pDepthStencilState=&ds;pi.pColorBlendState=&blend;pi.layout=layout;pi.renderPass=pass;
        check(vkCreateGraphicsPipelines(device,VK_NULL_HANDLE,1,&pi,nullptr,&pipeline),"create scene depth pipeline");vkDestroyShaderModule(device,vert,nullptr);vert=VK_NULL_HANDLE;vkDestroyShaderModule(device,frag,nullptr);frag=VK_NULL_HANDLE;
        makeBuffer(vertex,sizeof(vertices),VK_BUFFER_USAGE_VERTEX_BUFFER_BIT);writeBuffer(vertex,vertices);
    }
    void record(VkCommandBuffer cmd,float x) {
        if(!cmd||!std::isfinite(x)||std::abs(x)>.25f)throw std::runtime_error("invalid scene command/translation");
        VkClearValue clear[2]{};clear[0].color={{.02f,.04f,.08f,1}};clear[1].depthStencil={1,0};
        auto begin=info<VkRenderPassBeginInfo>(VK_STRUCTURE_TYPE_RENDER_PASS_BEGIN_INFO);begin.renderPass=pass;begin.framebuffer=framebuffer;begin.renderArea={{0,0},{width,height}};begin.clearValueCount=2;begin.pClearValues=clear;
        vkCmdBeginRenderPass(cmd,&begin,VK_SUBPASS_CONTENTS_INLINE);vkCmdBindPipeline(cmd,VK_PIPELINE_BIND_POINT_GRAPHICS,pipeline);VkDeviceSize offset=0;vkCmdBindVertexBuffers(cmd,0,1,&vertex.buffer,&offset);
        Push params{vp,{x,0,0,0}};vkCmdPushConstants(cmd,layout,VK_SHADER_STAGE_VERTEX_BIT,0,sizeof(params),&params);
        vkCmdDraw(cmd,3,1,0,0); // Near FIRST: painter's-order rendering would fail.
        vkCmdDraw(cmd,3,1,3,0); // Far SECOND, rejected wherever near depth won.
        vkCmdEndRenderPass(cmd);
    }
    void imageBarrier(VkCommandBuffer cmd,Image& image,VkImageAspectFlags aspect,VkImageLayout oldLayout,VkImageLayout newLayout,VkAccessFlags src,VkAccessFlags dst,VkPipelineStageFlags srcStage,VkPipelineStageFlags dstStage) {
        auto barrier=info<VkImageMemoryBarrier>(VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER);barrier.srcAccessMask=src;barrier.dstAccessMask=dst;barrier.oldLayout=oldLayout;barrier.newLayout=newLayout;barrier.srcQueueFamilyIndex=barrier.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;barrier.image=image.image;barrier.subresourceRange={aspect,0,1,0,1};vkCmdPipelineBarrier(cmd,srcStage,dstStage,0,0,nullptr,0,nullptr,1,&barrier);
    }
    void gpuPrefillReadbacks() {
        // A device-side visibility control for the same sentinel bytes. Both
        // buffers already have TRANSFER_DST usage; fill offset/size are 4-aligned.
        // Keep the existing independent CPU-prefill observation and every guard.
        VkBufferMemoryBarrier barriers[2]{};
        for(unsigned i=0;i<2;++i){auto& buffer=i?depthRead:colorRead;
            if(!buffer.bytes||buffer.bytes%4)throw std::runtime_error("invalid GPU prefill size");
            vkCmdFillBuffer(command,buffer.buffer,0,buffer.bytes,0xa5a5a5a5u);
            auto& b=barriers[i];b=info<VkBufferMemoryBarrier>(VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER);
            b.srcAccessMask=b.dstAccessMask=VK_ACCESS_TRANSFER_WRITE_BIT;
            b.srcQueueFamilyIndex=b.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;b.buffer=buffer.buffer;b.size=buffer.bytes;
        }
        // Order the fill writes before the later image-to-buffer copy writes.
        vkCmdPipelineBarrier(command,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT,
            0,0,nullptr,2,barriers,0,nullptr);
    }
    void makeDepthReadbackCompute() {
        VkPhysicalDeviceProperties properties{};vkGetPhysicalDeviceProperties(physical,&properties);
        const auto& limits=properties.limits;
        if(depthRead.bytes>limits.maxStorageBufferRange||limits.maxPushConstantsSize<16||
            limits.maxComputeWorkGroupInvocations<64||limits.maxComputeWorkGroupSize[0]<8||limits.maxComputeWorkGroupSize[1]<8||
            (width+7)/8>limits.maxComputeWorkGroupCount[0]||(height+7)/8>limits.maxComputeWorkGroupCount[1]||
            !limits.maxPerStageDescriptorSampledImages||!limits.maxPerStageDescriptorStorageBuffers)
            throw std::runtime_error("sampled-depth readback compute/storage limits unsupported");
        VkDescriptorSetLayoutBinding bindings[]{{0,VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE,1,VK_SHADER_STAGE_COMPUTE_BIT,nullptr},{1,VK_DESCRIPTOR_TYPE_STORAGE_BUFFER,1,VK_SHADER_STAGE_COMPUTE_BIT,nullptr}};
        auto sl=info<VkDescriptorSetLayoutCreateInfo>(VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO);sl.bindingCount=2;sl.pBindings=bindings;
        check(vkCreateDescriptorSetLayout(device,&sl,nullptr,&depthReadSetLayout),"depth readback descriptor layout");
        VkPushConstantRange push{VK_SHADER_STAGE_COMPUTE_BIT,0,16};auto pl=info<VkPipelineLayoutCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO);pl.setLayoutCount=1;pl.pSetLayouts=&depthReadSetLayout;pl.pushConstantRangeCount=1;pl.pPushConstantRanges=&push;
        check(vkCreatePipelineLayout(device,&pl,nullptr,&depthReadLayout),"depth readback pipeline layout");
        VkDescriptorPoolSize sizes[]{{VK_DESCRIPTOR_TYPE_SAMPLED_IMAGE,1},{VK_DESCRIPTOR_TYPE_STORAGE_BUFFER,1}};
        auto dp=info<VkDescriptorPoolCreateInfo>(VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO);dp.maxSets=1;dp.poolSizeCount=2;dp.pPoolSizes=sizes;
        check(vkCreateDescriptorPool(device,&dp,nullptr,&depthReadPool),"depth readback descriptor pool");
        auto da=info<VkDescriptorSetAllocateInfo>(VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO);da.descriptorPool=depthReadPool;da.descriptorSetCount=1;da.pSetLayouts=&depthReadSetLayout;
        check(vkAllocateDescriptorSets(device,&da,&depthReadSet),"depth readback descriptor set");
        VkDescriptorImageInfo image{VK_NULL_HANDLE,depth.view,VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL};VkDescriptorBufferInfo buffer{depthRead.buffer,0,depthRead.bytes};
        VkWriteDescriptorSet writes[2]{};for(unsigned i=0;i<2;++i){writes[i]=info<VkWriteDescriptorSet>(VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET);writes[i].dstSet=depthReadSet;writes[i].dstBinding=i;writes[i].descriptorCount=1;writes[i].descriptorType=bindings[i].descriptorType;}
        writes[0].pImageInfo=&image;writes[1].pBufferInfo=&buffer;vkUpdateDescriptorSets(device,2,writes,0,nullptr);
        auto sm=info<VkShaderModuleCreateInfo>(VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO);sm.codeSize=sizeof(fg_scene_depth_readback_shaders::comp);sm.pCode=fg_scene_depth_readback_shaders::comp;
        check(vkCreateShaderModule(device,&sm,nullptr,&depthReadShader),"depth readback compute shader");
        auto pi=info<VkComputePipelineCreateInfo>(VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO);pi.stage=info<VkPipelineShaderStageCreateInfo>(VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO);pi.stage.stage=VK_SHADER_STAGE_COMPUTE_BIT;pi.stage.module=depthReadShader;pi.stage.pName="main";pi.layout=depthReadLayout;
        check(vkCreateComputePipelines(device,VK_NULL_HANDLE,1,&pi,nullptr,&depthReadPipeline),"depth readback compute pipeline");
        vkDestroyShaderModule(device,depthReadShader,nullptr);depthReadShader=VK_NULL_HANDLE;
    }
    void copies() {
        imageBarrier(command,color,VK_IMAGE_ASPECT_COLOR_BIT,VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT,VK_ACCESS_TRANSFER_READ_BIT,VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT);
        VkBufferImageCopy copy{};copy.bufferOffset=GUARD;copy.bufferRowLength=pitch/4;copy.bufferImageHeight=height;copy.imageSubresource={VK_IMAGE_ASPECT_COLOR_BIT,0,0,1};copy.imageExtent={width,height,1};
        vkCmdCopyImageToBuffer(command,color.image,VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,colorRead.buffer,1,&copy);
        // Read the actual rasterized D32 texels, never a CPU-generated depth map.
        imageBarrier(command,depth,VK_IMAGE_ASPECT_DEPTH_BIT,VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL,VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_WRITE_BIT,VK_ACCESS_SHADER_READ_BIT,VK_PIPELINE_STAGE_EARLY_FRAGMENT_TESTS_BIT|VK_PIPELINE_STAGE_LATE_FRAGMENT_TESTS_BIT,VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT);
        auto fillToStorage=info<VkBufferMemoryBarrier>(VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER);fillToStorage.srcAccessMask=VK_ACCESS_TRANSFER_WRITE_BIT;fillToStorage.dstAccessMask=VK_ACCESS_SHADER_WRITE_BIT;fillToStorage.srcQueueFamilyIndex=fillToStorage.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;fillToStorage.buffer=depthRead.buffer;fillToStorage.size=depthRead.bytes;
        vkCmdPipelineBarrier(command,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT,0,0,nullptr,1,&fillToStorage,0,nullptr);
        vkCmdBindPipeline(command,VK_PIPELINE_BIND_POINT_COMPUTE,depthReadPipeline);vkCmdBindDescriptorSets(command,VK_PIPELINE_BIND_POINT_COMPUTE,depthReadLayout,0,1,&depthReadSet,0,nullptr);
        const uint32_t params[]{width,height,pitch/4,uint32_t(GUARD/4)};vkCmdPushConstants(command,depthReadLayout,VK_SHADER_STAGE_COMPUTE_BIT,0,sizeof(params),params);vkCmdDispatch(command,(width+7)/8,(height+7)/8,1);
        VkBufferMemoryBarrier barriers[2]{};for(unsigned i=0;i<2;++i){auto& b=barriers[i];b=info<VkBufferMemoryBarrier>(VK_STRUCTURE_TYPE_BUFFER_MEMORY_BARRIER);b.srcAccessMask=i?(VK_ACCESS_TRANSFER_WRITE_BIT|VK_ACCESS_SHADER_WRITE_BIT):VK_ACCESS_TRANSFER_WRITE_BIT;b.dstAccessMask=VK_ACCESS_HOST_READ_BIT;b.srcQueueFamilyIndex=b.dstQueueFamilyIndex=VK_QUEUE_FAMILY_IGNORED;b.buffer=i?depthRead.buffer:colorRead.buffer;b.size=VK_WHOLE_SIZE;}
        vkCmdPipelineBarrier(command,VK_PIPELINE_STAGE_TRANSFER_BIT|VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT,VK_PIPELINE_STAGE_HOST_BIT,0,0,nullptr,2,barriers,0,nullptr);
        imageBarrier(command,color,VK_IMAGE_ASPECT_COLOR_BIT,VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,VK_ACCESS_TRANSFER_READ_BIT,VK_ACCESS_SHADER_READ_BIT,VK_PIPELINE_STAGE_TRANSFER_BIT,VK_PIPELINE_STAGE_ALL_COMMANDS_BIT);
        imageBarrier(command,depth,VK_IMAGE_ASPECT_DEPTH_BIT,VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL,VK_IMAGE_LAYOUT_DEPTH_STENCIL_ATTACHMENT_OPTIMAL,VK_ACCESS_SHADER_READ_BIT,VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_READ_BIT|VK_ACCESS_DEPTH_STENCIL_ATTACHMENT_WRITE_BIT,VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT,VK_PIPELINE_STAGE_EARLY_FRAGMENT_TESTS_BIT|VK_PIPELINE_STAGE_LATE_FRAGMENT_TESTS_BIT);
    }
    std::vector<unsigned char> read(Buffer& buffer) {
        void* mapped=nullptr;check(vkMapMemory(device,buffer.memory,0,VK_WHOLE_SIZE,0,&mapped),"map scene readback");VkResult invalidated=VK_SUCCESS;
        if(!buffer.coherent){auto range=info<VkMappedMemoryRange>(VK_STRUCTURE_TYPE_MAPPED_MEMORY_RANGE);range.memory=buffer.memory;range.size=VK_WHOLE_SIZE;invalidated=vkInvalidateMappedMemoryRanges(device,1,&range);}
        std::vector<unsigned char> bytes;
        if(invalidated==VK_SUCCESS){bytes.resize(size_t(buffer.bytes));std::memcpy(bytes.data(),mapped,bytes.size());}
        vkUnmapMemory(device,buffer.memory);check(invalidated,"invalidate scene readback");return bytes;
    }
    void destroyBuffer(Buffer& b) { if(b.buffer)vkDestroyBuffer(device,b.buffer,nullptr);if(b.memory)vkFreeMemory(device,b.memory,nullptr);b={}; }
    void destroyImage(Image& i) { if(i.view)vkDestroyImageView(device,i.view,nullptr);if(i.image)vkDestroyImage(device,i.image,nullptr);if(i.memory)vkFreeMemory(device,i.memory,nullptr);i={}; }
    void retireQualification() {
        if(qualificationPending)return;
        if(pool)vkDestroyCommandPool(device,pool,nullptr);pool=VK_NULL_HANDLE;command=VK_NULL_HANDLE;
        if(depthReadPipeline)vkDestroyPipeline(device,depthReadPipeline,nullptr);depthReadPipeline=VK_NULL_HANDLE;
        if(depthReadShader)vkDestroyShaderModule(device,depthReadShader,nullptr);depthReadShader=VK_NULL_HANDLE;
        if(depthReadPool)vkDestroyDescriptorPool(device,depthReadPool,nullptr);depthReadPool=VK_NULL_HANDLE;depthReadSet=VK_NULL_HANDLE;
        if(depthReadLayout)vkDestroyPipelineLayout(device,depthReadLayout,nullptr);depthReadLayout=VK_NULL_HANDLE;
        if(depthReadSetLayout)vkDestroyDescriptorSetLayout(device,depthReadSetLayout,nullptr);depthReadSetLayout=VK_NULL_HANDLE;
        if(fence)vkDestroyFence(device,fence,nullptr);fence=VK_NULL_HANDLE;destroyBuffer(colorRead);destroyBuffer(depthRead);
    }
    void cleanup() {
        if(qualificationPending)return;retireQualification();
        if(pipeline)vkDestroyPipeline(device,pipeline,nullptr);if(vert)vkDestroyShaderModule(device,vert,nullptr);if(frag)vkDestroyShaderModule(device,frag,nullptr);if(layout)vkDestroyPipelineLayout(device,layout,nullptr);if(framebuffer)vkDestroyFramebuffer(device,framebuffer,nullptr);if(pass)vkDestroyRenderPass(device,pass,nullptr);destroyBuffer(vertex);destroyImage(color);destroyImage(depth);
    }
};
Scene::Scene(std::unique_ptr<Impl> implementation):impl_(std::move(implementation)) {}
std::unique_ptr<Scene> Scene::create(VkPhysicalDevice physical,VkDevice device,uint32_t width,uint32_t height) {
    auto impl=std::make_unique<Impl>();impl->physical=physical;impl->device=device;impl->width=width;impl->height=height;impl->init();return std::unique_ptr<Scene>(new Scene(std::move(impl)));
}
Scene::~Scene()=default;
VkImage Scene::colorImage()const{return impl_->color.image;} VkImageView Scene::colorView()const{return impl_->color.view;}
VkImage Scene::depthImage()const{return impl_->depth.image;} VkImageView Scene::depthView()const{return impl_->depth.view;}
const std::array<float,16>& Scene::viewProj()const{return impl_->vp;}const std::array<float,16>& Scene::invViewProj()const{return impl_->inverse;}
void Scene::record(VkCommandBuffer command,float translationX){impl_->record(command,translationX);}
std::string Scene::qualify(VkQueue queue,uint32_t family) {
    auto& s=*impl_;uint64_t guardChecks=0,guardErrors=0,prefillChecks=0,prefillErrors=0,colorErrors=0,depthErrors=0;unsigned completed=0,gpuPrefills=0;std::ostringstream samples,guardDiagnostics;
    const auto inspectGuards=[&](const std::vector<unsigned char>& data,unsigned frame,const char* name,const char* phase,const Buffer& buffer,bool afterCopy){
        uint64_t checks=0,errors=0,prefixErrors=0,paddingErrors=0,suffixErrors=0;unsigned recorded=0;std::ostringstream firstWrong;
        for(size_t i=0;i<data.size();++i){
            const bool prefix=i<GUARD,suffix=i>=GUARD+size_t(s.pitch)*s.height;
            const bool padding=!prefix&&!suffix&&(i-GUARD)%s.pitch>=size_t(s.width)*4;
            if(!prefix&&!suffix&&!padding)continue;++checks;
            if(data[i]==SENTINEL)continue;++errors;if(prefix)++prefixErrors;else if(suffix)++suffixErrors;else ++paddingErrors;
            if(recorded++<16){if(firstWrong.tellp()>0)firstWrong<<',';firstWrong<<"{\"offset\":"<<i<<",\"region\":\""<<(prefix?"prefix":suffix?"suffix":"row-padding")<<"\",\"expected\":165,\"actual\":"<<unsigned(data[i])<<'}';}
        }
        if(afterCopy){guardChecks+=checks;guardErrors+=errors;}else{prefillChecks+=checks;prefillErrors+=errors;}
        if(guardDiagnostics.tellp()>0)guardDiagnostics<<',';
        guardDiagnostics<<"{\"frame\":"<<frame<<",\"buffer\":\""<<name<<"\",\"phase\":\""<<phase<<"\",\"hostCoherent\":"<<(buffer.coherent?"true":"false")<<",\"bufferBytes\":"<<buffer.bytes<<",\"allocationBytes\":"<<buffer.allocation<<",\"rowBytes\":"<<s.width*4<<",\"rowPitch\":"<<s.pitch<<",\"guardBytesChecked\":"<<checks<<",\"guardErrors\":"<<errors<<",\"prefixErrors\":"<<prefixErrors<<",\"rowPaddingErrors\":"<<paddingErrors<<",\"suffixErrors\":"<<suffixErrors<<",\"firstWrongBytes\":["<<firstWrong.str()<<"]}";
    };
    try {
        if(!queue||s.qualificationPending||s.pool)throw std::runtime_error("scene qualification unavailable/pending");
        uint32_t count=0;vkGetPhysicalDeviceQueueFamilyProperties(s.physical,&count,nullptr);if(!count||count>256||family>=count)throw std::runtime_error("invalid qualification queue family");std::vector<VkQueueFamilyProperties> families(count);vkGetPhysicalDeviceQueueFamilyProperties(s.physical,&count,families.data());if((families[family].queueFlags&(VK_QUEUE_GRAPHICS_BIT|VK_QUEUE_COMPUTE_BIT))!=(VK_QUEUE_GRAPHICS_BIT|VK_QUEUE_COMPUTE_BIT))throw std::runtime_error("qualification queue lacks graphics/compute capability");
        auto pc=info<VkCommandPoolCreateInfo>(VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO);pc.queueFamilyIndex=family;pc.flags=VK_COMMAND_POOL_CREATE_TRANSIENT_BIT;check(vkCreateCommandPool(s.device,&pc,nullptr,&s.pool),"scene qualification pool");
        auto ca=info<VkCommandBufferAllocateInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO);ca.commandPool=s.pool;ca.level=VK_COMMAND_BUFFER_LEVEL_PRIMARY;ca.commandBufferCount=1;check(vkAllocateCommandBuffers(s.device,&ca,&s.command),"scene qualification command");auto fc=info<VkFenceCreateInfo>(VK_STRUCTURE_TYPE_FENCE_CREATE_INFO);check(vkCreateFence(s.device,&fc,nullptr,&s.fence),"scene qualification fence");
        s.pitch=((s.width*4+255)/256)*256;const VkDeviceSize bytes=2*GUARD+VkDeviceSize(s.pitch)*s.height;s.makeBuffer(s.colorRead,bytes,VK_BUFFER_USAGE_TRANSFER_DST_BIT);s.makeBuffer(s.depthRead,bytes,VK_BUFFER_USAGE_TRANSFER_DST_BIT|VK_BUFFER_USAGE_STORAGE_BUFFER_BIT);s.makeDepthReadbackCompute();
        for(unsigned frame=0;frame<2;++frame){const float x=frame?.18f:-.18f;s.writeBuffer(s.colorRead,nullptr);s.writeBuffer(s.depthRead,nullptr);
            // Independently remap after the host prefill/unmap, before any copy.
            // Keep this observation separate from post-copy guard acceptance.
            inspectGuards(s.read(s.colorRead),frame,"color","after-host-prefill",s.colorRead,false);
            inspectGuards(s.read(s.depthRead),frame,"depth","after-host-prefill",s.depthRead,false);
            if(frame)check(vkResetFences(s.device,1,&s.fence),"scene qualification reset fence");check(vkResetCommandPool(s.device,s.pool,0),"scene qualification reset pool");auto begin=info<VkCommandBufferBeginInfo>(VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO);begin.flags=VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT;check(vkBeginCommandBuffer(s.command,&begin),"scene qualification begin");s.gpuPrefillReadbacks();++gpuPrefills;s.record(s.command,x);s.copies();check(vkEndCommandBuffer(s.command),"scene qualification end");
            auto submit=info<VkSubmitInfo>(VK_STRUCTURE_TYPE_SUBMIT_INFO);submit.commandBufferCount=1;submit.pCommandBuffers=&s.command;s.qualificationPending=true;check(vkQueueSubmit(queue,1,&submit,s.fence),"scene qualification submit");check(vkWaitForFences(s.device,1,&s.fence,VK_TRUE,30'000'000'000ULL),"scene qualification wait");s.qualificationPending=false;
            auto rgba=s.read(s.colorRead),depth=s.read(s.depthRead);
            inspectGuards(rgba,frame,"color","after-GPU-copy",s.colorRead,true);
            inspectGuards(depth,frame,"depth","after-GPU-sampled-depth",s.depthRead,true);
            struct Point{const char* name;float x,y,z;int color[4];};
            const Point points[]{{"near-over-far",x,0,.25f,{230,26,38,255}},{"far-only",x,-.55f,.75f,{26,64,230,255}},{"background",.95f,.8f,1,{5,10,20,255}},{"translation-witness",-.25f,0,frame?.75f:.25f,{frame?26:230,frame?64:26,frame?230:38,255}}};
            for(const auto& point:points){const int px=int((point.x+1)*.5f*s.width),py=int((1-point.y)*.5f*s.height);for(int dy=-1;dy<=1;++dy)for(int dx=-1;dx<=1;++dx){const size_t offset=size_t(GUARD)+size_t(py+dy)*s.pitch+size_t(px+dx)*4;for(unsigned c=0;c<4;++c)if(std::abs(int(rgba[offset+c])-point.color[c])>2)++colorErrors;float value{};std::memcpy(&value,depth.data()+offset,4);if(!std::isfinite(value)||std::abs(value-point.z)>1e-5f)++depthErrors;}
                const size_t center=size_t(GUARD)+size_t(py)*s.pitch+size_t(px)*4;float value{};std::memcpy(&value,depth.data()+center,4);if(samples.tellp()>0)samples<<',';samples<<"{\"frame\":"<<frame<<",\"point\":\""<<point.name<<"\",\"rgba\":["<<int(rgba[center])<<','<<int(rgba[center+1])<<','<<int(rgba[center+2])<<','<<int(rgba[center+3])<<"],\"depth\":";if(std::isfinite(value))samples<<value;else samples<<"null";samples<<'}';}
            ++completed;
        }
        s.retireQualification();const bool pass=guardErrors==0&&prefillErrors==0&&colorErrors==0&&depthErrors==0;
        std::ostringstream out;out<<"{\"status\":\""<<(pass?"pass":"fail")<<"\",\"pass\":"<<(pass?"true":"false")<<",\"frames\":"<<completed<<",\"depthReadbackMethod\":\"sampled_D32_compute_to_SSBO\""<<",\"gpuGuardPrefillFramesRecorded\":"<<gpuPrefills<<",\"gpuGuardPrefillValue32\":\"0xa5a5a5a5\",\"gpuPrefillTransferWriteBarrier\":true,\"cpuPrefillObservationRetained\":true"<<",\"guardBytesChecked\":"<<guardChecks<<",\"guardErrors\":"<<guardErrors<<",\"prefillGuardBytesChecked\":"<<prefillChecks<<",\"prefillGuardErrors\":"<<prefillErrors<<",\"guardDiagnostics\":["<<guardDiagnostics.str()<<"]"<<",\"colorErrors\":"<<colorErrors<<",\"depthErrors\":"<<depthErrors<<",\"samples\":["<<samples.str()<<"],\"actualDepthTestAndWrite\":true,\"nearDrawBeforeFar\":true,\"motionWitnessFrames\":2,\"positiveViewportHeight\":true,\"columnMajorVP\":[1,0,0,0,0,-1,0,0,0,0,0.5,0,0,0,0,1],\"columnMajorInvVP\":[1,0,0,0,0,-1,0,0,0,0,2,0,0,0,0,1],\"colorFormat\":\"RGBA8_UNORM\",\"rasterDepthFormat\":\"D32_SFLOAT\",\"aiDepthInput\":false,\"qualificationResourcesDestroyed\":true,\"validationQualifiedByHelper\":false,\"validationMustBeObservedByCaller\":true,\"scope\":\"Real 3D geometry/depth/motion color and depth readback, 3x3 samples in four interior regions each frame; no FG dispatch, display, GTA camera, or FPS claim. Caller must separately qualify Vulkan validation and retire scene/SDK uses.\"}";return out.str();
    }catch(const std::exception& error){s.retireQualification();std::ostringstream out;out<<"{\"status\":\"fail\",\"pass\":false,\"frames\":"<<completed<<",\"depthReadbackMethod\":\"sampled_D32_compute_to_SSBO\""<<",\"gpuGuardPrefillFramesRecorded\":"<<gpuPrefills<<",\"qualificationPending\":"<<(s.qualificationPending?"true":"false")<<",\"unsafeDestructionSkipped\":"<<(s.qualificationPending?"true":"false")<<",\"error\":\"";for(char c:std::string(error.what())){if(c=='"'||c=='\\')out<<'\\';if(static_cast<unsigned char>(c)>=32)out<<c;}out<<"\"}";return out.str();}
}
}
