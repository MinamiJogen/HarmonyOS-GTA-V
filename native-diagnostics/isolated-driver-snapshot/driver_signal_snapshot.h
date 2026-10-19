#pragma once
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

enum { MG_SNAPSHOT_MAX_LOADS = 16 };
typedef struct MgSnapshotLoad {
    uint64_t begin, end;
    uint32_t flags, reserved;
} MgSnapshotLoad;

/* Binary layout is little endian. Raw siginfo_t, raw fixed ucontext_t and the
 * eight-byte MGDONE2 trailer follow this header. Pointer-referenced extra
 * signal-frame records are deliberately not dereferenced in a handler. */
typedef struct MgDriverSnapshotHeader {
    char magic[8];
    uint32_t version, header_bytes, siginfo_bytes, ucontext_bytes;
    uint32_t load_count, prior_flags, machine, filter_pc_or_lr;
    uint32_t mcontext_offset, fault_address_offset, regs_offset, sp_offset;
    uint32_t pc_offset, pstate_offset, lr_offset, siginfo_addr_offset;
    uint64_t process_id, driver_load_bias;
    MgSnapshotLoad loads[MG_SNAPSHOT_MAX_LOADS];
} MgDriverSnapshotHeader;

typedef struct MgDriverSnapshotInstallReport {
    uint32_t prior_flags, load_count, executable_load_count;
    uint32_t header_bytes, siginfo_bytes, ucontext_bytes;
} MgDriverSnapshotInstallReport;

/* ISOLATED PROTOTYPE ONLY. Not linked by native-diagnostics/CMakeLists.txt.
 * Call once, serialized with all other signal-handler registrations, after
 * Wasmtime handlers and a live Vulkan device exist. The driver must stay loaded.
 * Only a callable SA_SIGINFO predecessor without SA_RESETHAND is accepted.
 * Returns 0 or an errno value; no automatic fallback, uninstall or signal stack
 * is installed. File is created O_EXCL/0600 and fd/state remain process-lived.
 * Device DFX chaining still requires independent in-app qualification. */
int mg_driver_signal_snapshot_install(const char *absolute_file_path,
    const char *exact_driver_basename, MgDriverSnapshotInstallReport *report);

#ifdef __cplusplus
}
#endif
