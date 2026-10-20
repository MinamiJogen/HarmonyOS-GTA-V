#include "fg_rife_stream.h"

#ifndef MUGUET_HAS_RIFE
#define MUGUET_HAS_RIFE 0
#endif
#if MUGUET_HAS_RIFE
#include <harmony_rife_c.h>
#endif

#include <algorithm>
#include <array>
#include <chrono>
#include <cmath>
#include <condition_variable>
#include <cstring>
#include <deque>
#include <exception>
#include <mutex>
#include <sstream>
#include <thread>
#include <utility>

namespace muguet::fg_probe {
namespace {
using Clock = std::chrono::steady_clock;
constexpr int64_t kSecond = 1000000000, kBufferDelay = 200000000;
constexpr size_t kPoolCount = 12, kQueueCapacity = 8, kHistoryCapacity = 3;
constexpr size_t kGuard = 64, kSampleCapacity = 64;
int64_t Now() noexcept
{
    return std::chrono::duration_cast<std::chrono::nanoseconds>(Clock::now().time_since_epoch()).count();
}
Clock::time_point At(int64_t ns) { return Clock::time_point(std::chrono::nanoseconds(ns)); }
int64_t TickOffset(uint64_t index, int hz)
{
    return int64_t(index / unsigned(hz)) * kSecond + int64_t(index % unsigned(hz)) * kSecond / hz;
}
uint64_t IndexAt(int64_t elapsed, int hz)
{
    if (elapsed <= 0) return 0;
    uint64_t index = uint64_t(elapsed / kSecond) * unsigned(hz) +
        uint64_t(elapsed % kSecond) * unsigned(hz) / uint64_t(kSecond);
    // Integer nanosecond deadlines use floor(n*1e9/hz); correct its inverse at
    // those boundaries rather than dropping a slot because it is 1ns early.
    if (TickOffset(index + 1, hz) <= elapsed) ++index;
    return index;
}
std::string Quote(const std::string& value)
{
    constexpr char hex[] = "0123456789abcdef";
    std::string out = "\"";
    for (unsigned char c : value) {
        if (c == '"' || c == '\\') { out += '\\'; out += char(c); }
        else if (c < 32) { out += "\\u00"; out += hex[c >> 4]; out += hex[c & 15]; }
        else out += char(c);
    }
    return out + '"';
}
struct Pixels {
    std::vector<uint8_t> guarded;
    explicit Pixels(size_t bytes) : guarded(bytes + 2 * kGuard) {}
    uint8_t* data() { return guarded.data() + kGuard; }
    const uint8_t* data() const { return guarded.data() + kGuard; }
    void ResetGuards()
    {
        std::fill(guarded.begin(), guarded.begin() + kGuard, 0xD3);
        std::fill(guarded.end() - kGuard, guarded.end(), 0x7B);
    }
    bool GuardsIntact() const
    {
        return std::all_of(guarded.begin(), guarded.begin() + kGuard, [](uint8_t c) { return c == 0xD3; }) &&
            std::all_of(guarded.end() - kGuard, guarded.end(), [](uint8_t c) { return c == 0x7B; });
    }
};
struct Item {
    std::shared_ptr<Pixels> pixels;
    RifeStreamKind kind = RifeStreamKind::Source;
    uint64_t slot = 0, firstId = 0, secondId = 0, pairId = 0;
    int64_t firstNs = 0, secondNs = 0, contentNs = 0, producedNs = 0;
    bool sourceGap = false;
};
struct Counters {
    uint64_t sourceGenerated = 0, sourceTicksSkipped = 0, sourcePoolDrops = 0;
    uint64_t historyEvictions = 0, queueSourceInserted = 0, queueMidInserted = 0;
    uint64_t overflowSourceDrops = 0, overflowMidDrops = 0;
    uint64_t obsoleteSourceDrops = 0, obsoleteMidDrops = 0;
    uint64_t lateSourceDrops = 0, lateMidDrops = 0, inferencePoolDrops = 0;
    uint64_t pairsSkippedForFreshness = 0, nonconsecutivePairs = 0;
    uint64_t inferenceCalls = 0, inferenceCompleted = 0, inferenceErrors = 0, guardErrors = 0;
    uint64_t selectedSource = 0, selectedMid = 0, selectedRepeat = 0;
    uint64_t underruns = 0, emptyUnderruns = 0, firstFallbackRepeats = 0;
    uint64_t sameOrOlderSlotRequests = 0, displaySlotsSkipped = 0;
    uint64_t poolExhaustions = 0, poolHighWater = 0, queueHighWater = 0, historyHighWater = 0;
    uint64_t stopDiscardedQueue = 0, stopDiscardedHistory = 0;
    uint64_t stopDiscardedInFlightSource = 0, stopDiscardedInFlightMid = 0;
    int64_t sourceFirstProducedNs = 0, sourceLastProducedNs = 0, maxSourceLatenessNs = 0;
    int64_t firstSelectionNs = 0, lastSelectionNs = 0, inferenceTotalNs = 0, inferenceMaxNs = 0;
    std::array<int64_t, kSampleCapacity> samples{};
    uint64_t sampleCount = 0;
};

void Generate(Pixels& pixels, int width, int height, int64_t contentElapsedNs)
{
    // Continuous motion driven by the stream's steady-clock source timeline.
    // The texture is regenerated each source tick, never a precomputed pair.
    const int objectWidth = 96, objectHeight = 80, objectY = 64;
    const double travel = double(width - objectWidth - 32);
    const double distance = std::fmod(double(contentElapsedNs) / 1e9 * 240.0, 2.0 * travel);
    const int objectX = 16 + int(std::lround(distance <= travel ? distance : 2.0 * travel - distance));
    uint8_t* output = pixels.data();
    for (int y = 0; y < height; ++y) {
        for (int x = 0; x < width; ++x) {
            const int checker = ((x / 12) + (y / 12)) & 1;
            uint8_t r = uint8_t(24 + (x * 3 + y * 2) % 48 + checker * 16);
            uint8_t g = uint8_t(40 + (y * 5) % 56), b = uint8_t(70 + (x + y) % 72);
            if (x >= objectX && x < objectX + objectWidth && y >= objectY && y < objectY + objectHeight) {
                const int u = x - objectX, v = y - objectY;
                r = uint8_t(160 + (u * 7 + v * 3) % 80);
                g = uint8_t(35 + (((u / 6) ^ (v / 6)) & 1) * 150);
                b = uint8_t(25 + (u * 5 + v * 9) % 100);
            }
            const size_t offset = (size_t(y) * size_t(width) + size_t(x)) * 3;
            output[offset] = r; output[offset + 1] = g; output[offset + 2] = b;
        }
    }
}
} // namespace

const char* rifeStreamKindName(RifeStreamKind kind) noexcept
{
    switch (kind) {
        case RifeStreamKind::Source: return "source";
        case RifeStreamKind::Interpolated: return "interpolated";
        case RifeStreamKind::Repeat: return "repeat";
    }
    return "unknown";
}

struct RifeStream::Impl {
    mutable std::mutex mutex;
    std::mutex lifecycle;
    std::condition_variable cv;
    std::thread sourceThread, inferenceThread;
    std::array<std::shared_ptr<Pixels>, kPoolCount> pool{};
    std::deque<Item> history, queue;
    Item last;
    bool haveLast = false, ready = false, stopping = true, started = false;
    bool consumedSlot = false;
    uint64_t lastSlot = 0;
    int sourceFps = 15, targetFps = 30, width = 320, height = 192;
    size_t bytes = size_t(320) * 192 * 3;
    int64_t startCallNs = 0, readyNs = 0, epochNs = 0, modelLoadNs = 0;
    std::string state = "idle", error, modelDirectory, revision, deviceName;
    Counters counters;

