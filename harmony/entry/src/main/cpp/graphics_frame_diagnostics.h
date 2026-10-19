#ifndef MUGUET_GRAPHICS_FRAME_DIAGNOSTICS_H
#define MUGUET_GRAPHICS_FRAME_DIAGNOSTICS_H
#ifdef __cplusplus
extern "C" {
#endif
// Marker-only, independent Vulkan device. Returned JSON is thread-local.
// This stage qualifies the public context lifecycle, not generated/display FPS.
const char* muguet_graphics_frame_probe_json(const char* filesDirectory);
#ifdef __cplusplus
}
#endif
#endif
