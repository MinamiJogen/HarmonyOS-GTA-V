#include "game_resource_import_core.h"

#include <algorithm>
#include <cerrno>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dirent.h>
#include <fcntl.h>
#include <limits>
#include <map>
#include <set>
#include <stdexcept>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <unistd.h>
#include <vector>
#include <zlib.h>

namespace game_resources {
namespace {

struct Failure : std::runtime_error { using std::runtime_error::runtime_error; };
struct Cancelled : Failure { Cancelled() : Failure("资源导入已取消，原资源保持不变。") {} };
[[noreturn]] void Fail(const std::string& message) { throw Failure(message); }
void Need(bool condition, const std::string& message) { if (!condition) Fail(message); }
void Cancel(const std::atomic<bool>* flag) { if (flag && flag->load()) throw Cancelled(); }

struct Fd {
    int value = -1;
    explicit Fd(int descriptor = -1) : value(descriptor) {}
    ~Fd() { if (value >= 0) close(value); }
    Fd(const Fd&) = delete;
    Fd& operator=(const Fd&) = delete;
};

uint16_t U16(const unsigned char* p) { return uint16_t(p[0]) | uint16_t(p[1]) << 8; }
uint32_t U32(const unsigned char* p) { return uint32_t(U16(p)) | uint32_t(U16(p + 2)) << 16; }
uint64_t U64(const unsigned char* p) { return uint64_t(U32(p)) | uint64_t(U32(p + 4)) << 32; }
uint64_t Add(uint64_t a, uint64_t b) {
    Need(a <= std::numeric_limits<uint64_t>::max() - b, "ZIP 长度溢出。");
    return a + b;
}

void ReadAt(int fd, uint64_t offset, void* buffer, size_t size, const std::atomic<bool>* cancel) {
    Need(offset <= uint64_t(std::numeric_limits<off_t>::max()), "ZIP 偏移超出平台范围。");
    auto* target = static_cast<unsigned char*>(buffer);
    while (size) {
        Cancel(cancel);
        ssize_t count = pread(fd, target, size, static_cast<off_t>(offset));
        if (count < 0 && errno == EINTR) continue;
        Need(count > 0, "读取 ZIP 失败或文件尚未下载完整。");
        target += count; size -= size_t(count); offset = Add(offset, uint64_t(count));
    }
}

bool Exists(const std::string& path) {
    struct stat st{};
    if (lstat(path.c_str(), &st) == 0) return true;
    Need(errno == ENOENT, "无法检查资源目录。");
    return false;
}

void Directory(const std::string& path) {
    struct stat st{};
    Need(lstat(path.c_str(), &st) == 0 && S_ISDIR(st.st_mode) && !S_ISLNK(st.st_mode),
         "资源目录不可用或包含符号链接。");
}

void SafePath(const std::string& path, bool directory = false) {
    Need(!path.empty() && path.size() <= 1024 && path.front() != '/', "ZIP 文件路径无效。");
    std::string name = path;
    if (directory && name.back() == '/') name.pop_back();
    Need(!name.empty(), "ZIP 文件路径为空。");
    size_t start = 0;
    while (start <= name.size()) {
        auto end = name.find('/', start);
        auto part = name.substr(start, end == std::string::npos ? end : end - start);
        Need(!part.empty() && part != "." && part != "..", "ZIP 含路径穿越或不规范路径。");
        for (unsigned char c : part) Need(c >= 32 && c != 127 && c != '\\' && c != ':', "ZIP 含不安全文件名。");
        if (end == std::string::npos) break;
        start = end + 1;
    }
}

std::string Fold(std::string value) {
    for (char& c : value) if (c >= 'A' && c <= 'Z') c = char(c - 'A' + 'a');
    return value;
}

void Parents(const std::string& root, const std::string& relative) {
    size_t end = 0;
    while ((end = relative.find('/', end)) != std::string::npos) {
        auto path = root + '/' + relative.substr(0, end);
        if (mkdir(path.c_str(), 0700) != 0) Need(errno == EEXIST, "创建资源目录失败，可能存储空间不足。");
        Directory(path);
        ++end;
    }
}

void RemoveTree(const std::string& path) {
    struct stat st{};
    if (lstat(path.c_str(), &st) != 0) { Need(errno == ENOENT, "清理临时资源失败。"); return; }
    if (!S_ISDIR(st.st_mode) || S_ISLNK(st.st_mode)) {
        Need(unlink(path.c_str()) == 0, "清理临时资源文件失败。"); return;
    }
    DIR* stream = opendir(path.c_str());
    Need(stream != nullptr, "清理临时资源目录失败。");
    try {
        while (auto* row = readdir(stream)) {
            std::string name = row->d_name;
            if (name != "." && name != "..") RemoveTree(path + '/' + name);
        }
    } catch (...) { closedir(stream); throw; }
    closedir(stream);
    Need(rmdir(path.c_str()) == 0, "清理临时资源目录失败。");
}

void SyncDirectory(const std::string& path) {
    Fd directory(open(path.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW));
    Need(directory.value >= 0 && fsync(directory.value) == 0, "同步资源目录失败。");
}

std::string Root(const std::string& filesDir) {
    Need(!filesDir.empty() && filesDir.front() == '/', "应用资源目录必须为绝对路径。");
    Directory(filesDir);
    char* resolved = realpath(filesDir.c_str(), nullptr);
    Need(resolved != nullptr, "无法解析应用资源目录。");
    std::string root(resolved); free(resolved);
    return root;
}

// A crash between the two renames leaves the prior complete tree at this fixed
// backup name. Startup validation restores it before inspecting resources.
void Recover(const std::string& root) {
    const auto game = root + "/game", backup = root + "/game.import-backup";
    if (!Exists(game) && Exists(backup)) {
        Directory(backup);
        Need(rename(backup.c_str(), game.c_str()) == 0, "恢复上次游戏资源失败，请保留应用数据后重试。");
        SyncDirectory(root);
    }
}

constexpr const char* STAGE_PREFIX = ".game-import-stage-";
constexpr const char* STAGE_MARKER = ".resource-import-owner";
constexpr const char* STAGE_IDENTITY = "HarmonyGtaResourceImportV1\n8b0b5899ed\n";

void MarkStage(const std::string& stage) {
    Fd marker(open((stage + '/' + STAGE_MARKER).c_str(), O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600));
    const size_t length = std::strlen(STAGE_IDENTITY);
    Need(marker.value >= 0 && write(marker.value, STAGE_IDENTITY, length) == static_cast<ssize_t>(length) && fsync(marker.value) == 0,
         "无法标记资源临时目录。");
    SyncDirectory(stage);
}

void CleanupStages(const std::string& root, const std::atomic<bool>* cancel) {
    DIR* stream = opendir(root.c_str()); Need(stream != nullptr, "无法检查遗留临时资源。");
    std::vector<std::string> stages;
    while (auto* row = readdir(stream)) {
        std::string name = row->d_name;
        if (name.compare(0, std::strlen(STAGE_PREFIX), STAGE_PREFIX) == 0) stages.push_back(name);
    }
    closedir(stream);
    for (const auto& name : stages) {
        Cancel(cancel);
        Need(name.size() == std::strlen(STAGE_PREFIX) + 6 && std::all_of(name.begin() + std::strlen(STAGE_PREFIX), name.end(),
             [](unsigned char c) { return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9'); }),
             "存在无法识别的资源临时目录；为保护数据，已停止新导入。");
        std::string path = root + '/' + name;
        struct stat st{};
        Need(lstat(path.c_str(), &st) == 0 && S_ISDIR(st.st_mode) && !S_ISLNK(st.st_mode) && st.st_uid == getuid(),
             "资源临时目录所有权异常，未自动清理。");
        Fd marker(open((path + '/' + STAGE_MARKER).c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW));
        const size_t length = std::strlen(STAGE_IDENTITY);
        bool completeMarker = marker.value >= 0 && fstat(marker.value, &st) == 0 && S_ISREG(st.st_mode) &&
                              st.st_uid == getuid() && st.st_size == static_cast<off_t>(length);
        std::string identity;
        if (marker.value >= 0 && fstat(marker.value, &st) == 0 && S_ISREG(st.st_mode) && st.st_uid == getuid() &&
            st.st_size >= 0 && uint64_t(st.st_size) <= length) {
            identity.resize(size_t(st.st_size));
            if (!identity.empty()) ReadAt(marker.value, 0, &identity[0], identity.size(), cancel);
        }
        if (!(completeMarker && identity == STAGE_IDENTITY)) {
            // A kill immediately after mkdtemp/marker creation can only leave
            // an empty directory or the incomplete marker, never resource data.
            DIR* contents = opendir(path.c_str()); Need(contents != nullptr, "无法检查临时目录。");
            bool hasData = false, hasMarker = false;
            while (auto* row = readdir(contents)) {
                std::string child = row->d_name;
                if (child == "." || child == "..") continue;
                if (child == STAGE_MARKER) hasMarker = true; else hasData = true;
            }
            closedir(contents);
            bool partialMarker = hasMarker && marker.value >= 0 && fstat(marker.value, &st) == 0 &&
                                 S_ISREG(st.st_mode) && st.st_uid == getuid() && st.st_size >= 0 &&
                                 uint64_t(st.st_size) < length && identity.size() == uint64_t(st.st_size) &&
                                 std::string(STAGE_IDENTITY).compare(0, identity.size(), identity) == 0;
            Need(!hasData && (!hasMarker || partialMarker),
                 "遗留临时目录缺少有效导入标记；已保留该目录并停止新导入，避免占用更多空间。");
        }
        RemoveTree(path);
    }
}

struct Json {
    enum Kind { Null, Boolean, Number, String, Array, Object } kind = Null;
    bool boolean = false;
    std::string text;
    std::vector<Json> array;
    std::map<std::string, Json> object;
    const Json* Get(const std::string& key) const {
        auto found = object.find(key); return found == object.end() ? nullptr : &found->second;
    }
};

class JsonParser {
    const std::string& source_;
    size_t pos_ = 0, nodes_ = 0;
    void Space() { while (pos_ < source_.size() && (source_[pos_] == ' ' || source_[pos_] == '\n' || source_[pos_] == '\r' || source_[pos_] == '\t')) ++pos_; }
    char Take() { Need(pos_ < source_.size(), "资源 JSON 不完整。"); return source_[pos_++]; }
    bool Consume(char c) { Space(); if (pos_ < source_.size() && source_[pos_] == c) { ++pos_; return true; } return false; }
    uint32_t Hex() {
        uint32_t value = 0;
        for (int i = 0; i < 4; ++i) {
            char c = Take(); int n = c >= '0' && c <= '9' ? c - '0' : c >= 'a' && c <= 'f' ? c - 'a' + 10 : c >= 'A' && c <= 'F' ? c - 'A' + 10 : -1;
            Need(n >= 0, "资源 JSON 含无效 Unicode 转义。"); value = (value << 4) | uint32_t(n);
        }
        return value;
    }
    static void Utf8(std::string& out, uint32_t c) {
        if (c < 0x80) out += char(c);
        else if (c < 0x800) { out += char(0xc0 | c >> 6); out += char(0x80 | (c & 63)); }
        else if (c < 0x10000) { out += char(0xe0 | c >> 12); out += char(0x80 | ((c >> 6) & 63)); out += char(0x80 | (c & 63)); }
        else { out += char(0xf0 | c >> 18); out += char(0x80 | ((c >> 12) & 63)); out += char(0x80 | ((c >> 6) & 63)); out += char(0x80 | (c & 63)); }
    }
    std::string StringValue() {
        Need(Take() == '"', "资源 JSON 需要字符串。");
        std::string out;
        for (;;) {
            unsigned char c = static_cast<unsigned char>(Take());
            if (c == '"') return out;
            Need(c >= 32, "资源 JSON 字符串含控制字符。");
            if (c != '\\') { out += char(c); continue; }
            switch (Take()) {
                case '"': out += '"'; break; case '\\': out += '\\'; break; case '/': out += '/'; break;
                case 'b': out += '\b'; break; case 'f': out += '\f'; break; case 'n': out += '\n'; break; case 'r': out += '\r'; break; case 't': out += '\t'; break;
                case 'u': {
                    uint32_t value = Hex();
                    if (value >= 0xd800 && value <= 0xdbff) {
                        Need(Take() == '\\' && Take() == 'u', "资源 JSON 的 Unicode 代理对无效。");
                        uint32_t tail = Hex(); Need(tail >= 0xdc00 && tail <= 0xdfff, "资源 JSON 的 Unicode 代理对无效。");
                        value = 0x10000 + ((value - 0xd800) << 10) + tail - 0xdc00;
                    } else Need(!(value >= 0xdc00 && value <= 0xdfff), "资源 JSON 的 Unicode 代理对无效。");
                    Utf8(out, value); break;
                }
                default: Fail("资源 JSON 含无效转义。");
            }
        }
    }
    Json Value(unsigned depth) {
        Need(depth <= 64 && ++nodes_ <= 500000, "资源 JSON 结构过大或嵌套过深。");
        Space(); Need(pos_ < source_.size(), "资源 JSON 为空。");
        Json value; char c = source_[pos_];
        if (c == '"') { value.kind = Json::String; value.text = StringValue(); return value; }
        if (c == '{') {
            ++pos_; value.kind = Json::Object;
            if (Consume('}')) return value;
            for (;;) {
                Space(); auto key = StringValue(); Need(Consume(':'), "资源 JSON 缺少冒号。");
                Need(value.object.emplace(key, Value(depth + 1)).second, "资源 JSON 含重复字段。");
                if (Consume('}')) return value;
                Need(Consume(','), "资源 JSON 对象格式错误。");
            }
        }
        if (c == '[') {
            ++pos_; value.kind = Json::Array;
            if (Consume(']')) return value;
            for (;;) {
                value.array.push_back(Value(depth + 1));
                if (Consume(']')) return value;
                Need(Consume(','), "资源 JSON 数组格式错误。");
            }
        }
        for (const char* literal : {"true", "false", "null"}) {
            size_t length = std::strlen(literal);
            if (source_.compare(pos_, length, literal) == 0) {
                pos_ += length; value.kind = literal[0] == 'n' ? Json::Null : Json::Boolean; value.boolean = literal[0] == 't'; return value;
            }
        }
        size_t start = pos_;
        if (source_[pos_] == '-') ++pos_;
        Need(pos_ < source_.size(), "资源 JSON 数值无效。");
        if (source_[pos_] == '0') ++pos_;
        else { Need(source_[pos_] >= '1' && source_[pos_] <= '9', "资源 JSON 数值无效。"); while (pos_ < source_.size() && source_[pos_] >= '0' && source_[pos_] <= '9') ++pos_; }
        if (pos_ < source_.size() && source_[pos_] == '.') {
            size_t first = ++pos_; while (pos_ < source_.size() && source_[pos_] >= '0' && source_[pos_] <= '9') ++pos_;
            Need(pos_ > first, "资源 JSON 小数格式无效。");
        }
        if (pos_ < source_.size() && (source_[pos_] == 'e' || source_[pos_] == 'E')) {
            ++pos_; if (pos_ < source_.size() && (source_[pos_] == '+' || source_[pos_] == '-')) ++pos_;
            size_t first = pos_; while (pos_ < source_.size() && source_[pos_] >= '0' && source_[pos_] <= '9') ++pos_;
            Need(pos_ > first, "资源 JSON 指数格式无效。");
        }
        value.kind = Json::Number; value.text = source_.substr(start, pos_ - start);
        Need(std::isfinite(std::strtod(value.text.c_str(), nullptr)), "资源 JSON 数值超出范围。"); return value;
    }
public:
    explicit JsonParser(const std::string& source) : source_(source) {}
    Json Parse() { Json value = Value(0); Space(); Need(pos_ == source_.size(), "资源 JSON 含多余内容。"); return value; }
};

std::string Metadata(const std::string& root, const std::string& relative, const Limits& limits) {
    SafePath(relative);
    size_t end = 0;
    while ((end = relative.find('/', end)) != std::string::npos) { Directory(root + '/' + relative.substr(0, end)); ++end; }
    Fd source(open((root + '/' + relative).c_str(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW));
    struct stat st{};
    Need(source.value >= 0 && fstat(source.value, &st) == 0 && S_ISREG(st.st_mode), "缺少资源元数据：" + relative);
    Need(st.st_size >= 0 && uint64_t(st.st_size) <= limits.maxMetadataBytes, "资源元数据过大。");
    std::string text(size_t(st.st_size), '\0');
    if (!text.empty()) ReadAt(source.value, 0, &text[0], text.size(), nullptr);
    return text;
}

uint64_t FileSize(const std::string& root, const std::string& relative) {
    SafePath(relative);
    // lstat every parent, including existing resources inspected at startup.
    size_t end = 0;
    while ((end = relative.find('/', end)) != std::string::npos) { Directory(root + '/' + relative.substr(0, end)); ++end; }
    struct stat st{};
    Need(lstat((root + '/' + relative).c_str(), &st) == 0 && S_ISREG(st.st_mode) && !S_ISLNK(st.st_mode) && st.st_size >= 0,
         "资源文件缺失或不是普通文件：" + relative);
    return uint64_t(st.st_size);
}

uint64_t Unsigned(const Json& value) {
    Need(value.kind == Json::Number && !value.text.empty(), "manifest 文件长度必须为非负整数。");
    uint64_t result = 0;
    for (char c : value.text) {
        Need(c >= '0' && c <= '9' && result <= (std::numeric_limits<uint64_t>::max() - uint64_t(c - '0')) / 10, "manifest 文件长度无效。");
        result = result * 10 + uint64_t(c - '0');
    }
    return result;
}

Result Validate(const std::string& game, const Limits& limits, const std::atomic<bool>* cancel) {
    Directory(game);
    Directory(game + "/b");
    DIR* buildDirectory = opendir((game + "/b").c_str());
    Need(buildDirectory != nullptr, "无法读取游戏版本目录。");
    bool unexpectedBuild = false;
    while (auto* row = readdir(buildDirectory)) {
        std::string name = row->d_name;
        if (name == "." || name == "..") continue;
        struct stat st{};
        if (lstat((game + "/b/" + name).c_str(), &st) != 0 || S_ISLNK(st.st_mode) || (S_ISDIR(st.st_mode) && name != SUPPORTED_BUILD)) unexpectedBuild = true;
    }
    closedir(buildDirectory);
    Need(!unexpectedBuild, "资源包含其他游戏版本或不安全版本目录。");
    std::string manifestText = Metadata(game, "data/manifest.json", limits);
    Json manifest = JsonParser(manifestText).Parse();
    const Json* files = manifest.Get("files");
    Need(manifest.kind == Json::Object && files && files->kind == Json::Array && !files->array.empty(), "游戏 manifest 的 files 列表无效。");
    Result result;
    std::set<std::string> manifestPaths;
    for (const auto& row : files->array) {
        Cancel(cancel);
        Need(row.kind == Json::Array && row.array.size() >= 2 && row.array[0].kind == Json::String, "manifest 文件记录无效。");
        SafePath(row.array[0].text);
        Need(manifestPaths.insert(Fold(row.array[0].text)).second, "manifest 含重复或大小写冲突文件引用。");
        uint64_t size = FileSize(game, "data/" + row.array[0].text);
        Need(size <= limits.maxFileBytes && size == Unsigned(row.array[1]), "游戏数据长度与 manifest 不一致或超过限制：" + row.array[0].text);
        result.bytes = Add(result.bytes, size); ++result.files;
        Need(result.bytes <= limits.maxTotalBytes && result.files <= limits.maxEntries, "游戏资源超过导入限制。");
    }
    const std::string shaders = std::string("b/") + SUPPORTED_BUILD + "/shaders/";
    std::string indexText = Metadata(game, shaders + "index.json", limits);
    Json index = JsonParser(indexText).Parse();
    Need(index.kind == Json::Object, "游戏着色器 index.json 必须为对象。");
    uint64_t usable = 0;
    for (const auto& row : index.object) {
        Cancel(cancel);
        if (!row.first.empty() && row.first[0] == '_') continue;
        SafePath(row.first); Need(row.first.find('/') == std::string::npos, "着色器索引名称不安全。");
        Need(row.second.kind == Json::Object, "着色器索引记录无效。");
        const Json* ok = row.second.Get("ok");
        if (!ok || ok->kind != Json::Boolean || !ok->boolean) continue;
        Need(FileSize(game, shaders + row.first + ".wgsl") > 0, "着色器内容为空。");
        const Json* constants = row.second.Get("consts");
        if (constants && constants->kind != Json::Null && !(constants->kind == Json::Boolean && !constants->boolean)) {
            std::string text = Metadata(game, shaders + row.first + ".consts.json", limits);
            Need(JsonParser(text).Parse().kind == Json::Array, "着色器常量文件必须为数组。");
        }
        ++usable;
    }
    Need(usable > 0, "未找到与当前游戏版本匹配的可用着色器。");
    result.ready = true; return result;
}

struct Entry {
    std::string name, relative;
    uint64_t compressed = 0, size = 0, local = 0;
    uint32_t crc = 0;
    uint16_t flags = 0, method = 0;
    bool directory = false;
};

std::vector<Entry> Catalogue(int fd, const Limits& limits, std::atomic<bool>& cancel,
                             uint64_t& total, uint64_t& centralOffset) {
    struct stat st{};
    Need(fstat(fd, &st) == 0 && S_ISREG(st.st_mode) && st.st_size >= 22, "请选择已下载完成、可随机读取的本地 ZIP 文件。");
    uint64_t length = uint64_t(st.st_size);
    Need(length <= limits.maxArchiveBytes, "ZIP 文件超过大小限制。");
    std::vector<unsigned char> tail(size_t(std::min<uint64_t>(length, 65557)));
    ReadAt(fd, length - tail.size(), tail.data(), tail.size(), &cancel);
    size_t eocd = tail.size();
    for (size_t i = tail.size() - 22;; --i) {
        if (U32(tail.data() + i) == 0x06054b50 && i + 22 + U16(tail.data() + i + 20) == tail.size()) { eocd = i; break; }
        if (i == 0) break;
    }
    Need(eocd != tail.size(), "找不到 ZIP 中央目录，文件可能未下载完整。");
    const auto* end = tail.data() + eocd;
    Need(U16(end + 4) == 0 && U16(end + 6) == 0 && U16(end + 8) == U16(end + 10), "不支持分卷 ZIP。");
    uint64_t count = U16(end + 10), centralSize = U32(end + 12);
    centralOffset = U32(end + 16);
    uint64_t endOffset = length - tail.size() + eocd;
    if (count == 0xffff || centralSize == 0xffffffff || centralOffset == 0xffffffff) {
        Need(endOffset >= 20, "ZIP64 定位记录缺失。");
        unsigned char locator[20]; ReadAt(fd, endOffset - 20, locator, sizeof(locator), &cancel);
        Need(U32(locator) == 0x07064b50 && U32(locator + 4) == 0 && U32(locator + 16) == 1, "ZIP64 分卷或定位记录无效。");
        uint64_t position = U64(locator + 8);
        Need(Add(position, 56) <= endOffset - 20, "ZIP64 中央目录边界无效。");
        unsigned char record[56]; ReadAt(fd, position, record, sizeof(record), &cancel);
        Need(U32(record) == 0x06064b50 && U64(record + 4) >= 44 && Add(position, Add(12, U64(record + 4))) == endOffset - 20,
             "ZIP64 结束记录无效。");
        Need(U32(record + 16) == 0 && U32(record + 20) == 0 && U64(record + 24) == U64(record + 32), "不支持分卷 ZIP64。");
        count = U64(record + 32); centralSize = U64(record + 40); centralOffset = U64(record + 48); endOffset = position;
    }
    Need(count > 0 && count <= limits.maxEntries && Add(centralOffset, centralSize) <= endOffset, "ZIP 目录过大或范围无效。");
    std::vector<Entry> all; all.reserve(size_t(count));
    uint64_t position = centralOffset, nameBytes = 0;
    std::set<std::string> names;
    for (uint64_t i = 0; i < count; ++i) {
        Cancel(&cancel);
        Need(Add(position, 46) <= Add(centralOffset, centralSize), "ZIP 目录记录截断。");
        unsigned char header[46]; ReadAt(fd, position, header, sizeof(header), &cancel);
        Need(U32(header) == 0x02014b50, "ZIP 目录记录无效。");
        Entry entry; entry.flags = U16(header + 8); entry.method = U16(header + 10);
        entry.crc = U32(header + 16); entry.compressed = U32(header + 20); entry.size = U32(header + 24); entry.local = U32(header + 42);
        uint16_t nameLength = U16(header + 28), extraLength = U16(header + 30), commentLength = U16(header + 32);
        Need(nameLength > 0 && nameLength <= 1024 && !(entry.flags & (1 | 0x40 | 0x2000)), "ZIP 含加密条目或过长路径。");
        uint64_t next = Add(position, Add(46, uint64_t(nameLength) + extraLength + commentLength));
        Need(next <= Add(centralOffset, centralSize), "ZIP 文件名或扩展字段截断。");
        std::vector<unsigned char> variable(size_t(nameLength) + extraLength);
        ReadAt(fd, position + 46, variable.data(), variable.size(), &cancel);
        entry.name.assign(reinterpret_cast<const char*>(variable.data()), nameLength);
        entry.directory = entry.name.back() == '/'; SafePath(entry.name, entry.directory);
        nameBytes = Add(nameBytes, entry.name.size()); Need(nameBytes <= limits.maxMetadataBytes, "ZIP 路径元数据过大。");
        Need(names.insert(Fold(entry.name)).second, "ZIP 含重复或大小写冲突路径。");
        uint32_t mode = U32(header + 38) >> 16;
        uint32_t type = mode & S_IFMT;
        Need(type == 0 || type == S_IFREG || type == S_IFDIR, "ZIP 含符号链接或特殊文件。");
        Need((type != S_IFDIR || entry.directory) && (type != S_IFREG || !entry.directory), "ZIP 文件类型与路径不一致。");
        uint32_t disk = U16(header + 34); bool found64 = false;
        for (size_t extra = nameLength; extra < variable.size();) {
            Need(extra + 4 <= variable.size(), "ZIP 扩展字段无效。");
            uint16_t kind = U16(variable.data() + extra), size = U16(variable.data() + extra + 2); extra += 4;
            Need(extra + size <= variable.size(), "ZIP 扩展字段截断。");
            if (kind == 1) {
                Need(!found64, "ZIP64 扩展字段重复。"); found64 = true;
                size_t part = extra;
                auto read64 = [&]() { Need(part + 8 <= extra + size, "ZIP64 长度字段缺失。"); uint64_t n = U64(variable.data() + part); part += 8; return n; };
                if (entry.size == 0xffffffff) entry.size = read64();
                if (entry.compressed == 0xffffffff) entry.compressed = read64();
                if (entry.local == 0xffffffff) entry.local = read64();
                if (disk == 0xffff) { Need(part + 4 <= extra + size, "ZIP64 分卷字段缺失。"); disk = U32(variable.data() + part); }
            }
            extra += size;
        }
        Need(disk == 0 && (found64 || (entry.size != 0xffffffff && entry.compressed != 0xffffffff && entry.local != 0xffffffff)), "ZIP64 扩展信息缺失或分卷无效。");
        Need(entry.local < centralOffset, "ZIP 本地文件头偏移无效。");
        all.push_back(std::move(entry)); position = next;
    }
    Need(position == Add(centralOffset, centralSize), "ZIP 中央目录长度不一致。");
    int layouts = 0; std::string prefix;
    for (const std::string& candidate : {std::string(), std::string("game/")}) {
        if (names.count(Fold(candidate + "data/manifest.json"))) { ++layouts; prefix = candidate; }
    }
    Need(layouts == 1, "ZIP 必须有唯一的 data/manifest.json 或 game/data/manifest.json。");
    const std::string shaderRoot = std::string("b/") + SUPPORTED_BUILD + "/shaders/";
    std::vector<Entry> selected; total = 0;
    for (auto& entry : all) {
        if (entry.name.compare(0, prefix.size(), prefix) != 0) continue;
        std::string relative = entry.name.substr(prefix.size());
        if (relative.compare(0, 2, "b/") == 0) {
            size_t slash = relative.find('/', 2);
            if (slash != std::string::npos && relative.substr(2, slash - 2) != SUPPORTED_BUILD)
                Fail("游戏资源版本不匹配；当前仅支持 " + std::string(SUPPORTED_BUILD) + "。");
        }
        if (entry.directory || !(relative.compare(0, 5, "data/") == 0 || relative.compare(0, shaderRoot.size(), shaderRoot) == 0)) continue;
        auto lower = Fold(relative);
        Need(!(lower.size() >= 5 && lower.substr(lower.size() - 5) == ".wasm") &&
             !(lower.size() >= 6 && lower.substr(lower.size() - 6) == ".cwasm") &&
             !(lower.size() >= 4 && lower.substr(lower.size() - 4) == ".hap") &&
             !(lower.size() >= 4 && lower.substr(lower.size() - 4) == ".aot"), "资源目录中不允许程序或安装包文件。");
        Need(entry.method == 0 || entry.method == 8, "资源 ZIP 仅支持 Store 或 Deflate 压缩。");
        Need(entry.size <= limits.maxFileBytes && (entry.compressed > 0 || entry.size == 0), "资源文件过大或压缩长度无效。");
        Need(entry.size <= Add(entry.compressed > (std::numeric_limits<uint64_t>::max() >> 11) ? std::numeric_limits<uint64_t>::max() - (1 << 20) : entry.compressed * 2048, 1 << 20), "资源压缩比例异常。");
        total = Add(total, entry.size); Need(total <= limits.maxTotalBytes, "资源总大小超过导入限制。");
        entry.relative = relative; selected.push_back(std::move(entry));
    }
    Need(!selected.empty(), "ZIP 中没有可导入的游戏资源。");
    std::set<std::string> selectedNames;
    for (const auto& entry : selected) {
        selectedNames.insert(entry.relative);
        if (entry.relative == "data/manifest.json" || entry.relative == shaderRoot + "index.json" ||
            (entry.relative.compare(0, shaderRoot.size(), shaderRoot) == 0 && entry.relative.size() >= 12 &&
             entry.relative.substr(entry.relative.size() - 12) == ".consts.json"))
            Need(entry.size <= limits.maxMetadataBytes, "manifest 或着色器元数据超过大小限制。");
    }
    Need(selectedNames.count("data/manifest.json") && selectedNames.count(shaderRoot + "index.json"), "资源 ZIP 缺少 manifest 或当前版本的着色器索引。");
    return selected;
}

void Extract(int fd, const Entry& entry, uint64_t centralOffset, const std::string& stage,
             std::atomic<bool>& cancel, Progress& progress, const ProgressCallback& callback) {
    unsigned char header[30]; ReadAt(fd, entry.local, header, sizeof(header), &cancel);
    Need(U32(header) == 0x04034b50 && U16(header + 6) == entry.flags && U16(header + 8) == entry.method, "ZIP 本地头与中央目录不一致。");
    uint16_t nameLength = U16(header + 26), extraLength = U16(header + 28);
    Need(nameLength == entry.name.size(), "ZIP 本地路径与中央目录不一致。");
    std::string localName(nameLength, '\0'); ReadAt(fd, entry.local + 30, &localName[0], nameLength, &cancel);
    Need(localName == entry.name, "ZIP 本地路径与中央目录不一致。");
    uint64_t position = Add(entry.local, uint64_t(30) + nameLength + extraLength);
    Need(Add(position, entry.compressed) <= centralOffset, "ZIP 压缩数据超出边界。");
    Parents(stage, entry.relative);
    Fd output(open((stage + '/' + entry.relative).c_str(), O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW, 0600));
    Need(output.value >= 0, "创建资源文件失败，可能空间不足或路径冲突。");
    std::vector<unsigned char> input(256 << 10), decoded(256 << 10);
    uint64_t remaining = entry.compressed, written = 0;
    uLong crc = crc32(0L, Z_NULL, 0);
    auto write = [&](const unsigned char* data, size_t size) {
        Cancel(&cancel); Need(Add(written, size) <= entry.size, "ZIP 解压长度超出中央目录声明。");
        size_t offset = 0;
        while (offset < size) {
            Cancel(&cancel); ssize_t count = ::write(output.value, data + offset, size - offset);
            if (count < 0 && errno == EINTR) continue;
            Need(count > 0, "写入资源失败，可能存储空间不足。"); offset += size_t(count);
        }
        crc = crc32(crc, data, uInt(size)); written += size; progress.completedBytes += size;
        if (callback) callback(progress);
    };
    if (entry.method == 0) {
        Need(entry.compressed == entry.size, "Store 文件的压缩长度不一致。");
        while (remaining) {
            size_t count = size_t(std::min<uint64_t>(remaining, input.size()));
            ReadAt(fd, position, input.data(), count, &cancel); write(input.data(), count); position += count; remaining -= count;
        }
    } else {
        z_stream stream{}; Need(inflateInit2(&stream, -MAX_WBITS) == Z_OK, "初始化 ZIP 解压器失败。");
        int state = Z_OK;
        try {
            while (state != Z_STREAM_END) {
                Cancel(&cancel);
                if (stream.avail_in == 0 && remaining) {
                    size_t count = size_t(std::min<uint64_t>(remaining, input.size()));
                    ReadAt(fd, position, input.data(), count, &cancel); position += count; remaining -= count;
                    stream.next_in = input.data(); stream.avail_in = uInt(count);
                }
                stream.next_out = decoded.data(); stream.avail_out = uInt(decoded.size());
                uInt previousInput = stream.avail_in;
                state = inflate(&stream, Z_NO_FLUSH);
                Need(state == Z_OK || state == Z_STREAM_END, "ZIP Deflate 数据损坏或截断。");
                size_t size = decoded.size() - stream.avail_out;
                if (size) write(decoded.data(), size);
                Need(state == Z_STREAM_END || size || stream.avail_in != previousInput, "ZIP Deflate 数据不完整。");
            }
            Need(remaining == 0 && stream.avail_in == 0, "ZIP Deflate 数据存在多余内容。");
        } catch (...) { inflateEnd(&stream); throw; }
        inflateEnd(&stream);
    }
    Need(written == entry.size && uint32_t(crc) == entry.crc, "ZIP 资源 CRC 或长度校验失败。");
    Need(fsync(output.value) == 0, "同步资源文件失败，可能存储空间不足。");
    ++progress.files; if (callback) callback(progress);
}

std::string Escape(const std::string& source) {
    std::string out = "\"";
    for (unsigned char c : source) {
        if (c == '"' || c == '\\') { out += '\\'; out += char(c); }
        else if (c < 32) { char buffer[7]; std::snprintf(buffer, sizeof(buffer), "\\u%04x", c); out += buffer; }
        else out += char(c);
    }
    return out + '"';
}

} // namespace

Result Check(const std::string& filesDir, const Limits& limits) {
    try {
        auto root = Root(filesDir); Recover(root);
        std::string warning;
        try { CleanupStages(root, nullptr); } catch (const std::exception& error) { warning = error.what(); }
        Result result = Validate(root + "/game", limits, nullptr); result.warning = warning; return result;
    }
    catch (const std::exception& error) { Result result; result.error = error.what(); return result; }
}

Result ImportZip(int fd, const std::string& filesDir, std::atomic<bool>& cancel,
                 const ProgressCallback& callback, const Limits& limits) {
    Progress progress; progress.active = true; progress.phase = "checking";
    std::string stage;
    bool committed = false;
    Result result;
    try {
        if (callback) callback(progress);
        auto root = Root(filesDir); Recover(root); CleanupStages(root, &cancel);
        uint64_t total = 0, central = 0;
        auto entries = Catalogue(fd, limits, cancel, total, central);
        progress.totalBytes = total;
        struct statvfs capacity{};
        Need(statvfs(root.c_str(), &capacity) == 0, "无法检查存储空间。");
        long double available = static_cast<long double>(capacity.f_bavail) * capacity.f_frsize;
        Need(available >= static_cast<long double>(total) + (64ULL << 20), "空间不足；导入需要同时保留原资源和一份完整的新资源。");
        const auto game = root + "/game", backup = root + "/game.import-backup";
        if (Exists(backup)) {
            Need(Exists(game) && Validate(game, limits, &cancel).ready, "存在上次导入备份，请先恢复有效游戏资源。");
            Directory(backup); RemoveTree(backup);
        }
        std::string pattern = root + "/.game-import-stage-XXXXXX";
        std::vector<char> temporary(pattern.begin(), pattern.end()); temporary.push_back('\0');
        char* directory = mkdtemp(temporary.data()); Need(directory != nullptr, "创建资源临时目录失败。"); stage = directory;
        MarkStage(stage);
        progress.phase = "extracting"; if (callback) callback(progress);
        for (const auto& entry : entries) Extract(fd, entry, central, stage, cancel, progress, callback);
        progress.phase = "validating"; if (callback) callback(progress);
        result = Validate(stage, limits, &cancel); Cancel(&cancel); SyncDirectory(stage);
        bool movedPrevious = false;
        try {
            if (Exists(game)) { Directory(game); Need(rename(game.c_str(), backup.c_str()) == 0, "保存原资源备份失败。"); movedPrevious = true; }
            progress.phase = "committing"; if (callback) callback(progress);
            Cancel(&cancel);
            Need(rename(stage.c_str(), game.c_str()) == 0, "切换新资源失败。");
            committed = true; stage.clear(); SyncDirectory(root);
        } catch (...) {
            if (movedPrevious && !Exists(game)) Need(rename(backup.c_str(), game.c_str()) == 0, "切换失败且原资源恢复失败；请保留 game.import-backup 后重启应用。");
            throw;
        }
        if (movedPrevious) {
            try { RemoveTree(backup); } catch (...) { result.warning = "资源已导入；旧备份将在下次导入时清理。"; }
        }
        result.ready = true; result.bytes = total; result.files = entries.size(); progress.phase = "completed";
    } catch (const Cancelled& error) {
        result = {}; result.cancelled = true; result.error = error.what(); progress.phase = "cancelled";
    } catch (const std::exception& error) {
        result = {}; result.error = error.what(); progress.phase = "error";
        if (committed) { result.ready = true; result.warning = result.error; result.error.clear(); progress.phase = "completed"; }
    }
    if (!stage.empty()) { try { RemoveTree(stage); } catch (...) { if (result.error.empty()) result.error = "清理临时资源失败。"; } }
    progress.active = false; progress.error = result.error;
    try { if (callback) callback(progress); } catch (...) {}
    return result;
}

std::string ToJson(const Result& result) {
    return std::string("{\"ready\":") + (result.ready ? "true" : "false") + ",\"ok\":" + (result.ready ? "true" : "false") +
        ",\"build\":" + Escape(SUPPORTED_BUILD) + ",\"cancelled\":" + (result.cancelled ? "true" : "false") +
        ",\"files\":" + std::to_string(result.files) + ",\"bytes\":" + std::to_string(result.bytes) +
        ",\"error\":" + Escape(result.error) + ",\"warning\":" + Escape(result.warning) + '}';
}

std::string ToJson(const Progress& progress) {
    return "{\"phase\":" + Escape(progress.phase) + ",\"completedBytes\":" + std::to_string(progress.completedBytes) +
        ",\"totalBytes\":" + std::to_string(progress.totalBytes) + ",\"files\":" + std::to_string(progress.files) +
        ",\"error\":" + Escape(progress.error) + ",\"active\":" + (progress.active ? "true" : "false") + '}';
}

} // namespace game_resources
