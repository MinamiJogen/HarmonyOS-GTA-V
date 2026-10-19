#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include "driver_signal_snapshot.h"
#include <atomic>
#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <fcntl.h>
#include <signal.h>
#include <unistd.h>

#ifdef MG_DRIVER_SNAPSHOT_HOST_TEST
// Host tests exercise policy/forwarding with a synthetic context, never claim
// that this is a kernel-produced OHOS signal frame.
struct SnapshotTestMContext {
    uint64_t fault_address, regs[31], sp, pc, pstate;
    unsigned char opaque[4096];
};
struct SnapshotContext { SnapshotTestMContext uc_mcontext; };
extern "C" ssize_t mg_snapshot_test_write(int, const void *, size_t);
extern "C" int mg_snapshot_test_fsync(int);
#else
#if !defined(__aarch64__) || !defined(__OHOS__)
#error This isolated implementation targets OHOS AArch64 only
#endif
#include <elf.h>
#include <link.h>
#include <ucontext.h>
using SnapshotContext = ucontext_t;
static_assert(sizeof(siginfo_t) == 128, "Review SDK siginfo layout");
static_assert(sizeof(ucontext_t) == 4560, "Review SDK fixed ucontext layout");
static_assert(sizeof(((ucontext_t *)nullptr)->uc_mcontext.regs) == 31 * 8,
    "AArch64 general register layout changed");
static_assert(sizeof(((ucontext_t *)nullptr)->uc_mcontext.__reserved) == 4096,
    "AArch64 inline extension area changed");
static_assert(offsetof(ucontext_t, uc_mcontext) == 176, "Review SDK mcontext offset");
static_assert(offsetof(ucontext_t, uc_mcontext.regs) == 184, "Review SDK GPR offset");
static_assert(offsetof(ucontext_t, uc_mcontext.sp) == 432, "Review SDK SP offset");
static_assert(offsetof(ucontext_t, uc_mcontext.pc) == 440, "Review SDK PC offset");
static_assert(offsetof(ucontext_t, uc_mcontext.pstate) == 448, "Review SDK PSTATE offset");
static_assert(offsetof(siginfo_t, si_addr) == 16, "Review SDK siginfo address offset");
#endif

