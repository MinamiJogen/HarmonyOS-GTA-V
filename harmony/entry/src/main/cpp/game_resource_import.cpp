#include "game_resource_import.h"
#include "game_resource_import_core.h"

#include <atomic>
#include <fcntl.h>
#include <memory>
#include <mutex>
#include <string>
#include <unistd.h>

namespace {
std::mutex statusMutex;
std::mutex resourceMutex;
game_resources::Progress current;
std::atomic<bool> cancelled{false};
using ImportGate = bool (*)(bool);
std::atomic<ImportGate> importGate{nullptr};

napi_value String(napi_env env, const std::string& text) {
    napi_value value = nullptr;
    napi_create_string_utf8(env, text.c_str(), text.size(), &value);
    return value;
}

bool PathArgument(napi_env env, napi_value value, std::string& result) {
    size_t size = 0;
    if (napi_get_value_string_utf8(env, value, nullptr, 0, &size) != napi_ok || !size || size > 4096) return false;
    std::string buffer(size + 1, '\0');
    if (napi_get_value_string_utf8(env, value, &buffer[0], buffer.size(), &size) != napi_ok) return false;
    buffer.resize(size);
    if (buffer.find('\0') != std::string::npos || buffer.front() != '/') return false;
    result = std::move(buffer); return true;
}

struct Job {
    napi_env env = nullptr;
    napi_deferred deferred = nullptr;
    napi_async_work work = nullptr;
    bool importing = false;
    int fd = -1;
    std::string directory;
    game_resources::Result result;
    ImportGate gate = nullptr;
    ~Job() { if (fd >= 0) close(fd); }
};

void FinishReservation(Job* job) {
    if (!job->importing) return;
    if (job->gate) { job->gate(false); job->gate = nullptr; }
    std::lock_guard<std::mutex> lock(statusMutex);
    current.active = false;
}

void Execute(napi_env, void* data) {
    auto* job = static_cast<Job*>(data);
    // A startup check can inspect the old complete tree before an import, or
    // the new complete tree afterwards, but never the rename transaction.
    std::lock_guard<std::mutex> resources(resourceMutex);
    try {
        if (job->importing) {
            job->result = game_resources::ImportZip(job->fd, job->directory, cancelled,
                [](const game_resources::Progress& progress) {
                    std::lock_guard<std::mutex> lock(statusMutex); current = progress; current.active = true;
                });
        } else job->result = game_resources::Check(job->directory);
    } catch (const std::exception& error) {
        job->result.error = error.what();
        if (job->importing) {
            std::lock_guard<std::mutex> lock(statusMutex);
            current.phase = "error"; current.error = job->result.error;
        }
    } catch (...) {
        job->result.error = "资源检查或导入发生未知错误。";
    }
    FinishReservation(job);
}

void Complete(napi_env env, napi_status status, void* data) {
    std::unique_ptr<Job> job(static_cast<Job*>(data));
    if (status != napi_ok) {
        job->result = {}; job->result.error = "后台资源任务未能完成。";
        FinishReservation(job.get());
    }
    napi_resolve_deferred(env, job->deferred, String(env, game_resources::ToJson(job->result)));
    napi_delete_async_work(env, job->work);
}

napi_value Queue(napi_env env, std::unique_ptr<Job> job) {
    napi_value promise = nullptr;
    if (napi_create_promise(env, &job->deferred, &promise) != napi_ok) {
        FinishReservation(job.get()); napi_throw_error(env, nullptr, "无法创建资源任务。"); return nullptr;
    }
    napi_value name = String(env, job->importing ? "ImportGameResources" : "CheckGameResources");
    if (napi_create_async_work(env, nullptr, name, Execute, Complete, job.get(), &job->work) != napi_ok ||
        napi_queue_async_work(env, job->work) != napi_ok) {
        if (job->work) napi_delete_async_work(env, job->work);
        FinishReservation(job.get());
        game_resources::Result result; result.error = "无法安排后台资源任务。";
        napi_resolve_deferred(env, job->deferred, String(env, game_resources::ToJson(result)));
        return promise;
    }
    job.release(); return promise;
}

napi_value Import(napi_env env, napi_callback_info info) {
    napi_value args[2]; size_t count = 2;
    napi_get_cb_info(env, info, &count, args, nullptr, nullptr);
    int32_t fd = -1;
    auto job = std::make_unique<Job>(); job->env = env; job->importing = true;
    if (count != 2 || napi_get_value_int32(env, args[0], &fd) != napi_ok || fd < 0 || !PathArgument(env, args[1], job->directory)) {
        napi_throw_type_error(env, nullptr, "importGameResources 需要本地 ZIP 文件 fd 和应用 filesDir。"); return nullptr;
    }
    job->fd = fcntl(fd, F_DUPFD_CLOEXEC, 0);
    if (job->fd < 0) { napi_throw_error(env, nullptr, "无法持有游戏 ZIP；请保持所选文件可读。"); return nullptr; }
    {
        std::lock_guard<std::mutex> lock(statusMutex);
        if (current.active) { napi_throw_error(env, nullptr, "已有游戏资源正在导入。"); return nullptr; }
        current = {}; current.active = true; current.phase = "queued"; cancelled.store(false);
    }
    auto gate = importGate.load();
    if (!gate || !gate(true)) {
        { std::lock_guard<std::mutex> lock(statusMutex); current.active = false; current.phase = "error"; current.error = "游戏已启动或资源导入互斥尚未初始化，请退出游戏后重启应用再导入。"; }
        napi_throw_error(env, nullptr, "游戏已启动或资源导入互斥尚未初始化，请退出游戏后重启应用再导入。"); return nullptr;
    }
    job->gate = gate;
    return Queue(env, std::move(job));
}

napi_value Check(napi_env env, napi_callback_info info) {
    napi_value argument; size_t count = 1;
    napi_get_cb_info(env, info, &count, &argument, nullptr, nullptr);
    auto job = std::make_unique<Job>(); job->env = env;
    if (count != 1 || !PathArgument(env, argument, job->directory)) {
        napi_throw_type_error(env, nullptr, "checkGameResources 需要应用 filesDir。"); return nullptr;
    }
    return Queue(env, std::move(job));
}

napi_value Status(napi_env env, napi_callback_info) {
    std::lock_guard<std::mutex> lock(statusMutex);
    return String(env, game_resources::ToJson(current));
}

napi_value Cancel(napi_env env, napi_callback_info) {
    {
        std::lock_guard<std::mutex> lock(statusMutex);
        if (current.active) { cancelled.store(true); current.phase = "cancelling"; }
    }
    napi_value value; napi_get_undefined(env, &value); return value;
}

} // namespace

void SetGameResourceImportGate(bool (*gate)(bool begin)) { importGate.store(gate); }

napi_value ExportGameResourceImport(napi_env env, napi_value exports) {
    napi_property_descriptor properties[] = {
        {"importGameResources", nullptr, Import, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"checkGameResources", nullptr, Check, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"getGameImportStatus", nullptr, Status, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"cancelGameImport", nullptr, Cancel, nullptr, nullptr, nullptr, napi_default, nullptr}
    };
    napi_define_properties(env, exports, sizeof(properties) / sizeof(properties[0]), properties);
    return exports;
}
