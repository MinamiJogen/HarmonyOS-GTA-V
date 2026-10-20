#include "fg_model_alias_scope.h"

#include <atomic>
#include <cerrno>
#include <cstdlib>
#include <fcntl.h>
#include <sstream>
#include <sys/stat.h>
#include <unistd.h>
#include <utility>

namespace muguet::fg_probe {
namespace {
constexpr char kRequest[] = "fg-model-alias-request.txt";
constexpr char kShortName[] = "FrameGen_RPG_W432";
constexpr char kTarget1[] = "/sys_prod/etc/graphics_game/ai_model/aifi1.om";
constexpr char kTarget2[] = "/sys_prod/etc/graphics_game/ai_model/aifi2.om";
struct Fd {
    int value;
    explicit Fd(int fd = -1) : value(fd) {}
    ~Fd() { if (value >= 0) ::close(value); }
    Fd(const Fd&) = delete;
    Fd& operator=(const Fd&) = delete;
};
// Ownership may be restored by a different thread from preparation. A mutex
// retained across the FG trial would require same-thread unlocking.
std::atomic<bool>& CwdClaim() { static std::atomic<bool> claimed{false}; return claimed; }
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
bool SameFile(const struct stat& a, const struct stat& b)
{
    return a.st_dev == b.st_dev && a.st_ino == b.st_ino;
}
} // namespace

struct ModelAliasScope::Impl {
    std::string filesDir, target, aliasDirectory;
    int originalCwdFd = -1, restoreError = 0;
    bool attempted = false, requested = false, consumed = false;
    bool directoryCreated = false, symlinkCreated = false, active = false, ownsClaim = false;
    bool canonicalShortNameVerified = false;
    explicit Impl(std::string path) : filesDir(std::move(path)) {}
    ~Impl() noexcept
    {
        Restore();
        if (originalCwdFd >= 0) ::close(originalCwdFd);
    }
    int Restore() noexcept
    {
        const int incomingErrno = errno;
        if (active) {
            int code;
            do { code = ::fchdir(originalCwdFd); } while (code < 0 && errno == EINTR);
            restoreError = code < 0 ? errno : 0;
            if (code < 0) { errno = incomingErrno; return restoreError; }
            active = false;
        }
        if (originalCwdFd >= 0) { ::close(originalCwdFd); originalCwdFd = -1; }
        if (ownsClaim) { CwdClaim().store(false); ownsClaim = false; }
        errno = incomingErrno;
        return restoreError;
    }
    std::string Result(const char* status, const char* stage, int error) const
    {
        char cwd[4096];
        errno = 0;
        const bool haveCwd = ::getcwd(cwd, sizeof(cwd)) != nullptr;
        const int cwdError = haveCwd ? 0 : errno;
        std::ostringstream out;
        out << "{\"schemaVersion\":1,\"probe\":\"fg_model_alias_scope\","
            "\"scope\":\"isolated diagnostic process filesystem path adaptation only\","
            "\"modelBytesCopied\":false,\"systemFilesModified\":false,"
            "\"privateDescriptorModified\":false,\"compatibilityEstablished\":false,"
            "\"cwdProcessGlobal\":true,\"restorationMechanism\":\"fchdir(original_directory_fd)\","
            "\"status\":" << Quote(status) << ",\"stage\":" << Quote(stage)
            << ",\"errno\":" << error << ",\"filesDir\":" << Quote(filesDir)
            << ",\"requestPresent\":" << (requested ? "true" : "false")
            << ",\"requestConsumed\":" << (consumed ? "true" : "false")
            << ",\"target\":" << (target.empty() ? "null" : Quote(target))
            << ",\"aliasDirectory\":" << (aliasDirectory.empty() ? "null" : Quote(aliasDirectory))
            << ",\"shortName\":" << Quote(kShortName)
            << ",\"canonicalShortNameTarget\":" << (canonicalShortNameVerified ? Quote(target) : "null")
            << ",\"directoryCreated\":" << (directoryCreated ? "true" : "false")
            << ",\"symlinkCreated\":" << (symlinkCreated ? "true" : "false")
            << ",\"createdAliasFilesRetained\":" << (directoryCreated || symlinkCreated ? "true" : "false")
            << ",\"cwdAdaptationActive\":" << (active ? "true" : "false")
            << ",\"actualCwd\":" << (haveCwd ? Quote(cwd) : "null")
            << ",\"getcwdErrno\":" << cwdError << ",\"lastRestoreErrno\":" << restoreError << '}';
        return out.str();
    }
    std::string Fail(const char* stage, int error)
    {
        Restore();
        return Result("failed", stage, error);
    }
    std::string Prepare()
    {
        if (attempted) return Result(active ? "already_active" : "already_attempted", "one_shot", 0);
        attempted = true;
        if (filesDir.empty() || filesDir.front() != '/' || filesDir.find('\0') != std::string::npos)
            return Result("failed", "invalid_files_dir", EINVAL);
        Fd files(::open(filesDir.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW));
        if (files.value < 0) return Result("failed", "open_files_dir", errno);
        struct stat filesStat{};
        if (::fstat(files.value, &filesStat) != 0) return Result("failed", "stat_files_dir", errno);
        if (!S_ISDIR(filesStat.st_mode) || filesStat.st_uid != ::geteuid())
            return Result("failed", "files_dir_not_owned_directory", EACCES);
        Fd marker(::openat(files.value, kRequest, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK));
        if (marker.value < 0) {
            const int error = errno;
            return Result(error == ENOENT ? "not_requested" : "failed", "open_request", error == ENOENT ? 0 : error);
        }
        requested = true;
        struct stat markerStat{};
        if (::fstat(marker.value, &markerStat) != 0) return Result("failed", "stat_request", errno);
        if (!S_ISREG(markerStat.st_mode) || markerStat.st_uid != ::geteuid())
            return Result("failed", "request_not_owned_regular_file", EACCES);
        // One successful read, plus one byte to detect a request over 16 bytes.
        char contents[17];
        ssize_t count;
        do { count = ::read(marker.value, contents, sizeof(contents)); } while (count < 0 && errno == EINTR);
        const int readError = count < 0 ? errno : 0;
        struct stat currentMarker{};
        if (::fstatat(files.value, kRequest, &currentMarker, AT_SYMLINK_NOFOLLOW) != 0)
            return Result("failed", "request_changed_before_consume", errno);
        if (!S_ISREG(currentMarker.st_mode) || !SameFile(markerStat, currentMarker))
            return Result("failed", "request_replaced_before_consume", EAGAIN);
        if (::unlinkat(files.value, kRequest, 0) != 0) return Result("failed", "consume_request", errno);
        consumed = true;
        if (count < 0) return Result("failed", "read_request", readError);
        if (count > 16 || markerStat.st_size > 16 || currentMarker.st_size > 16)
            return Result("failed", "request_too_long", EFBIG);
        if (count != markerStat.st_size || count != currentMarker.st_size)
            return Result("failed", "request_changed_or_short_read", EAGAIN);
        std::string choice(contents, static_cast<size_t>(count));
        if (!choice.empty() && choice.back() == '\n') choice.pop_back();
        if (choice != "aifi1" && choice != "aifi2") return Result("failed", "invalid_request_content", EINVAL);
        target = choice == "aifi1" ? kTarget1 : kTarget2;
        aliasDirectory = filesDir + "/fg-model-alias-" + choice;
        bool expected = false;
        if (!CwdClaim().compare_exchange_strong(expected, true))
            return Result("failed", "another_alias_scope_active", EBUSY);
        ownsClaim = true;
        Fd model(::open(target.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK));
        if (model.value < 0) return Fail("open_readonly_target", errno);
        struct stat modelStat{};
        if (::fstat(model.value, &modelStat) != 0) return Fail("stat_target", errno);
        if (!S_ISREG(modelStat.st_mode)) return Fail("target_not_regular_file", EINVAL);
        errno = 0;
        char* canonical = ::realpath(target.c_str(), nullptr);
        const int canonicalError = canonical ? 0 : errno;
        const bool targetExact = canonical && target == canonical;
        std::free(canonical);
        if (!targetExact) return Fail("target_canonical_path_mismatch", canonicalError ? canonicalError : EINVAL);
        const std::string leaf = "fg-model-alias-" + choice;
        if (::mkdirat(files.value, leaf.c_str(), 0700) == 0) directoryCreated = true;
        else if (errno != EEXIST) return Fail("create_alias_directory", errno);
        Fd directory(::openat(files.value, leaf.c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
        if (directory.value < 0) return Fail("open_alias_directory", errno);
        struct stat directoryStat{};
        if (::fstat(directory.value, &directoryStat) != 0) return Fail("stat_alias_directory", errno);
        if (!S_ISDIR(directoryStat.st_mode) || directoryStat.st_uid != ::geteuid() ||
            (directoryStat.st_mode & 07777) != 0700)
            return Fail("alias_directory_owner_or_mode", EACCES);
        struct stat aliasStat{};
        if (::fstatat(directory.value, kShortName, &aliasStat, AT_SYMLINK_NOFOLLOW) != 0) {
            if (errno != ENOENT) return Fail("stat_alias", errno);
            if (::symlinkat(target.c_str(), directory.value, kShortName) != 0) return Fail("create_alias", errno);
            symlinkCreated = true;
            if (::fstatat(directory.value, kShortName, &aliasStat, AT_SYMLINK_NOFOLLOW) != 0)
                return Fail("stat_created_alias", errno);
        }
        if (!S_ISLNK(aliasStat.st_mode) || aliasStat.st_uid != ::geteuid())
            return Fail("alias_is_not_owned_symlink", EEXIST);
        char link[256];
        const ssize_t linkSize = ::readlinkat(directory.value, kShortName, link, sizeof(link));
        if (linkSize < 0) return Fail("read_existing_alias", errno);
        if (static_cast<size_t>(linkSize) != target.size() ||
            std::string(link, static_cast<size_t>(linkSize)) != target)
            return Fail("alias_target_conflict", EEXIST);
        originalCwdFd = ::open(".", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
        if (originalCwdFd < 0) return Fail("save_original_cwd_fd", errno);
        if (::fchdir(directory.value) != 0) return Fail("activate_alias_cwd", errno);
        active = true;
        errno = 0;
        canonical = ::realpath(kShortName, nullptr);
        const int aliasCanonicalError = canonical ? 0 : errno;
        const bool aliasExact = canonical && target == canonical;
        std::free(canonical);
        if (!aliasExact) return Fail("short_name_canonical_target_mismatch", aliasCanonicalError ? aliasCanonicalError : EINVAL);
        canonicalShortNameVerified = true;
        Fd resolved(::open(kShortName, O_RDONLY | O_CLOEXEC | O_NONBLOCK));
        if (resolved.value < 0) return Fail("open_alias_readonly", errno);
        struct stat resolvedStat{};
        if (::fstat(resolved.value, &resolvedStat) != 0) return Fail("stat_resolved_alias", errno);
        if (!S_ISREG(resolvedStat.st_mode) || !SameFile(modelStat, resolvedStat))
            return Fail("resolved_alias_identity_changed", EAGAIN);
        return Result("active", "path_adapted_only", 0);
    }
};
ModelAliasScope::ModelAliasScope(std::string filesDir) : impl_(std::make_unique<Impl>(std::move(filesDir))) {}
ModelAliasScope::~ModelAliasScope() noexcept = default;
std::string ModelAliasScope::prepareIfRequested()
{
    try { return impl_->Prepare(); }
    catch (...) { impl_->Restore(); throw; }
}
int ModelAliasScope::restore() noexcept { return impl_->Restore(); }
} // namespace muguet::fg_probe
