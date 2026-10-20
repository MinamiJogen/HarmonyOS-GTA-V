#include "fg_rife_benchmark.h"

#ifndef MUGUET_HAS_RIFE
#define MUGUET_HAS_RIFE 0
#endif

#if MUGUET_HAS_RIFE
#include <harmony_rife_c.h>

#include <algorithm>
#include <array>
#include <cerrno>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <exception>
#include <fcntl.h>
#include <iomanip>
#include <sstream>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>
#endif

namespace muguet::fg_probe {
#if MUGUET_HAS_RIFE
namespace {
constexpr int kWidth = 320, kHeight = 192;
constexpr int kObjectWidth = 96, kObjectHeight = 80, kObjectY = 64;
constexpr int kFrame0X = 64, kFrame1X = 80, kMidpointX = 72;
constexpr size_t kBytes = size_t(kWidth) * kHeight * 3;
constexpr size_t kGuardBytes = 64;
constexpr int kWarmups = 2, kTimedRuns = 5;
using Clock = std::chrono::steady_clock;

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
const char* Boolean(bool value) { return value ? "true" : "false"; }
uint64_t Nanoseconds(Clock::time_point start)
{
    return uint64_t(std::chrono::duration_cast<std::chrono::nanoseconds>(Clock::now() - start).count());
}

struct SavedFile {
    std::string path;
    size_t written = 0;
    int error = 0;
    std::string stage;
};
SavedFile WriteFile(const std::string& path, const std::string& bytes)
{
    SavedFile result{path, 0, 0, {}};
    const int fd = open(path.c_str(), O_CREAT | O_TRUNC | O_WRONLY | O_CLOEXEC, 0600);
    if (fd < 0) { result.error = errno; result.stage = "open"; return result; }
    while (result.written < bytes.size()) {
        const ssize_t n = write(fd, bytes.data() + result.written, bytes.size() - result.written);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) { result.error = n < 0 ? errno : EIO; result.stage = "write"; break; }
        result.written += size_t(n);
    }
    if (!result.error && fsync(fd) != 0) { result.error = errno; result.stage = "fsync"; }
    if (close(fd) != 0 && !result.error) { result.error = errno; result.stage = "close"; }
    return result;
}
SavedFile WritePpm(const std::string& path, const std::vector<uint8_t>& pixels)
{
    std::string bytes = "P6\n320 192\n255\n";
    bytes.append(reinterpret_cast<const char*>(pixels.data()), pixels.size());
    return WriteFile(path, bytes);
}

std::vector<uint8_t> Fixture(int objectX)
{
    std::vector<uint8_t> image(kBytes);
    for (int y = 0; y < kHeight; ++y) {
        for (int x = 0; x < kWidth; ++x) {
            std::array<uint8_t, 3> color;
            const int checker = ((x / 12) + (y / 12)) & 1;
            color = {uint8_t(24 + (x * 3 + y * 2) % 48 + checker * 16),
                     uint8_t(40 + (y * 5) % 56), uint8_t(70 + (x + y) % 72)};
            if (x >= objectX && x < objectX + kObjectWidth && y >= kObjectY && y < kObjectY + kObjectHeight) {
                const int u = x - objectX, v = y - kObjectY;
                const int foregroundChecker = ((u / 6) ^ (v / 6)) & 1;
                color = {uint8_t(160 + (u * 7 + v * 3) % 80),
                         uint8_t(35 + foregroundChecker * 150), uint8_t(25 + (u * 5 + v * 9) % 100)};
            }
            const size_t offset = (size_t(y) * kWidth + x) * 3;
            std::copy(color.begin(), color.end(), image.begin() + offset);
        }
    }
    return image;
}
double Mae(const uint8_t* a, const uint8_t* b)
{
    uint64_t error = 0;
    for (size_t i = 0; i < kBytes; ++i) error += unsigned(std::abs(int(a[i]) - int(b[i])));
    return double(error) / double(kBytes);
}
size_t NonidenticalPixels(const uint8_t* a, const uint8_t* b)
{
    size_t count = 0;
    for (size_t i = 0; i < kBytes; i += 3)
        if (a[i] != b[i] || a[i + 1] != b[i + 1] || a[i + 2] != b[i + 2]) ++count;
    return count;
}
bool GuardIntact(const std::vector<uint8_t>& output)
{
    return std::all_of(output.begin(), output.begin() + kGuardBytes, [](uint8_t c) { return c == 0xD3; }) &&
        std::all_of(output.end() - kGuardBytes, output.end(), [](uint8_t c) { return c == 0x7B; });
}
struct Run {
    int index, code;
    bool warmup, guard;
    uint64_t wallNs;
};
struct Context {
    HarmonyRifeContext* value = nullptr;
    bool destroyed = false;
    void Destroy() { if (value) { harmony_rife_destroy(value); value = nullptr; destroyed = true; } }
    ~Context() { Destroy(); }
};
struct Report {
    std::string directory, modelDirectory, phase = "prepare", error, buildRevision;
    int mkdirReturn = -1, mkdirError = 0, directoryStatReturn = -1, directoryStatError = 0, infoCode = 0;
    bool directoryReady = false, createCalled = false, createSucceeded = false;
    bool infoCalled = false, metricsReady = false, destroyed = false;
    uint64_t createWallNs = 0, medianNs = 0, p95Ns = 0;
    HarmonyRifeDeviceInfo info{};
    std::vector<Run> runs;
    std::vector<SavedFile> files;
    double aiMae = 0, repeatMae = 0, blendMae = 0, aiVsFrame0Mae = 0, aiVsFrame1Mae = 0, aiVsBlendMae = 0;
    size_t differentFromFrame0 = 0, differentFromFrame1 = 0, differentFromBlend = 0;
    bool Fail(const std::string& reason) { error = reason; return false; }
    bool Save(const char* name, const std::vector<uint8_t>& pixels)
    {
        files.push_back(WritePpm(directory + '/' + name, pixels));
        const auto& file = files.back();
        return !file.error || Fail("PPM " + file.stage + " failed errno=" + std::to_string(file.error));
    }
    std::string Json() const
    {
        std::ostringstream out;
        out << std::setprecision(12);
        out << "{\"status\":" << Quote(error.empty() ? "completed_offscreen_unverified_display" : "error")
            << ",\"probe\":\"rife_rgb8_offscreen_pair\",\"phase\":" << Quote(phase) << ",\"error\":" << Quote(error)
            << ",\"sdkCompiled\":true,\"buildRevision\":" << Quote(buildRevision)
            << ",\"directory\":" << Quote(directory) << ",\"modelDirectory\":" << Quote(modelDirectory)
            << ",\"model\":\"upstream_rife-v4.6\",\"backend\":\"ncnn_Vulkan\",\"silentCpuFallbackAllowed\":false"
            << ",\"width\":" << kWidth << ",\"height\":" << kHeight << ",\"format\":\"tightly_packed_RGB8\",\"bufferBytes\":" << kBytes
            << ",\"fixtureSourceHz\":15,\"requestedFutureDisplayHz\":30,\"timestep\":0.5"
            << ",\"fixture\":{\"version\":1,\"description\":\"integer-translated textured rectangle over fixed textured background\",\"objectFrame0X\":" << kFrame0X
            << ",\"objectFrame1X\":" << kFrame1X << ",\"objectGroundTruthMidX\":" << kMidpointX
            << ",\"objectY\":" << kObjectY << ",\"objectWidth\":" << kObjectWidth << ",\"objectHeight\":" << kObjectHeight
            << ",\"blendBaseline\":\"encoded_RGB8_byte_mean_round_half_up\",\"framesActuallySubmittedAt15Hz\":false}"
            << ",\"mkdirReturn\":" << mkdirReturn << ",\"mkdirErrno\":" << mkdirError
            << ",\"directoryStatReturn\":" << directoryStatReturn << ",\"directoryStatErrno\":" << directoryStatError
            << ",\"outputGuardBytesEachSide\":" << kGuardBytes
            << ",\"createCalled\":" << Boolean(createCalled) << ",\"createSucceeded\":" << Boolean(createSucceeded)
            << ",\"createWallNs\":" << (createCalled ? std::to_string(createWallNs) : "null")
            << ",\"getDeviceInfoCalled\":" << Boolean(infoCalled) << ",\"getDeviceInfoReturn\":" << (infoCalled ? std::to_string(infoCode) : "null")
            << ",\"device\":";
        if (infoCalled && infoCode == 0) {
            out << "{\"index\":" << info.gpu_index << ",\"name\":" << Quote(std::string(info.device_name, strnlen(info.device_name, sizeof(info.device_name))))
                << ",\"apiVersion\":" << info.api_version << ",\"driverVersion\":" << info.driver_version
                << ",\"vendorId\":" << info.vendor_id << ",\"deviceId\":" << info.device_id
                << ",\"supportsFp16Packed\":" << Boolean(info.supports_fp16_packed != 0)
                << ",\"supportsFp16Storage\":" << Boolean(info.supports_fp16_storage != 0)
                << ",\"supportsFp16Arithmetic\":" << Boolean(info.supports_fp16_arithmetic != 0)
                << ",\"supportsInt8Storage\":" << Boolean(info.supports_int8_storage != 0) << '}';
        } else out << "null";
        out << ",\"warmupRunsRequested\":" << kWarmups << ",\"timedRunsRequested\":" << kTimedRuns << ",\"runs\":[";
        for (size_t i = 0; i < runs.size(); ++i) {
            if (i) out << ',';
            const auto& run = runs[i];
            out << "{\"index\":" << run.index << ",\"warmup\":" << Boolean(run.warmup) << ",\"returnCode\":" << run.code
                << ",\"wallNs\":" << run.wallNs << ",\"outputGuardIntact\":" << Boolean(run.guard) << '}';
        }
        out << "],\"timing\":";
        if (metricsReady) out << "{\"samples\":" << kTimedRuns << ",\"medianNs\":" << medianNs << ",\"p95Ns\":" << p95Ns
            << ",\"medianMs\":" << double(medianNs) / 1e6 << ",\"p95Ms\":" << double(p95Ns) / 1e6
            << ",\"percentileMethod\":\"nearest_rank_p95_with_5_samples_is_maximum\",\"scope\":\"synchronous_C_ABI_wall_time_including_upload_inference_download_excluding_fixture_and_file_IO\"}";
        else out << "null";
        out << ",\"metrics\":";
        if (metricsReady) {
            out << "{\"maeUnits\":\"absolute_RGB8_channel_value_0_to_255\",\"maeToGroundTruth\":{\"ai\":" << aiMae
                << ",\"repeatFrame0\":" << repeatMae << ",\"byteMeanBlend\":" << blendMae
                << "},\"maeAiVs\":{\"frame0\":" << aiVsFrame0Mae << ",\"frame1\":" << aiVsFrame1Mae << ",\"byteMeanBlend\":" << aiVsBlendMae
                << "},\"pixelCount\":" << size_t(kWidth) * kHeight << ",\"nonidenticalAiPixelsVsFrame0\":" << differentFromFrame0
                << ",\"nonidenticalAiPixelsVsFrame1\":" << differentFromFrame1 << ",\"nonidenticalAiPixelsVsBlend\":" << differentFromBlend << '}';
        } else out << "null";
        out << ",\"files\":[";
        for (size_t i = 0; i < files.size(); ++i) {
            if (i) out << ',';
            const auto& file = files[i];
            out << "{\"path\":" << Quote(file.path) << ",\"bytesWritten\":" << file.written
                << ",\"errno\":" << file.error << ",\"failureStage\":" << Quote(file.stage) << '}';
        }
        out << "],\"destroyCalled\":" << Boolean(destroyed)
            << ",\"qualityAcceptance\":null,\"presentCalls\":0,\"generatedDisplayFramesVerified\":null,\"displayFpsVerified\":null"
            << ",\"observationScope\":\"Fixed synthetic offline interpolation only. Source cadence is fixture metadata. MAE and nonidentity are measurements, not game quality acceptance. No presentation or 30fps claim.\"}";
        return out.str();
    }
};
} // namespace
#endif

