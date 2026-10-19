#define MG_DRIVER_SNAPSHOT_HOST_TEST 1
#include "driver_signal_snapshot.cpp"
#include <algorithm>
#include <cassert>
#include <cstdio>
#include <vector>

namespace {
std::vector<unsigned char> written;
int writeCalls, fsyncCalls, chained, observedErrno;
bool interruptWrite, failWrite, repairContext;
size_t chunkSize;
siginfo_t *receivedInfo;
void *receivedContext;
int receivedSignal;

void Previous(int signal, siginfo_t *info, void *context)
{
    ++chained;
    receivedSignal = signal;
    receivedInfo = info;
    receivedContext = context;
    observedErrno = errno;
    if (repairContext) static_cast<SnapshotContext *>(context)->uc_mcontext.pc = 0xbeef;
}
void Legacy(int) {}

void Reset()
{
    written.clear();
    writeCalls = fsyncCalls = chained = observedErrno = 0;
    interruptWrite = failWrite = repairContext = false;
    chunkSize = 0;
    receivedInfo = nullptr; receivedContext = nullptr; receivedSignal = 0;
    SetHeaderLayout();
    state.header.load_count = 2;
    state.header.loads[0] = {0x1000, 0x2000, kExecutableLoad, 0};
    state.header.loads[1] = {0x3000, 0x4000, 0, 0};
    state.fd = 77;
    state.claimed.store(0);
    state.previous = {};
    state.previous.sa_sigaction = Previous;
    state.previous.sa_flags = SA_SIGINFO | SA_ONSTACK | SA_NODEFER;
    sigemptyset(&state.previous.sa_mask);
    sigaddset(&state.previous.sa_mask, SIGUSR1);
    state.header.prior_flags = static_cast<uint32_t>(state.previous.sa_flags);
    state.published.store(1, std::memory_order_release);
    errno = EDOM;
}

void CheckRecord(const siginfo_t &info, const SnapshotContext &context)
{
    const size_t expected = sizeof(MgDriverSnapshotHeader) + sizeof(siginfo_t) +
        sizeof(SnapshotContext) + sizeof(kTrailer);
    assert(written.size() == expected);
    MgDriverSnapshotHeader header{};
    std::memcpy(&header, written.data(), sizeof(header));
    assert(std::memcmp(header.magic, "MGSEGV2", 7) == 0);
    assert(header.machine == 0x53545542); // Explicit host-fixture marker.
    assert(header.siginfo_bytes == sizeof(info));
    assert(header.ucontext_bytes == sizeof(context));
    assert(std::memcmp(written.data() + sizeof(header), &info, sizeof(info)) == 0);
    assert(std::memcmp(written.data() + sizeof(header) + sizeof(info), &context, sizeof(context)) == 0);
    assert(std::memcmp(written.data() + written.size() - 8, kTrailer, 8) == 0);
}
} // namespace

extern "C" ssize_t mg_snapshot_test_write(int fd, const void *data, size_t count)
{
    assert(fd == 77);
    ++writeCalls;
    if (failWrite) { errno = ENOSPC; return -1; }
    if (interruptWrite) { interruptWrite = false; errno = EINTR; return -1; }
    if (chunkSize != 0) count = std::min(count, chunkSize);
    const auto *bytes = static_cast<const unsigned char *>(data);
    written.insert(written.end(), bytes, bytes + count);
    return static_cast<ssize_t>(count);
}
extern "C" int mg_snapshot_test_fsync(int fd)
{
    assert(fd == 77); ++fsyncCalls; return 0;
}

