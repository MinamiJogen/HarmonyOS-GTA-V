#ifndef MUGUET_VULKAN_BUFFER_PROBE_H
#define MUGUET_VULKAN_BUFFER_PROBE_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

typedef struct MuguetVulkanBufferProbeConfig {
    /* allocation: 0 shared/nonzero binding, 1 standalone/zero, 2 dedicated pNext */
    uint32_t allocation;
    /* upload: 0 vkCmdCopyBuffer, 1 vkCmdUpdateBuffer */
    uint32_t upload;
    /* pool_policy: 0 vkResetCommandPool(flags=0), 1 destroy/recreate */
    uint32_t pool_policy;
    uint32_t physical_device;
    uint32_t iterations;
    uint32_t commands_per_pool;
    uint32_t reset_every;
    uint32_t target_usage;
    uint64_t target_bytes;
    uint64_t destination_offset;
    uint64_t target_binding_offset;
    uint64_t staging_binding_offset;
    /* Default 0. A requested control passes only after observing its real VUID. */
    uint32_t validation_negative_control;
} MuguetVulkanBufferProbeConfig;

void muguet_vulkan_buffer_probe_defaults(MuguetVulkanBufferProbeConfig *config);
/* Real Vulkan allocations/commands; may reproduce a native driver crash.
 * One case per process is recommended. No surface, game, shader or wgpu is used.
 * Returned JSON is thread-local. trace_path records before/after critical calls.
 */
const char *muguet_vulkan_buffer_probe_json(const MuguetVulkanBufferProbeConfig *config,
                                          const char *trace_path);
#ifdef __cplusplus
}
#endif
#endif
