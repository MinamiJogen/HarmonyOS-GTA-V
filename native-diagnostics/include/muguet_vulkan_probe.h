#ifndef MUGUET_VULKAN_PROBE_H
#define MUGUET_VULKAN_PROBE_H

#ifdef __cplusplus
extern "C" {
#endif

// Enumerates Vulkan loader/device capabilities. Creates an instance but no
// logical device, native window, surface, swapchain, image or presentation.
// Returns one JSON object. Errors are reported in status/errors; never infer
// surface/presentation success from enumeration success.
// Owned by a thread-local string: do NOT free. Copy on this calling thread
// before the next call here or before this thread exits.
const char *muguet_vulkan_probe_json(void);

#ifdef __cplusplus
}
#endif
#endif