int main()
{
    siginfo_t info{};
    info.si_signo = SIGSEGV;
    info.si_code = SEGV_MAPERR;
    info.si_addr = reinterpret_cast<void *>(0x454c414d4554ULL);
    SnapshotContext context{};
    context.uc_mcontext.pc = 0x9999;
    context.uc_mcontext.sp = 0xaabb;
    context.uc_mcontext.regs[30] = 0x8888;

    Reset(); repairContext = true;
    SnapshotHandler(SIGSEGV, &info, &context);
    assert(chained == 1 && receivedInfo == &info && receivedContext == &context);
    assert(receivedSignal == SIGSEGV && observedErrno == EDOM);
    assert(context.uc_mcontext.pc == 0xbeef && writeCalls == 0 && fsyncCalls == 0);
    assert(state.claimed.load() == 0); // A benign Wasmtime-style handler can repair context.

    Reset(); context.uc_mcontext.pc = 0x1000; repairContext = true;
    for (unsigned int i = 0; i < 31; ++i) context.uc_mcontext.regs[i] = 0x12340000 + i;
    const SnapshotContext before = context;
    SnapshotHandler(SIGSEGV, &info, &context);
    CheckRecord(info, before);
    assert(chained == 1 && fsyncCalls == 1 && observedErrno == EDOM);
    assert(context.uc_mcontext.pc == 0xbeef); // Old handler still receives original frame.
    const size_t firstBytes = written.size();
    context.uc_mcontext.pc = 0x1004;
    SnapshotHandler(SIGSEGV, &info, &context);
    assert(chained == 2 && written.size() == firstBytes); // One snapshot per process.

    Reset(); context = {}; context.uc_mcontext.pc = 4; context.uc_mcontext.regs[30] = 0x1fff;
    SnapshotHandler(SIGSEGV, &info, &context);
    CheckRecord(info, context); // The observed bad indirect callback PC is included by LR.

    Reset(); context.uc_mcontext.pc = 0x2000; context.uc_mcontext.regs[30] = 0;
    SnapshotHandler(SIGSEGV, &info, &context);
    assert(chained == 1 && writeCalls == 0); // End-exclusive executable segment.
    context.uc_mcontext.pc = 0x3001;
    SnapshotHandler(SIGSEGV, &info, &context);
    assert(chained == 2 && writeCalls == 0); // Non-executable PT_LOAD excluded.

    Reset(); context.uc_mcontext.pc = 0x1100;
    info.si_code = 0; // OHOS SI_USER; Darwin's host-only SI_USER value differs.
    SnapshotHandler(SIGSEGV, &info, &context);
    assert(chained == 1 && writeCalls == 0);
    info.si_code = SEGV_MAPERR;
    SnapshotHandler(SIGBUS, &info, &context);
    assert(chained == 2 && writeCalls == 0);
    SnapshotHandler(SIGSEGV, nullptr, nullptr);
    assert(chained == 3 && receivedInfo == nullptr && receivedContext == nullptr);

    Reset(); interruptWrite = true; chunkSize = 512;
    SnapshotHandler(SIGSEGV, &info, &context);
    CheckRecord(info, context);
    assert(chained == 1 && fsyncCalls == 1 && observedErrno == EDOM);

    Reset(); failWrite = true;
    SnapshotHandler(SIGSEGV, &info, &context);
    assert(chained == 1 && fsyncCalls == 1 && written.empty() && observedErrno == EDOM);

    Reset(); chunkSize = 1; // Retry budget exhausted; no completion trailer.
    SnapshotHandler(SIGSEGV, &info, &context);
    assert(chained == 1 && writeCalls == 16 && written.size() == 16 && fsyncCalls == 1);

    Reset();
    assert(ValidatePrevious(state.previous) == 0);
    const auto replacement = ReplacementAction(state.previous);
    assert(replacement.sa_flags == state.previous.sa_flags);
    assert(std::memcmp(&replacement.sa_mask, &state.previous.sa_mask, sizeof(sigset_t)) == 0);
    assert(replacement.sa_sigaction == SnapshotHandler);
    auto bad = state.previous; bad.sa_flags |= SA_RESETHAND;
    assert(ValidatePrevious(bad) == ENOTSUP);
    bad = state.previous; bad.sa_handler = SIG_DFL;
    assert(ValidatePrevious(bad) == ENOTSUP);
    bad.sa_handler = SIG_IGN;
    assert(ValidatePrevious(bad) == ENOTSUP);
    bad = {}; bad.sa_handler = Legacy;
    assert(ValidatePrevious(bad) == ENOTSUP);
    assert(mg_driver_signal_snapshot_install("/tmp/not-installed", "driver.so", nullptr) == ENOTSUP);
    assert(mg_driver_signal_snapshot_install("relative", "driver.so", nullptr) == EINVAL);
    std::puts("PASS: isolated dispatch, PC/LR scope, raw frame, one-shot, errno, bounded I/O, predecessor policy; no live signal hook installed");
}
