#pragma once
#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Vulkan handles use ash::vk::Handle::as_raw() u64 values. All are borrowed.
// The wrapper neither creates nor owns a Vulkan instance/device/queue/texture.
// Only RGBA8_UNORM, 2D, mip0, one layer, sample count1 is qualified.
// Returns null on CPU-side validation, capability, symbol or create failure.
// Read/copy last_error immediately on the same calling thread for the reason.
void* muguet_xeg_spatial_create(uint64_t physicalDevice, uint64_t device,
    uint32_t inputWidth, uint32_t inputHeight, uint32_t outputWidth, uint32_t outputHeight, float sharpness);

// Must be outside a render/compute pass on a currently recording primary command
// buffer from the same device. Source is already SHADER_READ_ONLY_OPTIMAL.
// Output was initialized by an ordinary wgpu render pass and its tracker remains
// COLOR_TARGET/COLOR_ATTACHMENT_OPTIMAL. XEG discards/overwrites the full output.
// Source and output must be distinct, non-aliasing textures with matching sizes
// and formats from create(). The caller upholds this borrowed-resource contract.
// The bridge first transitions output COLOR_ATTACHMENT_OPTIMAL ->
// SHADER_READ_ONLY_OPTIMAL. XEG's opaque pass may discard prior contents and ends
// in SHADER_READ_ONLY_OPTIMAL, so real Vulkan and the visible layer state agree.
// After XEG's final SHADER_READ_ONLY layout, this bridge explicitly restores
// COLOR_ATTACHMENT_OPTIMAL, matching that unchanged wgpu tracker state.
// It does not end the command buffer, submit, wait, or destroy borrowed handles.
// False means CPU argument rejection only: public XEG render itself returns void.
bool muguet_xeg_spatial_record(void* context, uint64_t commandBuffer,
    uint64_t inputView, uint64_t outputView, uint64_t outputImage);

// Caller must first complete all submissions using this context and its borrowed
// image views. Destroy before the underlying VkDevice. No hidden wait/submit.
// The public SDK library mapping is held once for the process so it outlives
// borrowed devices; this function still destroys every owned SDK spatial object.
void muguet_xeg_spatial_destroy(void* context);

// Thread-local error text, valid until the next bridge call on this thread.
const char* muguet_xeg_spatial_last_error(void);

#ifdef __cplusplus
}
#endif
