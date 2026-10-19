#pragma once
#include <vulkan/vulkan.h>
#include <array>
#include <memory>
#include <string>

namespace muguet::fg_scene {
// Independent synthetic 3D scene. The caller owns the device and completes all
// GPU/SDK use of these images before destruction. This helper never calls FG.
class Scene {
public:
    static std::unique_ptr<Scene> create(VkPhysicalDevice physical, VkDevice device,
        uint32_t width, uint32_t height);
    ~Scene();
    Scene(const Scene&) = delete;
    Scene& operator=(const Scene&) = delete;
    VkImage colorImage() const;
    VkImageView colorView() const;
    VkImage depthImage() const;
    VkImageView depthView() const;
    // Actual matrices sent to the vertex shader, column-major. Positive Vulkan
    // viewport height; world Y points up, clip Y is flipped by this projection.
    const std::array<float,16>& viewProj() const;
    const std::array<float,16>& invViewProj() const;
    // Recording primary command buffer, outside a pass, same device/queue family.
    // Discards previous image contents; caller retires prior use before reuse.
    // Ends color in SHADER_READ_ONLY_OPTIMAL and depth in DEPTH_STENCIL_ATTACHMENT.
    void record(VkCommandBuffer command, float translationX);
    // Before FG image registration/use, renders two actual 3D frames, reads color
    // and actual sampled D32 texels via SSBO plus guards (no direct depth-image
    // to-buffer copy). Uses a private pool/fence and waits for it. This
    // is image qualification only; caller separately observes VVL/negative control
    // and externally synchronizes all host operations on this VkQueue.
    // Timeout reports pending work and intentionally skips unsafe GPU destruction.
    std::string qualify(VkQueue queue, uint32_t queueFamily);
private:
    struct Impl;
    explicit Scene(std::unique_ptr<Impl> implementation);
    std::unique_ptr<Impl> impl_;
};
}
