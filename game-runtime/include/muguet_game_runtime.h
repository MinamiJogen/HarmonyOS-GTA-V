#pragma once
#include <stdbool.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
int32_t mg_game_start(void* native_window,uint32_t width,uint32_t height,const char* files_dir);
/* Choose new/load/demo before constructing the guest. save_id is empty except
 * for load, where it is an app-private canonical relative save identifier. */
int32_t mg_game_set_launch(const char* action,const char* save_id);
/* Exact 0/1 compile capability. Does not initialize NNRT or validate a model/device. */
int32_t mg_game_frame_generation_available(void);
void mg_game_resize(uint32_t width,uint32_t height);
void mg_game_key(uint32_t key_code,bool pressed);
bool mg_game_key_event(uint32_t key_code,bool pressed,uint32_t unicode,bool suppress_text);
void mg_game_focus(bool focused);
/* App visibility is separate from input focus. Background is a reversible
 * presentation/inference pause; it does not request terminal game stop. */
void mg_game_app_foreground(bool foreground);
/* Pause a session whose XComponent window was destroyed. A replacement surface
 * is not attached automatically; status explicitly requires recovery. */
void mg_game_surface_lost(void);
/* Position is physical pixels; deltas are raw device movement units. */
void mg_game_pointer(float x,float y,float dx,float dy,bool look);
/* Fractional wheel notches are accumulated; positive is wheel up. */
void mg_game_wheel(float notches);
void mg_game_key_name(const char* dom_code,bool pressed);
void mg_game_mouse_button(uint32_t button,bool pressed);
void mg_game_mouse_delta(float dx,float dy);
void mg_game_touch(uint32_t action,float x,float y,uint32_t pointer_id);
void mg_game_control(int32_t control,float value);
void mg_game_stop(void);
const char* mg_game_status_json(void);
/* Latest cached read-only game observations. No guest calls from the UI thread;
 * copy the thread-local JSON immediately. Observation timestamps stay unchanged. */
const char* mg_game_touch_state_json(void);
bool mg_game_named_control(const char* name,float value,int32_t group);
bool mg_game_raw_key(const char* name,bool pressed,const char* source);
void mg_game_mouse_button_source(uint32_t button,bool pressed,const char* source);
void mg_game_release_inputs(void);
void mg_game_release_semantic_inputs(void);
/* Synchronous diagnostic worker call. Copy the returned thread-local JSON before
 * another call on the same thread. Does not start the game or create a surface. */
const char* mg_wgpu_atlas_probe_json(const char* files_dir,uint32_t iterations);
/* Marker-only differential worker; copy thread-local JSON immediately. */
const char* mg_depth_downsample_probe_json(const char* files_dir,uint32_t iterations);
/* Marker-only identical-WGSL workgroup initialization differential. */
const char* mg_luminance_probe_json(const char* files_dir,uint32_t iterations);
/* Independent actual GPU readback-pool qualification, iterations1..64.
 * Copy returned thread-local JSON immediately. One iteration covers both sizes
 * and six/two/four simultaneous requests, plus bypass/capacity/teardown cases. */
const char* mg_readback_pool_probe_json(const char* files_dir,uint32_t iterations);
/* Actual production WGPU conversion and XEngine spatial path, iterations1..4.
 * Independent marker-only worker. Copy returned thread-local JSON immediately. */
const char* mg_wgpu_spatial_probe_json(const char* files_dir,uint32_t iterations);
#ifdef __cplusplus
}
#endif
