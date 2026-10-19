#ifndef MUGUET_PLATFORM_H
#define MUGUET_PLATFORM_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Callback fills `frames * 2` interleaved 48 kHz float samples. */
typedef void (*MgAudioCallback)(void *user, float *samples, uint32_t frames);
int32_t mg_audio_start(MgAudioCallback callback, void *user);
void mg_audio_stop(void);
void mg_audio_pause(int32_t paused);

#ifdef __cplusplus
}
#endif

/* HarmonyOS API 26 presentation contract.
 *
 * XComponent passes the OHNativeWindow* from OnSurfaceCreated to the runtime's
 * start entry point before starting the guest. Rust takes a native reference,
 * creates raw_window_handle::OhosNdk, and creates a real VK_OHOS_surface wgpu
 * surface. The GPU thread owns the swapchain and all queue presentation.
 *
 * OnSurfaceChanged updates width/height. OnSurfaceDestroyed must suspend the
 * guest before returning. A new window requires an ability/process restart.
 * NativeWindow reference ownership keeps the underlying object valid until the
 * GPU and its wgpu surface have been dropped.
 *
 * Link libraries: libace_ndk.z.so (XComponent), libnative_window.so (window
 * references/geometry); libvulkan.so is loaded by wgpu-hal at run time.
 * libohaudio.so is the optional native audio output library.
 */

#endif