namespace {
static_assert(ATOMIC_INT_LOCK_FREE == 2, "Handler gate must be always lock-free");
static_assert(sizeof(MgDriverSnapshotHeader) == 472, "Review binary header layout");
constexpr uint32_t kExecutableLoad = 1; // ELF PF_X.
constexpr char kTrailer[8] = {'M', 'G', 'D', 'O', 'N', 'E', '2', '\0'};
struct SnapshotState {
    int fd = -1;
    struct sigaction previous{};
    MgDriverSnapshotHeader header{};
    std::atomic<unsigned int> claimed{0};
    std::atomic<unsigned int> published{0};
    bool installed = false;
};
SnapshotState state;

int ValidatePrevious(const struct sigaction &action)
{
    if (action.sa_handler == SIG_DFL || action.sa_handler == SIG_IGN ||
        action.sa_handler == SIG_ERR) return ENOTSUP;
    if ((action.sa_flags & SA_SIGINFO) == 0 ||
        (action.sa_flags & SA_RESETHAND) != 0) return ENOTSUP;
    return 0;
}

bool IsExecutableAddress(uint64_t address)
{
    for (uint32_t i = 0; i < state.header.load_count && i < MG_SNAPSHOT_MAX_LOADS; ++i) {
        const auto &load = state.header.loads[i];
        if ((load.flags & kExecutableLoad) != 0 &&
            load.begin <= address && address < load.end) return true;
    }
    return false;
}

ssize_t SnapshotWrite(int fd, const void *data, size_t count)
{
#ifdef MG_DRIVER_SNAPSHOT_HOST_TEST
    return mg_snapshot_test_write(fd, data, count);
#else
    return ::write(fd, data, count);
#endif
}

int SnapshotFsync(int fd)
{
#ifdef MG_DRIVER_SNAPSHOT_HOST_TEST
    return mg_snapshot_test_fsync(fd);
#else
    return ::fsync(fd);
#endif
}

bool WriteBounded(const void *data, size_t count)
{
    const auto *bytes = static_cast<const unsigned char *>(data);
    // Finite retries; incomplete records have no valid completion trailer.
    for (unsigned int attempt = 0; count != 0 && attempt < 16; ++attempt) {
        const ssize_t result = SnapshotWrite(state.fd, bytes, count);
        if (result < 0) {
            if (errno == EINTR) continue;
            return false;
        }
        if (result == 0 || static_cast<size_t>(result) > count) return false;
        const auto written = static_cast<size_t>(result);
        bytes += written;
        count -= written;
    }
    return count == 0;
}

void SnapshotHandler(int signal, siginfo_t *info, void *raw_context) noexcept
{
    // A release publication precedes registration; acquire pairs all immutable
    // state with delivery on another thread. No state is changed after install.
    (void)state.published.load(std::memory_order_acquire);
    if (signal == SIGSEGV && info != nullptr && raw_context != nullptr &&
        info->si_code > 0) {
        const auto *context = static_cast<const SnapshotContext *>(raw_context);
        const uint64_t pc = context->uc_mcontext.pc;
        const uint64_t lr = context->uc_mcontext.regs[30];
        if (IsExecutableAddress(pc) || IsExecutableAddress(lr)) {
            unsigned int expected = 0;
            if (state.claimed.compare_exchange_strong(expected, 1,
                std::memory_order_relaxed, std::memory_order_relaxed)) {
                const int saved_errno = errno;
                const bool complete = WriteBounded(&state.header, sizeof(state.header)) &&
                    WriteBounded(info, sizeof(*info)) &&
                    WriteBounded(context, sizeof(*context));
                if (complete) (void)WriteBounded(kTrailer, sizeof(kTrailer));
                for (unsigned int attempt = 0; attempt < 4; ++attempt) {
                    if (SnapshotFsync(state.fd) == 0 || errno != EINTR) break;
                }
                errno = saved_errno;
            }
        }
    }
    // Same original pointers, same kernel-applied mask/ONSTACK/NODEFER flags.
    // Never repair a PC, return "handled", raise a replacement signal, restore
    // a default disposition or bypass the existing Wasmtime -> DFX chain.
    state.previous.sa_sigaction(signal, info, raw_context);
}

struct sigaction ReplacementAction(const struct sigaction &previous)
{
    struct sigaction replacement = previous;
    replacement.sa_sigaction = SnapshotHandler;
    return replacement;
}

void SetHeaderLayout()
{
    auto &header = state.header;
    std::memset(&header, 0, sizeof(header));
    const char magic[8] = {'M', 'G', 'S', 'E', 'G', 'V', '2', '\0'};
    std::memcpy(header.magic, magic, sizeof(magic));
    header.version = 2;
    header.header_bytes = sizeof(header);
    header.siginfo_bytes = sizeof(siginfo_t);
    header.ucontext_bytes = sizeof(SnapshotContext);
    header.machine = 183; // ELF EM_AARCH64; host fixture is marked separately.
#ifdef MG_DRIVER_SNAPSHOT_HOST_TEST
    header.machine = 0x53545542; // STUB.
#endif
    header.filter_pc_or_lr = 1;
    header.mcontext_offset = offsetof(SnapshotContext, uc_mcontext);
    header.fault_address_offset = offsetof(SnapshotContext, uc_mcontext.fault_address);
    header.regs_offset = offsetof(SnapshotContext, uc_mcontext.regs);
    header.sp_offset = offsetof(SnapshotContext, uc_mcontext.sp);
    header.pc_offset = offsetof(SnapshotContext, uc_mcontext.pc);
    header.pstate_offset = offsetof(SnapshotContext, uc_mcontext.pstate);
    header.lr_offset = header.regs_offset + 30 * sizeof(uint64_t);
    header.siginfo_addr_offset = offsetof(siginfo_t, si_addr);
    header.process_id = static_cast<uint64_t>(::getpid());
}

#ifndef MG_DRIVER_SNAPSHOT_HOST_TEST
struct Discovery {
    const char *basename;
    unsigned int matches = 0;
    int error = 0;
};
int CollectDriver(struct dl_phdr_info *info, size_t, void *user)
{
    auto *discovery = static_cast<Discovery *>(user);
    if (info->dlpi_name == nullptr || info->dlpi_name[0] == '\0') return 0;
    const char *slash = std::strrchr(info->dlpi_name, '/');
    const char *basename = slash == nullptr ? info->dlpi_name : slash + 1;
    if (std::strcmp(basename, discovery->basename) != 0) return 0;
    if (++discovery->matches != 1) { discovery->error = EEXIST; return 0; }
    state.header.driver_load_bias = info->dlpi_addr;
    for (uint32_t i = 0; i < info->dlpi_phnum; ++i) {
        const auto &program = info->dlpi_phdr[i];
        if (program.p_type != PT_LOAD || program.p_memsz == 0) continue;
        if (state.header.load_count == MG_SNAPSHOT_MAX_LOADS) {
            discovery->error = E2BIG; return 0;
        }
        const uint64_t bias = info->dlpi_addr;
        if (program.p_vaddr > UINT64_MAX - bias ||
            program.p_memsz > UINT64_MAX - (bias + program.p_vaddr)) {
            discovery->error = EOVERFLOW; return 0;
        }
        auto &load = state.header.loads[state.header.load_count++];
        load.begin = bias + program.p_vaddr;
        load.end = load.begin + program.p_memsz;
        load.flags = program.p_flags;
    }
    return 0;
}
#endif
} // namespace

