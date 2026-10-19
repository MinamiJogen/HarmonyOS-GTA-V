#pragma once

#include <atomic>
#include <cstdint>
#include <functional>
#include <string>

namespace game_resources {

inline constexpr const char* SUPPORTED_BUILD = "8b0b5899ed";

struct Progress {
    std::string phase = "idle";
    uint64_t completedBytes = 0;
    uint64_t totalBytes = 0;
    uint64_t files = 0;
    std::string error;
    bool active = false;
};

struct Result {
    bool ready = false;
    bool cancelled = false;
    uint64_t files = 0;
    uint64_t bytes = 0;
    std::string error;
    std::string warning;
};

struct Limits {
    uint64_t maxArchiveBytes = 48ULL << 30;
    uint64_t maxTotalBytes = 32ULL << 30;
    uint64_t maxFileBytes = 8ULL << 30;
    uint64_t maxEntries = 100000;
    uint64_t maxMetadataBytes = 32ULL << 20;
};

using ProgressCallback = std::function<void(const Progress&)>;

// fd remains owned by the caller. It must be a seekable, completed local ZIP.
// Only filesDir/game is replaced; saves/config and the source ZIP are untouched.
Result ImportZip(int fd, const std::string& filesDir, std::atomic<bool>& cancel,
                 const ProgressCallback& callback = {}, const Limits& limits = {});
Result Check(const std::string& filesDir, const Limits& limits = {});
std::string ToJson(const Result& result);
std::string ToJson(const Progress& progress);

} // namespace game_resources
