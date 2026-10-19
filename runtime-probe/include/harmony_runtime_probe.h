#ifndef HARMONY_RUNTIME_PROBE_H
#define HARMONY_RUNTIME_PROBE_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
int32_t rp_add(int32_t left, int32_t right);
const char *rp_version(void);
int32_t rp_runtime_available(void);
/* Thread-local error text, valid until next error-producing probe call on this thread. */
const char *rp_last_error(void);
/* Only use trusted, unmodified Wasmtime 49.0.0 precompiled artifacts. 0 means success. */
int32_t rp_run_aot_add(const char *path, int32_t left, int32_t right, int32_t *result);
/* Memory must remain unchanged and alive throughout call, with its code already executable.
 * The pointer and length must describe our complete trusted precompiled ELF artifact. */
int32_t rp_run_raw_aot_add(const uint8_t *bytes, size_t length, int32_t left, int32_t right, int32_t *result);
#ifdef __cplusplus
}
#endif
#endif
