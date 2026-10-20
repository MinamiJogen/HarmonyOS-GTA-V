#include "game_save_import_core.h"

// ZIP framing, bounded raw-deflate and CRC handling follow the existing
// game_resource_import_core.cpp, narrowed to small save-only ZIP32 archives.
#include <algorithm>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dirent.h>
#include <fcntl.h>
#include <limits>
#include <mutex>
#include <set>
#include <stdexcept>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <time.h>
#include <unistd.h>
#include <utility>
#include <vector>
#include <zlib.h>

namespace game_saves {
namespace {
std::mutex operations;
struct Failure : std::runtime_error { using std::runtime_error::runtime_error; };
void Need(bool value, const std::string& error) { if (!value) throw Failure(error); }
struct Fd {
    int value = -1;
    explicit Fd(int v = -1) : value(v) {}
    ~Fd() { if (value >= 0) close(value); }
    Fd(const Fd&) = delete;
    Fd& operator=(const Fd&) = delete;
    Fd(Fd&& other) noexcept : value(std::exchange(other.value, -1)) {}
    Fd& operator=(Fd&& other) noexcept {
        if (this != &other) { if (value >= 0) close(value); value = std::exchange(other.value, -1); }
        return *this;
    }
};
uint16_t U16(const unsigned char* p) { return uint16_t(p[0]) | uint16_t(p[1]) << 8; }
uint32_t U32(const unsigned char* p) { return uint32_t(U16(p)) | uint32_t(U16(p + 2)) << 16; }
uint64_t Add(uint64_t a, uint64_t b) {
    Need(a <= std::numeric_limits<uint64_t>::max() - b, "ZIP 长度溢出。"); return a + b;
}
std::string Fold(std::string s) {
    for (char& c : s) if (c >= 'A' && c <= 'Z') c = char(c - 'A' + 'a');
    return s;
}
std::vector<std::string> Parts(const std::string& path, bool directory = false) {
    Need(!path.empty() && path.size() <= 1024 && path.front() != '/', "存档路径必须为有界相对路径。");
    std::string name = path;
    if (directory && name.back() == '/') name.pop_back();
    Need(!name.empty(), "存档路径为空。");
    std::vector<std::string> out;
    size_t at = 0;
    for (;;) {
        size_t end = name.find('/', at);
        std::string part = name.substr(at, end == std::string::npos ? end : end - at);
        Need(!part.empty() && part != "." && part != "..", "存档 ZIP 含路径穿越或空路径。");
        for (unsigned char c : part)
            Need(c >= 32 && c < 127 && c != '\\' && c != ':', "存档 ZIP 含不安全文件名。");
        out.push_back(part);
        Need(out.size() <= 12, "存档目录过深。");
        if (end == std::string::npos) break;
        at = end + 1;
    }
    return out;
}
bool SaveName(const std::string& name, std::string& canonical, bool& backup) {
    auto lower = Fold(name);
    backup = lower.size() == 13 && lower.substr(9) == ".bak";
    if (lower.size() != (backup ? 13U : 9U) || lower.compare(0, 5, "sgta5") != 0) return false;
    unsigned slot = 0;
    for (size_t i = 5; i < 9; ++i) {
        if (lower[i] < '0' || lower[i] > '9') return false;
        slot = slot * 10 + unsigned(lower[i] - '0');
    }
    if (slot > 15) return false; // 16 is mission replay, not normal story progress.
    canonical = "SGTA5" + lower.substr(5, 4) + (backup ? ".bak" : "");
    return true;
}
void ReadAt(int fd, uint64_t offset, void* buffer, size_t size) {
    Need(offset <= uint64_t(std::numeric_limits<off_t>::max()), "ZIP 偏移无效。");
    auto* p = static_cast<unsigned char*>(buffer);
    while (size) {
        ssize_t n = pread(fd, p, size, static_cast<off_t>(offset));
        if (n < 0 && errno == EINTR) continue;
        Need(n > 0, "读取存档 ZIP 失败或内容截断。");
        p += n; size -= size_t(n); offset = Add(offset, uint64_t(n));
    }
}
void WriteAll(int fd, const void* buffer, size_t size) {
    const auto* p = static_cast<const unsigned char*>(buffer);
    while (size) {
        ssize_t n = write(fd, p, size);
        if (n < 0 && errno == EINTR) continue;
        Need(n > 0, "写入存档或备份失败，可能存储空间不足。");
        p += n; size -= size_t(n);
    }
}
void Sync(int fd) { Need(fsync(fd) == 0, "同步存档数据失败。"); }
void CheckOuterHeader(int fd, uint64_t size) {
    Need(size >= 264, "存档小于原引擎 264 字节外层头，不能导入。");
    unsigned char version[4]; ReadAt(fd, 0, version, sizeof(version));
    Need(version[0] == 0 && version[1] == 0 && version[2] == 0 && version[3] == 1,
         "存档不符合当前原引擎的 version 1 外层头；ZIP 导入不会转换 PC/其他版本存档。");
}
uint32_t TitleHash(const unsigned char* data, size_t size, uint32_t h = 0) {
    // Original Wasm atDataHash sign-extends each byte, then finalizes EACH call.
    // Keep all arithmetic unsigned to obtain defined modulo-2^32 behavior.
    for (size_t i = 0; i < size; ++i) {
        const uint32_t byte = data[i] < 128 ? uint32_t(data[i]) : uint32_t(int32_t(data[i]) - 256);
        h = (h + byte) * 1025U; h ^= h >> 6;
    }
    h *= 9U; h ^= h >> 11; return h * 32769U;
}
std::string DisplayTitle(int fd, uint64_t size) {
    // Metadata is optional. Never read beyond the 256-byte title field, never
    // strlen guest bytes, and never promote a title into payload-validity proof.
    if (size < 264) return {};
    unsigned char h[264];
    try { ReadAt(fd, 0, h, sizeof(h)); } catch (const Failure&) { return {}; }
    if (h[0] || h[1] || h[2] || h[3] != 1) return {};
    const unsigned char nativeVersion[4] = {1, 0, 0, 0};
    const uint32_t expected = TitleHash(h + 4, 256, TitleHash(nativeVersion, sizeof(nativeVersion)));
    const uint32_t stored = uint32_t(h[260]) << 24 | uint32_t(h[261]) << 16 | uint32_t(h[262]) << 8 | h[263];
    if (expected != stored) return {};
    std::string title; title.reserve(384);
    auto append = [&](uint32_t c) {
        if (c <= 0x7f) title += char(c);
        else if (c <= 0x7ff) { title += char(0xc0 | (c >> 6)); title += char(0x80 | (c & 63)); }
        else if (c <= 0xffff) {
            title += char(0xe0 | (c >> 12)); title += char(0x80 | ((c >> 6) & 63)); title += char(0x80 | (c & 63));
        } else {
            title += char(0xf0 | (c >> 18)); title += char(0x80 | ((c >> 12) & 63));
            title += char(0x80 | ((c >> 6) & 63)); title += char(0x80 | (c & 63));
        }
    };
    for (size_t unit = 0; unit < 128; ++unit) {
        uint32_t c = U16(h + 4 + unit * 2);
        if (!c) {
            if (title.empty() || title.find_first_not_of(' ') == std::string::npos) return {};
            return title;
        }
        if (c >= 0xd800 && c <= 0xdbff) {
            if (++unit >= 128) return {};
            const uint32_t low = U16(h + 4 + unit * 2);
            if (low < 0xdc00 || low > 0xdfff) return {};
            c = 0x10000 + ((c - 0xd800) << 10) + low - 0xdc00;
        } else if (c >= 0xdc00 && c <= 0xdfff) return {};
        if (c < 32 || (c >= 0x7f && c <= 0x9f) || c == 0x2028 || c == 0x2029 ||
            (c >= 0x202a && c <= 0x202e) || (c >= 0x2066 && c <= 0x2069)) return {};
        append(c);
    }
    return {}; // No terminator within the field: preserve the filename label.
}
Fd OpenRoot(const std::string& path) {
    Need(!path.empty() && path.front() == '/' && path.size() <= 4096 && path.find('\0') == std::string::npos,
         "需要有效的应用 filesDir。");
    Fd root(open(path.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW));
    Need(root.value >= 0, "应用目录不存在或为符号链接。"); return root;
}
Fd OpenDirectory(int root, const std::string& relative, bool create, bool missingOk = false) {
    Fd current(fcntl(root, F_DUPFD_CLOEXEC, 0)); Need(current.value >= 0, "无法持有存档目录。");
    if (relative.empty()) return current;
    for (const auto& part : Parts(relative)) {
        if (create) {
            if (mkdirat(current.value, part.c_str(), 0700) == 0) Sync(current.value);
            else Need(errno == EEXIST, "创建存档目录失败。");
        }
        Fd next(openat(current.value, part.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW));
        if (next.value < 0 && missingOk && errno == ENOENT) return Fd();
        Need(next.value >= 0, "存档目录不安全、不可读或含符号链接。");
        current = std::move(next);
    }
    return current;
}
bool StatAt(int fd, const std::string& name, struct stat& st) {
    if (fstatat(fd, name.c_str(), &st, AT_SYMLINK_NOFOLLOW) == 0) return true;
    Need(errno == ENOENT, "无法检查现有存档。"); return false;
}
std::string Escape(const std::string& source) {
    std::string out = "\"";
    for (unsigned char c : source) {
        if (c == '"' || c == '\\') { out += '\\'; out += char(c); }
        else if (c < 32) { char text[7]; std::snprintf(text, sizeof(text), "\\u%04x", c); out += text; }
        else out += char(c);
    }
    return out + '"';
}
struct Entry {
    std::string name;
    Destination destination;
    uint64_t compressed = 0, size = 0, local = 0, data = 0, end = 0;
    uint32_t crc = 0;
    uint16_t flags = 0, method = 0;
    bool directory = false;
};
void Extras(const std::vector<unsigned char>& bytes, size_t start) {
    for (size_t p = start; p < bytes.size();) {
        Need(p + 4 <= bytes.size(), "ZIP 扩展字段截断。");
        uint16_t kind = U16(bytes.data() + p), n = U16(bytes.data() + p + 2); p += 4;
        Need(p + n <= bytes.size() && kind != 1, "存档 ZIP 不支持 ZIP64 或截断扩展字段。"); p += n;
    }
}
void LocalHeader(int fd, Entry& entry, uint64_t central) {
    Need(Add(entry.local, 30) <= central, "ZIP 本地头边界无效。");
    unsigned char h[30]; ReadAt(fd, entry.local, h, sizeof(h));
    Need(U32(h) == 0x04034b50 && U16(h + 6) == entry.flags && U16(h + 8) == entry.method,
         "ZIP 本地头与中央目录不一致。");
    uint16_t names = U16(h + 26), extras = U16(h + 28);
    Need(names == entry.name.size(), "ZIP 本地文件名长度不一致。");
    entry.data = Add(entry.local, uint64_t(30) + names + extras);
    Need(entry.data <= central && Add(entry.data, entry.compressed) <= central, "ZIP 数据超出边界。");
    std::vector<unsigned char> variable(size_t(names) + extras);
    ReadAt(fd, entry.local + 30, variable.data(), variable.size());
    Need(std::string(reinterpret_cast<char*>(variable.data()), names) == entry.name, "ZIP 本地路径不一致。");
    Extras(variable, names);
    if (!(entry.flags & 8)) {
        Need(U32(h + 14) == entry.crc && U32(h + 18) == entry.compressed && U32(h + 22) == entry.size,
             "ZIP 本地 CRC 或长度与中央目录不一致。");
    } else {
        Need((U32(h + 14) == 0 || U32(h + 14) == entry.crc) &&
             (U32(h + 18) == 0 || U32(h + 18) == entry.compressed) &&
             (U32(h + 22) == 0 || U32(h + 22) == entry.size), "ZIP 数据描述符头信息冲突。");
    }
    entry.end = Add(entry.data, entry.compressed);
    if (entry.flags & 8) {
        Need(Add(entry.end, 12) <= central, "ZIP 数据描述符截断。");
        unsigned char d[16]{}; ReadAt(fd, entry.end, d, 12);
        size_t prefix = U32(d) == 0x08074b50 ? 4 : 0;
        if (prefix) { Need(Add(entry.end, 16) <= central, "ZIP 数据描述符截断。"); ReadAt(fd, entry.end, d, 16); }
        Need(U32(d + prefix) == entry.crc && U32(d + prefix + 4) == entry.compressed && U32(d + prefix + 8) == entry.size,
             "ZIP 数据描述符校验不一致。");
        entry.end = Add(entry.end, 12 + prefix);
    }
}
std::vector<Entry> Catalogue(int fd, const Limits& limits, Result& result) {
    struct stat st{};
    Need(fstat(fd, &st) == 0 && S_ISREG(st.st_mode) && st.st_size >= 22,
         "请选择已下载完成、可随机读取的本地存档 ZIP。");
    const uint64_t length = uint64_t(st.st_size);
    Need(length <= limits.maxArchiveBytes, "存档 ZIP 超过大小限制。");
    std::vector<unsigned char> tail(size_t(std::min<uint64_t>(length, 65557)));
    ReadAt(fd, length - tail.size(), tail.data(), tail.size());
    size_t found = tail.size();
    for (size_t i = tail.size() - 22;; --i) {
        if (U32(tail.data() + i) == 0x06054b50 && i + 22 + U16(tail.data() + i + 20) == tail.size()) { found = i; break; }
        if (!i) break;
    }
    Need(found != tail.size(), "存档 ZIP 中央目录缺失或文件未下载完整。");
    const auto* end = tail.data() + found;
    uint64_t count = U16(end + 10), size = U32(end + 12), central = U32(end + 16);
    Need(!U16(end + 4) && !U16(end + 6) && U16(end + 8) == count, "不支持分卷存档 ZIP。");
    Need(count && count != 0xffff && count <= limits.maxEntries && size <= limits.maxMetadataBytes &&
         central != 0xffffffff && Add(central, size) == length - tail.size() + found,
         "存档 ZIP 目录过大、ZIP64 或边界无效。");
    uint64_t at = central, total = 0, metadata = 0;
    std::set<std::string> names, targets, profiles;
    std::vector<Entry> all, selected;
    for (uint64_t i = 0; i < count; ++i) {
        Need(Add(at, 46) <= central + size, "ZIP 中央记录截断。");
        unsigned char h[46]; ReadAt(fd, at, h, sizeof(h));
        Need(U32(h) == 0x02014b50 && !U16(h + 34), "ZIP 中央记录或分卷字段无效。");
        Entry e; e.flags = U16(h + 8); e.method = U16(h + 10); e.crc = U32(h + 16);
        e.compressed = U32(h + 20); e.size = U32(h + 24); e.local = U32(h + 42);
        const uint16_t n = U16(h + 28), extra = U16(h + 30), comment = U16(h + 32);
        Need(n && n <= 1024 && !(e.flags & ~uint16_t(0x080e)) && (e.method == 0 || e.method == 8),
             "存档 ZIP 仅支持未加密 Store/Deflate，文件名必须有界。");
        Need(e.method == 8 || !(e.flags & 6), "Store 条目的压缩标志无效。");
        Need(e.size != 0xffffffff && e.compressed != 0xffffffff && e.local != 0xffffffff,
             "存档 ZIP 不支持 ZIP64。");
        uint64_t next = Add(at, uint64_t(46) + n + extra + comment);
        Need(next <= central + size, "ZIP 文件名或扩展字段截断。");
        std::vector<unsigned char> v(size_t(n) + extra); ReadAt(fd, at + 46, v.data(), v.size());
        e.name.assign(reinterpret_cast<char*>(v.data()), n); e.directory = e.name.back() == '/';
        Parts(e.name, e.directory); Extras(v, n);
        metadata = Add(metadata, n + extra + comment); Need(metadata <= limits.maxMetadataBytes, "ZIP 元数据过大。");
        Need(names.insert(Fold(e.name)).second, "ZIP 含重复或大小写冲突路径。");
        uint32_t type = (U32(h + 38) >> 16) & S_IFMT;
        Need(type == 0 || type == S_IFREG || type == S_IFDIR, "存档 ZIP 不允许符号链接或特殊文件。");
        Need((type != S_IFDIR || e.directory) && (type != S_IFREG || !e.directory), "ZIP 类型与路径冲突。");
        Need(e.size <= limits.maxFileBytes && (e.compressed || !e.size) && e.size <= e.compressed * 128 + (1ULL << 20),
             "存档条目过大或压缩比例异常。");
        total = Add(total, e.size); Need(total <= limits.maxTotalBytes, "存档 ZIP 解压总量超过限制。");
        if (e.directory) Need(!e.size && e.compressed <= 64 && !e.crc, "ZIP 目录不能含有效载荷。");
        LocalHeader(fd, e, central); all.push_back(e);
        if (!e.directory) {
            e.destination = CanonicalSavePath(e.name);
            if (e.destination.ignored) ++result.ignored;
            else {
                Need(e.size > 0, "存档文件不能为空。");
                Need(targets.insert(Fold(e.destination.relative)).second, "不同 ZIP 文件映射到同一存档，已拒绝混合覆盖。");
                if (!e.destination.profile.empty()) profiles.insert(e.destination.profile);
                selected.push_back(std::move(e));
            }
        }
        at = next;
    }
    Need(at == central + size && !selected.empty(), "存档 ZIP 没有普通 SGTA5 存档或目录长度不一致。");
    Need(profiles.size() <= 1, "ZIP 含多个账号 profile，不能混合成当前引擎的 Default 存档。");
    Need(std::any_of(selected.begin(), selected.end(), [](const Entry& e) {
        return e.destination.relative.size() < 4 || e.destination.relative.substr(e.destination.relative.size() - 4) != ".bak";
    }), "ZIP 仅含 .bak 备份，没有主存档；请同时包含对应主 SGTA5 文件，不会自动冒充主 slot。");
    std::sort(all.begin(), all.end(), [](const Entry& a, const Entry& b) { return a.local < b.local; });
    Need(all.front().local == 0, "不支持带程序前缀的自解压存档 ZIP。");
    for (size_t i = 1; i < all.size(); ++i) Need(all[i - 1].end <= all[i].local, "ZIP 本地条目重叠。");
    return selected;
}
void Extract(int fd, const Entry& e, int directory, const std::string& name) {
    Fd out(openat(directory, name.c_str(), O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600));
    Need(out.value >= 0, "无法创建临时存档。");
    std::vector<unsigned char> input(64 << 10), decoded(64 << 10);
    uint64_t position = e.data, remaining = e.compressed, written = 0;
    uLong crc = crc32(0L, Z_NULL, 0);
    auto append = [&](const unsigned char* p, size_t n) {
        Need(Add(written, n) <= e.size, "ZIP 实际解压长度超过声明。");
        WriteAll(out.value, p, n); crc = crc32(crc, p, uInt(n)); written += n;
    };
    if (!e.method) {
        Need(e.compressed == e.size, "Store 存档长度不一致。");
        while (remaining) {
            size_t n = size_t(std::min<uint64_t>(remaining, input.size())); ReadAt(fd, position, input.data(), n);
            append(input.data(), n); remaining -= n; position += n;
        }
    } else {
        z_stream z{}; Need(inflateInit2(&z, -MAX_WBITS) == Z_OK, "无法初始化存档解压器。");
        try {
            int state = Z_OK;
            while (state != Z_STREAM_END) {
                if (!z.avail_in && remaining) {
                    size_t n = size_t(std::min<uint64_t>(remaining, input.size())); ReadAt(fd, position, input.data(), n);
                    z.next_in = input.data(); z.avail_in = uInt(n); remaining -= n; position += n;
                }
                z.next_out = decoded.data(); z.avail_out = uInt(decoded.size());
                uInt before = z.avail_in; state = inflate(&z, Z_NO_FLUSH);
                Need(state == Z_OK || state == Z_STREAM_END, "存档 Deflate 数据损坏或截断。");
                size_t n = decoded.size() - z.avail_out; if (n) append(decoded.data(), n);
                Need(state == Z_STREAM_END || n || z.avail_in != before, "存档 Deflate 未能完成。");
            }
            Need(!remaining && !z.avail_in, "存档 Deflate 尾部含多余数据。");
        } catch (...) { inflateEnd(&z); throw; }
        inflateEnd(&z);
    }
    Need(written == e.size && uint32_t(crc) == e.crc, "存档 CRC 或解压长度校验失败。");
    // Proven original guest Header::Import contract: 264-byte outer header,
    // big-endian version 1. Title hash/payload/AES/PSO compatibility is not
    // established by this minimum check; never manufacture progress metadata.
    CheckOuterHeader(out.value, written);
    Sync(out.value);
}
void Copy(int from, int to, uint64_t size) {
    std::vector<unsigned char> bytes(64 << 10);
    for (uint64_t at = 0; at < size;) {
        size_t n = size_t(std::min<uint64_t>(size - at, bytes.size())); ReadAt(from, at, bytes.data(), n);
        WriteAll(to, bytes.data(), n); at += n;
    }
    Sync(to);
}
void PreserveTimes(int fd, const struct stat& source) {
#ifdef __APPLE__
    const struct timespec times[2] = {source.st_atimespec, source.st_mtimespec};
#else
    const struct timespec times[2] = {source.st_atim, source.st_mtim};
#endif
    Need(futimens(fd, times) == 0, "保存旧存档时间信息失败。"); Sync(fd);
}
void Marker(int directory, const std::string& name, const std::string& text) {
    Fd out(openat(directory, name.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600));
    Need(out.value >= 0, "无法保存存档导入事务记录。"); WriteAll(out.value, text.data(), text.size()); Sync(out.value); Sync(directory);
}
struct Pending {
    Fd parent;
    std::string leaf, relative, incoming, original;
    bool existed = false;
    struct stat previous{};
};
bool Same(const struct stat& a, const struct stat& b) {
    return a.st_dev == b.st_dev && a.st_ino == b.st_ino && a.st_size == b.st_size && a.st_mtime == b.st_mtime;
}
void ListTree(int directory, const std::string& prefix, const Limits& limits, uint64_t& scanned, Result& result, unsigned depth) {
    Need(depth <= 12, "现有存档目录过深。");
    int duplicate = fcntl(directory, F_DUPFD_CLOEXEC, 0); Need(duplicate >= 0, "无法枚举存档。");
    DIR* stream = fdopendir(duplicate); if (!stream) { close(duplicate); throw Failure("无法枚举存档。"); }
    try {
        for (;;) {
            errno = 0;
            auto* row = readdir(stream);
            if (!row) { Need(errno == 0, "读取存档目录失败。"); break; }
            std::string name = row->d_name; if (name == "." || name == "..") continue;
            Need(++scanned <= limits.maxScanEntries, "现有存档目录条目超过枚举限制。");
            std::string relative = prefix.empty() ? name : prefix + '/' + name;
            struct stat st{}; Need(StatAt(directory, name, st), "存档枚举时文件发生变化，请重试。");
            if (S_ISLNK(st.st_mode)) { result.warning = "已跳过符号链接；仅枚举普通存档文件。"; continue; }
            if (S_ISDIR(st.st_mode)) {
                Fd child(openat(directory, name.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW));
                Need(child.value >= 0, "存档子目录不可读。"); ListTree(child.value, relative, limits, scanned, result, depth + 1);
            } else if (S_ISREG(st.st_mode)) {
                std::string canonical; bool backup = false; if (!SaveName(name, canonical, backup) || backup) continue;
                Destination d;
                try { d = CanonicalSavePath("saves/" + relative); } catch (const Failure&) { continue; }
                // Enumeration must identify an actual native path, never map a
                // PC Profiles tree to a path which does not exist on disk.
                if (d.relative != relative || st.st_size <= 0 || uint64_t(st.st_size) > limits.maxFileBytes) continue;
                Fd file(openat(directory, name.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW));
                try { Need(file.value >= 0, "存档不可读。"); CheckOuterHeader(file.value, uint64_t(st.st_size)); }
                catch (const Failure&) { result.warning = "已跳过不可读、截断或外层头版本不匹配的文件；未改动原文件。"; continue; }
                Save s; s.id = relative; s.file = relative; s.mode = d.mode; s.backup = backup;
                s.title = DisplayTitle(file.value, uint64_t(st.st_size));
                if (s.title.empty()) s.title = canonical;
                s.size = uint64_t(st.st_size); s.mtime = st.st_mtime;
                result.saves.push_back(std::move(s));
            }
        }
    } catch (...) { closedir(stream); throw; }
    closedir(stream);
}
Result ListUnlocked(int root, const Limits& limits) {
    Result r;
    Fd saves = OpenDirectory(root, "saves", false, true);
    if (saves.value >= 0) { uint64_t scanned = 0; ListTree(saves.value, "", limits, scanned, r, 0); }
    std::sort(r.saves.begin(), r.saves.end(), [](const Save& a, const Save& b) { return a.id < b.id; });
    r.ok = true; return r;
}
#ifdef GAME_SAVE_IMPORT_TESTING
int failAfterCommits = -1;
#endif
} // namespace

Destination CanonicalSavePath(const std::string& archivePath) {
    auto p = Parts(archivePath);
    Destination out;
    const auto leaf = Fold(p.back());
    if (leaf == "settings.xml" || leaf == "pc_settings.bin" || leaf == ".ds_store" ||
        p.front() == "__MACOSX") { out.ignored = true; return out; }
    std::string name; bool backup = false;
    Need(SaveName(p.back(), name, backup), "ZIP 只接受 SGTA50000..SGTA50015(.bak) 进度存档；不允许程序、资源或未知 profile 配置。");
    p.pop_back();
    if (!p.empty() && p.front() == "saves") p.erase(p.begin());
    bool native = !p.empty() && (p.front() == "userdata" || p.front() == "sandbox");
    if (native) {
        out.mode = p.front() == "sandbox" ? 2 : 1;
        Need(p.size() == 5 && p[1] == "Documents" && p[2] == "Rockstar Games" && p[3] == "GTA V" && p[4] == "Default",
             "自家存档备份必须保留真实 namespace/Documents/Rockstar Games/GTA V/Default 路径。");
    }
    if (!native) {
        if (p.size() >= 3 && p[0] == "Rockstar Games" && p[1] == "GTA V") p.erase(p.begin(), p.begin() + 2);
        if (p.size() == 2 && p[0] == "Profiles") {
            Need(p[1].size() <= 64 && !p[1].empty() && std::all_of(p[1].begin(), p[1].end(), [](unsigned char c) {
                return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || c == '_' || c == '-';
            }), "外部 profile id 无效。");
            out.profile = p[1];
        } else Need(p.empty() || (p.size() == 1 && p[0] == "Default"), "存档 ZIP 的目录格式不受支持。");
    }
    out.relative = std::string(out.mode == 2 ? "sandbox" : "userdata") +
        "/Documents/Rockstar Games/GTA V/Default/" + name;
    return out;
}

Result List(const std::string& filesDir, const Limits& limits) {
    std::lock_guard<std::mutex> lock(operations);
    try { auto root = OpenRoot(filesDir); return ListUnlocked(root.value, limits); }
    catch (const std::exception& e) { Result r; r.error = e.what(); return r; }
}

Result ImportZip(int fd, const std::string& filesDir, const Limits& limits) {
    std::lock_guard<std::mutex> lock(operations);
    Result r;
    Fd root, backups, transaction;
    std::vector<Pending> pending;
    size_t committed = 0;
    bool ready = false;
    std::string transactionName;
    try {
        root = OpenRoot(filesDir);
        auto entries = Catalogue(fd, limits, r);
        uint64_t total = 0; for (const auto& e : entries) total = Add(total, e.size);
        struct statvfs capacity{}; Need(fstatvfs(root.value, &capacity) == 0, "无法检查存储空间。");
        Need(static_cast<long double>(capacity.f_bavail) * capacity.f_frsize >=
             static_cast<long double>(total) + limits.maxTotalBytes + (8ULL << 20), "空间不足，需同时保留新存档和旧 slot 备份。");
        backups = OpenDirectory(root.value, "save-backups", true);
        struct timespec now{}; Need(clock_gettime(CLOCK_MONOTONIC, &now) == 0, "无法生成事务编号。");
        transactionName = "import-" + std::to_string(getpid()) + '-' + std::to_string(now.tv_sec) + '-' + std::to_string(now.tv_nsec);
        Need(mkdirat(backups.value, transactionName.c_str(), 0700) == 0, "无法创建唯一存档备份事务。");
        transaction = OpenDirectory(backups.value, transactionName, false);
        r.backupDirectory = "save-backups/" + transactionName;
        Marker(transaction.value, "OWNER", "HarmonyGtaSaveImportV1\n");
        for (size_t i = 0; i < entries.size(); ++i) Extract(fd, entries[i], transaction.value, "incoming-" + std::to_string(i));
        uint64_t oldTotal = 0;
        std::string plan = "HarmonyGtaSaveImportV1\n";
        for (size_t i = 0; i < entries.size(); ++i) {
            const auto& e = entries[i]; Pending item; item.relative = e.destination.relative;
            size_t slash = item.relative.rfind('/'); item.leaf = item.relative.substr(slash + 1);
            item.parent = OpenDirectory(root.value, "saves/" + item.relative.substr(0, slash), true);
            item.incoming = "incoming-" + std::to_string(i); item.original = "original-" + std::to_string(i);
            item.existed = StatAt(item.parent.value, item.leaf, item.previous);
            if (item.existed) {
                Need(S_ISREG(item.previous.st_mode) && item.previous.st_nlink == 1 && item.previous.st_size >= 0 &&
                     uint64_t(item.previous.st_size) <= limits.maxFileBytes, "现有同名存档不是安全普通文件或超过备份限制。");
                oldTotal = Add(oldTotal, uint64_t(item.previous.st_size)); Need(oldTotal <= limits.maxTotalBytes, "旧存档备份总量超过限制。");
                Fd original(openat(item.parent.value, item.leaf.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW));
                struct stat stable{}; Need(original.value >= 0 && fstat(original.value, &stable) == 0 && Same(stable, item.previous), "现有存档正在变化，已停止导入。");
                Fd copy(openat(transaction.value, item.original.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600));
                Need(copy.value >= 0, "创建旧存档备份失败。"); Copy(original.value, copy.value, uint64_t(stable.st_size));
                PreserveTimes(copy.value, item.previous);
                Need(fstat(original.value, &stable) == 0 && Same(stable, item.previous), "备份期间存档发生变化。"); ++r.conflicts;
            }
            plan += std::string(item.existed ? "1\t" : "0\t") + item.relative + '\t' + item.incoming + '\t' + item.original + '\n';
            pending.push_back(std::move(item));
        }
        Marker(transaction.value, "plan.txt", plan); Marker(transaction.value, "READY", "All payloads CRC-checked; originals preserved.\n");
        // Make original backups AND their ancestor directory entries durable
        // before replacing any destination, including first-ever save import.
        Sync(backups.value); Sync(root.value); ready = true;
        for (auto& item : pending) {
            struct stat current{}; bool exists = StatAt(item.parent.value, item.leaf, current);
            Need(exists == item.existed && (!exists || Same(current, item.previous)), "提交前存档发生变化，已停止导入。");
            Need(renameat(transaction.value, item.incoming.c_str(), item.parent.value, item.leaf.c_str()) == 0, "原子替换存档失败。");
            ++committed; Sync(item.parent.value);
#ifdef GAME_SAVE_IMPORT_TESTING
            if (failAfterCommits >= 0 && int(committed) >= failAfterCommits) throw Failure("injected commit failure");
#endif
        }
        Marker(transaction.value, "COMMITTED", "Import complete; originals retained for rollback.\n"); Sync(backups.value);
        r.imported = entries.size(); r.bytes = total; r.ok = true;
        if (r.ignored) r.warning = "ZIP 中的 settings/平台元数据未导入；当前画质配置保持不变。";
        if (r.conflicts) r.warning += (r.warning.empty() ? "" : " ") + std::string("已保留同名旧 slot 备份。");
        r.warning += (r.warning.empty() ? "" : " ") + std::string("文件名/ZIP CRC 校验不保证跨版本存档内容兼容。");
        try { r.saves = ListUnlocked(root.value, limits).saves; } catch (...) { r.warning += " 导入成功，但刷新存档列表失败，请重新枚举。"; }
        return r;
    } catch (const std::exception& e) {
        r.error = e.what(); bool rollbackOk = true;
        for (size_t i = committed; i > 0; --i) {
            auto& item = pending[i - 1];
            try {
                if (item.existed) {
                    Fd original(openat(transaction.value, item.original.c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW));
                    struct stat st{}; Need(original.value >= 0 && fstat(original.value, &st) == 0 && S_ISREG(st.st_mode), "旧 slot 备份不可读。");
                    const auto temporary = "restore-" + std::to_string(i);
                    Fd restored(openat(transaction.value, temporary.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600));
                    Need(restored.value >= 0, "无法创建回滚文件。"); Copy(original.value, restored.value, uint64_t(st.st_size));
                    PreserveTimes(restored.value, item.previous);
                    Need(renameat(transaction.value, temporary.c_str(), item.parent.value, item.leaf.c_str()) == 0, "恢复旧 slot 失败。");
                } else Need(unlinkat(item.parent.value, item.leaf.c_str(), 0) == 0, "清理未完成的新 slot 失败。");
                Sync(item.parent.value);
            } catch (...) { rollbackOk = false; }
        }
        if (ready) {
            try { Marker(transaction.value, rollbackOk ? "ROLLED_BACK" : "ROLLBACK_INCOMPLETE", r.error + '\n'); } catch (...) {}
        }
        if (!rollbackOk) r.warning = "回滚未完全成功；旧文件仍保存在 " + r.backupDirectory + "，请保留应用数据。";
        else if (committed) r.warning = "本次导入失败，已回滚被替换的 slot，旧备份仍保留。";
        // No destination changed before READY. Remove only this flat, owned
        // transaction's temporary regular files; never recursively delete saves.
        if (!ready && !committed && transaction.value >= 0 && !transactionName.empty()) {
            try {
                int duplicate = fcntl(transaction.value, F_DUPFD_CLOEXEC, 0);
                Need(duplicate >= 0, "无法清理临时存档。");
                DIR* stream = fdopendir(duplicate);
                if (!stream) { close(duplicate); throw Failure("无法清理临时存档。"); }
                bool clean = true;
                for (;;) {
                    errno = 0;
                    auto* row = readdir(stream);
                    if (!row) { if (errno) clean = false; break; }
                    std::string name = row->d_name; if (name == "." || name == "..") continue;
                    struct stat st{};
                    if (!StatAt(transaction.value, name, st) || !S_ISREG(st.st_mode) ||
                        unlinkat(transaction.value, name.c_str(), 0) != 0) { clean = false; break; }
                }
                closedir(stream);
                if (clean && unlinkat(backups.value, transactionName.c_str(), AT_REMOVEDIR) == 0) r.backupDirectory.clear();
            } catch (...) {}
        }
        r.imported = 0; r.bytes = 0; return r;
    }
}

std::string ToJson(const Result& r) {
    std::string out = std::string("{\"ok\":") + (r.ok ? "true" : "false") + ",\"error\":" + Escape(r.error) +
        ",\"warning\":" + Escape(r.warning) + ",\"backupDirectory\":" + Escape(r.backupDirectory) +
        ",\"imported\":" + std::to_string(r.imported) + ",\"bytes\":" + std::to_string(r.bytes) +
        ",\"conflicts\":" + std::to_string(r.conflicts) + ",\"ignored\":" + std::to_string(r.ignored) + ",\"saves\":[";
    for (size_t i = 0; i < r.saves.size(); ++i) {
        const auto& s = r.saves[i]; if (i) out += ',';
        out += "{\"id\":" + Escape(s.id) + ",\"title\":" + Escape(s.title) + ",\"file\":" + Escape(s.file) +
            ",\"size\":" + std::to_string(s.size) + ",\"mtime\":" + std::to_string(s.mtime) +
            ",\"mode\":" + std::to_string(s.mode) + ",\"backup\":" + (s.backup ? "true" : "false") + '}';
    }
    return out + "]}";
}
#ifdef GAME_SAVE_IMPORT_TESTING
void FailAfterCommitsForTesting(int count) { failAfterCommits = count; }
#endif
} // namespace game_saves