extern "C" int mg_driver_signal_snapshot_install(const char *path,
    const char *basename, MgDriverSnapshotInstallReport *report)
{
    if (state.installed) return EALREADY;
    if (path == nullptr || path[0] != '/' || basename == nullptr ||
        basename[0] == '\0' || std::strchr(basename, '/') != nullptr) return EINVAL;
#ifdef MG_DRIVER_SNAPSHOT_HOST_TEST
    (void)report;
    return ENOTSUP; // Host tests do not install any process signal handler.
#else
    struct sigaction previous{};
    if (::sigaction(SIGSEGV, nullptr, &previous) != 0) return errno;
    const int validation = ValidatePrevious(previous);
    if (validation != 0) return validation;
    SetHeaderLayout();
    Discovery discovery{basename};
    ::dl_iterate_phdr(CollectDriver, &discovery);
    if (discovery.error != 0) return discovery.error;
    if (discovery.matches != 1 || state.header.load_count == 0) return ENOENT;
    uint32_t executable = 0;
    for (uint32_t i = 0; i < state.header.load_count; ++i)
        if ((state.header.loads[i].flags & kExecutableLoad) != 0) ++executable;
    if (executable == 0) return ENOEXEC;
    const int fd = ::open(path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (fd < 0) return errno;
    // Resolve all handler-side libc imports before registration, including the
    // errno accessor. Require successful file preparation; do not first enter a
    // lazy loader path while already inside a fault handler.
    const int prior_errno = errno;
    if (::write(fd, kTrailer, 0) != 0 || ::fsync(fd) != 0) {
        const int error = errno;
        ::close(fd);
        ::unlink(path);
        return error == 0 ? EIO : error;
    }
    errno = prior_errno;
    // Publish immutable data before registration. Caller must serialize this
    // operation with all other sigaction users; no process-wide API can make a
    // query+install transaction atomic against arbitrary third-party changes.
    state.previous = previous;
    state.header.prior_flags = static_cast<uint32_t>(previous.sa_flags);
    state.fd = fd;
    state.claimed.store(0, std::memory_order_relaxed);
    state.published.store(1, std::memory_order_release);
    const struct sigaction replacement = ReplacementAction(previous);
    if (::sigaction(SIGSEGV, &replacement, nullptr) != 0) {
        const int error = errno;
        state.fd = -1;
        ::close(fd);
        ::unlink(path);
        return error;
    }
    state.installed = true;
    if (report != nullptr) {
        *report = {state.header.prior_flags, state.header.load_count, executable,
            sizeof(MgDriverSnapshotHeader), sizeof(siginfo_t), sizeof(ucontext_t)};
    }
    return 0;
#endif
}