    std::shared_ptr<Pixels> AcquireLocked()
    {
        std::shared_ptr<Pixels> available;
        uint64_t used = 0;
        for (const auto& candidate : pool) {
            if (candidate && candidate.use_count() == 1) {
                if (!available) available = candidate;
            } else if (candidate) ++used;
        }
        if (available) counters.poolHighWater = std::max(counters.poolHighWater, used + 1);
        else ++counters.poolExhaustions;
        return available;
    }
    int64_t Deadline(uint64_t slot) const { return epochNs + kBufferDelay + TickOffset(slot, targetFps); }
    void DropObsoleteLocked(const Item& item)
    {
        if (item.kind == RifeStreamKind::Interpolated) ++counters.obsoleteMidDrops;
        else ++counters.obsoleteSourceDrops;
    }
    void EnqueueLocked(Item item)
    {
        if (item.producedNs > Deadline(item.slot) || (consumedSlot && item.slot <= lastSlot)) {
            if (item.kind == RifeStreamKind::Interpolated) ++counters.lateMidDrops;
            else ++counters.lateSourceDrops;
            return;
        }
        const auto position = std::upper_bound(queue.begin(), queue.end(), item.contentNs,
            [](int64_t time, const Item& candidate) { return time < candidate.contentNs; });
        if (item.kind == RifeStreamKind::Interpolated) ++counters.queueMidInserted;
        else ++counters.queueSourceInserted;
        queue.insert(position, std::move(item));
        if (queue.size() > kQueueCapacity) {
            if (queue.front().kind == RifeStreamKind::Interpolated) ++counters.overflowMidDrops;
            else ++counters.overflowSourceDrops;
            queue.pop_front();
        }
        counters.queueHighWater = std::max(counters.queueHighWater, uint64_t(queue.size()));
        cv.notify_all();
    }
    void Fail(const std::string& message)
    {
        std::lock_guard<std::mutex> lock(mutex);
        error = message; state = "failed"; stopping = true; ready = false;
        cv.notify_all();
    }
    void SourceLoop()
    {
        try {
            std::unique_lock<std::mutex> lock(mutex);
            cv.wait(lock, [&] { return stopping || ready; });
            if (stopping) return;
            uint64_t next = 0, previous = 0;
            bool havePrevious = false;
            while (!stopping) {
                const int64_t scheduled = epochNs + TickOffset(next, sourceFps);
                cv.wait_until(lock, At(scheduled), [&] { return stopping; });
                if (stopping) break;
                const int64_t now = Now();
                const uint64_t current = IndexAt(now - epochNs, sourceFps);
                if (current > next) { counters.sourceTicksSkipped += current - next; next = current; }
                Item source;
                source.slot = next * 2; source.firstId = next; source.secondId = next; source.pairId = next;
                source.firstNs = epochNs + TickOffset(next, sourceFps);
                source.secondNs = source.contentNs = source.firstNs;
                source.sourceGap = havePrevious && next != previous + 1;
                counters.maxSourceLatenessNs = std::max(counters.maxSourceLatenessNs, now - source.firstNs);
                source.pixels = AcquireLocked();
                if (!source.pixels) { ++counters.sourcePoolDrops; ++next; continue; }
                lock.unlock();
                source.pixels->ResetGuards();
                Generate(*source.pixels, width, height, source.contentNs - epochNs);
                source.producedNs = Now();
                lock.lock();
                ++counters.sourceGenerated;
                if (!counters.sourceFirstProducedNs) counters.sourceFirstProducedNs = source.producedNs;
                counters.sourceLastProducedNs = source.producedNs;
                if (stopping) { ++counters.stopDiscardedInFlightSource; break; }
                previous = next; havePrevious = true;
                if (history.size() == kHistoryCapacity) { history.pop_front(); ++counters.historyEvictions; }
                history.push_back(source);
                counters.historyHighWater = std::max(counters.historyHighWater, uint64_t(history.size()));
                EnqueueLocked(std::move(source));
                cv.notify_all();
                ++next;
            }
        } catch (const std::exception& exception) { Fail(std::string("source_worker: ") + exception.what()); }
        catch (...) { Fail("source_worker: unknown exception"); }
    }
    void InferenceLoop()
    {
#if MUGUET_HAS_RIFE
        struct Context {
            HarmonyRifeContext* value = nullptr;
            ~Context() { if (value) harmony_rife_destroy(value); }
        } context;
        try {
            char message[512]{};
            const int64_t loadStart = Now();
            context.value = harmony_rife_create(modelDirectory.c_str(), 0, message, sizeof(message));
            const int64_t loaded = Now();
            if (!context.value) {
                Fail("harmony_rife_create: " + std::string(message, strnlen(message, sizeof(message))));
                return;
            }
            HarmonyRifeDeviceInfo device{};
            const int deviceCode = harmony_rife_get_device_info(context.value, &device);
            if (deviceCode != 0 || device.gpu_index != 0) {
                Fail("RIFE Vulkan device validation failed code=" + std::to_string(deviceCode)); return;
            }
            const char* build = harmony_rife_build_revision();
            std::unique_lock<std::mutex> lock(mutex);
            modelLoadNs = loaded - loadStart;
            deviceName.assign(device.device_name, strnlen(device.device_name, sizeof(device.device_name)));
            revision = build ? std::string(build, strnlen(build, 512)) : "unavailable";
            if (stopping) return;
            readyNs = epochNs = Now(); ready = true; state = "buffering";
            cv.notify_all();
            uint64_t attemptedRight = 0;
            while (!stopping) {
                cv.wait(lock, [&] {
                    return stopping || (history.size() >= 2 && history.back().secondId > attemptedRight);
                });
                if (stopping) break;
                Item left = history[history.size() - 2], right = history.back();
                if (right.secondId > attemptedRight + 1)
                    counters.pairsSkippedForFreshness += right.secondId - attemptedRight - 1;
                attemptedRight = right.secondId;
                if (right.secondId != left.secondId + 1) { ++counters.nonconsecutivePairs; continue; }
                Item middle;
                middle.kind = RifeStreamKind::Interpolated; middle.slot = right.slot - 1;
                middle.firstId = left.secondId; middle.secondId = right.secondId; middle.pairId = right.secondId;
                middle.firstNs = left.contentNs; middle.secondNs = right.contentNs;
                middle.contentNs = left.contentNs + (right.contentNs - left.contentNs) / 2;
                middle.pixels = AcquireLocked();
                if (!middle.pixels) { ++counters.inferencePoolDrops; continue; }
                middle.pixels->ResetGuards();
                ++counters.inferenceCalls;
                lock.unlock();
                const int64_t begin = Now();
                const int code = harmony_rife_process_rgb8(context.value, left.pixels->data(), right.pixels->data(),
                    width, height, middle.pixels->data(), bytes, 0.5f);
                middle.producedNs = Now();
                const int64_t duration = middle.producedNs - begin;
                const bool intact = middle.pixels->GuardsIntact();
                lock.lock();
                counters.inferenceTotalNs += duration;
                counters.inferenceMaxNs = std::max(counters.inferenceMaxNs, duration);
                counters.samples[counters.sampleCount % kSampleCapacity] = duration;
                ++counters.sampleCount;
                if (code != 0) ++counters.inferenceErrors;
                if (!intact) ++counters.guardErrors;
                if (code != 0 || !intact) {
                    error = code != 0 ? "RIFE process failed code=" + std::to_string(code) : "RIFE output guard corrupted";
                    state = "failed"; stopping = true; ready = false; cv.notify_all(); break;
                }
                ++counters.inferenceCompleted;
                if (!stopping) EnqueueLocked(std::move(middle));
                else ++counters.stopDiscardedInFlightMid;
            }
        } catch (const std::exception& exception) { Fail(std::string("inference_worker: ") + exception.what()); }
        catch (...) { Fail("inference_worker: unknown exception"); }
#else
        Fail("MUGUET_HAS_RIFE=0; optional RIFE SDK is not linked");
#endif
    }
    bool SelectLocked(int64_t now, RifeStreamFrame& output)
    {
        if (stopping || !ready || now < Deadline(0)) return false;
        const uint64_t slot = IndexAt(now - Deadline(0), targetFps);
        if (consumedSlot && slot <= lastSlot) { ++counters.sameOrOlderSlotRequests; return false; }
        const uint64_t skipped = consumedSlot ? slot - lastSlot - 1 : slot;
        counters.displaySlotsSkipped += skipped;
        lastSlot = slot; consumedSlot = true;
        Item past;
        bool havePast = false;
        while (!queue.empty() && queue.front().slot < slot) {
            if (havePast) DropObsoleteLocked(past);
            past = std::move(queue.front()); queue.pop_front(); havePast = true;
        }
        bool underrun = false, repeated = false;
        if (!queue.empty() && queue.front().slot == slot) {
            last = std::move(queue.front()); queue.pop_front(); haveLast = true;
            if (last.kind == RifeStreamKind::Interpolated) ++counters.selectedMid;
            else ++counters.selectedSource;
        } else {
            underrun = true; ++counters.underruns;
            if (!haveLast && havePast) {
                last = std::move(past); haveLast = true; havePast = false;
                ++counters.firstFallbackRepeats;
            }
            if (!haveLast) {
                if (havePast) DropObsoleteLocked(past);
                ++counters.emptyUnderruns; return false;
            }
            repeated = true; ++counters.selectedRepeat;
        }
        if (havePast) DropObsoleteLocked(past);
        output.rgb8.assign(last.pixels->data(), last.pixels->data() + bytes);
        output.width = width; output.height = height;
        output.originalKind = last.kind;
        output.kind = repeated ? RifeStreamKind::Repeat : last.kind;
        output.source0Id = last.firstId; output.source1Id = last.secondId; output.pairId = last.pairId;
        output.source0Ns = last.firstNs; output.source1Ns = last.secondNs;
        output.contentTimeNs = last.contentNs; output.producedAtNs = last.producedNs;
        output.presentationIndex = slot; output.deadlineNs = Deadline(slot); output.selectedAtNs = now;
        output.displayCallLateNs = std::max<int64_t>(0, now - output.deadlineNs);
        output.underrun = underrun; output.sourceGap = last.sourceGap;
        output.deadlineMissed = underrun || skipped != 0 || last.producedNs > output.deadlineNs;
        if (!counters.firstSelectionNs) counters.firstSelectionNs = now;
        counters.lastSelectionNs = now; state = "running";
        return true;
    }
    void StopLockedLifecycle() noexcept
    {
        {
            std::lock_guard<std::mutex> lock(mutex);
            stopping = true; ready = false; cv.notify_all();
        }
        if (sourceThread.joinable()) sourceThread.join();
        if (inferenceThread.joinable()) inferenceThread.join();
        std::lock_guard<std::mutex> lock(mutex);
        counters.stopDiscardedQueue += queue.size(); counters.stopDiscardedHistory += history.size();
        queue.clear(); history.clear(); last = Item{}; haveLast = false;
        for (auto& item : pool) item.reset();
        started = false;
        if (state != "failed") state = "stopped";
    }
};

RifeStream::RifeStream() : impl_(std::make_unique<Impl>()) {}
RifeStream::~RifeStream() noexcept { stop(); }
int64_t RifeStream::steadyNowNs() noexcept { return Now(); }
bool RifeStream::start(const std::string& modelDir, int sourceFps, int targetFps, int width, int height)
{
    std::lock_guard<std::mutex> lifecycle(impl_->lifecycle);
    std::unique_lock<std::mutex> lock(impl_->mutex);
    if (impl_->started || impl_->sourceThread.joinable() || impl_->inferenceThread.joinable()) return false;
    if (sourceFps != 15 || targetFps != 30 || width != 320 || height != 192 ||
        modelDir.empty() || modelDir.front() != '/' || modelDir.find('\0') != std::string::npos) {
        impl_->state = "failed"; impl_->error = "requires absolute modelDir and qualified 15->30Hz320x192"; return false;
    }
#if !MUGUET_HAS_RIFE
    impl_->state = "failed"; impl_->error = "MUGUET_HAS_RIFE=0; optional RIFE SDK is not linked"; return false;
#else
    try {
        impl_->counters = Counters{};
        impl_->sourceFps = sourceFps; impl_->targetFps = targetFps; impl_->width = width; impl_->height = height;
        impl_->bytes = size_t(width) * size_t(height) * 3; impl_->modelDirectory = modelDir;
        impl_->history.clear(); impl_->queue.clear(); impl_->last = Item{}; impl_->haveLast = false;
        impl_->ready = false; impl_->consumedSlot = false; impl_->lastSlot = 0;
        impl_->readyNs = impl_->epochNs = impl_->modelLoadNs = 0;
        impl_->error.clear(); impl_->revision.clear(); impl_->deviceName.clear();
        impl_->startCallNs = Now(); impl_->state = "starting"; impl_->stopping = false;
        for (auto& pixels : impl_->pool) pixels = std::make_shared<Pixels>(impl_->bytes);
        impl_->started = true;
        impl_->sourceThread = std::thread([this] { impl_->SourceLoop(); });
        impl_->inferenceThread = std::thread([this] { impl_->InferenceLoop(); });
        return true;
    } catch (const std::exception& exception) {
        impl_->error = std::string("start: ") + exception.what(); impl_->state = "failed";
        lock.unlock(); impl_->StopLockedLifecycle(); return false;
    } catch (...) {
        impl_->error = "start: unknown exception"; impl_->state = "failed";
        lock.unlock(); impl_->StopLockedLifecycle(); return false;
    }
#endif
}
bool RifeStream::getFrameForTime(int64_t steadyTimeNs, RifeStreamFrame& output)
{
    std::lock_guard<std::mutex> lock(impl_->mutex);
    return impl_->SelectLocked(steadyTimeNs, output);
}
bool RifeStream::waitNextFrame(RifeStreamFrame& output, uint32_t timeoutMs)
{
    const auto timeout = Clock::now() + std::chrono::milliseconds(std::min<uint32_t>(timeoutMs, 60000));
    std::unique_lock<std::mutex> lock(impl_->mutex);
    while (!impl_->stopping) {
        if (impl_->ready) {
            const int64_t deadline = impl_->Deadline(impl_->consumedSlot ? impl_->lastSlot + 1 : 0);
            if (Now() >= deadline) return impl_->SelectLocked(Now(), output);
            if (impl_->cv.wait_until(lock, std::min(timeout, At(deadline))) == std::cv_status::timeout &&
                Clock::now() >= timeout) return false;
        } else {
            if (impl_->cv.wait_until(lock, timeout) == std::cv_status::timeout) return false;
        }
        if (Clock::now() >= timeout) return false;
    }
    return false;
}
void RifeStream::stop() noexcept
{
    std::lock_guard<std::mutex> lifecycle(impl_->lifecycle);
    impl_->StopLockedLifecycle();
}
std::string RifeStream::errorMessage() const
{
    std::lock_guard<std::mutex> lock(impl_->mutex);
    return impl_->error;
}
std::string RifeStream::statsJson() const
{
    std::lock_guard<std::mutex> lock(impl_->mutex);
    const auto& c = impl_->counters;
    std::vector<int64_t> samples;
    for (size_t i = 0; i < std::min<uint64_t>(c.sampleCount, kSampleCapacity); ++i) samples.push_back(c.samples[i]);
    std::sort(samples.begin(), samples.end());
    const int64_t median = samples.empty() ? 0 : samples[samples.size() / 2];
    const int64_t p95 = samples.empty() ? 0 : samples[(samples.size() * 95 + 99) / 100 - 1];
    std::ostringstream out;
    out << "{\"schemaVersion\":1,\"probe\":\"rife_live_source_queue\",\"state\":" << Quote(impl_->state)
        << ",\"error\":" << (impl_->error.empty() ? "null" : Quote(impl_->error))
        << ",\"sdkCompiled\":" << (MUGUET_HAS_RIFE ? "true" : "false")
        << ",\"modelDirectory\":" << Quote(impl_->modelDirectory) << ",\"buildRevision\":" << Quote(impl_->revision)
        << ",\"deviceName\":" << Quote(impl_->deviceName)
        << ",\"sourceFps\":" << impl_->sourceFps << ",\"targetFps\":" << impl_->targetFps
        << ",\"width\":" << impl_->width << ",\"height\":" << impl_->height
        << ",\"format\":\"tightly_packed_RGB8\",\"source\":\"steady_clock_live_textured_motion\","
        "\"precomputedLoop\":false,\"timestep\":0.5,\"initialBufferDelayNs\":" << kBufferDelay
        << ",\"startCallNs\":" << impl_->startCallNs << ",\"modelReadyNs\":" << impl_->readyNs
        << ",\"modelLoadNs\":" << impl_->modelLoadNs << ",\"sourceEpochNs\":" << impl_->epochNs
        << ",\"firstDisplayDeadlineNs\":" << (impl_->epochNs ? impl_->Deadline(0) : 0)
        << ",\"poolCapacityFrames\":" << kPoolCount << ",\"queueCapacityFrames\":" << kQueueCapacity
        << ",\"historyCapacityFrames\":" << kHistoryCapacity
        << ",\"poolRGBBytesMaximum\":" << kPoolCount * (impl_->bytes + 2 * kGuard)
        << ",\"queuedFrames\":" << impl_->queue.size() << ",\"historyFrames\":" << impl_->history.size()
        << ",\"presentSuccessObserved\":false,\"stopWaitsForUncancellableDriverCall\":true";
#define FG_STREAM_COUNTER(name) out << ",\"" #name "\":" << c.name;
    FG_STREAM_COUNTER(sourceGenerated) FG_STREAM_COUNTER(sourceTicksSkipped) FG_STREAM_COUNTER(sourcePoolDrops)
    FG_STREAM_COUNTER(historyEvictions) FG_STREAM_COUNTER(queueSourceInserted) FG_STREAM_COUNTER(queueMidInserted)
    FG_STREAM_COUNTER(overflowSourceDrops) FG_STREAM_COUNTER(overflowMidDrops)
    FG_STREAM_COUNTER(obsoleteSourceDrops) FG_STREAM_COUNTER(obsoleteMidDrops)
    FG_STREAM_COUNTER(lateSourceDrops) FG_STREAM_COUNTER(lateMidDrops) FG_STREAM_COUNTER(inferencePoolDrops)
    FG_STREAM_COUNTER(pairsSkippedForFreshness) FG_STREAM_COUNTER(nonconsecutivePairs)
    FG_STREAM_COUNTER(inferenceCalls) FG_STREAM_COUNTER(inferenceCompleted) FG_STREAM_COUNTER(inferenceErrors)
    FG_STREAM_COUNTER(guardErrors) FG_STREAM_COUNTER(selectedSource) FG_STREAM_COUNTER(selectedMid)
    FG_STREAM_COUNTER(selectedRepeat) FG_STREAM_COUNTER(underruns) FG_STREAM_COUNTER(emptyUnderruns)
    FG_STREAM_COUNTER(firstFallbackRepeats) FG_STREAM_COUNTER(sameOrOlderSlotRequests)
    FG_STREAM_COUNTER(displaySlotsSkipped) FG_STREAM_COUNTER(poolExhaustions) FG_STREAM_COUNTER(poolHighWater)
    FG_STREAM_COUNTER(queueHighWater) FG_STREAM_COUNTER(historyHighWater)
    FG_STREAM_COUNTER(stopDiscardedQueue) FG_STREAM_COUNTER(stopDiscardedHistory)
    FG_STREAM_COUNTER(stopDiscardedInFlightSource) FG_STREAM_COUNTER(stopDiscardedInFlightMid)
    FG_STREAM_COUNTER(sourceFirstProducedNs) FG_STREAM_COUNTER(sourceLastProducedNs) FG_STREAM_COUNTER(maxSourceLatenessNs)
    FG_STREAM_COUNTER(firstSelectionNs) FG_STREAM_COUNTER(lastSelectionNs)
    FG_STREAM_COUNTER(inferenceTotalNs) FG_STREAM_COUNTER(inferenceMaxNs)
#undef FG_STREAM_COUNTER
    out << ",\"inferenceTimingSampleCount\":" << samples.size()
        << ",\"inferenceTimingSampleScope\":\"last64_including_first_live_inference\","
        "\"inferenceMedianNs\":" << median << ",\"inferenceP95Ns\":" << p95
        << ",\"repeatCounterMeaning\":\"all_repeat_outputs_including_firstFallbackRepeats; subtract those for previously_selected_content_repeats\","
        "\"historyEvictionsAreReferenceRetirement\":true}";
    return out.str();
}
} // namespace muguet::fg_probe