std::string inspectRifeInterpolation(const std::string& filesDir)
{
#if !MUGUET_HAS_RIFE
    (void)filesDir;
    return "{\"status\":\"unsupported\",\"probe\":\"rife_rgb8_offscreen_pair\",\"sdkCompiled\":false,\"reason\":\"MUGUET_HAS_RIFE=0; optional RIFE SDK is not linked\",\"presentCalls\":0,\"displayFpsVerified\":null}";
#else
    Report report;
    report.directory = filesDir + "/rife-benchmark";
    report.modelDirectory = filesDir + "/rife-v4.6";
    Context context;
    try {
        const auto run = [&]() {
            if (filesDir.empty()) { report.Fail("filesDir is empty"); return; }
            report.mkdirReturn = mkdir(report.directory.c_str(), 0700);
            report.mkdirError = report.mkdirReturn < 0 ? errno : 0;
            struct stat directory{};
            report.directoryStatReturn = stat(report.directory.c_str(), &directory);
            report.directoryStatError = report.directoryStatReturn < 0 ? errno : 0;
            if ((report.mkdirReturn < 0 && report.mkdirError != EEXIST) ||
                report.directoryStatReturn != 0 || !S_ISDIR(directory.st_mode)) {
                report.Fail("Cannot prepare app-owned benchmark directory: mkdir errno=" + std::to_string(report.mkdirError) +
                    "; stat errno=" + std::to_string(report.directoryStatError) + "; isDirectory=" + Boolean(S_ISDIR(directory.st_mode))); return;
            }
            report.directoryReady = true;
            report.phase = "save_fixed_fixtures";
            const auto frame0 = Fixture(kFrame0X), frame1 = Fixture(kFrame1X), truth = Fixture(kMidpointX);
            std::vector<uint8_t> blend(kBytes);
            for (size_t i = 0; i < kBytes; ++i) blend[i] = uint8_t((unsigned(frame0[i]) + frame1[i] + 1) / 2);
            if (!report.Save("frame0.ppm", frame0) || !report.Save("frame1.ppm", frame1) ||
                !report.Save("ground-truth-mid.ppm", truth) || !report.Save("byte-mean-blend.ppm", blend)) return;
            report.phase = "create_rife_vulkan";
            const char* revision = harmony_rife_build_revision();
            report.buildRevision = revision ? revision : "unavailable";
            char error[512]{};
            report.createCalled = true;
            auto start = Clock::now();
            context.value = harmony_rife_create(report.modelDirectory.c_str(), 0, error, sizeof(error));
            report.createWallNs = Nanoseconds(start);
            report.createSucceeded = context.value != nullptr;
            if (!context.value) { report.Fail("harmony_rife_create returned null: " + std::string(error, strnlen(error, sizeof(error)))); return; }
            report.infoCalled = true;
            report.infoCode = harmony_rife_get_device_info(context.value, &report.info);
            if (report.infoCode != 0) { report.Fail("harmony_rife_get_device_info return=" + std::to_string(report.infoCode)); return; }
            std::vector<uint8_t> guarded(kBytes + 2 * kGuardBytes);
            std::vector<uint64_t> times;
            for (int i = 0; i < kWarmups + kTimedRuns; ++i) {
                std::fill(guarded.begin(), guarded.begin() + kGuardBytes, 0xD3);
                std::fill(guarded.begin() + kGuardBytes, guarded.end() - kGuardBytes, 0xE1);
                std::fill(guarded.end() - kGuardBytes, guarded.end(), 0x7B);
                report.phase = i < kWarmups ? "warmup" : "timed_inference";
                start = Clock::now();
                const int code = harmony_rife_process_rgb8(context.value, frame0.data(), frame1.data(), kWidth, kHeight,
                    guarded.data() + kGuardBytes, kBytes, 0.5f);
                const uint64_t wallNs = Nanoseconds(start);
                const bool intact = GuardIntact(guarded);
                report.runs.push_back({i, code, i < kWarmups, intact, wallNs});
                if (code != 0) { report.Fail("harmony_rife_process_rgb8 return=" + std::to_string(code) + " at run " + std::to_string(i)); return; }
                if (!intact) { report.Fail("AI output prefix/suffix guard corrupted at run " + std::to_string(i)); return; }
                if (i >= kWarmups) times.push_back(wallNs);
            }
            std::sort(times.begin(), times.end());
            report.medianNs = times[times.size() / 2];
            report.p95Ns = times[size_t(std::ceil(0.95 * double(times.size()))) - 1];
            const uint8_t* output = guarded.data() + kGuardBytes;
            report.aiMae = Mae(output, truth.data()); report.repeatMae = Mae(frame0.data(), truth.data()); report.blendMae = Mae(blend.data(), truth.data());
            report.aiVsFrame0Mae = Mae(output, frame0.data()); report.aiVsFrame1Mae = Mae(output, frame1.data()); report.aiVsBlendMae = Mae(output, blend.data());
            report.differentFromFrame0 = NonidenticalPixels(output, frame0.data()); report.differentFromFrame1 = NonidenticalPixels(output, frame1.data());
            report.differentFromBlend = NonidenticalPixels(output, blend.data()); report.metricsReady = true;
            report.phase = "save_ai_output";
            const std::vector<uint8_t> ai(output, output + kBytes);
            if (!report.Save("ai-midpoint.ppm", ai)) return;
            report.phase = "completed";
        };
        run();
    } catch (const std::exception& e) { report.Fail(std::string("C++ exception: ") + e.what()); }
      catch (...) { report.Fail("Unexpected C++ exception"); }
    context.Destroy();
    report.destroyed = context.destroyed;
    const std::string result = report.Json();
    if (report.directoryReady) {
        const auto saved = WriteFile(report.directory + "/result.json", result + '\n');
        if (saved.error) {
            report.phase = "save_result";
            report.Fail("Result JSON " + saved.stage + " failed errno=" + std::to_string(saved.error));
            return report.Json();
        }
    }
    return result;
#endif
}
} // namespace muguet::fg_probe
