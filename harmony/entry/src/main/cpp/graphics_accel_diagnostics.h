#pragma once

// Runs in the existing diagnostic worker, before the game runtime starts. The
// returned thread-local JSON must be copied before the next call on that thread.
extern "C" const char* muguet_graphics_accel_probe_json(const char* filesDirectory);
