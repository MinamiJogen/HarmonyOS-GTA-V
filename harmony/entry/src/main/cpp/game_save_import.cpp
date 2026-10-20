#include "game_save_import.h"
#include "game_save_import_core.h"

#include <atomic>
#include <cmath>
#include <fcntl.h>
#include <limits>
#include <memory>
#include <string>
#include <unistd.h>

namespace {
using Gate = bool (*)(bool);
std::atomic<Gate> importGate{nullptr};
std::atomic<bool> active{false};

napi_value String(napi_env env, const std::string& text) {
    napi_value value = nullptr;
    napi_create_string_utf8(env, text.c_str(), text.size(), &value); return value;
}
bool DirectoryArgument(napi_env env, napi_value value, std::string& result) {
    size_t n = 0;
    if (napi_get_value_string_utf8(env, value, nullptr, 0, &n) != napi_ok || !n || n > 4096) return false;
    std::string text(n + 1, '\0');
    if (napi_get_value_string_utf8(env, value, text.data(), text.size(), &n) != napi_ok) return false;
    text.resize(n);
    if (text.empty() || text.front() != '/' || text.find('\0') != std::string::npos) return false;
    result = std::move(text); return true;
}
struct Job {
    napi_deferred deferred = nullptr;
    napi_async_work work = nullptr;
    std::string directory;
    game_saves::Result result;
    bool importing = false, reserved = false;
    int fd = -1;
    Gate gate = nullptr;
    ~Job() { if (fd >= 0) close(fd); }
};
void Release(Job* job) {
    if (!job->reserved) return;
    if (job->gate) { job->gate(false); job->gate = nullptr; }
    active.store(false); job->reserved = false;
}
void Execute(napi_env, void* data) {
    auto* job = static_cast<Job*>(data);
    try {
        job->result = job->importing ? game_saves::ImportZip(job->fd, job->directory) : game_saves::List(job->directory);
    } catch (const std::exception& e) { job->result = {}; job->result.error = e.what(); }
      catch (...) { job->result = {}; job->result.error = "存档后台任务发生未知错误。"; }
    Release(job);
}
void Complete(napi_env env, napi_status status, void* data) {
    std::unique_ptr<Job> job(static_cast<Job*>(data));
    if (status != napi_ok) { Release(job.get()); job->result = {}; job->result.error = "存档后台任务未能完成。"; }
    napi_resolve_deferred(env, job->deferred, String(env, game_saves::ToJson(job->result)));
    napi_delete_async_work(env, job->work);
}
napi_value Queue(napi_env env, std::unique_ptr<Job> job) {
    napi_value promise = nullptr;
    if (napi_create_promise(env, &job->deferred, &promise) != napi_ok) {
        Release(job.get()); napi_throw_error(env, nullptr, "无法创建存档任务。"); return nullptr;
    }
    napi_value name = String(env, job->importing ? "ImportGameSaves" : "ListGameSaves");
    if (napi_create_async_work(env, nullptr, name, Execute, Complete, job.get(), &job->work) != napi_ok ||
        napi_queue_async_work(env, job->work) != napi_ok) {
        if (job->work) napi_delete_async_work(env, job->work);
        Release(job.get()); job->result.error = "无法安排存档后台任务。";
        napi_resolve_deferred(env, job->deferred, String(env, game_saves::ToJson(job->result))); return promise;
    }
    job.release(); return promise;
}
napi_value List(napi_env env, napi_callback_info info) {
    napi_value args[1]; size_t count = 1;
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
    auto job = std::make_unique<Job>();
    if (count != 1 || !DirectoryArgument(env, args[0], job->directory)) {
        napi_throw_type_error(env, nullptr, "listGameSaves 需要应用 filesDir。"); return nullptr;
    }
    return Queue(env, std::move(job));
}
napi_value Import(napi_env env, napi_callback_info info) {
    napi_value args[2]; size_t count = 2;
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
    auto job = std::make_unique<Job>(); job->importing = true;
    double fdValue = -1;
    if (count != 2 || napi_get_value_double(env, args[0], &fdValue) != napi_ok || !std::isfinite(fdValue) ||
        fdValue < 0 || fdValue > std::numeric_limits<int>::max() || fdValue != std::floor(fdValue) ||
        !DirectoryArgument(env, args[1], job->directory)) {
        napi_throw_type_error(env, nullptr, "importGameSaves 需要本地存档 ZIP fd 和应用 filesDir。"); return nullptr;
    }
    job->fd = fcntl(static_cast<int>(fdValue), F_DUPFD_CLOEXEC, 0);
    if (job->fd < 0) { napi_throw_error(env, nullptr, "无法持有存档 ZIP 文件，请保持所选文件可读。"); return nullptr; }
    bool expected = false;
    if (!active.compare_exchange_strong(expected, true)) {
        napi_throw_error(env, nullptr, "已有存档 ZIP 正在导入。"); return nullptr;
    }
    job->reserved = true;
    auto gate = importGate.load();
    if (!gate || !gate(true)) {
        Release(job.get());
        napi_throw_error(env, nullptr, "游戏已启动或导入互斥未初始化，请退出游戏后重新打开应用再导入存档。"); return nullptr;
    }
    job->gate = gate;
    return Queue(env, std::move(job));
}
} // namespace

void SetGameSaveImportGate(bool (*gate)(bool begin)) { importGate.store(gate); }
napi_value ExportGameSaveImport(napi_env env, napi_value exports) {
    napi_property_descriptor properties[] = {
        {"listGameSaves", nullptr, List, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"importGameSaves", nullptr, Import, nullptr, nullptr, nullptr, napi_default, nullptr}
    };
    napi_define_properties(env, exports, sizeof(properties) / sizeof(properties[0]), properties);
    return exports;
}
