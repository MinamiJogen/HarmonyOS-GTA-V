#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace game_saves {

struct Limits {
    uint64_t maxArchiveBytes = 64ULL << 20;
    uint64_t maxTotalBytes = 64ULL << 20;
    uint64_t maxFileBytes = 16ULL << 20;
    uint64_t maxEntries = 512;
    uint64_t maxMetadataBytes = 1ULL << 20;
    uint64_t maxScanEntries = 4096;
};

struct Save {
    // Both id and file are canonical paths relative to filesDir/saves.
    std::string id, title, file;
    uint64_t size = 0;
    int64_t mtime = 0; // Unix seconds, not milliseconds.
    int mode = 1; // userdata=1, sandbox=2; never merge these namespaces.
    bool backup = false;
};

struct Result {
    bool ok = false;
    uint64_t imported = 0, bytes = 0, conflicts = 0, ignored = 0;
    std::string error, warning, backupDirectory;
    std::vector<Save> saves;
};

struct Destination {
    std::string relative; // relative to filesDir/saves
    std::string profile;  // input container identity, used to reject mixed accounts
    int mode = 1;
    bool ignored = false;
};

// ZIP container recognition is deliberately separate from extraction/commit.
// Throws on unsupported/ambiguous files. Only normal SGTA5 slots 0..15 and .bak
// are accepted; this does not certify the guest save payload's compatibility.
Destination CanonicalSavePath(const std::string& archivePath);
Result List(const std::string& filesDir, const Limits& limits = {});
// Caller retains fd. Requires startup/import exclusion supplied by the NAPI gate.
// All new payloads pass ZIP/length/CRC checks before an existing slot is changed.
// Originals remain in save-backups/<transaction>; exceptions roll back commits.
Result ImportZip(int fd, const std::string& filesDir, const Limits& limits = {});
std::string ToJson(const Result& result);

#ifdef GAME_SAVE_IMPORT_TESTING
void FailAfterCommitsForTesting(int count);
#endif
} // namespace game_saves
