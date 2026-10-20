#ifndef FG_RIFE_STREAM_H
#define FG_RIFE_STREAM_H

#include <cstdint>
#include <memory>
#include <string>
#include <vector>

namespace muguet::fg_probe {

enum class RifeStreamKind { Source, Interpolated, Repeat };
const char* rifeStreamKindName(RifeStreamKind kind) noexcept;

struct RifeStreamFrame {
    // Owned tightly packed RGB8 copy; no pointer into the bounded worker pool.
    std::vector<uint8_t> rgb8;
    int width = 0, height = 0;
    RifeStreamKind kind = RifeStreamKind::Source;
    RifeStreamKind originalKind = RifeStreamKind::Source;
    uint64_t source0Id = 0, source1Id = 0, pairId = 0, presentationIndex = 0;
    int64_t source0Ns = 0, source1Ns = 0, contentTimeNs = 0;
    // Scheduled display deadline for this output slot; source timestamps retain
    // the original content timing, including when this output is a repeat.
    int64_t deadlineNs = 0, producedAtNs = 0, selectedAtNs = 0;
    int64_t displayCallLateNs = 0;
    bool deadlineMissed = false, underrun = false, sourceGap = false;
};

class RifeStream {
public:
    RifeStream();
    ~RifeStream() noexcept;
    RifeStream(const RifeStream&) = delete;
    RifeStream& operator=(const RifeStream&) = delete;
    RifeStream(RifeStream&&) = delete;
    RifeStream& operator=(RifeStream&&) = delete;

    // Only 15->30Hz, 320x192 is currently qualified. Loads the SDK/model on the
    // inference worker, then establishes a common steady-clock source epoch.
    // true means workers started, not that model initialization/inference passed.
    bool start(const std::string& modelDir, int sourceFps = 15, int targetFps = 30,
               int width = 320, int height = 192);
    // False before epoch+200ms, for the same/older display slot, while starting,
    // after stop, or when no first image exists. Never selects future content.
    bool getFrameForTime(int64_t steadyTimeNs, RifeStreamFrame& output);
    bool waitNextFrame(RifeStreamFrame& output, uint32_t timeoutMs = 250);
    // Interrupts waits and joins both workers. The SDK C ABI cannot cancel an
    // in-flight driver call; stop waits for that one call to return.
    void stop() noexcept;
    // Thread-safe snapshot; empty means no detected initialization/worker error.
    std::string errorMessage() const;
    std::string statsJson() const;
    static int64_t steadyNowNs() noexcept;

private:
    struct Impl;
    std::unique_ptr<Impl> impl_;
};

} // namespace muguet::fg_probe
#endif
